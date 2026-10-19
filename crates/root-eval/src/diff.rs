//! Bounded repository change capture (Milestone 3, Phase A).
//!
//! [`capture_diff`] records what an agent did to the trial checkout: `git
//! status --porcelain=v1 -uall`, the unstaged diff, the staged diff, and the
//! untracked path list, all under one byte budget and one wall timeout.
//!
//! # Second sanctioned git-invocation site
//!
//! The trial materializer ([`crate::materialize_trial`]) is the crate's first
//! documented exception to the Milestone 1 no-spawn rule; this module is the
//! **second**, same class: `git` children only, `env_clear` plus a fixed
//! minimal environment, `--no-pager`, no network operations, captured
//! stdout/stderr, and a bounded wall timeout after which the child is killed.
//! Unlike the materializer's checkout calls, capture runs user-visible git
//! commands, so the child environment additionally pins
//! `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null` (or `NUL`),
//! `GIT_ATTR_NOSYSTEM=1`, and `LC_ALL=C`: user or system git configuration
//! (including `diff.external`, which executes programs) must never influence
//! or run during evidence capture.
//!
//! # Bounds
//!
//! [`DiffCaps::max_bytes`] caps the total captured bytes (stdout and stderr
//! across all three invocations). A capture that hits the cap stops reading,
//! kills the child, sets [`RepoDiff::truncated`], and returns what it has:
//! truncated evidence is explicitly flagged, never silently short.
//!
//! # Hash
//!
//! [`RepoDiff::diff_sha256`] is the SHA-256 of the canonical JSON projection
//! of the whole record (status, diff text, untracked list, truncation flag)
//! mixed with [`DIFF_HASH_DOMAIN`] — a single integrity hash for the captured
//! change evidence, usable directly as the manifest's `repo_diff_sha256`.

use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::digest::sha256_digest;

/// Hash domain mixed into the diff-record projection.
pub const DIFF_HASH_DOMAIN: &str = "repo-diff.v1";

/// Default cap on total captured diff bytes (512 KiB).
pub const MAX_DIFF_BYTES: u64 = 512 * 1024;

/// Default wall timeout per git invocation.
pub const DEFAULT_DIFF_TIMEOUT_MS: u64 = 30_000;

/// Cap on captured stderr per git invocation.
const MAX_DIFF_STDERR_BYTES: u64 = 64 * 1024;

/// Poll interval while waiting for a git child.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Null device for `GIT_CONFIG_GLOBAL`.
#[cfg(unix)]
const GIT_NULL_DEVICE: &str = "/dev/null";
#[cfg(not(unix))]
const GIT_NULL_DEVICE: &str = "NUL";

/// Bounds for one [`capture_diff`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffCaps {
    /// Total captured bytes (status + diffs + stderr) across all invocations.
    pub max_bytes: u64,
    /// Wall timeout for each git invocation, in milliseconds.
    pub timeout_ms: u64,
}

impl Default for DiffCaps {
    fn default() -> Self {
        Self {
            max_bytes: MAX_DIFF_BYTES,
            timeout_ms: DEFAULT_DIFF_TIMEOUT_MS,
        }
    }
}

/// The captured repository change record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoDiff {
    /// Raw `git status --porcelain=v1 -uall` output (trailing newline
    /// trimmed). When [`RepoDiff::truncated`] is set this may end mid-line.
    pub status_porcelain: String,
    /// Concatenated unstaged and staged `git diff` output.
    pub diff_text: String,
    /// Paths from `?? ` status lines, in status order. Incomplete when the
    /// capture was truncated.
    pub untracked: Vec<String>,
    /// SHA-256 of the canonical projection of this whole record.
    pub diff_sha256: String,
    /// True when the byte cap cut the capture short.
    pub truncated: bool,
}

impl RepoDiff {
    /// Recompute [`RepoDiff::diff_sha256`] from the record's fields.
    ///
    /// A caller that reads a persisted record can compare this with the stored
    /// digest to detect tampering with any captured field.
    pub fn computed_sha256(&self) -> Result<String, DiffError> {
        diff_record_sha256(
            &self.status_porcelain,
            &self.diff_text,
            &self.untracked,
            self.truncated,
        )
    }
}

/// Machine-matchable classification for every [`DiffError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffErrorKind {
    /// The target directory is not a git repository.
    NotARepository,
    /// A git child failed (spawn error or nonzero exit).
    GitFailed,
    /// A git child exceeded the capture timeout.
    Timeout,
    /// An I/O operation failed.
    Io,
    /// Canonical serialization of the diff projection failed.
    Canonicalization,
}

/// Errors produced while capturing a repository diff.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiffError {
    #[error("`{path}` is not a git repository")]
    NotARepository { path: PathBuf },

    #[error("git `{args}` failed: {message}")]
    GitFailed { args: String, message: String },

    #[error("git `{args}` exceeded the {timeout_ms}-ms capture timeout")]
    Timeout { args: String, timeout_ms: u64 },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },
}

impl DiffError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> DiffErrorKind {
        match self {
            Self::NotARepository { .. } => DiffErrorKind::NotARepository,
            Self::GitFailed { .. } => DiffErrorKind::GitFailed,
            Self::Timeout { .. } => DiffErrorKind::Timeout,
            Self::Io { .. } => DiffErrorKind::Io,
            Self::Canonicalization { .. } => DiffErrorKind::Canonicalization,
        }
    }
}

/// Capture the repository change record for `repo` under `caps`.
pub fn capture_diff(repo: &Path, caps: &DiffCaps) -> Result<RepoDiff, DiffError> {
    require_git_repo(repo)?;

    let mut remaining = caps.max_bytes;
    let mut truncated = false;

    let status = run_git_bounded(
        &["status", "--porcelain=v1", "-uall"],
        repo,
        remaining,
        caps.timeout_ms,
    )?;
    if status.timed_out {
        return Err(DiffError::Timeout {
            args: "status --porcelain=v1 -uall".to_string(),
            timeout_ms: caps.timeout_ms,
        });
    }
    // A cap-cut invocation is killed (or dies on the closed pipe), so a
    // nonzero status is an artifact of the cut, not a git failure; the
    // truncation flag records that the capture is incomplete.
    if !status.truncated {
        require_success("status --porcelain=v1 -uall", &status)?;
    }
    truncated |= status.truncated;
    remaining = remaining.saturating_sub(status.captured_len());

    let status_raw = String::from_utf8_lossy(&status.stdout).into_owned();
    let mut status_lines: Vec<&str> = status_raw.lines().collect();
    if status.truncated && !status_raw.is_empty() && !status_raw.ends_with('\n') {
        // A truncated capture may end mid-line; never parse a partial path.
        status_lines.pop();
    }
    let untracked: Vec<String> = status_lines
        .iter()
        .filter_map(|line| line.strip_prefix("?? ").map(str::to_string))
        .collect();
    let status_porcelain = status_raw.trim_end_matches(['\n', '\r']).to_string();

    let mut diff_text = String::new();
    for args in [
        &["diff", "--no-color"][..],
        &["diff", "--cached", "--no-color"][..],
    ] {
        if remaining == 0 {
            // The budget is exhausted and a diff invocation is still due.
            truncated = true;
            break;
        }
        let output = run_git_bounded(args, repo, remaining, caps.timeout_ms)?;
        let label = args.join(" ");
        if output.timed_out {
            return Err(DiffError::Timeout {
                args: label,
                timeout_ms: caps.timeout_ms,
            });
        }
        // See the status call above: a cap-cut invocation's nonzero status is
        // an artifact of the cut, and `truncated` records the incompleteness.
        if !output.truncated {
            require_success(&label, &output)?;
        }
        truncated |= output.truncated;
        remaining = remaining.saturating_sub(output.captured_len());
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !text.is_empty() {
            if !diff_text.is_empty() && !diff_text.ends_with('\n') {
                diff_text.push('\n');
            }
            diff_text.push_str(&text);
        }
    }

    let diff_sha256 = diff_record_sha256(&status_porcelain, &diff_text, &untracked, truncated)?;
    Ok(RepoDiff {
        status_porcelain,
        diff_text,
        untracked,
        diff_sha256,
        truncated,
    })
}

fn diff_record_sha256(
    status_porcelain: &str,
    diff_text: &str,
    untracked: &[String],
    truncated: bool,
) -> Result<String, DiffError> {
    let projection = json!({
        "domain": DIFF_HASH_DOMAIN,
        "status_porcelain": status_porcelain,
        "diff_text": diff_text,
        "untracked": untracked,
        "truncated": truncated,
    });
    let bytes = canonical_json_bytes(&projection)
        .map_err(|message| DiffError::Canonicalization { message })?;
    Ok(sha256_digest(&bytes))
}

fn require_git_repo(repo: &Path) -> Result<(), DiffError> {
    match fs::symlink_metadata(repo.join(".git")) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(DiffError::NotARepository {
            path: repo.to_path_buf(),
        }),
        Err(error) => Err(DiffError::Io {
            path: repo.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

fn require_success(label: &str, output: &GitOutput) -> Result<(), DiffError> {
    if output.exit_code == Some(0) {
        return Ok(());
    }
    let code = output
        .exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    Err(DiffError::GitFailed {
        args: label.to_string(),
        message: format!(
            "exit {code}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    })
}

struct GitOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    timed_out: bool,
    truncated: bool,
}

impl GitOutput {
    fn captured_len(&self) -> u64 {
        (self.stdout.len() + self.stderr.len()) as u64
    }
}

/// Run one bounded, local-only git invocation.
///
/// The child gets a cleared environment rebuilt from PATH plus the fixed
/// hermetic git variables (see the module docs). stdout and stderr are read on
/// helper threads that stop at their caps (setting an overflow flag that
/// triggers a kill); the main loop enforces the wall timeout.
fn run_git_bounded(
    args: &[&str],
    repo: &Path,
    cap_bytes: u64,
    timeout_ms: u64,
) -> Result<GitOutput, DiffError> {
    let label = args.join(" ");
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-c")
        .arg("core.quotepath=false")
        .args(args);
    command.current_dir(repo);
    command.env_clear();
    command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", GIT_NULL_DEVICE);
    command.env("GIT_ATTR_NOSYSTEM", "1");
    command.env("LC_ALL", "C");
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| DiffError::GitFailed {
        args: label.clone(),
        message: format!("failed to spawn git: {error}"),
    })?;

    let stdout_overflow = Arc::new(AtomicBool::new(false));
    let stderr_overflow = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_capped_reader(
        child.stdout.take().expect("stdout was piped"),
        cap_bytes,
        Arc::clone(&stdout_overflow),
    );
    let stderr_reader = spawn_capped_reader(
        child.stderr.take().expect("stderr was piped"),
        cap_bytes.min(MAX_DIFF_STDERR_BYTES),
        Arc::clone(&stderr_overflow),
    );

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let exit_code = loop {
        if stdout_overflow.load(Ordering::SeqCst) || stderr_overflow.load(Ordering::SeqCst) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                break child.wait().ok().and_then(|status| status.code());
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().ok().and_then(|status| status.code());
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(GitOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
        truncated: stdout_overflow.load(Ordering::SeqCst),
    })
}

fn spawn_capped_reader<R: Read + Send + 'static>(
    mut reader: R,
    cap_bytes: u64,
    overflow: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut captured: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if captured.len() as u64 >= cap_bytes {
                        overflow.store(true, Ordering::SeqCst);
                        break;
                    }
                    let room = (cap_bytes - captured.len() as u64) as usize;
                    let take = room.min(n);
                    captured.extend_from_slice(&chunk[..take]);
                    if take < n {
                        overflow.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
        captured
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::AtomicU64;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-m3-diff-{}-{}-{}",
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

    fn init_repo() -> PathBuf {
        let dir = unique_dir("repo");
        fs::write(dir.join("tracked.txt"), "line one\nline two\n").expect("write tracked");
        git(&["init"], &dir);
        git(&["add", "."], &dir);
        git(
            &[
                "-c",
                "user.name=diff-test",
                "-c",
                "user.email=diff-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
            &dir,
        );
        dir
    }

    #[test]
    fn clean_repo_captures_an_empty_diff() {
        let repo = init_repo();
        let diff = capture_diff(&repo, &DiffCaps::default()).expect("capture");
        assert_eq!(diff.status_porcelain, "");
        assert_eq!(diff.diff_text, "");
        assert!(diff.untracked.is_empty());
        assert!(!diff.truncated);
        assert!(diff.diff_sha256.starts_with("sha256:"));
        assert_eq!(diff.diff_sha256, diff.computed_sha256().expect("recompute"));
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn modified_and_untracked_files_both_appear() {
        let repo = init_repo();
        fs::write(repo.join("tracked.txt"), "line one\nline two changed\n").expect("modify");
        fs::write(repo.join("untracked.txt"), "new file\n").expect("add untracked");

        let diff = capture_diff(&repo, &DiffCaps::default()).expect("capture");
        assert!(
            diff.status_porcelain.contains("tracked.txt"),
            "status must name the modified file: {}",
            diff.status_porcelain
        );
        assert!(
            diff.status_porcelain.contains("?? untracked.txt"),
            "status must name the untracked file: {}",
            diff.status_porcelain
        );
        assert_eq!(diff.untracked, vec!["untracked.txt".to_string()]);
        assert!(
            diff.diff_text.contains("-line two"),
            "diff must contain the removed line: {}",
            diff.diff_text
        );
        assert!(
            diff.diff_text.contains("+line two changed"),
            "diff must contain the added line: {}",
            diff.diff_text
        );
        assert!(!diff.truncated);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn staged_changes_are_captured_too() {
        let repo = init_repo();
        fs::write(repo.join("tracked.txt"), "line one\nstaged change\n").expect("modify");
        git(&["add", "tracked.txt"], &repo);

        let diff = capture_diff(&repo, &DiffCaps::default()).expect("capture");
        assert!(
            diff.diff_text.contains("+staged change"),
            "staged diff must be captured: {}",
            diff.diff_text
        );
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn byte_cap_marks_the_capture_truncated() {
        let repo = init_repo();
        // A change far larger than the cap.
        let big = "x".repeat(64 * 1024);
        fs::write(repo.join("tracked.txt"), &big).expect("write big");

        let caps = DiffCaps {
            max_bytes: 512,
            timeout_ms: DEFAULT_DIFF_TIMEOUT_MS,
        };
        let diff = capture_diff(&repo, &caps).expect("capture");
        assert!(diff.truncated, "cap breach must be flagged");
        assert!(
            (diff.diff_text.len() as u64) <= 512,
            "captured diff must stay within the cap, got {} bytes",
            diff.diff_text.len()
        );
        // The record still hashes and re-hashes deterministically.
        assert_eq!(diff.diff_sha256, diff.computed_sha256().expect("recompute"));
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn non_repository_is_refused() {
        let dir = unique_dir("not-a-repo");
        fs::write(dir.join("file.txt"), "x\n").expect("write");
        let error = capture_diff(&dir, &DiffCaps::default()).expect_err("refused");
        assert_eq!(error.kind(), DiffErrorKind::NotARepository);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_hash_covers_status_text_and_truncation() {
        let repo = init_repo();
        fs::write(repo.join("untracked.txt"), "new\n").expect("add untracked");
        let diff = capture_diff(&repo, &DiffCaps::default()).expect("capture");

        let mut mutated = diff.clone();
        mutated.untracked = vec!["other.txt".to_string()];
        assert_ne!(
            mutated.computed_sha256().expect("recompute"),
            diff.diff_sha256
        );

        let mut mutated = diff.clone();
        mutated.truncated = true;
        assert_ne!(
            mutated.computed_sha256().expect("recompute"),
            diff.diff_sha256
        );

        let mut mutated = diff;
        mutated.diff_text.push('x');
        assert_ne!(
            mutated.computed_sha256().expect("recompute"),
            mutated.diff_sha256
        );
        let _ = fs::remove_dir_all(&repo);
    }
}
