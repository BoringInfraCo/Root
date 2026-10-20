//! Milestone 6: the trial's auditability facts (smoke-run defects D-d and
//! the trial wall-time cap).
//!
//! What this suite proves:
//!
//! - **every** harness attempt of a trial is recorded — the superseded ones
//!   included — with its index, its terminal classification, the
//!   provider-error marker it hit, and its wall time, both in the trial's
//!   sealed `attempt-log` artifact and in `summary.json`;
//! - the driver-level wall-time cap (`--trial-timeout-seconds`) is
//!   **recorded**: the effective cap is `min(record budget, driver cap)`, it
//!   is hashed into the plan, and it appears in `summary.json` and in the
//!   trial's sealed `trial-policy` artifact;
//! - the trial's effective permission set and the prewarm description are
//!   recorded with every trial.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    file_sha256, run_feasibility_matrix, FeasibilityConfig, HarnessChoice, RealHarness,
    RealHarnessConfig,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const EXPERIMENT_ID: &str = "m6-trial-evidence";

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m6-evidence-{prefix}-{}-{id}",
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

/// Write an executable file (mode `0755`).
fn write_executable(path: &Path, body: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, body).expect("write file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod +x");
    }
    path.to_path_buf()
}

/// Build the four shared prewarm assets (the real ones are ~155 MB; the test
/// double's are a few bytes).
fn write_prewarm(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join("npm-cache")).expect("npm cache");
    fs::write(root.join("npm-cache").join("_cacache"), b"warm").expect("cache bytes");

    let plugin = root
        .join("config-template")
        .join("opencode")
        .join("node_modules")
        .join("@opencode-ai")
        .join("plugin");
    fs::create_dir_all(&plugin).expect("plugin tree");
    fs::write(plugin.join("package.json"), b"{}").expect("plugin file");

    fs::create_dir_all(root.join("models")).expect("models dir");
    fs::write(
        root.join("models").join("models.json"),
        b"{\"providers\":[]}",
    )
    .expect("models");

    fs::create_dir_all(root.join("rg")).expect("rg dir");
    write_executable(
        &root.join("rg").join("rg"),
        "#!/bin/sh\nprintf 'rg 15.1.0 (trial-local stub)\\n'\n",
    );
    root.to_path_buf()
}

/// The test double for a **provider stall**: the CLI finishes with exit code
/// 0 and a truncated stream, and the provider error shows up only in the
/// trial's OpenCode log.
const PROVIDER_STALL_STUB: &str = r#"#!/bin/sh
set -u
if [ -x /bin/mkdir ]; then
  /bin/mkdir -p "$XDG_DATA_HOME/opencode/log"
  printf '%s\n' 'timestamp=2026-10-19T21:57:54.856Z level=ERROR run=ad0687e6 message="stream error" providerID=opencode modelID=big-pickle session.id=ses_stub_stall error.error="ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms"' > "$XDG_DATA_HOME/opencode/log/opencode.log"
fi
session=ses_stub_stall
printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":1,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-start\"}}"
printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":2,\"sessionID\":\"$session\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_probe\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"root doctor --json\"},\"output\":\"ok\"}}}"
"#;

/// Copy the M5 task 01 fixtures into a scratch tree.
fn copy_fixtures(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
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
    fs::copy(
        m5_dir().join("agents").join("01-install-cli.sh"),
        agents_dir.join("01-install-cli.sh"),
    )
    .expect("copy agent");
    (tasks_dir, prompts_dir, agents_dir)
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

// ---------------------------------------------------------------------------
// The attempt record
// ---------------------------------------------------------------------------

#[test]
fn every_attempt_of_a_retried_trial_is_recorded() {
    let root = unique_dir("attempts");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&root);
    let prewarm_root = write_prewarm(&root.join("prewarm"));
    let binary = write_executable(&root.join("bin").join("opencode-stub"), PROVIDER_STALL_STUB);

    let digest = file_sha256(&binary).expect("digest");
    let harness = RealHarness::open_with_expected_digest(
        RealHarnessConfig::new(&binary),
        &digest,
        "stub-1.18.27",
    )
    .expect("the test double opens");

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.harness = HarnessChoice::OpenCodeReal;
    config.real = Some(RealHarnessConfig::new(&binary));
    config.prewarm_root = Some(prewarm_root);
    config.real_harness_override = Some(harness);
    config.repetitions = 1;
    config.treatment_skill = None;

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.completed_trials, 2);
    for trial in &summary.trials {
        assert_eq!(trial.attempts, 2, "the provider stall is retried once");
        assert_eq!(trial.attempt_records.len(), 2, "{trial:#?}");
        let first = &trial.attempt_records[0];
        let second = &trial.attempt_records[1];
        assert_eq!(first.index, 1, "{trial:#?}");
        assert_eq!(second.index, 2, "{trial:#?}");
        assert_eq!(first.termination, "provider_error", "{trial:#?}");
        assert_eq!(
            first.provider_error.as_deref(),
            Some("ProviderHeaderTimeoutError"),
            "{trial:#?}"
        );
        assert_eq!(
            first.retry_reason.as_deref(),
            Some("provider_header_timeout"),
            "{trial:#?}"
        );
        assert!(first.retried, "the first attempt was retried: {trial:#?}");
        assert!(!second.retried, "the second attempt was not: {trial:#?}");
        assert!(
            first.wall_ms > 0,
            "the attempt's wall time is recorded: {trial:#?}"
        );
    }

    // The attempts are in the trial's sealed evidence.
    let artifact = find_artifact(&evals_root, "attempt-log.v1");
    assert!(artifact.contains("provider_error"), "{artifact}");
    assert!(
        artifact.contains("ProviderHeaderTimeoutError"),
        "{artifact}"
    );
    assert!(artifact.contains("\"index\":1"), "{artifact}");
    assert!(artifact.contains("\"index\":2"), "{artifact}");

    // The prewarm description and the effective permission set are recorded
    // per trial.
    for trial in &summary.trials {
        let prewarm = trial.prewarm.as_deref().expect("the prewarm description");
        assert!(prewarm.contains("prewarm_root="), "{prewarm}");
        assert!(prewarm.contains("npm_cache="), "{prewarm}");
        assert_eq!(trial.permission_seeding, "allow_all");
        assert!(
            trial.effective_permission.contains("\"allow\""),
            "{trial:#?}"
        );
    }
    assert!(summary
        .prewarm
        .as_deref()
        .expect("the run-level prewarm description")
        .contains("prewarm_root="));

    let policy = find_artifact(&evals_root, "trial-policy.v1");
    assert!(policy.contains("\"effective_permission\""), "{policy}");
    assert!(policy.contains("\"allow\""), "{policy}");
    assert!(policy.contains("prewarm_root="), "{policy}");

    fs::remove_dir_all(&root).expect("remove temp root");
}

// ---------------------------------------------------------------------------
// The trial wall-time cap
// ---------------------------------------------------------------------------

#[test]
fn the_driver_trial_cap_is_recorded_and_never_exceeds_the_record_budget() {
    let root = unique_dir("cap");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&root);

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.repetitions = 1;
    config.treatment_skill = None;
    // A cap **below** the record's 900 s budget: the driver cap binds.
    config.trial_timeout_seconds = Some(120);

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.trial_timeout_seconds, Some(120));
    for trial in &summary.trials {
        assert_eq!(trial.effective_wall_timeout_seconds, 120, "{trial:#?}");
    }

    let policy = find_artifact(&evals_root, "trial-policy.v1");
    assert!(
        policy.contains("\"driver_trial_timeout_seconds\":120"),
        "{policy}"
    );
    assert!(
        policy.contains("\"effective_wall_timeout_seconds\":120"),
        "{policy}"
    );
    assert!(
        policy.contains("\"record_wall_time_seconds_max\":900"),
        "{policy}"
    );

    // The cap changes the trial semantics, so it changes the plan hash.
    let capped_plan_hash = summary.plan_hash.clone();

    // A cap **above** the record's budget: the record's budget still binds.
    let second_root = unique_dir("cap-2");
    let second_evals = second_root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&second_root);
    let mut second = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &second_evals,
        EXPERIMENT_ID,
    );
    second.repetitions = 1;
    second.treatment_skill = None;
    second.trial_timeout_seconds = Some(1800);
    let second_summary = run_feasibility_matrix(&second).expect("matrix run");
    assert_eq!(second_summary.trial_timeout_seconds, Some(1800));
    for trial in &second_summary.trials {
        assert_eq!(
            trial.effective_wall_timeout_seconds, 900,
            "the record's budget always binds: {trial:#?}"
        );
    }
    assert_ne!(
        second_summary.plan_hash, capped_plan_hash,
        "a different cap must produce a different plan hash"
    );

    // A run without a driver cap keeps the record's budget, and its plan
    // hash differs from the capped run's.
    let third_root = unique_dir("cap-3");
    let third_evals = third_root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&third_root);
    let mut third = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &third_evals,
        EXPERIMENT_ID,
    );
    third.repetitions = 1;
    third.treatment_skill = None;
    let third_summary = run_feasibility_matrix(&third).expect("matrix run");
    assert_eq!(third_summary.trial_timeout_seconds, None);
    for trial in &third_summary.trials {
        assert_eq!(
            trial.effective_wall_timeout_seconds, 900,
            "without a driver cap the record's budget applies: {trial:#?}"
        );
    }
    assert_ne!(third_summary.plan_hash, capped_plan_hash);

    fs::remove_dir_all(&root).expect("remove temp root");
    fs::remove_dir_all(&second_root).expect("remove temp root");
    fs::remove_dir_all(&third_root).expect("remove temp root");
}

// ---------------------------------------------------------------------------
// The fake path records the same facts
// ---------------------------------------------------------------------------

#[test]
fn the_fake_path_records_the_permission_set_and_the_attempt_record() {
    let root = unique_dir("fake");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = copy_fixtures(&root);

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.repetitions = 1;
    config.treatment_skill = None;

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.permission_seeding, "ask_mode");
    assert!(summary.prewarm.is_none(), "the fake path has no prewarm");
    for trial in &summary.trials {
        assert_eq!(trial.permission_seeding, "ask_mode", "{trial:#?}");
        assert!(trial.effective_permission.contains("\"ask\""), "{trial:#?}");
        assert_eq!(trial.attempt_records.len(), 1, "{trial:#?}");
        assert_eq!(trial.attempt_records[0].index, 1);
        assert_eq!(trial.attempt_records[0].termination, "completed");
        assert!(!trial.attempt_records[0].retried);
        assert_eq!(trial.effective_wall_timeout_seconds, 900);
    }

    fs::remove_dir_all(&root).expect("remove temp root");
}
