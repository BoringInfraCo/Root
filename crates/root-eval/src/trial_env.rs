//! Real-harness trial environment: the one-time prewarm assets and the exact
//! per-trial environment a real OpenCode trial runs in.
//!
//! The design follows the Milestone 6 integration probe
//! (`Docs/Eval/milestone-6/INTEGRATION-PROBE.md` §A.8 / §B.4): a real trial
//! must be **hermetic** (no host `PATH`, no host state) and **cheap** (zero
//! registry bytes per trial).
//!
//! # One-time prewarm
//!
//! [`Prewarm::prepare`] owns a directory tree with four shared assets:
//!
//! | Asset | Path | Size (measured) | Mode |
//! |---|---|---|---|
//! | shared warm npm cache | `<root>/npm-cache` | ~86 MB | `0700` (writable) |
//! | config template (plugin `node_modules`) | `<root>/config-template/opencode` | ~61 MB | copied per trial |
//! | provider/model catalog | `<root>/models/models.json` | 5.6 MB | `0444` (read-only) |
//! | `rg` for the `skill` tool | `<root>/rg/rg` | ~4.2 MB | `0755` |
//!
//! The three non-cache assets are produced by the operator's single prewarm
//! run (the probe's §A.8 recipe) and then *harvested* into this layout with
//! [`Prewarm::harvest`]. The npm cache is **not** a build product: it is the
//! registry cache the prewarm run filled, and it must stay writable because
//! the npm client writes index entries into it (probe §A.4). It is safe to
//! share **sequentially**; concurrent trials would race on the same cache
//! and are not supported.
//!
//! # Per-trial setup
//!
//! [`prepare_trial_env`] seeds one materialized trial and returns the exact
//! child environment:
//!
//! 1. the config template is **cloned** into the trial's global
//!    `XDG_CONFIG_HOME/opencode` — this is the first of the two plugin
//!    install targets, and seeding it is what makes the npm install skip
//!    entirely (probe §A.3);
//! 2. the same template's `node_modules` is **cloned** into the trial
//!    project's `.opencode/` — the *second* install target. A project skill
//!    creates `<cwd>/.opencode`, so seeding only the global directory still
//!    pulls ~48 MB from the registry (probe §A.6-3);
//! 3. `models.json` is copied read-only (`0444`) into the trial's
//!    `XDG_CACHE_HOME/opencode/` — it can be shared read-only: OpenCode does
//!    not rewrite it (probe §A.5);
//! 4. `rg` is copied (`0755`) into the trial's
//!    `XDG_CACHE_HOME/opencode/bin/` — the `skill` tool is implemented on top
//!    of a ripgrep service that resolves `which rg` → `$XDG_CACHE_HOME/
//!    opencode/bin/rg` → *download* ripgrep from GitHub. Without it the tool
//!    fails with `ripgrep execution failed` and a 1.78 MB tarball that a
//!    minimal `PATH` cannot even extract (probe §B.4).
//!
//! ## Cloning, never hardlinking
//!
//! The template is cloned with the platform's copy-on-write clone
//! (`cp -Rc` on macOS/APFS) and falls back to a plain recursive copy. A
//! **hardlink** is never used: npm rewrites
//! `node_modules/.package-lock.json` *through the shared inode*, so every
//! trial would mutate the shared template and trials would contaminate each
//! other (probe §A.7, measured: 12 161 B → 6 889 B).
//!
//! ## `rg` is trial-local, never the host's
//!
//! The `rg` this module installs is the **prewarmed, trial-local copy**. The
//! host's `rg` is never resolved (a `PATH` lookup would leak a host binary
//! into the trial — the M5 postmortem's host-`PATH` leakage finding) and is
//! never copied. The trial `PATH` therefore contains the fake shims and the
//! trial's stub-bin only; the `skill`-tool `rg` lives *outside* `PATH`, in
//! the trial's XDG cache, so the baseline condition still observes
//! `rg` as absent.
//!
//! # The exact per-trial environment
//!
//! [`real_trial_env`] returns the base [`crate::materializer::allowed_env`]
//! map plus one variable. Every entry, and why it exists:
//!
//! | Variable | Value | Why |
//! |---|---|---|
//! | `HOME` | `<trial>/home` | isolated home; OpenCode never touches the operator's home |
//! | `XDG_CONFIG_HOME` | `<trial>/xdg-config` | global config + plugin install target |
//! | `XDG_DATA_HOME` | `<trial>/xdg-data` | session store, logs, locks (trial-local) |
//! | `XDG_CACHE_HOME` | `<trial>/xdg-cache` | `models.json` + the `skill`-tool `rg` |
//! | `XDG_STATE_HOME` | `<trial>/xdg-state` | lock/heartbeat state (trial-local) |
//! | `TMPDIR` | `<trial>/tmp` | scratch space stays inside the trial |
//! | `PATH` | `<trial>/fake-bin:<trial>/stub-bin` | **no host `PATH`**: fake shims and the trial-local stub-bin (the `rg` the fake `root install` writes) only |
//! | `TRIAL_AUDIT_LOG` | `<trial>/audit/fake-tool-audit.jsonl` | the fake-tool side channel |
//! | `TRIAL_STUB_BIN` | `<trial>/stub-bin` | where the fake `root install` puts package binaries |
//! | `OPENCODE_DISABLE_AUTOUPDATE` | `1` | the CLI must not phone home to update itself |
//! | `OPENCODE_DB` | `:memory:` | M0 hermetic policy marker |
//! | `npm_config_cache` | `<prewarm>/npm-cache` | the shared warm npm cache: keeps the registry traffic at **0 bytes** per trial (probe §A.2 — the lowercase spelling is npm's canonical one) |
//!
//! The child environment is built from this map alone: the supervisor clears
//! the environment and rebuilds it, so nothing of the parent process leaks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use crate::materializer::allowed_env;
use crate::trial::TrialLayout;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Directory name of the shared warm npm cache inside the prewarm root.
pub const PREWARM_NPM_CACHE_DIR: &str = "npm-cache";

/// Directory name of the config template inside the prewarm root.
pub const PREWARM_CONFIG_TEMPLATE_DIR: &str = "config-template";

/// Directory name of the provider/model catalog inside the prewarm root.
pub const PREWARM_MODELS_DIR: &str = "models";

/// File name of the provider/model catalog inside the prewarm root.
pub const PREWARM_MODELS_FILE: &str = "models.json";

/// Directory name of the `rg` asset inside the prewarm root.
pub const PREWARM_RG_DIR: &str = "rg";

/// File name of the `rg` asset inside the prewarm root.
pub const PREWARM_RG_FILE: &str = "rg";

/// Trial-relative directory that holds the staged verifier shell.
pub const VERIFIER_BIN_DIR_NAME: &str = "verifier-bin";

/// Trial-relative path of the project's **OpenCode config dir**
/// (`<trial>/repo/.opencode`).
///
/// It is the harness's own directory inside the agent's workspace, and it
/// holds three kinds of content, none of it the agent's work:
///
/// - the **driver-seeded** permission config (`opencode.json`) and, in the
///   treatment condition, the project skill (`skill/<name>/SKILL.md`);
/// - the **driver-seeded** plugin install target (`node_modules/`) — the
///   second of the two plugin install targets (probe §A.6): a project skill
///   creates `<cwd>/.opencode`, and OpenCode installs the plugin into every
///   `.opencode` config directory it discovers. Seeding it keeps the
///   registry traffic at zero bytes per trial;
/// - the **harness-written** files OpenCode and its npm client create in
///   every config dir (`$schema` writeback into `opencode.json`, a
///   `.gitignore`, and `package.json` / `package-lock.json`).
///
/// The directory is therefore excluded from every workspace observation (the
/// tree snapshot, the tree fingerprint, and the verifier's mutation check)
/// and hidden from the git-based change capture (see
/// [`project_config_dir`] and [`prepare_trial_env`]): the harness's own
/// writes inside it must never be read as agent activity.
pub const PROJECT_CONFIG_DIR_RELATIVE: &str = ".opencode";

/// Absolute host shell candidates, tried in order.
///
/// These are **absolute paths**, never a `PATH` lookup: a verifier command
/// must never resolve its interpreter through the host `PATH`.
pub const HOST_SHELL_CANDIDATES: [&str; 2] = ["/bin/sh", "/usr/bin/sh"];

/// Mode of the shared provider/model catalog file (read-only).
pub const MODELS_FILE_MODE: u32 = 0o444;

/// Mode of the trial-local `rg` binary.
pub const RG_FILE_MODE: u32 = 0o755;

/// Mode of the staged verifier shell.
pub const VERIFIER_SHELL_MODE: u32 = 0o755;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`TrialEnvError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrialEnvErrorKind {
    /// A required prewarm asset is missing or unusable.
    MissingAsset,
    /// An I/O operation failed.
    Io,
    /// A tree clone failed.
    CloneFailed,
    /// A file mode could not be set.
    Permission,
    /// No host shell could be staged for the verifier commands.
    ShellUnavailable,
}

/// Errors produced while building the real-harness trial environment.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TrialEnvError {
    #[error("missing prewarm asset `{path}`: {detail}")]
    MissingAsset { path: PathBuf, detail: String },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("failed to clone `{from}` into `{to}`: {message}")]
    CloneFailed {
        from: PathBuf,
        to: PathBuf,
        message: String,
    },

    #[error("failed to set mode {mode:04o} on `{path}`: {message}")]
    Permission {
        path: PathBuf,
        mode: u32,
        message: String,
    },

    #[error("no host shell available for the verifier commands (tried: {candidates:?})")]
    ShellUnavailable { candidates: Vec<String> },
}

impl TrialEnvError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> TrialEnvErrorKind {
        match self {
            Self::MissingAsset { .. } => TrialEnvErrorKind::MissingAsset,
            Self::Io { .. } => TrialEnvErrorKind::Io,
            Self::CloneFailed { .. } => TrialEnvErrorKind::CloneFailed,
            Self::Permission { .. } => TrialEnvErrorKind::Permission,
            Self::ShellUnavailable { .. } => TrialEnvErrorKind::ShellUnavailable,
        }
    }
}

fn io_error(path: &Path, message: impl Into<String>) -> TrialEnvError {
    TrialEnvError::Io {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

// ---------------------------------------------------------------------------
// Prewarm
// ---------------------------------------------------------------------------

/// The shared, one-time assets every real trial clones from.
///
/// See the [module docs](crate::trial_env) for the asset table, the sizes
/// measured by the integration probe, and the operator recipe that produces
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prewarm {
    /// The prewarm root directory.
    pub root: PathBuf,
    /// Shared warm npm cache (writable; shared sequentially across trials).
    pub npm_cache: PathBuf,
    /// Config template directory (`<root>/config-template/opencode`), holding
    /// the plugin `node_modules` that makes the npm install skip.
    pub config_template: PathBuf,
    /// Read-only provider/model catalog (`<root>/models/models.json`).
    pub models_json: PathBuf,
    /// Trial-local `rg` binary for the `skill` tool (`<root>/rg/rg`).
    pub rg: PathBuf,
}

impl Prewarm {
    /// Validate (and normalize the modes of) a prewarm tree.
    ///
    /// The npm cache directory is created when missing; the other three
    /// assets must already exist — they are products of the operator's
    /// prewarm run and cannot be synthesized without a real OpenCode
    /// invocation. A missing asset is a typed
    /// [`TrialEnvError::MissingAsset`] naming the exact expected path.
    ///
    /// Normalization: `models.json` becomes `0444` (read-only, shareable) and
    /// `rg` becomes `0755` (executable by the trial).
    pub fn prepare(root: &Path) -> Result<Prewarm, TrialEnvError> {
        let npm_cache = root.join(PREWARM_NPM_CACHE_DIR);
        let config_template = root.join(PREWARM_CONFIG_TEMPLATE_DIR).join("opencode");
        let models_json = root.join(PREWARM_MODELS_DIR).join(PREWARM_MODELS_FILE);
        let rg = root.join(PREWARM_RG_DIR).join(PREWARM_RG_FILE);

        fs::create_dir_all(&npm_cache).map_err(|error| io_error(&npm_cache, error.to_string()))?;
        set_mode(&npm_cache, 0o700)?;

        for directory in [models_json.parent(), rg.parent()].into_iter().flatten() {
            fs::create_dir_all(directory)
                .map_err(|error| io_error(directory, error.to_string()))?;
        }

        let node_modules = config_template.join("node_modules");
        if !node_modules.is_dir() {
            return Err(TrialEnvError::MissingAsset {
                path: node_modules,
                detail: "the config template must carry the plugin `node_modules`; run the \
                         documented prewarm invocation once and harvest it with \
                         `Prewarm::harvest`"
                    .to_string(),
            });
        }
        require_file(
            &models_json,
            "the provider/model catalog is produced by the prewarm run",
        )?;
        require_file(
            &rg,
            "the `skill` tool needs a ripgrep binary; see probe §B.4",
        )?;

        set_mode(&models_json, MODELS_FILE_MODE)?;
        set_mode(&rg, RG_FILE_MODE)?;

        Ok(Prewarm {
            root: root.to_path_buf(),
            npm_cache,
            config_template,
            models_json,
            rg,
        })
    }

    /// Harvest the prewarm assets out of a completed prewarm run directory.
    ///
    /// The source directory is the scratch tree of one OpenCode run that used
    /// the shared npm cache, laid out as:
    ///
    /// ```text
    /// <source>/npm-cache/                 shared warm npm cache
    /// <source>/config/opencode/           global config dir (plugin `node_modules`)
    /// <source>/cache/opencode/models.json provider/model catalog
    /// <source>/cache/opencode/bin/rg      ripgrep for the `skill` tool
    /// ```
    ///
    /// The npm cache is **moved** (not copied) when it is still empty, and
    /// copied otherwise; the other three assets are copied. The resulting
    /// tree is validated by [`Prewarm::prepare`].
    pub fn harvest(root: &Path, source: &Path) -> Result<Prewarm, TrialEnvError> {
        let source_cache = source.join(PREWARM_NPM_CACHE_DIR);
        let target_cache = root.join(PREWARM_NPM_CACHE_DIR);
        if source_cache.is_dir() && !target_cache.exists() {
            // The warm cache is the expensive asset (~86 MB): move it when
            // the rename is possible, copy it otherwise. An existing cache
            // is kept as is — re-harvesting it would re-download the whole
            // registry payload.
            if fs::rename(&source_cache, &target_cache).is_err() {
                copy_tree(&source_cache, &target_cache)?;
            }
        }

        let source_config = source.join("config").join("opencode");
        let target_config = root.join(PREWARM_CONFIG_TEMPLATE_DIR).join("opencode");
        if source_config.is_dir() {
            if target_config.exists() {
                fs::remove_dir_all(&target_config)
                    .map_err(|error| io_error(&target_config, error.to_string()))?;
            }
            clone_tree(&source_config, &target_config)?;
        }

        let source_models = source
            .join("cache")
            .join("opencode")
            .join(PREWARM_MODELS_FILE);
        if source_models.is_file() {
            let target = root.join(PREWARM_MODELS_DIR).join(PREWARM_MODELS_FILE);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
            }
            fs::copy(&source_models, &target)
                .map_err(|error| io_error(&target, error.to_string()))?;
        }

        let source_rg = source
            .join("cache")
            .join("opencode")
            .join("bin")
            .join(PREWARM_RG_FILE);
        if source_rg.is_file() {
            let target = root.join(PREWARM_RG_DIR).join(PREWARM_RG_FILE);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
            }
            fs::copy(&source_rg, &target).map_err(|error| io_error(&target, error.to_string()))?;
        }

        Prewarm::prepare(root)
    }

    /// A short, stable description of the prewarm assets (for evidence).
    pub fn describe(&self) -> String {
        format!(
            "prewarm_root={} npm_cache={} config_template={} models_json={} rg={}",
            self.root.display(),
            self.npm_cache.display(),
            self.config_template.display(),
            self.models_json.display(),
            self.rg.display()
        )
    }
}

// ---------------------------------------------------------------------------
// Per-trial environment
// ---------------------------------------------------------------------------

/// The exact child environment of a real trial.
///
/// See the [module docs](crate::trial_env) for the per-variable rationale.
pub fn real_trial_env(layout: &TrialLayout, prewarm: &Prewarm) -> BTreeMap<String, String> {
    let mut env = allowed_env(layout);
    env.insert(
        "npm_config_cache".to_string(),
        prewarm.npm_cache.to_string_lossy().into_owned(),
    );
    env
}

/// Seed one materialized trial with the real-harness assets.
///
/// Performs the four seeding steps documented at the module level (global
/// config clone, project config clone, read-only `models.json`, trial-local
/// `rg`), rewrites `<trial>/trial.env` from the resulting environment, and
/// returns the exact environment map for the child process.
pub fn prepare_trial_env(
    layout: &TrialLayout,
    prewarm: &Prewarm,
) -> Result<BTreeMap<String, String>, TrialEnvError> {
    // 1. Global install target: `<trial>/xdg-config/opencode`.
    let global_config = layout.xdg_config_home.join("opencode");
    if global_config.exists() {
        fs::remove_dir_all(&global_config)
            .map_err(|error| io_error(&global_config, error.to_string()))?;
    }
    clone_tree(&prewarm.config_template, &global_config)?;

    // 2. Project install target: `<trial>/repo/.opencode/node_modules`.
    //    A project skill creates `<cwd>/.opencode`, so this is a second
    //    install target and must be seeded too (probe §A.6).
    let project_dir = layout.repo_dir.join(".opencode");
    fs::create_dir_all(&project_dir).map_err(|error| io_error(&project_dir, error.to_string()))?;
    let project_modules = project_dir.join("node_modules");
    if project_modules.exists() {
        fs::remove_dir_all(&project_modules)
            .map_err(|error| io_error(&project_modules, error.to_string()))?;
    }
    clone_tree(
        &prewarm.config_template.join("node_modules"),
        &project_modules,
    )?;
    // The seeded plugin tree is the harness's own plumbing: hide it from the
    // git-based change capture, which would otherwise list every one of its
    // ~3 600 files as untracked. The rule lives in the repository's local
    // exclude file (`$GIT_DIR/info/exclude`) — not in the workspace — so the
    // agent never sees an extra file.
    exclude_from_git(&layout.repo_dir, PROJECT_CONFIG_DIR_RELATIVE)?;

    // 3. Read-only provider/model catalog.
    let cache_dir = layout.xdg_cache_home.join("opencode");
    fs::create_dir_all(&cache_dir).map_err(|error| io_error(&cache_dir, error.to_string()))?;
    let models_json = cache_dir.join(PREWARM_MODELS_FILE);
    fs::copy(&prewarm.models_json, &models_json)
        .map_err(|error| io_error(&models_json, error.to_string()))?;
    set_mode(&models_json, MODELS_FILE_MODE)?;

    // 4. Trial-local `rg` for the `skill` tool (never the host's `rg`).
    let bin_dir = cache_dir.join("bin");
    fs::create_dir_all(&bin_dir).map_err(|error| io_error(&bin_dir, error.to_string()))?;
    let rg = bin_dir.join(PREWARM_RG_FILE);
    fs::copy(&prewarm.rg, &rg).map_err(|error| io_error(&rg, error.to_string()))?;
    set_mode(&rg, RG_FILE_MODE)?;

    let env = real_trial_env(layout, prewarm);
    write_trial_env_file(&layout.env_file, &env)?;
    Ok(env)
}

/// The project's OpenCode config dir of a trial.
///
/// The absolute path of `<trial>/repo/.opencode` — the harness's own
/// directory inside the agent's workspace. The driver excludes it from
/// every workspace observation (see [`PROJECT_CONFIG_DIR_RELATIVE`]).
pub fn project_config_dir(layout: &TrialLayout) -> PathBuf {
    layout.repo_dir.join(PROJECT_CONFIG_DIR_RELATIVE)
}

/// Add a repository-local exclude rule to the trial repository.
///
/// The rule is written to `$GIT_DIR/info/exclude` (the repository-local,
/// uncommitted ignore file), so it applies to `git status`/`git diff` in the
/// trial without adding a file to the agent's workspace. The rule is
/// repo-relative and anchored at the repository root.
fn exclude_from_git(repo: &Path, relative: &str) -> Result<(), TrialEnvError> {
    let exclude = repo.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
    }
    let mut body = String::new();
    if let Ok(existing) = fs::read_to_string(&exclude) {
        body.push_str(&existing);
        if !body.ends_with('\n') {
            body.push('\n');
        }
    }
    let rule = format!("/{relative}/");
    if !body.lines().any(|line| line.trim() == rule) {
        body.push_str(&rule);
        body.push('\n');
    }
    fs::write(&exclude, body.as_bytes()).map_err(|error| io_error(&exclude, error.to_string()))?;
    Ok(())
}

/// Stage a trial-local POSIX shell for the verifier commands.
///
/// The verifier environment carries **no host `PATH`**, so a record whose
/// verifier argv is `["sh", "verify.sh"]` cannot resolve `sh` any more. The
/// shell is therefore staged *inside* the trial (`<trial>/verifier-bin/sh`,
/// mode `0755`) and the verifier argv is rewritten to that absolute path —
/// never to a host `PATH` lookup.
///
/// The staged file is a tiny wrapper that execs the host's POSIX shell
/// through an **absolute** path (resolved once, from
/// [`HOST_SHELL_CANDIDATES`]). A *copy* of the host shell is not used: on
/// macOS the kernel refuses to execute a copied platform binary (it is
/// killed with `SIGKILL`), and a wrapper keeps the trial's own file a plain
/// regular file. The wrapper's shebang and its `exec` target are both
/// absolute, so no `PATH` search happens anywhere in the chain.
pub fn stage_verifier_shell(trial_root: &Path) -> Result<PathBuf, TrialEnvError> {
    let dir = trial_root.join(VERIFIER_BIN_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error.to_string()))?;
    set_mode(&dir, 0o700)?;

    let mut candidates = Vec::new();
    for candidate in HOST_SHELL_CANDIDATES {
        candidates.push(candidate.to_string());
        if !Path::new(candidate).is_file() {
            continue;
        }
        let target = dir.join("sh");
        let body = format!(
            "#!/bin/sh\n\
             # Trial-local shell staged by root-eval for the verifier commands.\n\
             # The trial PATH carries no host PATH, so a verifier argv that names\n\
             # `sh` is resolved to this absolute path instead of a PATH lookup.\n\
             exec {candidate} \"$@\"\n"
        );
        fs::write(&target, body.as_bytes())
            .map_err(|error| io_error(&target, error.to_string()))?;
        set_mode(&target, VERIFIER_SHELL_MODE)?;
        return Ok(target);
    }
    Err(TrialEnvError::ShellUnavailable { candidates })
}

/// Rewrite `<trial>/trial.env` from an explicit environment map.
///
/// The file is shell-sourceable and mirrors the materializer's format
/// (`KEY='value'` per line, sorted by key).
pub fn write_trial_env_file(
    path: &Path,
    env: &BTreeMap<String, String>,
) -> Result<(), TrialEnvError> {
    let mut body = String::new();
    for (key, value) in env {
        body.push_str(key);
        body.push('=');
        body.push_str(&shell_quote(value));
        body.push('\n');
    }
    fs::write(path, body.as_bytes()).map_err(|error| io_error(path, error.to_string()))?;
    set_mode(path, 0o600)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for chunk in value.split('\'') {
        quoted.push_str(chunk);
        quoted.push_str("'\\''");
    }
    for _ in "'\\''".chars() {
        quoted.pop();
    }
    quoted.push('\'');
    quoted
}

fn require_file(path: &Path, detail: &str) -> Result<(), TrialEnvError> {
    if path.is_file() {
        return Ok(());
    }
    Err(TrialEnvError::MissingAsset {
        path: path.to_path_buf(),
        detail: detail.to_string(),
    })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), TrialEnvError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        TrialEnvError::Permission {
            path: path.to_path_buf(),
            mode,
            message: error.to_string(),
        }
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), TrialEnvError> {
    Ok(())
}

/// Clone `source` into `target` without ever hardlinking.
///
/// The platform's copy-on-write clone is used when available (`cp -Rc` on
/// macOS/APFS); otherwise a plain recursive copy is performed. A hardlink is
/// never used: npm rewrites `node_modules/.package-lock.json` through the
/// shared inode and would mutate the shared template (probe §A.7).
fn clone_tree(source: &Path, target: &Path) -> Result<(), TrialEnvError> {
    if target.exists() {
        return Err(TrialEnvError::CloneFailed {
            from: source.to_path_buf(),
            to: target.to_path_buf(),
            message: "target already exists".to_string(),
        });
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error.to_string()))?;
    }
    if clone_via_cp(source, target) {
        return Ok(());
    }
    copy_tree(source, target)
}

/// Try the platform copy-on-write clone (`cp -Rc`).
///
/// Returns false when no `cp` supports `-c`, so the caller falls back to a
/// recursive copy. `cp` is invoked through an absolute path: the trial `PATH`
/// is not in scope here and must never be consulted.
fn clone_via_cp(source: &Path, target: &Path) -> bool {
    for candidate in ["/bin/cp", "/usr/bin/cp"] {
        let binary = Path::new(candidate);
        if !binary.is_file() {
            continue;
        }
        let status = Command::new(binary)
            .arg("-Rc")
            .arg(source)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Ok(status) = status {
            if status.success() && target.exists() {
                return true;
            }
            // A failed clone can leave a partial target behind.
            let _ = fs::remove_dir_all(target);
        }
    }
    false
}

/// Plain recursive copy (the non-APFS fallback for [`clone_tree`]).
fn copy_tree(source: &Path, target: &Path) -> Result<(), TrialEnvError> {
    let metadata =
        fs::symlink_metadata(source).map_err(|error| io_error(source, error.to_string()))?;
    if !metadata.is_dir() {
        return Err(TrialEnvError::CloneFailed {
            from: source.to_path_buf(),
            to: target.to_path_buf(),
            message: "source is not a directory".to_string(),
        });
    }
    fs::create_dir_all(target).map_err(|error| io_error(target, error.to_string()))?;
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(source).map_err(|error| io_error(source, error.to_string()))? {
        let entry = entry.map_err(|error| io_error(source, error.to_string()))?;
        entries.push(entry.path());
    }
    entries.sort();
    for entry in entries {
        let child_target = target.join(entry.file_name().unwrap_or_default());
        let child_metadata =
            fs::symlink_metadata(&entry).map_err(|error| io_error(&entry, error.to_string()))?;
        if child_metadata.is_dir() {
            copy_tree(&entry, &child_target)?;
        } else if child_metadata.is_file() {
            fs::copy(&entry, &child_target)
                .map_err(|error| io_error(&child_target, error.to_string()))?;
        } else {
            return Err(TrialEnvError::CloneFailed {
                from: entry,
                to: child_target,
                message: "only regular files and directories are copied".to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-trialenv-{prefix}-{}-{id}",
            std::process::id()
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("clear stale temp dir");
        }
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Build a fake prewarm tree with the four assets.
    fn fake_prewarm(root: &Path) {
        let npm_cache = root.join(PREWARM_NPM_CACHE_DIR);
        fs::create_dir_all(&npm_cache).expect("npm cache");
        fs::write(npm_cache.join("_cacache"), b"warm").expect("cache bytes");

        let template = root.join(PREWARM_CONFIG_TEMPLATE_DIR).join("opencode");
        let modules = template
            .join("node_modules")
            .join("@opencode-ai")
            .join("plugin");
        fs::create_dir_all(&modules).expect("plugin tree");
        fs::write(modules.join("package.json"), b"{}").expect("plugin file");
        fs::write(
            template.join("node_modules").join(".package-lock.json"),
            b"{\"lock\":1}",
        )
        .expect("lock file");

        let models = root.join(PREWARM_MODELS_DIR);
        fs::create_dir_all(&models).expect("models dir");
        fs::write(models.join(PREWARM_MODELS_FILE), b"{\"providers\":[]}").expect("models");

        let rg = root.join(PREWARM_RG_DIR);
        fs::create_dir_all(&rg).expect("rg dir");
        fs::write(rg.join(PREWARM_RG_FILE), b"#!/bin/sh\nexit 0\n").expect("rg");
    }

    #[test]
    fn shell_quote_round_trips_hostile_values() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("o'clock"), "'o'\\''clock'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn prepare_reports_the_missing_asset_and_normalizes_modes() {
        let root = unique_dir("missing");
        let error = Prewarm::prepare(&root).expect_err("missing assets must fail closed");
        assert_eq!(error.kind(), TrialEnvErrorKind::MissingAsset);

        fake_prewarm(&root);
        let prewarm = Prewarm::prepare(&root).expect("prewarm prepares");
        assert!(prewarm.npm_cache.is_dir());
        assert!(prewarm.config_template.join("node_modules").is_dir());
        assert!(prewarm.models_json.is_file());
        assert!(prewarm.rg.is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let models_mode = fs::metadata(&prewarm.models_json)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            let rg_mode = fs::metadata(&prewarm.rg)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(models_mode, MODELS_FILE_MODE);
            assert_eq!(rg_mode, RG_FILE_MODE);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn harvest_moves_the_prewarm_run_assets_into_the_canonical_layout() {
        let root = unique_dir("harvest");
        let source = unique_dir("harvest-source");
        fs::create_dir_all(source.join("npm-cache")).expect("cache");
        fs::write(source.join("npm-cache").join("x"), b"1").expect("cache file");
        fs::create_dir_all(source.join("config").join("opencode").join("node_modules"))
            .expect("config");
        fs::write(
            source
                .join("config")
                .join("opencode")
                .join("node_modules")
                .join("p.js"),
            b"x",
        )
        .expect("plugin");
        fs::create_dir_all(source.join("cache").join("opencode").join("bin")).expect("cache");
        fs::write(
            source
                .join("cache")
                .join("opencode")
                .join(PREWARM_MODELS_FILE),
            b"{}",
        )
        .expect("models");
        fs::write(
            source
                .join("cache")
                .join("opencode")
                .join("bin")
                .join(PREWARM_RG_FILE),
            b"rg",
        )
        .expect("rg");

        let prewarm = Prewarm::harvest(&root, &source).expect("harvest");
        assert!(prewarm.npm_cache.is_dir());
        assert!(prewarm
            .config_template
            .join("node_modules")
            .join("p.js")
            .is_file());
        assert!(prewarm.models_json.is_file());
        assert!(prewarm.rg.is_file());
        assert!(prewarm.describe().contains("npm_cache="));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn staged_shell_is_inside_the_trial_and_executable() {
        let root = unique_dir("shell");
        let shell = stage_verifier_shell(&root).expect("stage shell");
        assert!(shell.starts_with(&root));
        assert_eq!(shell.file_name().and_then(|name| name.to_str()), Some("sh"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&shell).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, VERIFIER_SHELL_MODE);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn env_file_is_shell_sourceable_and_sorted() {
        let root = unique_dir("envfile");
        let mut env = BTreeMap::new();
        env.insert("B".to_string(), "two words".to_string());
        env.insert("A".to_string(), "x".to_string());
        let path = root.join("trial.env");
        write_trial_env_file(&path, &env).expect("write env file");
        let body = fs::read_to_string(&path).expect("read env file");
        assert_eq!(body, "A='x'\nB='two words'\n");
        let _ = fs::remove_dir_all(&root);
    }
}
