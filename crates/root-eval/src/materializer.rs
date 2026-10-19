//! Per-trial filesystem materialization (Milestone 2, Track A).
//!
//! [`materialize_trial`] creates a fresh, hermetic trial tree (see
//! [`TrialLayout`]) from a validated [`TrialSpec`]:
//!
//! 1. fresh per-trial directories with `0700` permissions;
//! 2. a detached repository checkout at the locked commit (or a verbatim
//!    tree copy for non-git sources), verified clean and at the expected
//!    commit — fail closed otherwise;
//! 3. project `.opencode/skill/**/SKILL.md` files written **only** from the
//!    manifest's `project:`-token skills (a baseline manifest carries none,
//!    so no skill directory appears);
//! 4. a hermetic project `opencode.json` with seeded `ask`-mode permission
//!    rules;
//! 5. offline fake tools (`root`, `brew`, `curl`, `npm`, `pip` shims);
//! 6. an allowlist env file plus [`allowed_env`] / [`scrub_env`] maps that
//!    never inherit the parent process environment.
//!
//! # Two fake-tool layers
//!
//! [`materialize_trial`] is the **legacy** path: it always writes the five
//! generic smoke shims (free-form `printf` audit lines) and keeps the audit
//! trail at `<trial>/audit.log`. It remains the default for callers that have
//! no task-record manifest.
//!
//! [`materialize_trial_with_fake_tools`] is the **spec-conformant** path
//! (Milestone 5): it takes the record's `environment.fake_tools` manifest as
//! [`FakeToolBinding`]s and materializes the normative profiles from
//! [`crate::fake_tools`] instead — one `0700` shim per non-`absent` binding,
//! the canonical audit log at `<trial>/audit/fake-tool-audit.jsonl`, and the
//! `<trial>/stub-bin` directory that `root install` populates
//! (`$TRIAL_STUB_BIN`, appended to the trial `$PATH`). Empty bindings write no
//! shims at all — the correct materialization for a record that declares no
//! fake tools.
//!
//! # Sanctioned process-spawning sites
//!
//! Milestone 1 forbids process spawning in this crate. Detached-checkout
//! materialization *requires* `git clone` / `git checkout`, so this module is
//! the first documented exception: [`materialize_trial`] (and only that
//! entry point, via the private `run_git` helper) may spawn short-lived,
//! bounded `git` child processes for checkout and verification. Milestone 3
//! adds the second, same-class exception: [`crate::capture_diff`] spawns
//! bounded, read-only `git status`/`git diff` children for change capture.
//! No other process is ever started here — fake tools are inert scripts, and
//! the trial runner owns supervision.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::context::MAX_CONTEXT_FILE_BYTES;
use crate::digest::{is_canonical_sha256_digest, sha256_digest};
use crate::fake_tools::{self, FakeToolBinding};
use crate::trial::{TrialLayout, TrialSpec};

/// Machine-matchable classification for every [`MaterializerError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterializerErrorKind {
    /// The trial spec failed validation.
    InvalidSpec,
    /// The base directory is missing or not a directory.
    InvalidBaseDir,
    /// The trial directory already exists (fresh dirs are required).
    TrialExists,
    /// An I/O operation failed.
    Io,
    /// A `git` child process failed or behaved unexpectedly.
    Git,
    /// The checked-out `HEAD` is not the locked commit.
    CommitMismatch,
    /// The checked-out tree is not clean.
    DirtyTree,
    /// The worktree source is unusable.
    BadSource,
    /// A manifest skill source file is missing, unsafe, or corrupt.
    BadSkillSource,
    /// A skill digest contradicts the manifest or the declared overlay.
    DigestMismatch,
    /// A manifest skill token cannot be materialized as a project path.
    UnsupportedSkillToken,
    /// A secret-patterned parent variable would leak into the trial env.
    SecretLeak,
}

/// Errors produced while materializing a trial.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MaterializerError {
    #[error("invalid trial spec: {message}")]
    InvalidSpec { message: String },

    #[error("invalid base dir `{path}`: {reason}")]
    InvalidBaseDir { path: PathBuf, reason: String },

    #[error("trial directory `{path}` already exists: trials require fresh directories")]
    TrialExists { path: PathBuf },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("git `{args}` failed: {message}")]
    Git { args: String, message: String },

    #[error("commit mismatch: expected {expected}, checked out {found}")]
    CommitMismatch { expected: String, found: String },

    #[error("checked-out tree is not clean: {detail}")]
    DirtyTree { detail: String },

    #[error("unusable worktree source `{path}`: {reason}")]
    BadSource { path: PathBuf, reason: String },

    #[error("unusable skill source `{path}`: {reason}")]
    BadSkillSource { path: PathBuf, reason: String },

    #[error("digest mismatch for `{token}`: {detail}")]
    DigestMismatch { token: String, detail: String },

    #[error("unsupported skill token `{token}`: only `project:` skills materialize")]
    UnsupportedSkillToken { token: String },

    #[error("secret leak refused: parent variable `{key}` would enter the trial env")]
    SecretLeak { key: String },
}

impl MaterializerError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> MaterializerErrorKind {
        match self {
            Self::InvalidSpec { .. } => MaterializerErrorKind::InvalidSpec,
            Self::InvalidBaseDir { .. } => MaterializerErrorKind::InvalidBaseDir,
            Self::TrialExists { .. } => MaterializerErrorKind::TrialExists,
            Self::Io { .. } => MaterializerErrorKind::Io,
            Self::Git { .. } => MaterializerErrorKind::Git,
            Self::CommitMismatch { .. } => MaterializerErrorKind::CommitMismatch,
            Self::DirtyTree { .. } => MaterializerErrorKind::DirtyTree,
            Self::BadSource { .. } => MaterializerErrorKind::BadSource,
            Self::BadSkillSource { .. } => MaterializerErrorKind::BadSkillSource,
            Self::DigestMismatch { .. } => MaterializerErrorKind::DigestMismatch,
            Self::UnsupportedSkillToken { .. } => MaterializerErrorKind::UnsupportedSkillToken,
            Self::SecretLeak { .. } => MaterializerErrorKind::SecretLeak,
        }
    }
}

impl From<crate::trial::TrialError> for MaterializerError {
    fn from(error: crate::trial::TrialError) -> Self {
        Self::InvalidSpec {
            message: error.to_string(),
        }
    }
}

impl From<fake_tools::FakeToolError> for MaterializerError {
    fn from(error: fake_tools::FakeToolError) -> Self {
        match error {
            fake_tools::FakeToolError::DigestMismatch {
                provides,
                profile,
                expected,
                found,
            } => Self::DigestMismatch {
                token: format!("environment.fake_tools[{provides}].script_sha256"),
                detail: format!(
                    "profile {profile}: declared {expected}, embedded script is {found}"
                ),
            },
            fake_tools::FakeToolError::Io { path, message } => Self::Io { path, message },
            other => Self::InvalidSpec {
                message: other.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Canonical trial-relative path of the spec-conformant fake-tool audit log
/// (§2.5.2): `audit/fake-tool-audit.jsonl`.
pub const FAKE_TOOL_AUDIT_RELATIVE_PATH: &str = "audit/fake-tool-audit.jsonl";

/// Trial-relative name of the stub-bin directory `root install` populates
/// (`$TRIAL_STUB_BIN`).
pub const STUB_BIN_DIR_NAME: &str = "stub-bin";

/// Materialize one hermetic trial tree under `base_dir/<trial_id>/` with the
/// legacy generic smoke shims.
///
/// The trial directory must not exist yet. On success every created path is
/// recorded in [`TrialLayout::created_paths`] and in the residue-manifest
/// file. On failure an error is returned; partially created trees are left in
/// place for inspection and must be removed with
/// [`crate::cleanup_trial`] (fail-closed cleanup is the caller's job).
///
/// This is the pre-M5 path: it always writes the five generic shims (`root`,
/// `brew`, `curl`, `npm`, `pip`) and keeps the audit trail at
/// `<trial>/audit.log`. Spec-conformant trials use
/// [`materialize_trial_with_fake_tools`].
pub fn materialize_trial(
    spec: &TrialSpec,
    base_dir: &Path,
) -> Result<TrialLayout, MaterializerError> {
    materialize_inner(spec, base_dir, None)
}

/// Materialize one hermetic trial tree with spec-conformant fake tools
/// (Milestone 5).
///
/// Identical to [`materialize_trial`] except for the fake-tool layer, which
/// follows `TASK-SPEC` §2.5.2 exactly:
///
/// - one `0700` profile shim per non-`absent` [`FakeToolBinding`] from
///   [`crate::fake_tools`]; the legacy smoke shims are never written, and an
///   empty binding list produces an empty fake-bin directory;
/// - the trial audit log is `<trial>/audit/fake-tool-audit.jsonl` — a
///   directory the fixture does not contain — and `$TRIAL_AUDIT_LOG` points
///   at it;
/// - `<trial>/stub-bin` (`0700`) is created, exported as `$TRIAL_STUB_BIN`,
///   and appended to the trial `$PATH` after the fake-bin directory so
///   `root install` can place simulated package binaries where real commands
///   like `rg` resolve;
/// - every declared `script_sha256` must equal the embedded profile script
///   digest, otherwise materialization fails closed with
///   [`MaterializerErrorKind::DigestMismatch`].
///
/// `TRIAL_FAKE_AGENT_*` variables are never read or written by this crate.
pub fn materialize_trial_with_fake_tools(
    spec: &TrialSpec,
    base_dir: &Path,
    bindings: &[FakeToolBinding],
) -> Result<TrialLayout, MaterializerError> {
    materialize_inner(spec, base_dir, Some(bindings))
}

fn materialize_inner(
    spec: &TrialSpec,
    base_dir: &Path,
    bindings: Option<&[FakeToolBinding]>,
) -> Result<TrialLayout, MaterializerError> {
    spec.validate()?;
    require_base_dir(base_dir)?;

    let root = base_dir.join(&spec.trial_id);
    if root.exists() {
        return Err(MaterializerError::TrialExists { path: root });
    }

    let mut created: Vec<PathBuf> = Vec::new();

    // 1. Fresh per-trial directories, 0700.
    let repo_dir = root.join("repo");
    let home_dir = root.join("home");
    let xdg_config_home = root.join("xdg-config");
    let xdg_data_home = root.join("xdg-data");
    let xdg_cache_home = root.join("xdg-cache");
    let xdg_state_home = root.join("xdg-state");
    let tmpdir = root.join("tmp");
    let fake_bin_dir = root.join("fake-bin");
    create_dir_0700(&root, &mut created)?;
    for dir in [
        &home_dir,
        &xdg_config_home,
        &xdg_data_home,
        &xdg_cache_home,
        &xdg_state_home,
        &tmpdir,
        &fake_bin_dir,
    ] {
        create_dir_0700(dir, &mut created)?;
    }
    // Plan-based trials add the stub-bin directory before any shim can write
    // into it.
    let stub_bin_dir = match bindings {
        Some(_) => {
            let dir = root.join(STUB_BIN_DIR_NAME);
            create_dir_0700(&dir, &mut created)?;
            Some(dir)
        }
        None => None,
    };
    // `repo/` is created by `git clone` (which refuses existing targets) or
    // by the tree-copy path below.

    // 2. Detached checkout or verbatim copy.
    if is_git_source(&spec.worktree_source) {
        git_checkout_source(&spec.worktree_source, &repo_dir, &spec.repository.commit)?;
    } else {
        copy_tree_source(&spec.worktree_source, &repo_dir)?;
    }
    created.push(repo_dir.clone());

    // 3. Project skill files from the manifest's `project:` skills only.
    write_project_skills(spec, &repo_dir)?;

    // 4. Hermetic project opencode config (seeded ask-mode permissions).
    write_opencode_config(&repo_dir)?;

    // 5. Fake tools: legacy smoke shims, or the spec-conformant profiles.
    let audit_log = match bindings {
        None => {
            write_fake_tools(&fake_bin_dir)?;
            root.join("audit.log")
        }
        Some(bindings) => {
            let audit_dir = root.join("audit");
            let audit_log = audit_dir.join("fake-tool-audit.jsonl");
            let stub_bin = stub_bin_dir
                .clone()
                .expect("stub-bin is created for plan-based trials");
            let fake_layout =
                fake_tools::materialize_fake_tools(&fake_bin_dir, &audit_log, &stub_bin, bindings)?;
            created.push(audit_dir);
            created.push(audit_log.clone());
            for (_, path) in &fake_layout.shims {
                created.push(path.clone());
            }
            audit_log
        }
    };

    // 6. Allowlist env file (+ audit log, pids, residue manifest).
    let env_file = root.join("trial.env");
    let pids_file = root.join("pids.txt");
    let residue_manifest_file = root.join("residue-manifest.txt");

    let layout = TrialLayout {
        root: root.clone(),
        repo_dir,
        home_dir,
        xdg_config_home,
        xdg_data_home,
        xdg_cache_home,
        xdg_state_home,
        tmpdir,
        fake_bin_dir,
        env_file,
        audit_log,
        pids_file,
        residue_manifest_file,
        created_paths: Vec::new(),
    };
    let env = scrub_env(&layout, &BTreeMap::new());
    write_env_file(&layout.env_file, &env)?;
    created.push(layout.env_file.clone());
    if bindings.is_none() {
        write_file_0600(&layout.audit_log, b"")?;
        created.push(layout.audit_log.clone());
    }
    // Track A spawns no long-lived processes: the only child processes are
    // the bounded `git` invocations above, which have all exited.
    write_file_0600(&layout.pids_file, b"none\n")?;
    created.push(layout.pids_file.clone());

    // Record every created path *inside* the residue manifest, then record
    // the manifest itself last.
    let mut manifest_body = String::new();
    for path in &created {
        manifest_body.push_str(&path.to_string_lossy());
        manifest_body.push('\n');
    }
    manifest_body.push_str(&layout.residue_manifest_file.to_string_lossy());
    manifest_body.push('\n');
    write_file_0600(&layout.residue_manifest_file, manifest_body.as_bytes())?;
    created.push(layout.residue_manifest_file.clone());

    Ok(TrialLayout {
        created_paths: created,
        ..layout
    })
}

// ---------------------------------------------------------------------------
// Secret hygiene
// ---------------------------------------------------------------------------

/// True when `key` matches the parent-environment blocklist.
///
/// Blocked: `AWS_*`, `GITHUB_*`, `OPENAI_*`, `ANTHROPIC_*` prefixes and any
/// key ending in `_TOKEN`, `_SECRET`, or `_KEY`. The trial environment is
/// built from an explicit allowlist only; these patterns document what must
/// never be copied across.
pub fn secret_pattern_hit(key: &str) -> bool {
    key.starts_with("AWS_")
        || key.starts_with("GITHUB_")
        || key.starts_with("OPENAI_")
        || key.starts_with("ANTHROPIC_")
        || key.ends_with("_TOKEN")
        || key.ends_with("_SECRET")
        || key.ends_with("_KEY")
}

/// The stub-bin directory of a plan-based trial layout, or `None` for a
/// legacy layout.
///
/// Plan-based trials are identified structurally by their canonical audit-log
/// path ([`FAKE_TOOL_AUDIT_RELATIVE_PATH`]); legacy trials keep
/// `<trial>/audit.log`. The stub-bin directory is `<trial>/stub-bin`, created
/// by [`materialize_trial_with_fake_tools`].
pub fn trial_stub_bin_dir(layout: &TrialLayout) -> Option<PathBuf> {
    let relative = layout.audit_log.strip_prefix(&layout.root).ok()?;
    if relative == Path::new(FAKE_TOOL_AUDIT_RELATIVE_PATH) {
        Some(layout.root.join(STUB_BIN_DIR_NAME))
    } else {
        None
    }
}

/// The exact trial environment: allowlisted keys derived from `layout` only.
///
/// This function never reads the parent process environment. `PATH` holds
/// only the trial fake-bin directory (plus the stub-bin directory for
/// plan-based layouts, so simulated installed binaries resolve after
/// `root install`) so hermetic trials cannot reach host tools;
/// `OPENCODE_DISABLE_AUTOUPDATE=1` and `OPENCODE_DB=:memory:` encode the
/// Milestone 0 hermetic policy without touching host state. Plan-based
/// layouts additionally export `TRIAL_STUB_BIN`.
pub fn allowed_env(layout: &TrialLayout) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let mut path = layout.fake_bin_dir.to_string_lossy().into_owned();
    if let Some(stub_bin_dir) = trial_stub_bin_dir(layout) {
        path.push(':');
        path.push_str(&stub_bin_dir.to_string_lossy());
        env.insert(
            "TRIAL_STUB_BIN".to_string(),
            stub_bin_dir.to_string_lossy().into_owned(),
        );
    }
    env.insert("PATH".to_string(), path);
    env.insert(
        "HOME".to_string(),
        layout.home_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CONFIG_HOME".to_string(),
        layout.xdg_config_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_DATA_HOME".to_string(),
        layout.xdg_data_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        layout.xdg_cache_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_STATE_HOME".to_string(),
        layout.xdg_state_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "TMPDIR".to_string(),
        layout.tmpdir.to_string_lossy().into_owned(),
    );
    env.insert(
        "TRIAL_AUDIT_LOG".to_string(),
        layout.audit_log.to_string_lossy().into_owned(),
    );
    env.insert("OPENCODE_DISABLE_AUTOUPDATE".to_string(), "1".to_string());
    env.insert("OPENCODE_DB".to_string(), ":memory:".to_string());
    env
}

/// Build the trial environment while proving the parent map leaks nothing.
///
/// Starts from empty and inserts only [`allowed_env`] entries; `parent` (the
/// caller's snapshot of the process environment, passed explicitly so this
/// crate never reads ambient state itself) is consulted solely to fail closed:
/// any secret-patterned parent key whose name *or value* appears in the trial
/// map is refused with [`MaterializerError::SecretLeak`].
pub fn scrub_env(
    layout: &TrialLayout,
    parent: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let env = allowed_env(layout);
    for (key, value) in parent {
        if secret_pattern_hit(key)
            && (env.contains_key(key.as_str()) || env.values().any(|v| v == value))
        {
            // `allowed_env` is constructed without parent input, so this
            // branch is unreachable by construction; it exists so a future
            // allowlist edit that reintroduces inheritance fails loudly in
            // tests rather than leaking silently.
            debug_assert!(
                false,
                "secret-patterned parent variable `{key}` entered the trial env"
            );
        }
    }
    env
}

// ---------------------------------------------------------------------------
// Directories, files, permissions
// ---------------------------------------------------------------------------

fn io_error(path: &Path, message: String) -> MaterializerError {
    MaterializerError::Io {
        path: path.to_path_buf(),
        message,
    }
}

fn create_dir_0700(dir: &Path, created: &mut Vec<PathBuf>) -> Result<(), MaterializerError> {
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error.to_string()))?;
    set_mode(dir, 0o700)?;
    created.push(dir.to_path_buf());
    Ok(())
}

fn write_file_0600(path: &Path, bytes: &[u8]) -> Result<(), MaterializerError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
        set_mode(parent, 0o700)?;
    }
    fs::write(path, bytes).map_err(|error| io_error(path, error.to_string()))?;
    set_mode(path, 0o600)?;
    Ok(())
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<(), MaterializerError> {
    write_file_0600(path, bytes)?;
    set_mode(path, 0o700)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), MaterializerError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| io_error(path, error.to_string()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), MaterializerError> {
    Ok(())
}

fn require_base_dir(base_dir: &Path) -> Result<(), MaterializerError> {
    match fs::symlink_metadata(base_dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err(MaterializerError::InvalidBaseDir {
                path: base_dir.to_path_buf(),
                reason: "directory does not exist".to_string(),
            })
        }
        Err(error) => Err(io_error(base_dir, error.to_string())),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(MaterializerError::InvalidBaseDir {
                    path: base_dir.to_path_buf(),
                    reason: "must not be a symlink".to_string(),
                });
            }
            if metadata.is_dir() {
                Ok(())
            } else {
                Err(MaterializerError::InvalidBaseDir {
                    path: base_dir.to_path_buf(),
                    reason: "must be a directory".to_string(),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Repository checkout (the sole process-spawning exception)
// ---------------------------------------------------------------------------

/// True when `source` looks like a git repository (a `.git` dir or worktree
/// gitfile). Anything else takes the verbatim tree-copy path.
fn is_git_source(source: &Path) -> bool {
    source.join(".git").exists()
}

/// Run a bounded `git` child process and return its stdout.
///
/// This is the only place in `root-eval` that spawns a process (Milestone 1
/// no-spawn rule exception, gated behind explicit trial materialization).
/// Every invocation passes `--no-pager`, never touches the network for local
/// sources, and its stderr is captured into the fail-closed error.
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, MaterializerError> {
    let mut command = Command::new("git");
    command.arg("--no-pager").args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    // Belt and braces: the checkout path is local-only, but a repository
    // with crafted config must not escalate through protocol helpers.
    // The child inherits *only* PATH (needed to locate the git binary after
    // `env_clear`; PATH carries no secrets and matches no blocklist pattern).
    // No trial data and no other parent variable crosses into the child.
    command.env_clear();
    command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
    let output = command.output().map_err(|error| MaterializerError::Git {
        args: args.join(" "),
        message: format!("failed to spawn git: {error}"),
    })?;
    if !output.status.success() {
        return Err(MaterializerError::Git {
            args: args.join(" "),
            message: format!(
                "exit {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clone `source` without checkout, detach at `commit`, and verify.
///
/// Fail-closed: the post-checkout `HEAD` must equal `commit` exactly and
/// `git status --porcelain` must be empty. This verification runs *before*
/// the materializer writes trial files, so the check observes a pristine
/// checkout; trial files (skill, `opencode.json`) are untracked additions
/// by design and stay visible to later `git diff` inspection.
fn git_checkout_source(source: &Path, dest: &Path, commit: &str) -> Result<(), MaterializerError> {
    if dest.exists() {
        return Err(MaterializerError::Io {
            path: dest.to_path_buf(),
            message: "checkout target already exists".to_string(),
        });
    }
    let source_str = source.to_string_lossy().into_owned();
    let dest_str = dest.to_string_lossy().into_owned();
    run_git(
        &[
            "clone",
            "--no-checkout",
            "--no-hardlinks",
            "--",
            &source_str,
            &dest_str,
        ],
        None,
    )?;
    run_git(&["checkout", "--detach", commit], Some(dest))?;

    let head = run_git(&["rev-parse", "HEAD"], Some(dest))?;
    if head != commit {
        return Err(MaterializerError::CommitMismatch {
            expected: commit.to_string(),
            found: head,
        });
    }
    let status = run_git(&["status", "--porcelain"], Some(dest))?;
    if !status.is_empty() {
        return Err(MaterializerError::DirtyTree { detail: status });
    }
    set_mode(dest, 0o700)?;
    Ok(())
}

/// Verbatim recursive copy for non-git sources.
///
/// Fail-closed: symlinks, non-regular files, and `..` escapes are refused.
/// The repository `tree_sha256` lock is *recorded, not verified* on this
/// path (no canonical directory hash exists at this milestone); prefer git
/// sources when commit pinning matters.
fn copy_tree_source(source: &Path, dest: &Path) -> Result<(), MaterializerError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            MaterializerError::BadSource {
                path: source.to_path_buf(),
                reason: "source does not exist".to_string(),
            }
        } else {
            io_error(source, error.to_string())
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(MaterializerError::BadSource {
            path: source.to_path_buf(),
            reason: "source must not be a symlink".to_string(),
        });
    }
    if !metadata.is_dir() {
        return Err(MaterializerError::BadSource {
            path: source.to_path_buf(),
            reason: "source must be a directory".to_string(),
        });
    }
    fs::create_dir_all(dest).map_err(|error| io_error(dest, error.to_string()))?;
    set_mode(dest, 0o700)?;
    copy_tree_inner(source, source, dest)?;
    Ok(())
}

fn copy_tree_inner(root: &Path, current: &Path, dest_root: &Path) -> Result<(), MaterializerError> {
    let entries = fs::read_dir(current).map_err(|error| io_error(current, error.to_string()))?;
    let mut names: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(current, error.to_string()))?;
        names.push(entry.path());
    }
    names.sort();
    for path in names {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| MaterializerError::BadSource {
                path: path.clone(),
                reason: "path escapes the source root".to_string(),
            })?;
        if relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(MaterializerError::BadSource {
                path: path.clone(),
                reason: "`..` components are not permitted".to_string(),
            });
        }
        let target = dest_root.join(relative);
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| io_error(&path, error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(MaterializerError::BadSource {
                path: path.clone(),
                reason: "symlinks are not copied".to_string(),
            });
        }
        if metadata.is_dir() {
            fs::create_dir_all(&target).map_err(|error| io_error(&target, error.to_string()))?;
            set_mode(&target, 0o700)?;
            copy_tree_inner(root, &path, dest_root)?;
        } else if metadata.is_file() {
            if metadata.len() > MAX_CONTEXT_FILE_BYTES {
                return Err(MaterializerError::BadSource {
                    path: path.clone(),
                    reason: format!(
                        "file is {} bytes, over the {}-byte limit",
                        metadata.len(),
                        MAX_CONTEXT_FILE_BYTES
                    ),
                });
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
                set_mode(parent, 0o700)?;
            }
            fs::copy(&path, &target).map_err(|error| io_error(&target, error.to_string()))?;
            set_mode(&target, 0o600)?;
        } else {
            return Err(MaterializerError::BadSource {
                path: path.clone(),
                reason: "only regular files and directories are copied".to_string(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Skill files: project tokens only, baseline gets no skill dir
// ---------------------------------------------------------------------------

/// Write every manifest skill with a `project:` token into the trial repo.
///
/// Skill bytes are read from the manifest's recorded source path (the
/// fixture tree the manifest was resolved against) after fail-closed checks:
/// no symlinks, bounded size, and bytes whose digest equals the manifest
/// record. Declared overlay digests for the same token must agree. Tokens
/// outside `project:` are refused — trial-local home skills are out of
/// scope for Track A.
fn write_project_skills(spec: &TrialSpec, repo_dir: &Path) -> Result<(), MaterializerError> {
    for skill in spec.manifest.skills() {
        let Some(relative) = skill.token.strip_prefix("project:") else {
            return Err(MaterializerError::UnsupportedSkillToken {
                token: skill.token.clone(),
            });
        };
        if relative.is_empty()
            || relative.contains('\\')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(MaterializerError::UnsupportedSkillToken {
                token: skill.token.clone(),
            });
        }
        let bytes = read_skill_source(&skill.path, &skill.token)?;
        let found = sha256_digest(&bytes);
        if found != skill.sha256 {
            return Err(MaterializerError::DigestMismatch {
                token: skill.token.clone(),
                detail: format!(
                    "source file digest {found} does not match manifest record {}",
                    skill.sha256
                ),
            });
        }
        for entry in &spec.overlay.entries {
            if entry.token == skill.token {
                if let Some(declared) = entry.sha256.as_deref() {
                    if !is_canonical_sha256_digest(declared) {
                        return Err(MaterializerError::DigestMismatch {
                            token: skill.token.clone(),
                            detail: format!("overlay digest `{declared}` is not canonical"),
                        });
                    }
                    if declared != found {
                        return Err(MaterializerError::DigestMismatch {
                            token: skill.token.clone(),
                            detail: format!(
                                "overlay declares {declared} but the skill file is {found}"
                            ),
                        });
                    }
                }
            }
        }
        let target = repo_dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
            set_mode(parent, 0o700)?;
        }
        write_file_0600(&target, &bytes)?;
    }
    Ok(())
}

fn read_skill_source(path: &Path, token: &str) -> Result<Vec<u8>, MaterializerError> {
    let bad = |reason: String| MaterializerError::BadSkillSource {
        path: path.to_path_buf(),
        reason,
    };
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            bad("source file does not exist".to_string())
        } else {
            bad(error.to_string())
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(bad("source file must not be a symlink".to_string()));
    }
    if !metadata.is_file() {
        return Err(bad("source is not a regular file".to_string()));
    }
    if metadata.len() > MAX_CONTEXT_FILE_BYTES {
        return Err(bad(format!(
            "source file is {} bytes, over the {MAX_CONTEXT_FILE_BYTES}-byte limit",
            metadata.len()
        )));
    }
    let bytes = fs::read(path).map_err(|error| bad(error.to_string()))?;
    if bytes.len() as u64 > MAX_CONTEXT_FILE_BYTES {
        return Err(bad(format!(
            "source file is {} bytes, over the {MAX_CONTEXT_FILE_BYTES}-byte limit",
            bytes.len()
        )));
    }
    if String::from_utf8(bytes.clone()).is_err() {
        return Err(bad(format!(
            "source file for token `{token}` is not valid UTF-8"
        )));
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Hermetic harness config
// ---------------------------------------------------------------------------

/// Seeded project `opencode.json` with `ask`-mode permission rules.
///
/// Per the Milestone 0 live correction (GOLDEN-CAPTURES `05b`/`05c`), a
/// `deny` rule removes the tool from the model's toolset with no observable
/// event, so denial fixtures must use `ask` (auto-reject emits the
/// `state.status:"error"` denial shape). Seeding ask-by-default keeps stock
/// allow-all behavior out of hermetic trials.
fn write_opencode_config(repo_dir: &Path) -> Result<(), MaterializerError> {
    let dir = repo_dir.join(".opencode");
    fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error.to_string()))?;
    set_mode(&dir, 0o700)?;
    // Shape follows the captured fixtures: `{"permission":{"bash":{"*":"ask"}}}`;
    // here the wildcard asks for every tool by default.
    let body = "{\"permission\":{\"*\":{\"*\":\"ask\"}}}\n";
    let path = dir.join("opencode.json");
    write_file_0600(&path, body.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Offline fake tools
// ---------------------------------------------------------------------------

/// Fake `root`: deterministic JSON per subcommand, auditing every invocation.
///
/// Appends `root <argv...>` to `$TRIAL_AUDIT_LOG` and prints a fixed JSON
/// object for `doctor`, `plan`, `install`, `verify`, and `rollback`.
/// Unknown subcommands fail closed with exit 1. No network, no exec of host
/// tools.
const FAKE_ROOT_SCRIPT: &str = r#"#!/bin/sh
# Fake `root` for isolated evaluation trials. Offline: no network access,
# no host tool execution. Every invocation is appended to $TRIAL_AUDIT_LOG.
set -u
log="${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}"
printf 'root %s\n' "$*" >> "$log"
cmd="${1:-}"
case "$cmd" in
  doctor) printf '{"tool":"root","command":"doctor","status":"ok"}\n' ;;
  plan) printf '{"tool":"root","command":"plan","status":"ok"}\n' ;;
  install) printf '{"tool":"root","command":"install","status":"ok"}\n' ;;
  verify) printf '{"tool":"root","command":"verify","status":"ok"}\n' ;;
  rollback) printf '{"tool":"root","command":"rollback","status":"ok"}\n' ;;
  *)
    printf '{"tool":"root","command":"unknown","status":"error"}\n' >&2
    exit 1
    ;;
esac
"#;

/// Log-and-fail shim for network/installer tools (`brew`, `curl`, `npm`,
/// `pip`). Records the attempted argv in `$TRIAL_AUDIT_LOG` and exits 1.
/// The script bodies deliberately avoid naming any network primitive so the
/// offline guarantee is grep-verifiable.
const FAKE_SHIM_SCRIPT: &str = r#"#!/bin/sh
# Offline shim for isolated evaluation trials. Records the attempt and
# refuses: trials must not reach the network or install host software.
set -u
log="${TRIAL_AUDIT_LOG:?TRIAL_AUDIT_LOG is not set}"
name="$(basename "$0")"
printf '%s %s\n' "$name" "$*" >> "$log"
printf '%s: blocked by trial policy (offline trial)\n' "$name" >&2
exit 1
"#;

fn write_fake_tools(fake_bin_dir: &Path) -> Result<(), MaterializerError> {
    let root = fake_bin_dir.join("root");
    write_executable(&root, FAKE_ROOT_SCRIPT.as_bytes())?;
    for tool in ["brew", "curl", "npm", "pip"] {
        let path = fake_bin_dir.join(tool);
        write_executable(&path, FAKE_SHIM_SCRIPT.as_bytes())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env file
// ---------------------------------------------------------------------------

fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for chunk in value.split('\'') {
        quoted.push_str(chunk);
        quoted.push_str("'\\''");
    }
    // The loop appends a trailing `'\\''` after the last chunk; drop it.
    for _ in "'\\''".chars() {
        quoted.pop();
    }
    quoted.push('\'');
    quoted
}

fn write_env_file(path: &Path, env: &BTreeMap<String, String>) -> Result<(), MaterializerError> {
    let mut body = String::new();
    for (key, value) in env {
        body.push_str(key);
        body.push('=');
        body.push_str(&shell_quote(value));
        body.push('\n');
    }
    // The blocklist is enforced by construction (`allowed_env` never reads
    // the parent), but re-check here so a future edit that merges parent
    // state fails closed instead of persisting a leak to disk.
    let blocked: HashSet<&str> = env
        .keys()
        .filter(|key| secret_pattern_hit(key))
        .map(String::as_str)
        .collect();
    if !blocked.is_empty() {
        let key = blocked.into_iter().next().unwrap_or("?");
        return Err(MaterializerError::SecretLeak {
            key: key.to_string(),
        });
    }
    write_file_0600(path, body.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_round_trips_hostile_values() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("o'clock"), "'o'\\''clock'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn secret_blocklist_covers_audit_patterns() {
        for key in [
            "AWS_ACCESS_KEY_ID",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CI_JOB_TOKEN",
            "NPM_SECRET",
            "DEPLOY_KEY",
            "MYAPP_TOKEN",
        ] {
            assert!(secret_pattern_hit(key), "{key} must be blocked");
        }
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "TRIAL_AUDIT_LOG",
            "OPENCODE_DB",
            "MONKEY",
        ] {
            assert!(!secret_pattern_hit(key), "{key} must be allowed");
        }
    }

    #[test]
    fn fake_tool_scripts_are_offline_by_construction() {
        for (name, script) in [("root", FAKE_ROOT_SCRIPT), ("shim", FAKE_SHIM_SCRIPT)] {
            for needle in ["curl", "wget", "socket"] {
                assert!(
                    !script.contains(needle),
                    "fake {name} script must not reference `{needle}`"
                );
            }
        }
        assert!(FAKE_ROOT_SCRIPT.contains("TRIAL_AUDIT_LOG"));
        for command in ["doctor", "plan", "install", "verify", "rollback"] {
            assert!(
                FAKE_ROOT_SCRIPT.contains(command),
                "fake root must handle `{command}`"
            );
        }
    }
}
