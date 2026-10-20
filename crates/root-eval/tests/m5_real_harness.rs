//! Milestone 6: the real OpenCode harness path, end to end, with a stub binary.
//!
//! No real model call is ever made: the "CLI" is a test double that mimics
//! `opencode run --format json` (golden-shaped NDJSON on stdout, the fake
//! tool protocol on the trial `PATH`, and a provider marker in the trial's
//! OpenCode log when asked to stall).
//!
//! What the suite proves:
//!
//! - the **binary-hash gate** fails closed on a foreign binary and opens the
//!   harness only when the caller states the expectation (the documented
//!   test-double seam);
//! - the **per-trial environment** is hermetic: the child sees only the
//!   documented variables, the `PATH` holds the trial's fake-bin and stub-bin
//!   and nothing else, the host's `rg` is unreachable, and the driver's
//!   canary never crosses into the trial;
//! - the **verifier environment** carries no host `PATH`: the record's
//!   `sh verify.sh` resolves through the trial-local staged shell and the
//!   verifier's own `PATH` has exactly the two trial directories;
//! - the **retry policy** is exercised twice — a provider stall and a wall
//!   timeout — each retried exactly once with a fresh materialized trial;
//! - the **fake path** still passes with the new hermetic verifier
//!   environment.
//!
//! `ROOT_DIR` is never set; every tree lives under a unique
//! `std::env::temp_dir()` subdirectory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    file_sha256, reproduce_feasibility, run_feasibility_matrix, seal_draft_at, sha256_digest,
    EvalStore, FeasibilityConfig, HarnessChoice, RealHarness, RealHarnessConfig, RealHarnessError,
    RealHarnessErrorKind, TaskDraft, TrialOutcomeClass,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const EXPERIMENT_ID: &str = "m6-real-harness";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m6-real-{prefix}-{}-{id}",
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

/// Build the four shared prewarm assets (the real ones are ~155 MB; the
/// test double's are a few bytes).
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
/// It records what the child actually saw (the environment variable names,
/// the `PATH` shape, the `rg`/`brew`/`root` resolutions) into one
/// `tool_use` event, drives the fake-tool protocol through the trial `PATH`,
/// and closes with a final assistant text plus a `step_finish`.
const GOOD_STUB: &str = r#"#!/bin/sh
# Test double for `opencode run --format json` (Milestone 6, no model call).
set -u

session=ses_stub_real_01

_model=
_format=
_prev=
for _arg in "$@"; do
  if [ "$_prev" = "--model" ]; then _model=$_arg; fi
  if [ "$_prev" = "--format" ]; then _format=$_arg; fi
  _prev=$_arg
done
_last=
for _arg in "$@"; do _last=$_arg; done

_path_entries=0
_remaining=$PATH
while [ -n "$_remaining" ]; do
  _path_entries=$((_path_entries+1))
  case "$_remaining" in
    *:*) _remaining=${_remaining#*:} ;;
    *) _remaining= ;;
  esac
done

_names=
if [ -x /usr/bin/env ]; then
  /usr/bin/env > "$TMPDIR/stub-env.txt"
  while IFS= read -r _line; do
    _name=${_line%%=*}
    _names="$_names$_name,"
  done < "$TMPDIR/stub-env.txt"
fi

_canary=absent
if [ -n "${M5_CANARY_SECRET:-}" ]; then _canary=present; fi

_rg=$(command -v rg 2>/dev/null || printf 'NOTFOUND')
_brew=$(command -v brew 2>/dev/null || printf 'NOTFOUND')
_root=$(command -v root 2>/dev/null || printf 'NOTFOUND')
_prompt=no
if [ -n "$_last" ]; then _prompt=yes; fi

printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":1,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-start\"}}"
printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":2,\"sessionID\":\"$session\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_env\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"env\"},\"output\":\"model=$_model format=$_format prompt=$_prompt path_entries=$_path_entries env_names=$_names canary=$_canary rg=$_rg brew=$_brew root=$_root home=$HOME xdg_config=$XDG_CONFIG_HOME npmcache=$npm_config_cache\"}}}"

root doctor --json >/dev/null 2>&1
root plan install ripgrep --json >/dev/null 2>&1
root install ripgrep --json >/dev/null 2>&1
root verify ripgrep --json >/dev/null 2>&1

printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":3,\"sessionID\":\"$session\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_install\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"root install ripgrep --json\"},\"output\":\"installed\"}}}"
printf '%s\n' "{\"type\":\"text\",\"timestamp\":4,\"sessionID\":\"$session\",\"part\":{\"type\":\"text\",\"text\":\"Installed ripgrep via Root. Rollback is available via root rollback --last --json.\"}}"
printf '%s\n' "{\"type\":\"step_finish\",\"timestamp\":5,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\",\"tokens\":{\"total\":1200,\"input\":1100,\"output\":60,\"reasoning\":40,\"cache\":{\"write\":0,\"read\":0}},\"cost\":0}}"
"#;

/// The test double for a **provider stall**: the CLI finishes with exit code
/// 0 and a truncated stream, and the provider error shows up only in the
/// trial's OpenCode log — exactly what the M6 probe observed.
const PROVIDER_STALL_STUB: &str = r#"#!/bin/sh
# Test double for a provider stall (Milestone 6, no model call).
set -u

if [ -x /bin/mkdir ]; then
  /bin/mkdir -p "$XDG_DATA_HOME/opencode/log"
  printf '%s\n' 'timestamp=2026-10-19T21:57:54.856Z level=ERROR run=ad0687e6 message="stream error" providerID=opencode modelID=big-pickle session.id=ses_stub_stall error.error="ProviderHeaderTimeoutError: Provider response headers timed out after 300000ms"' > "$XDG_DATA_HOME/opencode/log/opencode.log"
fi

session=ses_stub_stall
printf '%s\n' "{\"type\":\"step_start\",\"timestamp\":1,\"sessionID\":\"$session\",\"part\":{\"type\":\"step-start\"}}"
printf '%s\n' "{\"type\":\"tool_use\",\"timestamp\":2,\"sessionID\":\"$session\",\"part\":{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_probe\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"root doctor --json\"},\"output\":\"ok\"}}}"
"#;

/// The test double for a wall timeout: it never finishes.
const HANGING_STUB: &str = r#"#!/bin/sh
# Test double for a wall timeout (Milestone 6, no model call).
set -u
printf '%s\n' '{"type":"step_start","timestamp":1,"sessionID":"ses_stub_hang","part":{"type":"step-start"}}'
exec /bin/sleep 30
"#;

/// The stdout the patched verifier prints (trial-independent, so the
/// record can declare its digest).
const VERIFIER_STDOUT: &str = "verifier-path-entries=2\nall checks passed\n";

/// The task record used by the suite: the M5 calibration task with a
/// verifier that proves — and reports — the hermetic verifier environment.
fn write_task_record(tasks_dir: &Path, wall_seconds: u32) -> PathBuf {
    fs::create_dir_all(tasks_dir).expect("create tasks dir");
    let source = fs::read_to_string(m5_dir().join("tasks").join("01-install-cli.task.toml"))
        .expect("read the M5 task record");
    // Patch only the `verify.sh` fixture (the last fixture block): the
    // `echo all checks passed` line also appears in `test/run-tests.sh`,
    // whose digest must stay untouched.
    let marker = "# Post-agent dependency check";
    let split = source
        .find(marker)
        .expect("the M5 record carries the verify.sh fixture");
    let (head, tail) = source.split_at(split);
    let patched_tail = tail
        .replace(
            "echo all checks passed",
            // The verifier proves the hermetic PATH: exactly two entries,
            // the trial's fake-bin and stub-bin, and nothing else.
            "_entries=0\n\
             _rest=$PATH\n\
             while [ -n \"$_rest\" ]; do\n\
             \x20 _entries=$((_entries+1))\n\
             \x20 case \"$_rest\" in\n\
             \x20   *:*) _rest=${_rest#*:} ;;\n\
             \x20   *) _rest= ;;\n\
             \x20 esac\n\
             done\n\
             [ \"$_entries\" = 2 ] || exit 1\n\
             case \"$PATH\" in\n\
             \x20 */fake-bin:*/stub-bin) ;;\n\
             \x20 *) exit 1 ;;\n\
             esac\n\
             command -v rg >/dev/null 2>&1 || { echo \"missing dependency: rg\" >&2; exit 1; }\n\
             rg --version >/dev/null\n\
             echo \"verifier-path-entries=$_entries\"\n\
             echo all checks passed",
        )
        .replace(
            "content_sha256 = \"sha256:dd3a0b9fc583c7792a9232cd12c23cf548a6bf4d246f26bb9b1b797b4373d760\"",
            "content_sha256 = \"sha256:PENDING\"",
        )
        .replace(
            "stdout_sha256 = \"sha256:f070c33a000f06c7facd7c86e2e60eabab2c8f8a1373604b166d970fb2482ad5\"",
            &format!(
                "stdout_sha256 = \"{}\"",
                sha256_digest(VERIFIER_STDOUT.as_bytes())
            ),
        );
    let patched = format!("{head}{patched_tail}").replace(
        "wall_time_seconds_max = 900",
        &format!("wall_time_seconds_max = {wall_seconds}"),
    );
    let draft = TaskDraft::parse_str(&patched).expect("the patched record parses as a draft");
    let sealed = seal_draft_at(&draft, tasks_dir).expect("seal the patched record");
    let path = tasks_dir.join("01-install-cli.task.toml");
    fs::write(&path, sealed).expect("write the sealed record");
    path
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

/// The value of `name=` inside a space-separated dump line.
fn field(dump: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let start = dump
        .find(&needle)
        .unwrap_or_else(|| panic!("the dump has no `{name}` field: {dump}"))
        + needle.len();
    let rest = &dump[start..];
    let end = rest
        .find(|character: char| character.is_whitespace() || character == '"')
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The per-trial environment the real harness must produce.
const EXPECTED_ENV_NAMES: [&str; 12] = [
    "HOME",
    "OPENCODE_DB",
    "OPENCODE_DISABLE_AUTOUPDATE",
    "PATH",
    "TRIAL_AUDIT_LOG",
    "TRIAL_STUB_BIN",
    "TMPDIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "npm_config_cache",
];

/// Shell variables a POSIX shell may export on its own.
const SHELL_OWNED_ENV_NAMES: [&str; 4] = ["_", "OLDPWD", "PWD", "SHLVL"];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn binary_hash_gate_fails_closed_on_a_foreign_binary() {
    let root = unique_dir("gate");
    let binary = write_executable(&root.join("opencode-stub"), "#!/bin/sh\nexit 0\n");

    // The pinned expectation refuses the stub.
    let error = RealHarness::open(RealHarnessConfig::new(&binary))
        .expect_err("a foreign binary must be refused");
    assert_eq!(error.kind(), RealHarnessErrorKind::BinaryDigestMismatch);
    assert!(matches!(
        error,
        RealHarnessError::BinaryDigestMismatch { .. }
    ));

    // The documented test-double seam opens it when the caller states the
    // expectation explicitly.
    let digest = file_sha256(&binary).expect("digest");
    let harness = RealHarness::open_with_expected_digest(
        RealHarnessConfig::new(&binary),
        &digest,
        "stub-1.18.27",
    )
    .expect("the test double opens");
    assert_eq!(harness.identity().version, "stub-1.18.27");
    assert_eq!(harness.model(), "opencode/big-pickle");

    fs::remove_dir_all(&root).expect("remove temp root");
}

#[test]
fn real_harness_path_runs_the_matrix_in_a_hermetic_environment() {
    let root = unique_dir("matrix");
    let evals_root = root.join("evals");
    let tasks_dir = root.join("fixtures").join("tasks");
    let prompts_dir = root.join("fixtures").join("prompts");
    let agents_dir = root.join("fixtures").join("agents");
    fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    fs::create_dir_all(&agents_dir).expect("create agents dir");
    let prewarm_root = write_prewarm(&root.join("prewarm"));
    let binary = write_executable(&root.join("bin").join("opencode-stub"), GOOD_STUB);

    write_task_record(&tasks_dir, 900);

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
    config.prewarm_root = Some(prewarm_root.clone());
    config.real_harness_override = Some(harness);
    config.repetitions = 1;
    // The marker treatment skill keeps the suite self-contained.
    config.treatment_skill = None;

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.planned_trials, 2, "{summary:#?}");
    assert_eq!(summary.completed_trials, 2);
    assert_eq!(summary.harness_name, "opencode");
    assert_eq!(summary.harness_version, "stub-1.18.27");

    for trial in &summary.trials {
        assert_eq!(trial.outcome_class, "completed_passed", "{trial:#?}");
        assert_eq!(trial.termination, "completed");
        assert_eq!(trial.attempts, 1, "no retry for a clean run");
        assert_eq!(trial.retry_reason, None);
        assert_eq!(trial.provider_error, None);
        assert_eq!(trial.requested_model, "opencode/big-pickle");
        assert_eq!(
            trial.observed_model, None,
            "the observed model stays unavailable"
        );
        assert_eq!(trial.turn_state, "completed");
        assert!(trial.acceptance_all_primary_passed, "{trial:#?}");
        assert!(trial.canary.clean);
    }

    // The store carries the requested model and no observed model.
    let store = EvalStore::open_read_only(&evals_root, EXPERIMENT_ID).expect("open store");
    let trials = store.list_trials(EXPERIMENT_ID).expect("list trials");
    assert_eq!(trials.len(), 2);
    for trial in &trials {
        assert_eq!(
            trial.outcome_class,
            Some(TrialOutcomeClass::CompletedPassed)
        );
        let executions = store
            .executions_for_trial(&trial.trial_id)
            .expect("executions for trial");
        assert_eq!(executions.len(), 1);
        for execution in &executions {
            assert_eq!(execution.requested_model, "opencode/big-pickle");
            assert_eq!(execution.observed_model, None);
            assert_eq!(execution.termination.as_str(), "completed");
        }
    }
    drop(store);

    // The report builds and reproduces byte-identically from sealed evidence.
    let outcome = reproduce_feasibility(&evals_root, EXPERIMENT_ID).expect("reproduce");
    assert!(outcome.store_integrity_intact, "{outcome:#?}");
    assert!(outcome.report_byte_identical);

    // The child's environment: hermetic, trial-local, and free of the host.
    let dump = find_artifact(&evals_root, "env_names=");
    assert_eq!(field(&dump, "model"), "opencode/big-pickle", "{dump}");
    assert_eq!(field(&dump, "format"), "json", "{dump}");
    assert_eq!(field(&dump, "prompt"), "yes", "{dump}");
    assert_eq!(
        field(&dump, "path_entries"),
        "2",
        "the trial PATH holds exactly the fake-bin and the stub-bin: {dump}"
    );
    assert_eq!(
        field(&dump, "rg"),
        "NOTFOUND",
        "the host `rg` must be unreachable from the trial: {dump}"
    );
    assert_eq!(
        field(&dump, "canary"),
        "absent",
        "the driver canary must not cross into the trial: {dump}"
    );
    assert!(
        field(&dump, "brew").ends_with("/fake-bin/brew"),
        "the fake shim must be the only `brew`: {dump}"
    );
    assert!(
        field(&dump, "root").ends_with("/fake-bin/root"),
        "the fake shim must be the only `root`: {dump}"
    );
    assert!(
        field(&dump, "home").ends_with("/home") && field(&dump, "home").contains("/trials/"),
        "the trial HOME lives inside the trial: {dump}"
    );
    assert!(
        field(&dump, "xdg_config").ends_with("/xdg-config"),
        "{dump}"
    );
    assert_eq!(
        field(&dump, "npmcache"),
        prewarm_root
            .join("npm-cache")
            .to_string_lossy()
            .into_owned(),
        "the shared warm npm cache must be the prewarm cache: {dump}"
    );
    for name in EXPECTED_ENV_NAMES {
        assert!(
            field(&dump, "env_names").contains(&format!("{name},")),
            "the trial env must carry `{name}`: {dump}"
        );
    }
    // No host variable beyond the documented set and the shell's own.
    let env_names = field(&dump, "env_names");
    let names: Vec<&str> = env_names
        .trim_end_matches(',')
        .split(',')
        .filter(|name| !name.is_empty())
        .collect();
    for name in &names {
        assert!(
            EXPECTED_ENV_NAMES.contains(name) || SHELL_OWNED_ENV_NAMES.contains(name),
            "unexpected environment variable `{name}` in the trial: {dump}"
        );
    }

    // The verifier ran without the host PATH: the staged shell resolved
    // `sh`, and the verifier itself proves the hermetic PATH (it exits
    // non-zero unless the PATH holds exactly the trial's fake-bin and
    // stub-bin) and reports the entry count.
    let verifier = find_artifact(&evals_root, "verifier-path-entries=");
    assert_eq!(verifier, VERIFIER_STDOUT, "{verifier}");
    assert!(
        !verifier.contains("/usr/bin") && !verifier.contains("/bin:"),
        "the host PATH must not reach the verifier: {verifier}"
    );

    fs::remove_dir_all(&root).expect("remove temp root");
}

#[test]
fn provider_stall_is_classified_and_retried_once() {
    let root = unique_dir("provider");
    let evals_root = root.join("evals");
    let tasks_dir = root.join("fixtures").join("tasks");
    let prompts_dir = root.join("fixtures").join("prompts");
    let agents_dir = root.join("fixtures").join("agents");
    fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    fs::create_dir_all(&agents_dir).expect("create agents dir");
    let prewarm_root = write_prewarm(&root.join("prewarm"));
    let binary = write_executable(&root.join("bin").join("opencode-stub"), PROVIDER_STALL_STUB);
    write_task_record(&tasks_dir, 900);

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
        assert_eq!(trial.termination, "provider_error");
        assert_eq!(trial.outcome_class, "provider_error");
        assert_eq!(
            trial.retry_reason.as_deref(),
            Some("provider_header_timeout"),
            "{trial:#?}"
        );
        assert_eq!(
            trial.provider_error.as_deref(),
            Some("ProviderHeaderTimeoutError"),
            "{trial:#?}"
        );
    }

    fs::remove_dir_all(&root).expect("remove temp root");
}

#[test]
fn wall_timeout_is_retried_once_with_a_fresh_trial() {
    let root = unique_dir("timeout");
    let evals_root = root.join("evals");
    let tasks_dir = root.join("fixtures").join("tasks");
    let prompts_dir = root.join("fixtures").join("prompts");
    let agents_dir = root.join("fixtures").join("agents");
    fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    fs::create_dir_all(&agents_dir).expect("create agents dir");
    let prewarm_root = write_prewarm(&root.join("prewarm"));
    let binary = write_executable(&root.join("bin").join("opencode-stub"), HANGING_STUB);
    // A two-second wall budget: the stub sleeps for 30 seconds.
    write_task_record(&tasks_dir, 2);

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
        assert_eq!(trial.attempts, 2, "the truncated stream is retried once");
        assert_eq!(trial.termination, "timeout_killed");
        assert_eq!(trial.outcome_class, "timeout_killed");
        assert_eq!(trial.retry_reason.as_deref(), Some("truncated_stream"));
    }

    fs::remove_dir_all(&root).expect("remove temp root");
}

#[test]
fn fake_path_still_passes_with_the_hermetic_verifier_environment() {
    let root = unique_dir("fake");
    let evals_root = root.join("evals");
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
    assert_eq!(summary.completed_trials, 2);
    assert_eq!(summary.harness_name, "m5-fake-agent");
    for trial in &summary.trials {
        // The calibrated fake agent installs with `brew` in the baseline
        // condition, so the baseline fails the acceptance layer by design.
        let expected = if trial.condition_id == "baseline" {
            "completed_failed"
        } else {
            "completed_passed"
        };
        assert_eq!(trial.outcome_class, expected, "{trial:#?}");
        assert_eq!(trial.attempts, 1);
        assert_eq!(trial.requested_model, "m5-fake-agent/deterministic-v1");
    }

    fs::remove_dir_all(&root).expect("remove temp root");
}
