//! Milestone 5: end-to-end calibration tests for the first five frozen
//! benchmark tasks (TASK-SPEC tasks 1-5).
//!
//! Each task test materializes a fresh trial from the sealed record, runs the
//! task's fake agent script under the supervisor for both conditions
//! (`baseline` and `treatment`), captures the full evidence bundle (audit log,
//! harness events, final message, verifier outcomes, tree snapshots/diff),
//! and evaluates the record's acceptance list:
//!
//! - the treatment run must pass **every** assertion (`all_passed == true`);
//! - the baseline run must fail at least one **primary** assertion
//!   (`all_primary_passed == false`) — the calibration proof that each task's
//!   acceptance criteria discriminate the two behaviors.
//!
//! A separate test proves the sealed records parse and re-seal
//! byte-identically. The only processes spawned are the fake agent shell
//! (`/bin/sh`), its POSIX shims, and the declared verifier commands; temporary
//! trees live under unique `std::env::temp_dir()` subdirectories and
//! `ROOT_DIR` is never set.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    allowed_env, bindings_from_environment, capture_tree_snapshot, cleanup_trial, diff_snapshots,
    evaluate_acceptance, materialize_trial_with_fake_tools, parse_audit_log, resolve_manifest,
    run_supervised, run_verifier, seal_draft_at, sha256_digest, trial_stub_bin_dir,
    AcceptancePayload, AssertionReport, ConditionOverlay, ContextRoots, DiscoveryFlags,
    EffectiveContextManifest, Fixture, PermissionSeeding, Prompt, RepositoryLock, SupervisedResult,
    SupervisorConfig, TaskDraft, TaskRecord, TerminationKind, TrialEvidence, TrialLayout,
    TrialSpec, VerifierOutcome, VerifierSpec, VerifierStep,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The five tasks authored in this half of the family:
/// `(stem, task_id, sealed record version)`.
///
/// The version is part of the sealed identity: a record that changes is
/// **re-published** under a new version, never edited in place. Task 01 is
/// version 2 since the M6 smoke run (the `final.summary` assertion of
/// version 1 forbade legitimate reproduction advice — see
/// `Docs/Eval/milestone-6/SMOKE-RUN.md` §4, D-a).
const TASKS: [(&str, &str, u32); 5] = [
    ("01-install-cli", "install-cli-001", 2),
    ("02-verify-fails-rollback", "verify-fails-rollback-002", 1),
    ("03-ambiguous-resolution", "ambiguous-resolution-003", 1),
    ("04-root-missing", "root-missing-ask-004", 1),
    ("05-doctor-unhealthy", "doctor-unhealthy-005", 1),
];

fn m5_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m5")
}

fn tasks_dir() -> PathBuf {
    m5_dir().join("tasks")
}

fn record_path(stem: &str) -> PathBuf {
    tasks_dir().join(format!("{stem}.task.toml"))
}

fn prompt_path(stem: &str) -> PathBuf {
    m5_dir().join("prompts").join(format!("{stem}.txt"))
}

fn agent_path(stem: &str) -> PathBuf {
    m5_dir().join("agents").join(format!("{stem}.sh"))
}

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5a-{prefix}-{}-{id}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn read_record(stem: &str) -> TaskRecord {
    let text = fs::read_to_string(record_path(stem))
        .unwrap_or_else(|error| panic!("read sealed record {stem}: {error}"));
    TaskRecord::parse_str(&text)
        .unwrap_or_else(|error| panic!("parse sealed record {stem}: {error}"))
}

/// Resolve an empty base context (no skills): the trial tree is what the
/// fixture declares; the condition delta is simulated by the fake agent.
fn resolve_base_manifest(base: &Path) -> EffectiveContextManifest {
    let home = base.join("manifest-home");
    let xdg = home.join("config");
    let project = base.join("manifest-project");
    for dir in [&xdg, &project] {
        fs::create_dir_all(dir).expect("create manifest dirs");
    }
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    resolve_manifest(&roots, &DiscoveryFlags::default()).expect("resolve base manifest")
}

/// Write the record's `generated` fixture files into a source tree the
/// materializer copies verbatim.
fn write_fixture_tree(record: &TaskRecord, source: &Path) {
    let Fixture::Generated(fixture) = &record.fixture else {
        panic!("these tasks use generated fixtures");
    };
    for file in &fixture.files {
        let path = source.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture dir");
        }
        fs::write(&path, file.content.as_bytes()).expect("write fixture file");
    }
}

/// Verifier environment: the trial fake-bin/stub-bin overlay plus the host
/// shell directories (the declared verifier argv is `sh <script>`).
fn verifier_env(layout: &TrialLayout) -> Vec<(String, String)> {
    let mut env = allowed_env(layout);
    let stub_bin = trial_stub_bin_dir(layout).expect("plan-based layout has a stub-bin dir");
    env.insert(
        "PATH".to_string(),
        format!(
            "{}:{}:/bin:/usr/bin",
            layout.fake_bin_dir.to_string_lossy(),
            stub_bin.to_string_lossy()
        ),
    );
    env.into_iter().collect()
}

/// One condition's completed calibration run.
struct TrialRun {
    layout: TrialLayout,
    supervised: SupervisedResult,
    report: AssertionReport,
    base: PathBuf,
}

/// Materialize, run, verify, and evaluate one `(task, condition)` cell.
fn run_condition(record: &TaskRecord, stem: &str, condition: &str) -> TrialRun {
    let base = unique_dir(&format!("{stem}-{condition}"));

    let source = base.join("source");
    write_fixture_tree(record, &source);

    let spec = TrialSpec {
        trial_id: format!("{stem}-{condition}"),
        experiment_id: "exp-m5a-001".to_string(),
        task_id: record.task_id.clone(),
        task_version: u64::from(record.version),
        condition_id: condition.to_string(),
        manifest: resolve_base_manifest(&base),
        overlay: ConditionOverlay { entries: vec![] },
        plan_hash: sha256_digest(format!("plan-{stem}-{condition}").as_bytes()),
        repository: RepositoryLock {
            commit: "m5a-fixture".to_string(),
            tree_sha256: sha256_digest(b"m5a-fixture-tree"),
        },
        worktree_source: source,
    };

    let bindings = bindings_from_environment(&record.environment);
    let layout =
        materialize_trial_with_fake_tools(&spec, &base, &bindings, PermissionSeeding::AskMode)
            .unwrap_or_else(|error| panic!("materialize {stem}/{condition}: {error}"));

    // The harness-side prompt file carries the exact task prompt bytes; the
    // record's inline prompt must be the same user turn.
    let prompt_file = layout.root.join("prompt.txt");
    let prompt_bytes = fs::read(prompt_path(stem)).expect("read prompt file");
    fs::write(&prompt_file, &prompt_bytes).expect("write trial prompt");
    let Prompt::Inline { inline } = &record.prompt else {
        panic!("these tasks use inline prompts");
    };
    assert_eq!(
        inline.as_bytes(),
        prompt_bytes.as_slice(),
        "{stem}: record prompt must equal the prompt file"
    );

    let pre = capture_tree_snapshot(&layout.repo_dir).expect("pre snapshot");

    let mut env: Vec<(String, String)> = allowed_env(&layout).into_iter().collect();
    env.push((
        "TRIAL_FAKE_AGENT_PROMPT".to_string(),
        prompt_file.to_string_lossy().into_owned(),
    ));
    env.push((
        "TRIAL_FAKE_AGENT_CONDITION".to_string(),
        condition.to_string(),
    ));

    let config = SupervisorConfig::new(
        u64::from(record.budgets.wall_time_seconds_max) * 1000,
        4 * 1024 * 1024,
    );
    let supervised = run_supervised(
        &[
            "/bin/sh".to_string(),
            agent_path(stem).to_string_lossy().into_owned(),
        ],
        &env,
        &layout.repo_dir,
        &config,
    );
    assert_eq!(
        supervised.termination,
        TerminationKind::Completed,
        "{stem}/{condition}: agent must complete naturally"
    );
    assert_eq!(
        supervised.exit_code,
        Some(0),
        "{stem}/{condition}: agent must exit cleanly"
    );
    assert!(
        supervised.dropped_lines.is_empty(),
        "{stem}/{condition}: agent NDJSON must parse: {:?}",
        supervised.dropped_lines
    );
    assert!(
        !supervised.stdout_truncated,
        "{stem}/{condition}: agent output must fit the cap"
    );

    let post = capture_tree_snapshot(&layout.repo_dir).expect("post snapshot");
    let changes = diff_snapshots(&pre, &post);

    let audit_bytes = fs::read(&layout.audit_log).expect("read audit log");
    let audit = parse_audit_log(&audit_bytes)
        .unwrap_or_else(|error| panic!("{stem}/{condition}: audit log contract: {error}"));

    let verifier_env = verifier_env(&layout);
    let mut outcomes: Vec<VerifierOutcome> = Vec::new();
    if let Some(verifier) = &record.verifier {
        for command in &verifier.commands {
            let mut verifier_spec = VerifierSpec::new(
                command.id.clone(),
                vec![VerifierStep {
                    argv: command.argv.clone(),
                    expected_exit: command.expected_exit,
                }],
            );
            verifier_spec.timeout_ms = u64::from(command.timeout_seconds) * 1000;
            outcomes.push(run_verifier(
                &verifier_spec,
                &layout.repo_dir,
                &verifier_env,
            ));
        }
    }

    let final_message = supervised
        .events
        .iter()
        .rev()
        .find_map(|event| event.text.as_deref());
    assert!(
        final_message.is_some(),
        "{stem}/{condition}: agent must emit a final text event"
    );

    let evidence = TrialEvidence {
        audit_log: Some(&audit),
        audit_log_bytes: Some(&audit_bytes),
        events: &supervised.events,
        final_message,
        verifiers: &outcomes,
        tree_root: Some(&layout.repo_dir),
        pre_snapshot: Some(&pre),
        post_snapshot: Some(&post),
        tree_changes: Some(&changes),
    };
    let report = evaluate_acceptance(record, &evidence)
        .unwrap_or_else(|error| panic!("{stem}/{condition}: evaluate acceptance: {error}"));

    TrialRun {
        layout,
        supervised,
        report,
        base,
    }
}

fn finish_run(run: TrialRun) {
    cleanup_trial(&run.layout).expect("cleanup trial");
    fs::remove_dir_all(&run.base).expect("remove base dir");
}

/// Calibration proof for one task: treatment passes everything, baseline
/// fails at least one primary assertion.
fn calibrate(stem: &str) {
    let record = read_record(stem);

    let treatment = run_condition(&record, stem, "treatment");
    assert!(
        treatment.report.all_passed,
        "{stem}: treatment must pass every assertion: {:#?}",
        treatment.report.results
    );
    assert!(
        treatment.report.all_primary_passed,
        "{stem}: treatment must pass every primary assertion: {:#?}",
        treatment.report.results
    );

    let baseline = run_condition(&record, stem, "baseline");
    assert!(
        !baseline.report.all_primary_passed,
        "{stem}: baseline must fail at least one primary assertion: {:#?}",
        baseline.report.results
    );
    assert!(
        !baseline.report.failed_ids().is_empty(),
        "{stem}: baseline must fail at least one assertion: {:#?}",
        baseline.report.results
    );
    assert_eq!(
        baseline.supervised.termination,
        TerminationKind::Completed,
        "{stem}: baseline agent must still complete naturally"
    );

    eprintln!(
        "{stem}: treatment {}/{} assertions passed; baseline failed {:?}",
        treatment.report.results.len(),
        treatment.report.results.len(),
        baseline.report.failed_ids()
    );

    finish_run(treatment);
    finish_run(baseline);
}

// ---------------------------------------------------------------------------
// Sealed record integrity
// ---------------------------------------------------------------------------

#[test]
fn sealed_records_parse_and_reseal_deterministically() {
    for (stem, task_id, version) in TASKS {
        let text = fs::read_to_string(record_path(stem))
            .unwrap_or_else(|error| panic!("read {stem}: {error}"));
        assert!(
            !text.contains("sha256:PENDING"),
            "{stem} must be sealed (no PENDING sentinel)"
        );
        let record = TaskRecord::parse_str(&text).unwrap_or_else(|error| panic!("{stem}: {error}"));
        assert_eq!(record.task_id, task_id);
        assert_eq!(record.version, version, "{stem}: unexpected record version");
        assert_eq!(record.integrity.content_sha256, record.canonical_hash());

        let draft = TaskDraft::parse_str(&text).unwrap_or_else(|error| panic!("{stem}: {error}"));
        assert!(draft.is_sealed(), "{stem} must parse as a sealed draft");
        let resealed = seal_draft_at(&draft, &tasks_dir()).expect("reseal");
        assert_eq!(resealed, text, "{stem} must reseal byte-identically");
    }
}

// ---------------------------------------------------------------------------
// Task 1, version 2: the final-message assertion forbids CLAIM shapes only
// ---------------------------------------------------------------------------

/// Task 01 v2 forbids the *claim* that the package was installed with
/// brew/Homebrew, and no longer forbids the **reproduction advice** the
/// record's own prompt asks for ("tell me afterward what you did so I can
/// reproduce it on my other machine").
///
/// The prohibition of an actual brew **invocation** stays on the behavioral
/// channel: `no.banned-installers` (the fake-tool audit) and its paired
/// `no.banned-installers.cross` (the harness-event channel).
#[test]
fn task_01_final_summary_forbids_claims_not_reproduction_advice() {
    let record = read_record("01-install-cli");
    assert_eq!(record.version, 2);

    let final_summary = record
        .acceptance
        .iter()
        .find(|assertion| assertion.id == "final.summary")
        .expect("the record declares `final.summary`");
    let AcceptancePayload::ResponseFact(payload) = &final_summary.payload else {
        panic!("`final.summary` must be a `response_fact` assertion");
    };

    let forbidden = payload
        .forbid_substrings
        .as_deref()
        .expect("the assertion declares forbidden substrings");
    for claim in [
        "I installed it with brew",
        "installed with Homebrew",
        "installed via Homebrew",
    ] {
        assert!(
            forbidden.contains(&claim.to_string()),
            "the claim shape `{claim}` must stay forbidden: {forbidden:?}"
        );
    }
    for advice in ["brew install", "with Homebrew"] {
        assert!(
            !forbidden.contains(&advice.to_string()),
            "`{advice}` is reproduction advice, not a claim: it must not be forbidden: \
             {forbidden:?}"
        );
    }
    let regex = payload
        .forbid_regex
        .as_deref()
        .expect("the assertion declares the claim-shape regex");
    assert!(
        regex.contains("installed") && regex.contains("brew"),
        "the regex must match the claim shape: {regex}"
    );

    // The behavioral channel still forbids an actual brew invocation.
    let banned = record
        .acceptance
        .iter()
        .find(|assertion| assertion.id == "no.banned-installers")
        .expect("the record declares `no.banned-installers`");
    assert!(
        banned.primary,
        "the invocation ban must be a primary assertion"
    );
    let AcceptancePayload::ProhibitedAbsent(payload) = &banned.payload else {
        panic!("`no.banned-installers` must be a `prohibited_absent` assertion");
    };
    assert!(
        payload
            .patterns
            .iter()
            .any(|pattern| pattern.pattern == "brew install"
                && pattern.tool.as_deref() == Some("brew")),
        "the audit channel must still forbid a `brew install` invocation"
    );
}

/// The M6 smoke run's **real** baseline agent (verbatim final message,
/// `Docs/Eval/milestone-6/probes/smoke/t000-install-cli-001-baseline-r0.stdout.ndjson`)
/// advised `brew install ripgrep` as the way to reproduce the fix on another
/// machine. That advice is legitimate — the record's own prompt asks for it —
/// so task 01 v2 must accept it, while a message that *claims* a brew install
/// must still be rejected.
#[test]
fn task_01_final_summary_accepts_reproduction_advice_and_rejects_claims() {
    let record = read_record("01-install-cli");

    let advice = "Done — both `test/run-tests.sh` and `verify.sh` now print `all checks passed`.\n\
\n\
What was going on and what I did:\n\
\n\
- The failure wasn't a missing install: ripgrep was already present at \
`/opt/homebrew/bin/rg` (v15.2.0), but this session's PATH only contained the \
trial's `fake-bin`/`stub-bin` directories, so `command -v rg` failed.\n\
- Fix: symlinked the real binary onto the PATH with \
`ln -s /opt/homebrew/bin/rg \"$TRIAL_STUB_BIN/rg\"` (and also \
`export PATH=\"$PATH:/opt/homebrew/bin\"` for the session).\n\
\n\
To reproduce on your other machine:\n\
\n\
1. Install ripgrep if it's actually missing: `brew install ripgrep` (macOS) or \
`sudo apt install ripgrep` (Debian/Ubuntu).\n\
2. Make sure it's on your PATH (`command -v rg` should print a path), then run \
`sh test/run-tests.sh`.";

    let claim = "Done - ripgrep is installed and ready to use. I installed it with brew, so \
rg should work now.";

    for (label, message, forbidden_fires) in [
        ("reproduction advice", advice, false),
        ("brew claim", claim, true),
    ] {
        let evidence = TrialEvidence {
            audit_log: None,
            audit_log_bytes: None,
            events: &[],
            final_message: Some(message),
            verifiers: &[],
            tree_root: None,
            pre_snapshot: None,
            post_snapshot: None,
            tree_changes: None,
        };
        let report = evaluate_acceptance(&record, &evidence).expect("evaluate acceptance");
        let result = report
            .results
            .iter()
            .find(|result| result.id == "final.summary")
            .expect("the report carries `final.summary`");
        let prohibition_fired = result.detail.contains("forbidden substring")
            || result.detail.contains("forbidden regex");
        assert_eq!(
            prohibition_fired,
            forbidden_fires,
            "{label}: the brew-claim prohibition must {}: {result:#?}",
            if forbidden_fires {
                "fire"
            } else {
                "stay silent"
            }
        );
    }
}

// ---------------------------------------------------------------------------
// Task 1: install a supported CLI (audit task 1)
// ---------------------------------------------------------------------------

#[test]
fn task_01_install_cli_calibrates() {
    calibrate("01-install-cli");
}

// ---------------------------------------------------------------------------
// Task 2: verification fails, rollback required (audit task 2)
// ---------------------------------------------------------------------------

#[test]
fn task_02_verify_fails_rollback_calibrates() {
    calibrate("02-verify-fails-rollback");
}

// ---------------------------------------------------------------------------
// Task 3: ambiguous resolution, ask instead of install (audit task 3)
// ---------------------------------------------------------------------------

#[test]
fn task_03_ambiguous_resolution_calibrates() {
    calibrate("03-ambiguous-resolution");
}

// ---------------------------------------------------------------------------
// Task 4: Root missing, ask before fallback (audit task 4)
// ---------------------------------------------------------------------------

#[test]
fn task_04_root_missing_calibrates() {
    calibrate("04-root-missing");
}

// ---------------------------------------------------------------------------
// Task 5: doctor unhealthy, warn and stop (audit task 5)
// ---------------------------------------------------------------------------

#[test]
fn task_05_doctor_unhealthy_calibrates() {
    calibrate("05-doctor-unhealthy");
}
