//! Milestone 6 P0 fix: the **harness-aware permission seeding**.
//!
//! The defect: the materializer seeded `{"permission":{"*":{"*":"ask"}}}` into
//! *every* trial. With the real OpenCode harness in non-interactive mode
//! every tool call is then ASKED and immediately AUTO-REJECTED (M0
//! UPSTREAM-CONTRACT §5.1), so the agent cannot execute anything at all —
//! the trial would measure a harness artifact instead of the agent's
//! behaviour. The successful real probe run (Milestone 6 integration probe,
//! Task B) used opencode's *default* allow-all.
//!
//! What this suite proves:
//!
//! - the materializer seeds exactly the posture it is asked for
//!   ([`PermissionSeeding::AskMode`] / [`PermissionSeeding::AllowAll`]);
//! - the **fake**-agent path keeps the ask-mode seed (its denial fixtures
//!   depend on it) and records it;
//! - the **real** path seeds allow-all, the seeded set is what the trial
//!   really ran with (the stub CLI reads the project config back), and the
//!   effective set is recorded with the trial's evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    file_sha256, materialize_trial_with_fake_tools, run_feasibility_matrix, ConditionOverlay,
    ContextRoots, DiscoveryFlags, EffectiveContextManifest, FakeToolBinding, HarnessChoice,
    PermissionSeeding, ProfileId, RealHarness, RealHarnessConfig, RepositoryLock, TrialSpec,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const EXPERIMENT_ID: &str = "m6-permission-seeding";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m6-perm-{prefix}-{}-{id}",
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

fn git(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn fake_digest(seed: &str) -> String {
    format!("sha256:{}", root_lockfile::compute_sha256(seed.as_bytes()))
}

fn init_source_repo() -> (PathBuf, String) {
    let dir = unique_dir("source");
    fs::write(dir.join("README.md"), "# fixture\n").expect("write README");
    git(&["init"], &dir);
    git(&["add", "."], &dir);
    git(
        &[
            "-c",
            "user.name=trial-test",
            "-c",
            "user.email=trial-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
        &dir,
    );
    let commit = git(&["rev-parse", "HEAD"], &dir);
    (dir, commit)
}

fn resolve_base_manifest() -> EffectiveContextManifest {
    let base = unique_dir("base-tree");
    for dir in ["home/config", "project"] {
        fs::create_dir_all(base.join(dir)).expect("create base dirs");
    }
    let home = base.join("home");
    let xdg = base.join("home/config");
    let project = base.join("project");
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    root_eval::resolve_manifest(&roots, &DiscoveryFlags::default()).expect("resolve fixture")
}

fn make_spec(trial_id: &str, source: &Path, commit: &str) -> TrialSpec {
    TrialSpec {
        trial_id: trial_id.to_string(),
        experiment_id: EXPERIMENT_ID.to_string(),
        task_id: "install-ripgrep-001".to_string(),
        task_version: 1,
        condition_id: "treatment".to_string(),
        manifest: resolve_base_manifest(),
        overlay: ConditionOverlay { entries: vec![] },
        plan_hash: fake_digest("plan"),
        repository: RepositoryLock {
            commit: commit.to_string(),
            tree_sha256: fake_digest("tree"),
        },
        worktree_source: source.to_path_buf(),
    }
}

/// The seeded project config of a materialized trial.
fn seeded_config(layout: &root_eval::TrialLayout) -> String {
    fs::read_to_string(layout.repo_dir.join(".opencode").join("opencode.json"))
        .expect("read the seeded project config")
}

/// The first stored artifact whose bytes contain `needle`.
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

/// The four shared prewarm assets (the real ones are ~155 MB; the test
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
    // The npm layout carries **symlinks** in `node_modules/.bin` — the real
    // prewarm tree has seven of them. They are what made the fail-closed
    // tree snapshot refuse the real-harness trial; the driver excludes the
    // seeded plugin tree from every workspace observation, so the trial must
    // still complete with them in place.
    let bin_dir = root
        .join("config-template")
        .join("opencode")
        .join("node_modules")
        .join(".bin");
    fs::create_dir_all(&bin_dir).expect("plugin bin dir");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "../@opencode-ai/plugin/package.json",
        bin_dir.join("plugin-bin"),
    )
    .expect("plugin bin symlink");

    fs::create_dir_all(root.join("models")).expect("models dir");
    fs::write(
        root.join("models").join("models.json"),
        b"{\"providers\":[]}",
    )
    .expect("models file");

    fs::create_dir_all(root.join("rg")).expect("rg dir");
    write_executable(
        &root.join("rg").join("rg"),
        "#!/bin/sh\nprintf 'rg 15.1.0 (trial-local stub)\\n'\n",
    );
    root.to_path_buf()
}

/// The test double for `opencode run --format json`.
///
/// It reads the trial's **project config** back (the cwd *is* the trial
/// repository) and reports it in a `tool_use` event, so the test can prove
/// which permission set the trial really ran with — not merely which one the
/// driver intended. It then drives the fake-tool protocol through the trial
/// `PATH` and closes with a final assistant text plus a `step_finish`.
const CONFIG_REPORTING_STUB: &str = r#"#!/bin/sh
# Test double for `opencode run --format json` (Milestone 6, no model call).
set -u

session=ses_stub_perm_01

# The project config of the trial, read back with shell builtins only (the
# trial PATH carries no host binaries).
_config=
if [ -f .opencode/opencode.json ]; then
  while IFS= read -r _line || [ -n "$_line" ]; do
    _config="$_config$_line"
  done < .opencode/opencode.json
fi

printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":1,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-start\"}}"
printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":2,\"sessionID\":\"$session\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_config\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"cat .opencode/opencode.json\"},\"output\":\"project_config=$_config\"}}}"

root doctor --json >/dev/null 2>&1
root plan install ripgrep --json >/dev/null 2>&1
root install ripgrep --json >/dev/null 2>&1
root verify ripgrep --json >/dev/null 2>&1

printf '%s\n' "{\"type\":\"text\",\"timestamp\":4,\"sessionID\":\"$session\",\"part\":{\"type\":\"text\",\"text\":\"Installed ripgrep via Root. Rollback is available via root rollback --last --json.\"}}"
printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":5,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\",\"tokens\":{\"total\":1200,\"input\":1100,\"output\":60,\"reasoning\":40,\"cache\":{\"write\":0,\"read\":0}},\"cost\":0}}"
"#;

/// Copy the M5 task 01 record, prompt, and fake-agent script into a scratch
/// fixture tree.
fn write_fixtures(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let tasks_dir = root.join("fixtures").join("tasks");
    let prompts_dir = root.join("fixtures").join("prompts");
    let agents_dir = root.join("fixtures").join("agents");
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

// ---------------------------------------------------------------------------
// 1. The materializer seeds exactly the posture it is asked for
// ---------------------------------------------------------------------------

#[test]
fn materializer_seeds_the_requested_permission_set() {
    let base_dir = unique_dir("seed-base");
    let (source, commit) = init_source_repo();
    let bindings = vec![FakeToolBinding::new(
        "root",
        ProfileId::RootHealthyV1.token(),
        Some(root_eval::script_sha256(ProfileId::RootHealthyV1)),
    )];

    let ask = make_spec("trial-ask", &source, &commit);
    let ask_layout =
        materialize_trial_with_fake_tools(&ask, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect("ask-mode materialization");
    assert_eq!(
        seeded_config(&ask_layout),
        "{\"permission\":{\"*\":{\"*\":\"ask\"}}}\n",
        "the fake path keeps the ask-mode seed"
    );
    assert_eq!(ask_layout.permission_seeding, PermissionSeeding::AskMode);
    assert_eq!(
        ask_layout.effective_permission,
        PermissionSeeding::AskMode.effective_config()
    );

    let allow = make_spec("trial-allow", &source, &commit);
    let allow_layout = materialize_trial_with_fake_tools(
        &allow,
        &base_dir,
        &bindings,
        PermissionSeeding::AllowAll,
    )
    .expect("allow-all materialization");
    assert_eq!(
        seeded_config(&allow_layout),
        "{\"permission\":{\"*\":{\"*\":\"allow\"}}}\n",
        "the real path seeds an explicit allow-all set"
    );
    assert_eq!(allow_layout.permission_seeding, PermissionSeeding::AllowAll);
    assert_eq!(
        allow_layout.effective_permission,
        PermissionSeeding::AllowAll.effective_config()
    );

    // The two postures differ, and neither is the other's default.
    assert_ne!(
        PermissionSeeding::AskMode.effective_config(),
        PermissionSeeding::AllowAll.effective_config()
    );
    assert_eq!(
        PermissionSeeding::from_token("ask_mode"),
        Some(PermissionSeeding::AskMode)
    );
    assert_eq!(
        PermissionSeeding::from_token("allow_all"),
        Some(PermissionSeeding::AllowAll)
    );
    assert_eq!(PermissionSeeding::from_token("nope"), None);

    fs::remove_dir_all(&base_dir).expect("remove temp root");
}

// ---------------------------------------------------------------------------
// 2. The fake path keeps ask-mode and records it
// ---------------------------------------------------------------------------

#[test]
fn fake_harness_seeds_ask_mode_and_records_it() {
    let root = unique_dir("fake");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = write_fixtures(&root);

    let mut config = root_eval::FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        EXPERIMENT_ID,
    );
    config.repetitions = 1;
    config.treatment_skill = None;

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.completed_trials, 2);
    assert_eq!(summary.harness_name, "m5-fake-agent");
    assert_eq!(
        summary.permission_seeding, "ask_mode",
        "the fake path keeps the ask-mode seed"
    );
    for trial in &summary.trials {
        assert_eq!(trial.permission_seeding, "ask_mode", "{trial:#?}");
        assert_eq!(
            trial.effective_permission, "{\"permission\":{\"*\":{\"*\":\"ask\"}}}",
            "{trial:#?}"
        );
    }

    // The recorded artifact carries the same effective set.
    let artifact = find_artifact(&evals_root, "permission-config.v1");
    assert!(artifact.contains("\"seeding\":\"ask_mode\""), "{artifact}");
    assert!(
        artifact.contains("\"effective_permission\":{\"permission\":{\"*\":{\"*\":\"ask\"}}}"),
        "{artifact}"
    );

    fs::remove_dir_all(&root).expect("remove temp root");
}

// ---------------------------------------------------------------------------
// 3. The real path seeds allow-all, really runs with it, and records it
// ---------------------------------------------------------------------------

#[test]
fn real_harness_seeds_allow_all_and_records_it() {
    let root = unique_dir("real");
    let evals_root = root.join("evals");
    let (tasks_dir, prompts_dir, agents_dir) = write_fixtures(&root);
    let prewarm_root = write_prewarm(&root.join("prewarm"));
    let binary = write_executable(
        &root.join("bin").join("opencode-stub"),
        CONFIG_REPORTING_STUB,
    );

    let digest = file_sha256(&binary).expect("digest");
    let harness = RealHarness::open_with_expected_digest(
        RealHarnessConfig::new(&binary),
        &digest,
        "stub-1.18.27",
    )
    .expect("the test double opens");

    let mut config = root_eval::FeasibilityConfig::new(
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
    assert_eq!(summary.harness_name, "opencode");
    assert_eq!(
        summary.permission_seeding, "allow_all",
        "the real path must seed an explicit allow-all set"
    );
    for trial in &summary.trials {
        assert_eq!(trial.permission_seeding, "allow_all", "{trial:#?}");
        // The seeded plugin tree carries npm symlinks: the trial must still
        // complete, which proves the driver excludes the harness's own
        // subtree from every workspace observation (the fail-closed snapshot
        // and fingerprint would otherwise refuse it).
        assert_eq!(trial.termination, "completed", "{trial:#?}");
        assert_ne!(
            trial.outcome_class, "invalid_infrastructure",
            "the seeded plugin tree must not invalidate the trial: {trial:#?}"
        );
        assert_ne!(
            trial.outcome_class, "verifier_mutation",
            "the harness's own npm activity must not read as a mutation: {trial:#?}"
        );
        assert_eq!(
            trial.effective_permission, "{\"permission\":{\"*\":{\"*\":\"allow\"}}}",
            "{trial:#?}"
        );
    }

    // The trial really ran with the allow-all set: the stub CLI read the
    // project config back out of the trial repository.
    let dump = find_artifact(&evals_root, "project_config=");
    assert!(
        dump.contains("project_config={\"permission\":{\"*\":{\"*\":\"allow\"}}}"),
        "the trial must have run with the allow-all permission set: {dump}"
    );
    assert!(
        !dump.contains("\"ask\""),
        "no ask-mode permission may reach the real harness: {dump}"
    );

    // The recorded artifact carries the same effective set.
    let artifact = find_artifact(&evals_root, "permission-config.v1");
    assert!(artifact.contains("\"seeding\":\"allow_all\""), "{artifact}");
    assert!(
        artifact.contains("\"effective_permission\":{\"permission\":{\"*\":{\"*\":\"allow\"}}}"),
        "{artifact}"
    );

    fs::remove_dir_all(&root).expect("remove temp root");
}
