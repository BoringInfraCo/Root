//! # Verifier execution and repository tree fingerprints (Milestone 3, Phase A).
//!
//! [`run_verifier`] executes a trial's predeclared verifier commands **after**
//! the agent has exited, through [`crate::supervisor::run_supervised`] with the
//! caller's explicit (already minimal) environment, and folds the result into
//! a machine-matchable [`VerifierOutcome`]:
//!
//! 1. fingerprint the scored tree ([`tree_fingerprint`]);
//! 2. run each command under the spec's wall timeout and output cap;
//! 3. compare each exit code with the command's `expected_exit`;
//! 4. fingerprint again and compare.
//!
//! # Environment hygiene
//!
//! A verifier command runs with the **hermetic** environment built by
//! [`hermetic_verifier_env`]: the trial's `PATH` holds the fake-tool
//! directory and the trial's stub-bin directory and **nothing else** — the
//! host `PATH` is excluded entirely. A verifier that could reach host tools
//! could satisfy a task by using the host's software instead of the trial's,
//! which would silently invalidate the measurement.
//!
//! Because the host `PATH` is gone, a record whose verifier argv names a
//! shell (`["sh", "verify.sh"]`) can no longer resolve it. [`resolve_verifier_argv`]
//! rewrites such an argv[0] to the **absolute path of a shell staged inside
//! the trial** — never to a host `PATH` lookup. The staged shell is a copy of
//! the host's `/bin/sh` placed in `<trial>/verifier-bin/sh` (see
//! [`crate::trial_env::stage_verifier_shell`]); every other argv[0] keeps
//! resolving through the trial `PATH` (the fake shims and the stub-bin).
//!
//! # Verdict policy
//!
//! - [`VerifierVerdict::Pass`]: every command exited with its expected code and
//!   the tree fingerprint is unchanged.
//! - [`VerifierVerdict::Fail`]: every command ran to completion but at least
//!   one exit code differed. A verifier "failure" here is the *task* outcome,
//!   not an infrastructure problem.
//! - [`VerifierVerdict::Invalid`]: the verifier itself could not be trusted —
//!   a command timed out, was killed by the output cap, failed to spawn, the
//!   spec declared no commands, or the scored tree changed (mutation).
//!
//! A verifier must never mutate the scored artifacts: any pre/post fingerprint
//! difference sets [`VerifierOutcome::mutation_detected`] and forces
//! `Invalid`, regardless of exit codes (audit M3 exit criterion). Fingerprint
//! reads are bounded and fail closed: symlinks, non-regular entries, oversized
//! files, and over-deep trees are typed [`VerifierError`]s rather than being
//! skipped.
//!
//! # Notes on reuse
//!
//! Verifier stdout is plain text, not harness NDJSON, so every line lands in
//! [`crate::SupervisedResult::dropped_lines`]; this module ignores that view
//! and keeps the raw bounded bytes ([`VerifierOutcome::stdout`] /
//! [`VerifierOutcome::stderr`]) as the evidence artifact. `run_verifier` never
//! panics and never returns an error type: infrastructure failures are folded
//! into the outcome (`verdict = Invalid`, reason in
//! [`VerifierOutcome::notes`]) so the trial classifier always receives a
//! record.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::digest::sha256_digest;
use crate::supervisor::{run_supervised, SupervisorConfig, TerminationKind, DEFAULT_KILL_GRACE_MS};

/// Hash domain mixed into the tree-fingerprint projection.
pub const TREE_FINGERPRINT_DOMAIN: &str = "tree-fingerprint.v1";

/// Largest regular file [`tree_fingerprint`] will read.
pub const MAX_FINGERPRINT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Deepest directory level [`tree_fingerprint`] will descend.
pub const MAX_FINGERPRINT_DEPTH: usize = 64;

/// Default per-verifier wall budget in milliseconds.
pub const DEFAULT_VERIFIER_TIMEOUT_MS: u64 = 120_000;

/// Default aggregate stdout/stderr capture cap for one verifier run.
pub const DEFAULT_VERIFIER_OUTPUT_CAP_BYTES: u64 = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// One verifier command and its expected exit code.
///
/// Maps 1:1 onto the task record's `[[verifier.commands]]` entry
/// (`crate::VerifierCommand`): `argv` is the declared `argv` and
/// `expected_exit` the declared `expected_exit`. The task record's optional
/// per-command `cwd` and `timeout_seconds` are not modeled here yet; every
/// command runs in the trial repository root under the spec-level timeout
/// (documented M3-A limit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierStep {
    /// Command argv; `argv[0]` is the program, the rest are arguments.
    pub argv: Vec<String>,
    /// Exit code that counts as this command passing.
    pub expected_exit: i32,
}

/// A trial's predeclared verifier: identity, commands, and budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierSpec {
    /// Stable verifier identifier recorded in the evidence manifest.
    pub verifier_id: String,
    /// Commands executed in declaration order (stop at the first
    /// infrastructure failure).
    pub commands: Vec<VerifierStep>,
    /// Wall budget per command, in milliseconds.
    pub timeout_ms: u64,
    /// Aggregate stdout/stderr capture cap for this verifier run, in bytes.
    pub output_cap_bytes: u64,
}

impl VerifierSpec {
    /// Build a spec with the default timeout and output cap.
    pub fn new(verifier_id: impl Into<String>, commands: Vec<VerifierStep>) -> Self {
        Self {
            verifier_id: verifier_id.into(),
            commands,
            timeout_ms: DEFAULT_VERIFIER_TIMEOUT_MS,
            output_cap_bytes: DEFAULT_VERIFIER_OUTPUT_CAP_BYTES,
        }
    }
}

// ---------------------------------------------------------------------------
// Verdict and outcome
// ---------------------------------------------------------------------------

/// Machine-matchable verifier verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierVerdict {
    /// All commands matched their expected exit codes; tree unchanged.
    Pass,
    /// All commands ran to completion; at least one exit code differed.
    Fail,
    /// The verifier could not be trusted (timeout, cap kill, spawn failure,
    /// empty spec, fingerprint failure, or tree mutation).
    Invalid,
}

impl VerifierVerdict {
    /// Stable snake-case token used by evidence manifests and eval storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Invalid => "invalid",
        }
    }

    /// Parse the stable token produced by [`VerifierVerdict::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// The complete verifier-run outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierOutcome {
    /// Verifier identity from the spec.
    pub verifier_id: String,
    /// Machine-matchable verdict (see the module docs for the policy).
    pub verdict: VerifierVerdict,
    /// Exit code per declared command (`None` when a command never ran or was
    /// signaled). Always the same length as `spec.commands`.
    pub exit_codes: Vec<Option<i32>>,
    /// `sha256:<hex>` of [`VerifierOutcome::stdout`].
    pub stdout_sha256: String,
    /// `sha256:<hex>` of [`VerifierOutcome::stderr`].
    pub stderr_sha256: String,
    /// Retained stdout byte count (bounded by the aggregate cap).
    pub stdout_bytes_len: u64,
    /// Retained stderr byte count (bounded by the aggregate cap).
    pub stderr_bytes_len: u64,
    /// Fingerprint of the scored tree before the verifier ran (`""` when the
    /// pre-fingerprint itself failed).
    pub pre_tree_sha256: String,
    /// Fingerprint of the scored tree after the verifier ran (`""` when it
    /// could not be computed).
    pub post_tree_sha256: String,
    /// True when the pre/post fingerprints differ (verifier mutated the tree).
    pub mutation_detected: bool,
    /// Wall time of the whole verifier run (fingerprints included).
    pub wall_ms: u64,
    /// True when a command hit the wall timeout (verdict is then `Invalid`).
    pub timed_out: bool,
    /// Retained stdout bytes for the caller to store content-addressed.
    pub stdout: Vec<u8>,
    /// Retained stderr bytes for the caller to store content-addressed.
    pub stderr: Vec<u8>,
    /// True when stdout was cut off by the aggregate cap.
    pub stdout_truncated: bool,
    /// True when stderr was cut off by the aggregate cap.
    pub stderr_truncated: bool,
    /// Human-readable reasons for an `Invalid` verdict or non-matching exits.
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tree fingerprint
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`VerifierError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifierErrorKind {
    /// The fingerprint root does not exist or is not a directory.
    NotADirectory,
    /// An I/O operation failed.
    Io,
    /// A symlink was encountered (fingerprints refuse symlinks).
    SymlinkRefused,
    /// A regular file exceeds the fingerprint size cap.
    FileTooLarge,
    /// A non-regular entry (fifo, socket, device) was encountered.
    UnsupportedEntry,
    /// The tree is deeper than [`MAX_FINGERPRINT_DEPTH`].
    TooDeep,
    /// Canonical serialization of the fingerprint projection failed.
    Canonicalization,
}

/// Errors produced while fingerprinting a repository tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerifierError {
    #[error("fingerprint root `{path}` does not exist or is not a directory")]
    NotADirectory { path: PathBuf },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("symlink refused while fingerprinting: `{path}`")]
    SymlinkRefused { path: PathBuf },

    #[error("file `{path}` is {bytes} bytes, over the {cap}-byte fingerprint cap")]
    FileTooLarge { path: PathBuf, bytes: u64, cap: u64 },

    #[error("unsupported entry `{path}`: only regular files and directories are fingerprinted")]
    UnsupportedEntry { path: PathBuf },

    #[error(
        "directory `{path}` is deeper than the {MAX_FINGERPRINT_DEPTH}-level fingerprint limit"
    )]
    TooDeep { path: PathBuf },

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },
}

impl VerifierError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> VerifierErrorKind {
        match self {
            Self::NotADirectory { .. } => VerifierErrorKind::NotADirectory,
            Self::Io { .. } => VerifierErrorKind::Io,
            Self::SymlinkRefused { .. } => VerifierErrorKind::SymlinkRefused,
            Self::FileTooLarge { .. } => VerifierErrorKind::FileTooLarge,
            Self::UnsupportedEntry { .. } => VerifierErrorKind::UnsupportedEntry,
            Self::TooDeep { .. } => VerifierErrorKind::TooDeep,
            Self::Canonicalization { .. } => VerifierErrorKind::Canonicalization,
        }
    }
}

/// Deterministic content fingerprint of a repository tree.
///
/// Walks every regular file under `repo` (excluding any entry named `.git`),
/// sorted by relative path, and hashes a canonical JSON projection of
/// `(relative path, sha256 of file bytes)` pairs mixed with
/// [`TREE_FINGERPRINT_DOMAIN`]. Content-only: mtimes, permissions, and
/// `.git/` state never contribute, so an untouched tree fingerprints
/// identically and any file edit, addition, or removal changes the digest.
///
/// Fail closed: symlinks, non-regular entries, files over
/// [`MAX_FINGERPRINT_FILE_BYTES`], and trees deeper than
/// [`MAX_FINGERPRINT_DEPTH`] are typed errors rather than silent skips.
pub fn tree_fingerprint(repo: &Path) -> Result<String, VerifierError> {
    tree_fingerprint_excluding(repo, &[])
}

/// Deterministic content fingerprint of a repository tree, skipping the
/// given subtrees.
///
/// The excluded paths are **absolute** directory paths; a directory whose
/// path equals one of them is never entered, so neither its files nor its
/// symlinks are observed. The driver excludes the project's OpenCode config
/// dir: it is the harness's own directory inside the agent's workspace (it
/// carries npm symlinks, and the harness's background npm activity inside
/// it must never be read as agent activity), so it is excluded from the
/// scored tree.
///
/// Every other rule of [`tree_fingerprint`] is unchanged.
pub fn tree_fingerprint_excluding(
    repo: &Path,
    excluded: &[PathBuf],
) -> Result<String, VerifierError> {
    tree_fingerprint_capped_excluding(repo, MAX_FINGERPRINT_FILE_BYTES, excluded)
}

fn tree_fingerprint_capped_excluding(
    repo: &Path,
    max_file_bytes: u64,
    excluded: &[PathBuf],
) -> Result<String, VerifierError> {
    let metadata = fs::symlink_metadata(repo).map_err(|error| VerifierError::Io {
        path: repo.to_path_buf(),
        message: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(VerifierError::SymlinkRefused {
            path: repo.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(VerifierError::NotADirectory {
            path: repo.to_path_buf(),
        });
    }

    let mut files: Vec<(String, String)> = Vec::new();
    walk_tree(repo, repo, 0, max_file_bytes, excluded, &mut files)?;
    files.sort();

    let entries: Vec<serde_json::Value> = files
        .iter()
        .map(|(path, digest)| json!([path, digest]))
        .collect();
    let projection = json!({
        "domain": TREE_FINGERPRINT_DOMAIN,
        "files": entries,
    });
    let bytes = canonical_json_bytes(&projection)
        .map_err(|message| VerifierError::Canonicalization { message })?;
    Ok(sha256_digest(&bytes))
}

fn walk_tree(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_file_bytes: u64,
    excluded: &[PathBuf],
    files: &mut Vec<(String, String)>,
) -> Result<(), VerifierError> {
    if depth > MAX_FINGERPRINT_DEPTH {
        return Err(VerifierError::TooDeep {
            path: dir.to_path_buf(),
        });
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| VerifierError::Io {
        path: dir.to_path_buf(),
        message: error.to_string(),
    })? {
        let entry = entry.map_err(|error| VerifierError::Io {
            path: dir.to_path_buf(),
            message: error.to_string(),
        })?;
        entries.push(entry.path());
    }
    entries.sort();

    for path in entries {
        // `.git` never contributes: directory, worktree gitfile, or anything
        // else carrying that name.
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| VerifierError::Io {
            path: path.clone(),
            message: "path escapes the fingerprint root".to_string(),
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| VerifierError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(VerifierError::SymlinkRefused { path });
        }
        if metadata.is_dir() {
            if excluded.iter().any(|skip| skip == &path) {
                // The harness's own seeded subtree: never observed.
                continue;
            }
            walk_tree(root, &path, depth + 1, max_file_bytes, excluded, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(VerifierError::UnsupportedEntry { path });
        }
        if metadata.len() > max_file_bytes {
            return Err(VerifierError::FileTooLarge {
                path: path.clone(),
                bytes: metadata.len(),
                cap: max_file_bytes,
            });
        }
        let bytes = fs::read(&path).map_err(|error| VerifierError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        files.push((
            relative.to_string_lossy().into_owned(),
            sha256_digest(&bytes),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verifier runner
// ---------------------------------------------------------------------------

/// Build the hermetic verifier environment of a trial.
///
/// `PATH` holds the trial's fake-tool directory and — when the trial has one
/// — its stub-bin directory. The **host `PATH` is excluded entirely**: a
/// verifier that could reach host tools could satisfy a task with host
/// software instead of the trial's, which would invalidate the measurement
/// (the M5 postmortem's host-leakage finding).
///
/// The returned map is the *only* environment the verifier child inherits.
pub fn hermetic_verifier_env(fake_bin: &Path, stub_bin: Option<&Path>) -> Vec<(String, String)> {
    let mut path = fake_bin.to_string_lossy().into_owned();
    if let Some(stub_bin) = stub_bin {
        path.push(':');
        path.push_str(&stub_bin.to_string_lossy());
    }
    vec![("PATH".to_string(), path)]
}

/// Rewrite a verifier argv so that a shell argument resolves inside the trial.
///
/// `argv[0] == "sh"` is replaced by `shell` — the absolute path of the shell
/// staged inside the trial. Every other argv[0] is left untouched and keeps
/// resolving through the hermetic trial `PATH`.
pub fn resolve_verifier_argv(argv: &[String], shell: Option<&Path>) -> Vec<String> {
    let mut resolved = argv.to_vec();
    if let (Some(shell), Some(first)) = (shell, resolved.first_mut()) {
        if first == "sh" {
            *first = shell.to_string_lossy().into_owned();
        }
    }
    resolved
}

/// Run `spec`'s commands in `repo` and report the verdict.
///
/// Every command runs through [`run_supervised`] with the explicit `env` map
/// (the child inherits nothing else), `cwd = repo`, the spec's wall timeout
/// and output cap. The scored tree is fingerprinted before and after; any
/// difference forces [`VerifierVerdict::Invalid`] with
/// [`VerifierOutcome::mutation_detected`]. Infrastructure failures never
/// panic or return `Err`; they are folded into the outcome with a reason in
/// [`VerifierOutcome::notes`].
pub fn run_verifier(spec: &VerifierSpec, repo: &Path, env: &[(String, String)]) -> VerifierOutcome {
    run_verifier_excluding(spec, repo, env, &[])
}

/// Run `spec`'s commands in `repo` and report the verdict, scoring the tree
/// without the excluded subtrees.
///
/// The excluded paths are **absolute** directory paths that are never entered
/// by the pre/post fingerprint (see [`tree_fingerprint_excluding`]). The
/// real harness excludes the seeded plugin install target, so the harness's
/// own npm activity can never be reported as a verifier mutation.
pub fn run_verifier_excluding(
    spec: &VerifierSpec,
    repo: &Path,
    env: &[(String, String)],
    excluded: &[PathBuf],
) -> VerifierOutcome {
    let started = Instant::now();
    let mut notes: Vec<String> = Vec::new();
    let mut exit_codes: Vec<Option<i32>> = vec![None; spec.commands.len()];
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let mut timed_out = false;
    let mut mutation_detected = false;

    let (pre_ok, pre_tree_sha256, mut invalid_reason) =
        match tree_fingerprint_excluding(repo, excluded) {
            Ok(hash) => (true, hash, None),
            Err(error) => (
                false,
                String::new(),
                Some(format!("pre_tree_fingerprint_failed: {error}")),
            ),
        };
    if spec.verifier_id.is_empty() {
        invalid_reason.get_or_insert_with(|| "verifier_id is empty".to_string());
    }
    if spec.commands.is_empty() {
        invalid_reason.get_or_insert_with(|| "verifier spec declares no commands".to_string());
    }

    let mut all_matched = true;
    if invalid_reason.is_none() {
        for (index, step) in spec.commands.iter().enumerate() {
            if step.argv.is_empty() {
                invalid_reason = Some(format!("command[{index}] has an empty argv"));
                break;
            }
            let config = SupervisorConfig {
                wall_timeout_ms: spec.timeout_ms,
                output_cap_bytes: spec.output_cap_bytes,
                kill_grace_ms: DEFAULT_KILL_GRACE_MS,
                cancel: None,
            };
            let supervised = run_supervised(&step.argv, env, repo, &config);
            exit_codes[index] = supervised.exit_code;
            append_capped(
                &mut stdout,
                &supervised.raw_stdout,
                spec.output_cap_bytes,
                &mut stdout_truncated,
            );
            append_capped(
                &mut stderr,
                &supervised.raw_stderr,
                spec.output_cap_bytes,
                &mut stderr_truncated,
            );
            match supervised.termination {
                TerminationKind::SpawnFailed => {
                    invalid_reason = Some(format!("command[{index}] failed to spawn"));
                    break;
                }
                TerminationKind::TimeoutKilled => {
                    timed_out = true;
                    invalid_reason = Some(format!(
                        "command[{index}] exceeded the {}-ms verifier timeout",
                        spec.timeout_ms
                    ));
                    break;
                }
                TerminationKind::CancelledKilled => {
                    invalid_reason = Some(format!("command[{index}] was cancelled"));
                    break;
                }
                TerminationKind::OutputCapExceeded => {
                    invalid_reason = Some(format!(
                        "command[{index}] exceeded the {}-byte output cap",
                        spec.output_cap_bytes
                    ));
                    break;
                }
                TerminationKind::Completed
                | TerminationKind::ProviderError
                | TerminationKind::Denied => {
                    if supervised.exit_code != Some(step.expected_exit) {
                        all_matched = false;
                        notes.push(format!(
                            "command[{index}] exited {:?}, expected {}",
                            supervised.exit_code, step.expected_exit
                        ));
                    }
                }
            }
        }
    }

    // Re-fingerprint whenever the pre-fingerprint succeeded — even after an
    // aborted command — so a verifier that mutated the scored tree is always
    // caught.
    let mut post_tree_sha256 = String::new();
    if pre_ok {
        match tree_fingerprint_excluding(repo, excluded) {
            Ok(hash) => {
                if hash != pre_tree_sha256 {
                    mutation_detected = true;
                }
                post_tree_sha256 = hash;
            }
            Err(error) => {
                invalid_reason
                    .get_or_insert_with(|| format!("post_tree_fingerprint_failed: {error}"));
            }
        }
    }
    if mutation_detected {
        notes.push("verifier mutated the scored tree (pre/post fingerprint mismatch)".to_string());
    }
    if let Some(reason) = &invalid_reason {
        notes.push(reason.clone());
    }

    let verdict = if mutation_detected || invalid_reason.is_some() {
        VerifierVerdict::Invalid
    } else if all_matched {
        VerifierVerdict::Pass
    } else {
        VerifierVerdict::Fail
    };

    VerifierOutcome {
        verifier_id: spec.verifier_id.clone(),
        verdict,
        exit_codes,
        stdout_sha256: sha256_digest(&stdout),
        stderr_sha256: sha256_digest(&stderr),
        stdout_bytes_len: stdout.len() as u64,
        stderr_bytes_len: stderr.len() as u64,
        pre_tree_sha256,
        post_tree_sha256,
        mutation_detected,
        wall_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        timed_out,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        notes,
    }
}

/// Append `src` to `dst` while honoring the aggregate cap.
fn append_capped(dst: &mut Vec<u8>, src: &[u8], cap: u64, truncated: &mut bool) {
    if src.is_empty() {
        return;
    }
    let cap = cap as usize;
    if dst.len() >= cap {
        *truncated = true;
        return;
    }
    let take = (cap - dst.len()).min(src.len());
    dst.extend_from_slice(&src[..take]);
    if take < src.len() {
        *truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-m3-verifier-{}-{}-{}",
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

    fn write_script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, body).expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("chmod +x");
        }
        path.to_string_lossy().into_owned()
    }

    fn spec_for(scripts: &[String]) -> VerifierSpec {
        VerifierSpec {
            verifier_id: "verifier-test".to_string(),
            commands: scripts
                .iter()
                .map(|script| VerifierStep {
                    argv: vec!["sh".to_string(), script.clone()],
                    expected_exit: 0,
                })
                .collect(),
            timeout_ms: 10_000,
            output_cap_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn fingerprint_is_stable_and_content_sensitive() {
        let dir = unique_dir("fingerprint");
        fs::write(dir.join("a.txt"), "alpha\n").expect("write a");
        fs::create_dir_all(dir.join("nested")).expect("mkdir nested");
        fs::write(dir.join("nested/b.txt"), "beta\n").expect("write b");
        // `.git` never contributes, whatever it holds.
        fs::create_dir_all(dir.join(".git/objects")).expect("mkdir .git");
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

        let first = tree_fingerprint(&dir).expect("fingerprint");
        let second = tree_fingerprint(&dir).expect("fingerprint again");
        assert_eq!(first, second, "same tree must fingerprint identically");
        assert!(first.starts_with("sha256:"));

        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/other\n").expect("mutate HEAD");
        assert_eq!(
            tree_fingerprint(&dir).expect("fingerprint"),
            first,
            ".git changes must not alter the fingerprint"
        );

        fs::write(dir.join("a.txt"), "alpha changed\n").expect("edit a");
        let edited = tree_fingerprint(&dir).expect("fingerprint");
        assert_ne!(edited, first, "file edit must change the fingerprint");

        fs::write(dir.join("c.txt"), "new\n").expect("add c");
        assert_ne!(tree_fingerprint(&dir).expect("fingerprint"), edited);

        fs::remove_file(dir.join("c.txt")).expect("remove c");
        assert_eq!(
            tree_fingerprint(&dir).expect("fingerprint"),
            edited,
            "removal restores the prior fingerprint"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_refuses_symlinks_and_oversized_files() {
        let dir = unique_dir("fingerprint-refusals");
        fs::write(dir.join("ok.txt"), "ok\n").expect("write ok");

        let capped = tree_fingerprint_capped_excluding(&dir, 2, &[]);
        assert_eq!(
            capped.expect_err("oversized file refused").kind(),
            VerifierErrorKind::FileTooLarge
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("ok.txt"), dir.join("link.txt"))
                .expect("create symlink");
            let error = tree_fingerprint(&dir).expect_err("symlink refused");
            assert_eq!(error.kind(), VerifierErrorKind::SymlinkRefused);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_requires_a_directory() {
        let dir = unique_dir("fingerprint-missing");
        let error = tree_fingerprint(&dir.join("nope")).expect_err("missing refused");
        assert_eq!(error.kind(), VerifierErrorKind::Io);
        let file = dir.join("file.txt");
        fs::write(&file, "x\n").expect("write file");
        let error = tree_fingerprint(&file).expect_err("file refused");
        assert_eq!(error.kind(), VerifierErrorKind::NotADirectory);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_pass_and_fail_from_exit_codes() {
        let dir = unique_dir("exit-codes");
        let ok = write_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n");
        let bad = write_script(&dir, "bad.sh", "#!/bin/sh\nexit 3\n");

        let pass = run_verifier(&spec_for(std::slice::from_ref(&ok)), &dir, &[]);
        assert_eq!(pass.verdict, VerifierVerdict::Pass);
        assert_eq!(pass.exit_codes, vec![Some(0)]);
        assert!(!pass.mutation_detected && !pass.timed_out);
        assert_eq!(pass.stdout_sha256, sha256_digest(&pass.stdout));
        assert_eq!(pass.stderr_sha256, sha256_digest(&pass.stderr));
        assert_eq!(pass.stdout_bytes_len, pass.stdout.len() as u64);
        assert_eq!(pass.pre_tree_sha256, pass.post_tree_sha256);
        assert!(pass.notes.is_empty());

        let fail = run_verifier(&spec_for(&[bad]), &dir, &[]);
        assert_eq!(fail.verdict, VerifierVerdict::Fail);
        assert_eq!(fail.exit_codes, vec![Some(3)]);
        assert!(fail.notes.iter().any(|note| note.contains("expected 0")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_passes_its_environment_to_commands() {
        let dir = unique_dir("env");
        let script = write_script(
            &dir,
            "env.sh",
            "#!/bin/sh\nprintf 'VALUE=%s\\n' \"$ROOT_EVAL_VERIFIER_VAR\"\n",
        );
        let env = vec![("ROOT_EVAL_VERIFIER_VAR".to_string(), "trial".to_string())];
        let outcome = run_verifier(&spec_for(&[script]), &dir, &env);
        assert_eq!(outcome.verdict, VerifierVerdict::Pass);
        assert_eq!(outcome.stdout, b"VALUE=trial\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_mutation_is_detected_and_invalid() {
        let dir = unique_dir("mutation");
        let script = write_script(
            &dir,
            "mutate.sh",
            "#!/bin/sh\necho 'side effect' > mutation.txt\nexit 0\n",
        );
        let outcome = run_verifier(&spec_for(&[script]), &dir, &[]);
        assert!(outcome.mutation_detected);
        assert_eq!(outcome.verdict, VerifierVerdict::Invalid);
        assert_ne!(outcome.pre_tree_sha256, outcome.post_tree_sha256);
        assert!(outcome
            .notes
            .iter()
            .any(|note| note.contains("mutated the scored tree")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_timeout_is_invalid() {
        let dir = unique_dir("timeout");
        let script = write_script(&dir, "slow.sh", "#!/bin/sh\nexec sleep 30\n");
        let mut spec = spec_for(&[script]);
        spec.timeout_ms = 400;
        let outcome = run_verifier(&spec, &dir, &[]);
        assert!(outcome.timed_out);
        assert_eq!(outcome.verdict, VerifierVerdict::Invalid);
        assert!(outcome
            .notes
            .iter()
            .any(|note| note.contains("exceeded the 400-ms verifier timeout")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_without_commands_is_invalid() {
        let dir = unique_dir("no-commands");
        fs::write(dir.join("file.txt"), "x\n").expect("write file");
        let spec = VerifierSpec {
            verifier_id: "empty".to_string(),
            commands: Vec::new(),
            timeout_ms: 1_000,
            output_cap_bytes: 1024,
        };
        let outcome = run_verifier(&spec, &dir, &[]);
        assert_eq!(outcome.verdict, VerifierVerdict::Invalid);
        assert!(outcome.exit_codes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verifier_missing_repo_is_invalid_not_a_panic() {
        let dir = unique_dir("missing-repo");
        let spec = VerifierSpec::new(
            "missing",
            vec![VerifierStep {
                argv: vec!["true".to_string()],
                expected_exit: 0,
            }],
        );
        let outcome = run_verifier(&spec, &dir.join("nope"), &[]);
        assert_eq!(outcome.verdict, VerifierVerdict::Invalid);
        assert!(outcome.pre_tree_sha256.is_empty());
        assert!(outcome
            .notes
            .iter()
            .any(|note| note.starts_with("pre_tree_fingerprint_failed")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verdict_tokens_round_trip() {
        for verdict in [
            VerifierVerdict::Pass,
            VerifierVerdict::Fail,
            VerifierVerdict::Invalid,
        ] {
            assert_eq!(VerifierVerdict::from_token(verdict.as_str()), Some(verdict));
            let serialized = serde_json::to_value(verdict).expect("serializable");
            assert_eq!(
                serialized,
                serde_json::Value::String(verdict.as_str().to_string())
            );
        }
        assert_eq!(VerifierVerdict::from_token("nope"), None);
    }
}
