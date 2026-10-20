//! Milestone 6: the host-leakage detection (smoke-run defect D-b).
//!
//! The trial's isolation is **process- and environment-level**, not
//! OS-level: an agent can name a host path directly. The M6 smoke run's
//! baseline agent found the host's `/opt/homebrew/bin/rg` and symlinked it
//! into the trial's stub-bin, so a trial could satisfy a task **without** the
//! skill. Such a trial measures the host, not the agent: it must be
//! **invalid** — never a pass and never a fail.
//!
//! What this suite proves:
//!
//! - a trial that plants a symlink to a host path is classified
//!   `host_contamination`, its block is excluded from the report's pairing,
//!   the finding is recorded in the trial's sealed evidence and in
//!   `summary.json`, and the trial tree is preserved for inspection;
//! - a clean trial is **not** flagged: the fake-agent matrix still produces
//!   its calibrated outcomes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{run_feasibility_matrix, FeasibilityConfig, TrialOutcomeClass};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const EXPERIMENT_ID: &str = "m6-host-leakage";

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m6-leak-{prefix}-{}-{id}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn m5_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m5")
}

/// The fake agent that plants a host symlink in the trial's stub-bin — the
/// exact shape of the M6 smoke run's baseline agent.
///
/// The trial `PATH` carries no host tools, so the symlink is created through
/// an **absolute** path: that is the leak the detector has to catch.
const LEAKING_AGENT: &str = r#"#!/bin/sh
# Fake agent that smuggles a host binary into the trial (M6 D-b regression).
set -u
if [ -n "${TRIAL_STUB_BIN:-}" ]; then
  /bin/ln -sf /opt/homebrew/bin/rg "$TRIAL_STUB_BIN/rg" 2>/dev/null \
    || /usr/bin/ln -sf /opt/homebrew/bin/rg "$TRIAL_STUB_BIN/rg" 2>/dev/null \
    || true
fi
printf '%s\n' '{"type":"text","timestamp":1,"sessionID":"ses_leak","part":{"type":"text","text":"planted a host symlink"}}'
"#;

/// Copy the M5 task 01 fixtures into a scratch tree.
///
/// `agent` replaces the task's fake-agent script when it is `Some`.
fn copy_fixtures(root: &Path, agent: Option<&str>) -> (PathBuf, PathBuf, PathBuf) {
    let tasks_dir = root.join("tasks");
    let prompts_dir = root.join("prompts");
    let agents_dir = root.join("agents");
    for dir in [&tasks_dir, &prompts_dir, &agents_dir] {
        fs::create_dir_all(dir).expect("create fixture dir");
    }
    fs::copy(
        m5_dir().join("tasks").join("01-install-cli.task.toml"),
        tasks_dir.join("01-install-cli.task.toml"),
    )
    .expect("copy task record");
    fs::copy(
        m5_dir().join("prompts").join("01-install-cli.txt"),
        prompts_dir.join("01-install-cli.txt"),
    )
    .expect("copy prompt");
    match agent {
        Some(body) => {
            fs::write(agents_dir.join("01-install-cli.sh"), body).expect("write agent");
        }
        None => {
            fs::copy(
                m5_dir().join("agents").join("01-install-cli.sh"),
                agents_dir.join("01-install-cli.sh"),
            )
            .expect("copy agent");
        }
    }
    (tasks_dir, prompts_dir, agents_dir)
}

/// The stored `summary.json` of a run, parsed.
fn read_summary(evals_root: &Path) -> serde_json::Value {
    let bytes = fs::read(evals_root.join("summary.json")).expect("read summary.json");
    serde_json::from_slice(&bytes).expect("parse summary.json")
}

/// The stored `report.json` of a run, parsed.
fn read_report(evals_root: &Path) -> serde_json::Value {
    let bytes = fs::read(evals_root.join("report.json")).expect("read report.json");
    serde_json::from_slice(&bytes).expect("parse report.json")
}

/// Find the first stored artifact whose bytes contain `needle`.
fn find_artifact(evals_root: &Path, needle: &str) -> String {
    let root = evals_root.join("artifacts").join("sha256");
    let mut found: Option<String> = None;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            if text.contains(needle) {
                found = Some(text.into_owned());
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    found.unwrap_or_else(|| panic!("no stored artifact contains `{needle}`"))
}

#[test]
fn a_trial_that_reaches_the_host_is_invalidated() {
    let root = unique_dir("leak");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&root, Some(LEAKING_AGENT));

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.repetitions = 1;
    config.treatment_skill = None;

    // The run fails closed: the contamination is reported after the report,
    // the summary and the sealed store are written.
    let error = run_feasibility_matrix(&config).expect_err("the leak must fail the run closed");
    assert!(
        error.to_string().contains("host_leakage_clean=false"),
        "the error must name the host-leakage check: {error}"
    );

    // The trial is INVALID — never a pass and never a fail.
    let summary = read_summary(&evals_root);
    let trials = summary["trials"].as_array().expect("trials array");
    assert_eq!(trials.len(), 2, "{summary:#?}");
    for trial in trials {
        assert_eq!(
            trial["outcome_class"], "host_contamination",
            "the leaked trial must be invalid: {trial:#?}"
        );
        assert_eq!(
            trial["host_leakage"]["clean"], false,
            "the trial must carry the scan result: {trial:#?}"
        );
        let findings = trial["host_leakage"]["findings"]
            .as_array()
            .expect("findings array");
        assert!(!findings.is_empty(), "{trial:#?}");
        let finding = &findings[0];
        assert_eq!(finding["kind"], "symlink_outside_root", "{finding:#?}");
        assert_eq!(finding["path"], "stub-bin/rg", "{finding:#?}");
    }

    // The run-level contamination aggregate names the finding.
    assert_eq!(
        summary["contamination"]["host_leakage_clean"], false,
        "{summary:#?}"
    );
    let run_findings = summary["contamination"]["host_leakage_findings"]
        .as_array()
        .expect("run findings array");
    assert_eq!(run_findings.len(), 2, "{run_findings:#?}");
    assert!(
        run_findings[0]
            .as_str()
            .expect("finding token")
            .contains("symlink_outside_root"),
        "{run_findings:#?}"
    );

    // The report excludes the block and names the reason.
    let report = read_report(&evals_root);
    assert_eq!(report["valid_blocks"], 0, "{report:#?}");
    let invalid_blocks = report["invalid_blocks"].as_array().expect("invalid blocks");
    assert_eq!(invalid_blocks.len(), 1, "{invalid_blocks:#?}");
    let reason = invalid_blocks[0]["reason"].as_str().expect("reason");
    assert!(
        reason.contains("host_contamination"),
        "the invalid block must name the contamination: {reason}"
    );
    assert!(
        reason.contains("symlink_outside_root"),
        "the invalid block must name the finding: {reason}"
    );

    // The limitations name the contaminated trials.
    let limitations = report["limitations"].as_array().expect("limitations");
    assert!(
        limitations
            .iter()
            .filter_map(|line| line.as_str())
            .any(|line| line.contains("host contamination: 2 trial(s) invalidated")),
        "{limitations:#?}"
    );

    // The finding is in the trial's sealed evidence.
    let artifact = find_artifact(&evals_root, "contamination-report.v1");
    assert!(artifact.contains("symlink_outside_root"), "{artifact}");
    assert!(artifact.contains("stub-bin/rg"), "{artifact}");

    // The contaminated trial tree is preserved for inspection.
    let trials_dir = evals_root
        .join("scratch")
        .join(EXPERIMENT_ID)
        .join("trials");
    let preserved: Vec<PathBuf> = fs::read_dir(&trials_dir)
        .expect("read trials dir")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(preserved.len(), 2, "{preserved:#?}");

    fs::remove_dir_all(&root).expect("remove temp root");
}

#[test]
fn a_clean_trial_is_not_flagged() {
    let root = unique_dir("clean");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&root, None);

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.repetitions = 1;
    config.treatment_skill = None;

    let summary = run_feasibility_matrix(&config).expect("the clean run must succeed");
    assert_eq!(summary.completed_trials, 2);
    assert!(summary.contamination.host_leakage_clean, "{summary:#?}");
    assert!(summary.contamination.host_leakage_findings.is_empty());
    for trial in &summary.trials {
        assert!(trial.host_leakage.clean, "{trial:#?}");
        assert!(trial.host_leakage.findings.is_empty(), "{trial:#?}");
        // The calibrated outcomes are unchanged: the baseline installs with
        // brew, the treatment follows the Root protocol.
        let expected = if trial.condition_id == "baseline" {
            "completed_failed"
        } else {
            "completed_passed"
        };
        assert_eq!(trial.outcome_class, expected, "{trial:#?}");
        assert_ne!(
            TrialOutcomeClass::from_token(&trial.outcome_class),
            Some(TrialOutcomeClass::HostContamination)
        );
    }

    // The report's limitations state that no host leakage was detected.
    let report = read_report(&evals_root);
    let limitations = report["limitations"].as_array().expect("limitations");
    assert!(
        limitations
            .iter()
            .filter_map(|line| line.as_str())
            .any(|line| line.starts_with("host contamination: none detected")),
        "{limitations:#?}"
    );

    fs::remove_dir_all(&root).expect("remove temp root");
}
