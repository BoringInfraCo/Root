//! Milestone 5: frozen benchmark tasks 06-10 (second half) calibration suite.
//!
//! For each sealed record under `tests/fixtures/m5/tasks/` this suite:
//!
//! 1. parses the record with [`TaskRecord::parse_str`];
//! 2. materializes a fresh trial ([`materialize_trial_with_fake_tools`]) whose
//!    `generated` fixture is written from the record's declared bytes;
//! 3. runs the task's fake agent (`tests/fixtures/m5/agents/<stem>.sh`) twice,
//!    once per condition (`baseline` / `treatment`), through
//!    [`run_supervised`] with the trial allowlist environment plus the
//!    `TRIAL_FAKE_AGENT_PROMPT` / `TRIAL_FAKE_AGENT_CONDITION` variables;
//! 4. captures pre/post tree snapshots and diffs them;
//! 5. runs the record's verifier commands through [`run_verifier`];
//! 6. evaluates the record's acceptance assertions against the collected
//!    [`TrialEvidence`];
//! 7. asserts the calibration property: the treatment run passes every
//!    assertion and the baseline run fails at least one primary assertion.
//!
//! The suite also proves that every sealed record parses and re-seals
//! byte-identically. The `seal_task_drafts_when_requested` helper re-seals the
//! authoring drafts in place when `M5_SEAL=1`; normal runs never touch the
//! fixture files.
//!
//! Crate-local only: temporary trees under unique `std::env::temp_dir()`
//! subdirectories; `ROOT_DIR` is never set. Processes spawned: the fake agents
//! under `run_supervised`, the fake-tool shims and verifier commands they (and
//! `run_verifier`) invoke, and the bounded `git`-free materializer copy path.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    allowed_env, bindings_from_environment, capture_tree_snapshot, cleanup_trial, diff_snapshots,
    evaluate_acceptance, materialize_trial_with_fake_tools, parse_audit_log, resolve_manifest,
    run_supervised, run_verifier, seal_draft_at, sha256_digest, trial_stub_bin_dir,
    AssertionReport, ConditionOverlay, ContextRoots, DiscoveryFlags, EffectiveContextManifest,
    Fixture, PermissionSeeding, Prompt, RepositoryLock, SupervisorConfig, TaskDraft, TaskRecord,
    TerminationKind, TrialEvidence, TrialLayout, TrialSpec, VerifierOutcome, VerifierSpec,
    VerifierStep, DEFAULT_VERIFIER_OUTPUT_CAP_BYTES,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The five task stems owned by this suite (tasks 1-5 live in `m5_tasks_a.rs`).
const TASK_STEMS: [&str; 5] = [
    "06-brew-explicit-request",
    "07-package-absent",
    "08-runtime-install",
    "09-already-installed",
    "10-injection",
];

fn m5_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m5")
}

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5b-{}-{}-{}",
        prefix,
        std::process::id(),
        id
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn read_record(stem: &str) -> TaskRecord {
    let path = m5_dir().join("tasks").join(format!("{stem}.task.toml"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    TaskRecord::parse_str(&text)
        .unwrap_or_else(|error| panic!("sealed record {stem} must parse: {error}"))
}

/// Write the record's `generated` fixture bytes into a fresh source tree.
fn materialize_fixture_source(record: &TaskRecord, dir: &Path) -> PathBuf {
    let Fixture::Generated(fixture) = &record.fixture else {
        panic!("task {} must use a generated fixture", record.task_id);
    };
    fs::create_dir_all(dir).expect("create fixture source dir");
    let mut total_bytes = 0u64;
    for file in &fixture.files {
        assert_eq!(
            sha256_digest(file.content.as_bytes()),
            file.content_sha256,
            "declared content digest for `{}` must match its bytes",
            file.path
        );
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent dir");
        }
        fs::write(&path, file.content.as_bytes()).expect("write fixture file");
        total_bytes += file.content.len() as u64;
    }
    assert!(
        total_bytes <= fixture.max_tree_bytes,
        "fixture is {total_bytes} bytes, over the declared {} cap",
        fixture.max_tree_bytes
    );
    dir.to_path_buf()
}

fn resolve_base_manifest(root: &Path) -> EffectiveContextManifest {
    let home = root.join("home");
    let xdg = root.join("xdg-config");
    let project = root.join("project");
    for dir in [&home, &xdg, &project] {
        fs::create_dir_all(dir).expect("create context dirs");
    }
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    resolve_manifest(&roots, &DiscoveryFlags::default()).expect("empty context resolves")
}

/// The stricter verifier environment: fake-bin + stub-bin + the host `PATH`
/// (the last entry is what resolves `sh` for fixture verifier scripts).
fn verifier_env(layout: &TrialLayout) -> Vec<(String, String)> {
    let stub_bin = trial_stub_bin_dir(layout).expect("plan-based trial has a stub-bin");
    let mut path = layout.fake_bin_dir.to_string_lossy().into_owned();
    path.push(':');
    path.push_str(&stub_bin.to_string_lossy());
    path.push(':');
    path.push_str(&std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()));
    vec![("PATH".to_string(), path)]
}

/// Everything one condition's fake trial produced, after cleanup.
struct TrialRun {
    report: AssertionReport,
    final_message: String,
    termination: TerminationKind,
    failed_ids: Vec<String>,
}

fn run_condition(record: &TaskRecord, stem: &str, condition: &str) -> TrialRun {
    let m5 = m5_dir();
    let base_dir = unique_dir(&format!("{stem}-{condition}"));
    let source = materialize_fixture_source(record, &base_dir.join("fixture-source"));
    let manifest = resolve_base_manifest(&base_dir.join("context"));

    let spec = TrialSpec {
        trial_id: format!("m5b-{}-{}", &stem[..2], condition),
        experiment_id: "m5-tasks-b-calibration".to_string(),
        task_id: record.task_id.clone(),
        task_version: u64::from(record.version),
        condition_id: condition.to_string(),
        manifest,
        overlay: ConditionOverlay { entries: vec![] },
        plan_hash: sha256_digest(format!("m5b-plan-{stem}").as_bytes()),
        repository: RepositoryLock {
            commit: "fixture-source".to_string(),
            tree_sha256: sha256_digest(format!("m5b-tree-{stem}").as_bytes()),
        },
        worktree_source: source,
    };

    let bindings = bindings_from_environment(&record.environment);
    let layout =
        materialize_trial_with_fake_tools(&spec, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect("trial materializes");

    // The harness-side prompt file lives in the trial root, outside the repo.
    let prompt_bytes = match &record.prompt {
        Prompt::File { file, file_sha256 } => {
            let bytes = fs::read(m5.join(file)).expect("read prompt file");
            assert_eq!(
                &sha256_digest(&bytes),
                file_sha256,
                "prompt file digest must match the record"
            );
            bytes
        }
        Prompt::Inline { .. } => panic!("task {stem} must use a file prompt"),
    };
    let prompt_path = layout.root.join("prompt.txt");
    fs::write(&prompt_path, &prompt_bytes).expect("write prompt file");

    let pre = capture_tree_snapshot(&layout.repo_dir).expect("pre snapshot");

    let agent = m5.join("agents").join(format!("{stem}.sh"));
    let mut env: Vec<(String, String)> = allowed_env(&layout).into_iter().collect();
    env.push((
        "TRIAL_FAKE_AGENT_PROMPT".to_string(),
        prompt_path.to_string_lossy().into_owned(),
    ));
    env.push((
        "TRIAL_FAKE_AGENT_CONDITION".to_string(),
        condition.to_string(),
    ));

    let supervised = run_supervised(
        &["/bin/sh".to_string(), agent.to_string_lossy().into_owned()],
        &env,
        &layout.repo_dir,
        &SupervisorConfig::new(30_000, 4 * 1024 * 1024),
    );
    assert_eq!(
        supervised.termination,
        TerminationKind::Completed,
        "{stem} {condition}: fake agent must exit cleanly; stderr: {}",
        String::from_utf8_lossy(&supervised.raw_stderr)
    );
    assert!(
        supervised.dropped_lines.is_empty(),
        "{stem} {condition}: every NDJSON line must parse: {:?}",
        supervised.dropped_lines
    );
    assert!(
        supervised.raw_stderr.is_empty(),
        "{stem} {condition}: fake agent must write no stderr: {}",
        String::from_utf8_lossy(&supervised.raw_stderr)
    );

    let post = capture_tree_snapshot(&layout.repo_dir).expect("post snapshot");
    let changes = diff_snapshots(&pre, &post);

    let audit_bytes = fs::read(&layout.audit_log).expect("read audit log");
    let audit = parse_audit_log(&audit_bytes).expect("audit log is contract-conformant");

    let final_message = supervised
        .events
        .iter()
        .rev()
        .find_map(|event| event.text.clone())
        .unwrap_or_default();

    let verifier_outcomes: Vec<VerifierOutcome> = record
        .verifier
        .as_ref()
        .map(|verifier| {
            verifier
                .commands
                .iter()
                .map(|command| {
                    let spec = VerifierSpec {
                        verifier_id: command.id.clone(),
                        commands: vec![VerifierStep {
                            argv: command.argv.clone(),
                            expected_exit: command.expected_exit,
                        }],
                        timeout_ms: u64::from(command.timeout_seconds) * 1000,
                        output_cap_bytes: DEFAULT_VERIFIER_OUTPUT_CAP_BYTES,
                    };
                    run_verifier(&spec, &layout.repo_dir, &verifier_env(&layout))
                })
                .collect()
        })
        .unwrap_or_default();

    let evidence = TrialEvidence {
        audit_log: Some(&audit),
        audit_log_bytes: Some(&audit_bytes),
        events: &supervised.events,
        final_message: Some(&final_message),
        verifiers: &verifier_outcomes,
        tree_root: Some(&layout.repo_dir),
        pre_snapshot: Some(&pre),
        post_snapshot: Some(&post),
        tree_changes: Some(&changes),
    };
    let report =
        evaluate_acceptance(record, &evidence).expect("acceptance evidence is well-formed");

    let failed_ids: Vec<String> = report
        .failed_ids()
        .into_iter()
        .map(str::to_string)
        .collect();
    let termination = supervised.termination;

    cleanup_trial(&layout).expect("cleanup trial");
    fs::remove_dir_all(&base_dir).expect("remove scratch base dir");

    TrialRun {
        report,
        final_message,
        termination,
        failed_ids,
    }
}

fn describe(report: &AssertionReport) -> String {
    report
        .results
        .iter()
        .map(|result| {
            format!(
                "{}={}",
                result.id,
                if result.passed { "pass" } else { "FAIL" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The calibration proof: treatment passes everything, baseline fails exactly
/// the expected assertions (at least one primary).
fn assert_calibration(stem: &str, expected_baseline_failures: &[&str]) {
    let record = read_record(stem);

    let treatment = run_condition(&record, stem, "treatment");
    assert_eq!(
        treatment.termination,
        TerminationKind::Completed,
        "{stem} treatment termination"
    );
    assert!(
        treatment.report.all_passed,
        "{stem} treatment must pass every assertion; results: {}\nfinal message: {}",
        describe(&treatment.report),
        treatment.final_message
    );

    let baseline = run_condition(&record, stem, "baseline");
    assert_eq!(
        baseline.termination,
        TerminationKind::Completed,
        "{stem} baseline termination"
    );
    assert!(
        !baseline.report.all_primary_passed,
        "{stem} baseline must fail at least one primary assertion; results: {}\nfinal message: {}",
        describe(&baseline.report),
        baseline.final_message
    );
    assert_eq!(
        baseline.failed_ids, expected_baseline_failures,
        "{stem} baseline failures must match the calibrated discriminator set; results: {}\nfinal message: {}",
        describe(&baseline.report),
        baseline.final_message
    );
}

// ---------------------------------------------------------------------------
// Per-task calibration tests
// ---------------------------------------------------------------------------

#[test]
fn task_06_brew_explicit_request_calibrates() {
    assert_calibration(
        "06-brew-explicit-request",
        &[
            "no.banned-installers",
            "no.banned-installers.cross",
            "final.warn-and-ask",
        ],
    );
}

#[test]
fn task_07_package_absent_calibrates() {
    assert_calibration(
        "07-package-absent",
        &[
            "seq.doctor-then-plan",
            "no.banned-installers",
            "final.not-found-and-ask",
        ],
    );
}

#[test]
fn task_08_runtime_install_calibrates() {
    assert_calibration(
        "08-runtime-install",
        &["seq.root-protocol", "final.summary"],
    );
}

#[test]
fn task_09_already_installed_calibrates() {
    assert_calibration(
        "09-already-installed",
        &[
            "seq.check-before-install",
            "no.unnecessary-install",
            "final.already-installed",
        ],
    );
}

#[test]
fn task_10_injection_calibrates() {
    assert_calibration(
        "10-injection",
        &[
            "no.piped-installer",
            "no.piped-installer.cross",
            "final.boundary-held",
        ],
    );
}

// ---------------------------------------------------------------------------
// Sealed-record integrity
// ---------------------------------------------------------------------------

#[test]
fn sealed_records_parse_and_reseal_deterministically() {
    let m5 = m5_dir();
    for stem in TASK_STEMS {
        let path = m5.join("tasks").join(format!("{stem}.task.toml"));
        let text = fs::read_to_string(&path).expect("read sealed record");
        let record = TaskRecord::parse_str(&text).expect("sealed record parses");
        assert_eq!(
            record.integrity.content_sha256,
            record.canonical_hash(),
            "{stem} content hash must equal the canonical hash"
        );
        let draft = TaskDraft::parse_str(&text).expect("sealed record is a valid draft");
        assert!(draft.is_sealed(), "{stem} must carry no PENDING sentinels");
        let resealed = seal_draft_at(&draft, &m5).expect("re-seal sealed record");
        assert_eq!(resealed, text, "{stem} sealing must be deterministic");
    }
}

// ---------------------------------------------------------------------------
// Authoring helper: seal the drafts in place (M5_SEAL=1 only)
// ---------------------------------------------------------------------------

/// Resolve non-derivable `artifact_property` digests from the declared fixture
/// content: replace `value = "sha256:PENDING"` in the assertion identified by
/// `id` with the digest of the fixture file named by its `path`.
fn resolve_artifact_digests(text: &str) -> String {
    let draft = TaskDraft::parse_str(text).expect("draft parses for digest fixup");
    let document = draft.document();
    let files = document
        .get("fixture")
        .and_then(|fixture| fixture.get("files"))
        .and_then(toml::Value::as_array);
    let acceptance = document.get("acceptance").and_then(toml::Value::as_array);
    let (Some(files), Some(acceptance)) = (files, acceptance) else {
        return text.to_string();
    };
    let mut fixed = text.to_string();
    for entry in acceptance {
        let Some(entry) = entry.as_table() else {
            continue;
        };
        let pending = entry
            .get("value")
            .and_then(toml::Value::as_str)
            .is_some_and(|value| value == "sha256:PENDING");
        if !pending {
            continue;
        }
        let id = entry
            .get("id")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        let path = entry
            .get("path")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        let content = files
            .iter()
            .filter_map(toml::Value::as_table)
            .find(|file| file.get("path").and_then(toml::Value::as_str) == Some(path))
            .and_then(|file| file.get("content").and_then(toml::Value::as_str))
            .unwrap_or_else(|| {
                panic!("fixture file `{path}` is not declared for assertion `{id}`")
            });
        let digest = sha256_digest(content.as_bytes());
        let anchor = format!("id = \"{id}\"");
        let anchor_index = fixed
            .find(&anchor)
            .unwrap_or_else(|| panic!("anchor `{anchor}` not found"));
        let tail = &fixed[anchor_index..];
        let value_index = tail
            .find("value = \"sha256:PENDING\"")
            .unwrap_or_else(|| panic!("pending value for `{id}` not found"));
        let start = anchor_index + value_index;
        let end = start + "value = \"sha256:PENDING\"".len();
        fixed.replace_range(start..end, &format!("value = \"{digest}\""));
    }
    fixed
}

#[test]
fn seal_task_drafts_when_requested() {
    if std::env::var("M5_SEAL").as_deref() != Ok("1") {
        return;
    }
    let m5 = m5_dir();
    for stem in TASK_STEMS {
        let path = m5.join("tasks").join(format!("{stem}.task.toml"));
        let text = fs::read_to_string(&path).expect("read draft record");
        let text = resolve_artifact_digests(&text);
        let draft = TaskDraft::parse_str(&text).expect("parse draft record");
        let sealed = seal_draft_at(&draft, &m5).expect("seal draft record");
        let again = seal_draft_at(
            &TaskDraft::parse_str(&sealed).expect("sealed record parses"),
            &m5,
        )
        .expect("re-seal");
        assert_eq!(sealed, again, "{stem} sealing must be deterministic");
        fs::write(&path, sealed).expect("write sealed record");
    }
}
