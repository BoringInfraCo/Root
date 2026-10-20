//! Real OpenCode trial harness (Milestone 6).
//!
//! [`RealHarness`] runs one **real** OpenCode trial: the pinned `1.18.27`
//! CLI is executed as `opencode run --format json --model <model> <prompt>`
//! under the existing [`crate::supervisor::run_supervised`] wrapper (process
//! group kill, output cap, wall timeout), inside a trial repository whose
//! environment comes from [`crate::trial_env`].
//!
//! The output shape is deliberately identical to the fake path's: a
//! [`SupervisedResult`] with the parsed event stream, the raw stdout/stderr
//! bytes, and the terminal classification. Everything downstream — the
//! acceptance evaluator, the evidence seal, the eval store, the feasibility
//! report — is unchanged.
//!
//! # Binary identity gate (fail closed)
//!
//! The harness refuses to run anything but the hash-verified `1.18.27` CLI:
//! the SHA-256 of the binary is computed and compared against
//! [`OPENCODE_PINNED_SHA256`] **before the first use**; a mismatch is a hard
//! error ([`RealHarnessError::BinaryDigestMismatch`]) and no process is
//! spawned. The digest is the one recorded by the Milestone 0 host probe
//! (`Docs/Eval/milestone-0/HOST-PROBE.md` §1) and re-verified by the
//! Milestone 6 integration probe (§0) — the npm platform package, the
//! assembled wrapper, and the GitHub release asset all agree on it.
//!
//! The pin matters: the operator's host carries OpenCode `2.0.32`, and
//! running *that* binary in a scored trial would silently invalidate the
//! experiment.
//!
//! [`RealHarness::open_with_expected_digest`] is the documented **test-double
//! seam**: it performs the very same digest verification, but against a
//! caller-supplied expectation, so the crate's own end-to-end tests can drive
//! the real path with a stub binary. A scored run uses [`RealHarness::open`].
//!
//! # Invocation
//!
//! ```text
//! <binary> run --format json --model <model> [extra args…] <prompt>
//! ```
//!
//! `--format json` is the frozen NDJSON contract of
//! `Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md`; the prompt is the task
//! record's prompt (inline or file) passed as the message argument; stdin is
//! null; the working directory is the trial repository.
//!
//! `--pure` is **not** a default extra argument. The Milestone 6 probe used
//! it in its §A.8 recipe, but it is a behavioural deviation from the pinned
//! upstream CLI surface, and the reason the prewarm exists in the first place
//! is that the plugin install is a *detached background task* that is killed
//! with the CLI when it exits — a cold trial is therefore non-deterministic
//! and may leave the trial without the plugin at all. The prewarmed config
//! removes that race, so the default is the unmodified CLI surface; an
//! operator who wants the probe's exact recipe can pass `--pure` as an extra
//! argument.
//!
//! # Provider errors and the retry policy
//!
//! The `1.18.27` NDJSON stream carries **no `error` envelope** for a
//! provider failure: the probe observed a `ProviderHeaderTimeoutError` only
//! in `$XDG_DATA_HOME/opencode/log/opencode.log`, with a *truncated* stream
//! and a **zero** exit code. A harness that only watched stdout would score
//! that trial as a completed success.
//!
//! [`RealHarness::run`] therefore classifies the run itself:
//!
//! - a provider marker in the trial's OpenCode log (or in stderr) makes the
//!   run a [`TerminationKind::ProviderError`] — the existing taxonomy class
//!   that is *neither* an agent failure *nor* an invalid trial;
//! - the classification is what the driver scores, and what triggers the
//!   single documented retry.
//!
//! # Retry policy
//!
//! **One** retry, never more ([`REAL_HARNESS_MAX_RETRIES`] = 1). The retry
//! is taken only for
//!
//! - a detected provider error (any marker), or
//! - a wall-timeout kill, whose stream is truncated mid-turn.
//!
//! The retry uses a **fresh materialized trial** (the driver re-materializes
//! the trial tree, re-seeds it, and re-runs), so a retry never observes the
//! residue of the failed attempt. Nothing else is ever retried: a denial, a
//! completed run, an output-cap kill, a cancellation, and a spawn failure
//! are all final. The decision is never silent — it is carried in
//! [`RetryDecision`] and recorded in the trial summary.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::events::MODEL_OBSERVED_LIMITATION;
use crate::supervisor::{run_supervised, SupervisedResult, SupervisorConfig, TerminationKind};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The pinned OpenCode CLI version.
pub const OPENCODE_PINNED_VERSION: &str = "1.18.27";

/// The pinned SHA-256 (bare hex) of the OpenCode `1.18.27` CLI binary.
///
/// Recorded by the Milestone 0 host probe and re-verified by the Milestone 6
/// integration probe. The harness refuses any other binary.
pub const OPENCODE_PINNED_SHA256: &str =
    "f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6";

/// The pinned digest of the OpenCode `1.18.27` CLI binary (canonical form).
pub const OPENCODE_PINNED_SHA256_DIGEST: &str =
    "sha256:f35111b79296714194653fc8e74f28f6732589bacf0fbf59504c592ef6a727b6";

/// The default requested model (the built-in free provider of `1.18.27`).
pub const DEFAULT_REAL_MODEL: &str = "opencode/big-pickle";

/// The harness name recorded in the plan's harness lock.
pub const REAL_HARNESS_NAME: &str = "opencode";

/// The documented retry cap: one retry, never more.
pub const REAL_HARNESS_MAX_RETRIES: u32 = 1;

/// The `--format` value of the frozen NDJSON contract.
pub const OPENCODE_RUN_FORMAT: &str = "json";

/// Strong provider-error markers, matched anywhere in the run's output.
pub const PROVIDER_ERROR_MARKERS: [&str; 3] = [
    "ProviderHeaderTimeoutError",
    "Provider response headers timed out",
    "ProviderError",
];

/// Weak provider-error marker (OpenCode's log-level `stream error`).
///
/// Only matched in stderr and in the OpenCode log — never in the model's own
/// stdout text, where the phrase could be model-authored content.
pub const PROVIDER_STREAM_MARKER: &str = "stream error";

/// Largest OpenCode log file scanned for provider markers.
pub const MAX_PROVIDER_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Largest number of OpenCode log files scanned for provider markers.
pub const MAX_PROVIDER_LOG_FILES: usize = 8;

/// Length of the excerpt kept with a detected provider error.
pub const PROVIDER_EXCERPT_LEN: usize = 240;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`RealHarnessError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RealHarnessErrorKind {
    /// The binary does not exist or cannot be read.
    BinaryUnreadable,
    /// The binary's digest does not match the expected one.
    BinaryDigestMismatch,
    /// The configuration is unusable (empty model, empty binary path, …).
    InvalidConfig,
}

/// Errors produced while opening or configuring the real harness.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RealHarnessError {
    #[error("harness binary `{path}` cannot be read: {message}")]
    BinaryUnreadable { path: PathBuf, message: String },

    #[error("harness binary `{path}` has sha256 {found}, expected {expected}")]
    BinaryDigestMismatch {
        path: PathBuf,
        found: String,
        expected: String,
    },

    #[error("invalid real-harness configuration: {message}")]
    InvalidConfig { message: String },
}

impl RealHarnessError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> RealHarnessErrorKind {
        match self {
            Self::BinaryUnreadable { .. } => RealHarnessErrorKind::BinaryUnreadable,
            Self::BinaryDigestMismatch { .. } => RealHarnessErrorKind::BinaryDigestMismatch,
            Self::InvalidConfig { .. } => RealHarnessErrorKind::InvalidConfig,
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration of the real OpenCode harness.
///
/// - `binary` — path to the hash-verified `1.18.27` CLI;
/// - `model` — the requested model (`provider/model`), default
///   [`DEFAULT_REAL_MODEL`];
/// - `extra_args` — extra CLI arguments appended after `--model <model>` and
///   before the prompt. Empty by default (see the module docs for why
///   `--pure` is not a default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealHarnessConfig {
    /// Path to the OpenCode CLI binary.
    pub binary: PathBuf,
    /// Requested model, recorded verbatim in the execution row.
    pub model: String,
    /// Extra CLI arguments (appended after `--model <model>`).
    pub extra_args: Vec<String>,
}

impl RealHarnessConfig {
    /// Build a config with the documented defaults.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            model: DEFAULT_REAL_MODEL.to_string(),
            extra_args: Vec::new(),
        }
    }
}

/// The verified identity of a harness binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessIdentity {
    /// Harness name (recorded in the plan's harness lock).
    pub name: String,
    /// Exact harness version string.
    pub version: String,
    /// Canonical `sha256:<hex>` digest of the harness binary.
    pub binary_sha256: String,
}

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// A real OpenCode harness bound to a digest-verified binary.
#[derive(Debug, Clone)]
pub struct RealHarness {
    config: RealHarnessConfig,
    identity: HarnessIdentity,
}

impl RealHarness {
    /// Open the harness against the **pinned** OpenCode `1.18.27` binary.
    ///
    /// The binary's SHA-256 is verified before the first use; a mismatch
    /// fails closed and no process is ever spawned.
    pub fn open(config: RealHarnessConfig) -> Result<Self, RealHarnessError> {
        Self::open_with_expected_digest(config, OPENCODE_PINNED_SHA256, OPENCODE_PINNED_VERSION)
    }

    /// Open the harness against a caller-supplied digest expectation.
    ///
    /// **Test-double seam.** The verification itself is identical to
    /// [`RealHarness::open`] — the digest is computed and compared before the
    /// first use, and a mismatch fails closed — but the *expected* digest is
    /// the caller's. This exists so the crate's own end-to-end tests can
    /// drive the real path with a stub binary that mimics
    /// `opencode run --format json`. A scored run must use
    /// [`RealHarness::open`], which pins the upstream `1.18.27` digest.
    pub fn open_with_expected_digest(
        config: RealHarnessConfig,
        expected_sha256: &str,
        version: &str,
    ) -> Result<Self, RealHarnessError> {
        if config.binary.as_os_str().is_empty() {
            return Err(RealHarnessError::InvalidConfig {
                message: "the harness binary path must not be empty".to_string(),
            });
        }
        if config.model.trim().is_empty() {
            return Err(RealHarnessError::InvalidConfig {
                message: "the requested model must not be empty".to_string(),
            });
        }
        let expected = expected_sha256.trim().to_ascii_lowercase();
        let found = file_sha256(&config.binary)?;
        if found != expected {
            return Err(RealHarnessError::BinaryDigestMismatch {
                path: config.binary.clone(),
                found,
                expected,
            });
        }
        Ok(Self {
            config,
            identity: HarnessIdentity {
                name: REAL_HARNESS_NAME.to_string(),
                version: version.to_string(),
                binary_sha256: format!("sha256:{found}"),
            },
        })
    }

    /// The verified identity of the harness binary.
    pub fn identity(&self) -> &HarnessIdentity {
        &self.identity
    }

    /// The requested model, recorded verbatim in the execution row.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// The full argv of one run, including the prompt.
    ///
    /// ```text
    /// <binary> run --format json --model <model> [extra args…] <prompt>
    /// ```
    pub fn argv(&self, prompt: &str) -> Vec<String> {
        let mut argv = vec![
            self.config.binary.to_string_lossy().into_owned(),
            "run".to_string(),
            "--format".to_string(),
            OPENCODE_RUN_FORMAT.to_string(),
            "--model".to_string(),
            self.config.model.clone(),
        ];
        argv.extend(self.config.extra_args.iter().cloned());
        argv.push(prompt.to_string());
        argv
    }

    /// Run one real trial and classify the outcome.
    ///
    /// The child runs with `request.env` only (the supervisor clears the
    /// environment and rebuilds it), with a null stdin, inside
    /// `request.cwd`, under the wall timeout and output cap of the request.
    pub fn run(&self, request: &RealRunRequest) -> RealRunOutcome {
        let argv = self.argv(&request.prompt);
        let config = SupervisorConfig::new(request.wall_timeout_ms, request.output_cap_bytes);
        let supervised = run_supervised(&argv, &request.env, &request.cwd, &config);
        let provider_error = detect_provider_error(&supervised, &request.env);
        let termination = if provider_error.is_some() {
            TerminationKind::ProviderError
        } else {
            supervised.termination
        };
        let retry = decide_retry(&supervised, provider_error.as_ref());
        RealRunOutcome {
            supervised,
            identity: self.identity.clone(),
            argv,
            requested_model: self.config.model.clone(),
            observed_model: None,
            observed_model_limitation: MODEL_OBSERVED_LIMITATION.to_string(),
            provider_error,
            termination,
            retry,
        }
    }
}

// ---------------------------------------------------------------------------
// Run request and outcome
// ---------------------------------------------------------------------------

/// One real-harness invocation.
#[derive(Debug, Clone)]
pub struct RealRunRequest {
    /// Trial working directory (the trial repository checkout).
    pub cwd: PathBuf,
    /// Explicit child environment (the child inherits nothing else).
    pub env: Vec<(String, String)>,
    /// The prompt text (the task record's prompt).
    pub prompt: String,
    /// Wall budget in milliseconds.
    pub wall_timeout_ms: u64,
    /// Stdout cap in bytes.
    pub output_cap_bytes: u64,
}

/// Why a run is eligible for the single documented retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetryReason {
    /// The provider failed (rate limit, outage, header timeout).
    ProviderError,
    /// The provider's response headers timed out.
    ProviderHeaderTimeout,
    /// The stream was cut mid-turn by the wall timeout.
    TruncatedStream,
}

impl RetryReason {
    /// Stable snake-case token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderError => "provider_error",
            Self::ProviderHeaderTimeout => "provider_header_timeout",
            Self::TruncatedStream => "truncated_stream",
        }
    }
}

/// The (never silent) retry decision for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    /// True when the run may be retried once.
    pub retryable: bool,
    /// The reason, present exactly when `retryable` is true.
    pub reason: Option<RetryReason>,
    /// Human-readable explanation (always present).
    pub detail: String,
}

impl RetryDecision {
    /// A final decision: no retry.
    pub fn none(detail: impl Into<String>) -> Self {
        Self {
            retryable: false,
            reason: None,
            detail: detail.into(),
        }
    }
}

/// A detected provider failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderErrorDetail {
    /// The marker that matched.
    pub marker: String,
    /// Where the marker was found (`stdout`, `stderr`, or `opencode-log`).
    pub source: String,
    /// A bounded excerpt around the marker.
    pub excerpt: String,
}

/// The complete outcome of one real-harness run.
#[derive(Debug, Clone)]
pub struct RealRunOutcome {
    /// The supervised result (events, raw bytes, residue report).
    pub supervised: SupervisedResult,
    /// The verified harness identity.
    pub identity: HarnessIdentity,
    /// The argv that was executed.
    pub argv: Vec<String>,
    /// The requested model, recorded verbatim.
    pub requested_model: String,
    /// Always `None` (M0 identity policy: the stream names no model).
    pub observed_model: Option<String>,
    /// Why the observed model is unavailable.
    pub observed_model_limitation: String,
    /// The detected provider failure, when the run hit one.
    pub provider_error: Option<ProviderErrorDetail>,
    /// The effective terminal classification (the provider overlay applied).
    pub termination: TerminationKind,
    /// The retry decision.
    pub retry: RetryDecision,
}

// ---------------------------------------------------------------------------
// Provider detection and retry policy
// ---------------------------------------------------------------------------

/// Detect a provider failure in the run's output or in the trial's OpenCode log.
///
/// The `1.18.27` NDJSON contract carries no `error` envelope for a provider
/// failure: the probe observed `ProviderHeaderTimeoutError` only in
/// `$XDG_DATA_HOME/opencode/log/opencode.log`, with a truncated stream and a
/// zero exit code. Detection order: stdout (strong markers only), stderr
/// (strong and weak markers), then the OpenCode log (strong and weak).
pub fn detect_provider_error(
    supervised: &SupervisedResult,
    env: &[(String, String)],
) -> Option<ProviderErrorDetail> {
    if let Some(detail) = scan_bytes(
        &supervised.raw_stdout,
        "stdout",
        PROVIDER_ERROR_MARKERS.iter().copied(),
    ) {
        return Some(detail);
    }
    if let Some(detail) = scan_bytes(
        &supervised.raw_stderr,
        "stderr",
        PROVIDER_ERROR_MARKERS
            .iter()
            .copied()
            .chain(std::iter::once(PROVIDER_STREAM_MARKER)),
    ) {
        return Some(detail);
    }
    scan_opencode_log(env)
}

/// Search one buffer for a provider marker.
fn scan_bytes(
    bytes: &[u8],
    source: &str,
    markers: impl Iterator<Item = &'static str>,
) -> Option<ProviderErrorDetail> {
    let text = String::from_utf8_lossy(bytes);
    for marker in markers {
        if let Some(position) = text.find(marker) {
            return Some(ProviderErrorDetail {
                marker: marker.to_string(),
                source: source.to_string(),
                excerpt: excerpt(&text, position, marker.len()),
            });
        }
    }
    None
}

/// Search the trial's OpenCode log directory for a provider marker.
fn scan_opencode_log(env: &[(String, String)]) -> Option<ProviderErrorDetail> {
    let data_home = env
        .iter()
        .find(|(key, _)| key == "XDG_DATA_HOME")
        .map(|(_, value)| PathBuf::from(value))?;
    let log_dir = data_home.join("opencode").join("log");
    let entries = fs_read_dir_sorted(&log_dir);
    let mut scanned = 0usize;
    for path in entries {
        if scanned >= MAX_PROVIDER_LOG_FILES {
            break;
        }
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() || metadata.len() > MAX_PROVIDER_LOG_BYTES {
            continue;
        }
        scanned += 1;
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for marker in PROVIDER_ERROR_MARKERS
            .iter()
            .copied()
            .chain(std::iter::once(PROVIDER_STREAM_MARKER))
        {
            if let Some(position) = text.find(marker) {
                return Some(ProviderErrorDetail {
                    marker: marker.to_string(),
                    source: "opencode-log".to_string(),
                    excerpt: excerpt(&text, position, marker.len()),
                });
            }
        }
    }
    None
}

/// Sorted list of the regular files in `dir` (empty when unreadable).
fn fs_read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return paths;
    };
    for entry in entries.flatten() {
        paths.push(entry.path());
    }
    paths.sort();
    paths
}

/// A bounded excerpt around a marker.
fn excerpt(text: &str, position: usize, marker_len: usize) -> String {
    let start = text[..position]
        .char_indices()
        .rev()
        .take(PROVIDER_EXCERPT_LEN / 2)
        .last()
        .map(|(index, _)| index)
        .unwrap_or(position);
    let end = text[position + marker_len..]
        .char_indices()
        .take(PROVIDER_EXCERPT_LEN / 2)
        .last()
        .map(|(index, char)| index + char.len_utf8())
        .unwrap_or_else(|| text.len().min(position + marker_len));
    text[start..end.max(start)].to_string()
}

/// Decide whether a run may be retried once.
///
/// Retryable: a detected provider error, or a wall-timeout kill (whose
/// stream is truncated mid-turn). Everything else is final — a denial, a
/// completed run, an output-cap kill, a cancellation, and a spawn failure
/// are never retried.
pub fn decide_retry(
    supervised: &SupervisedResult,
    provider_error: Option<&ProviderErrorDetail>,
) -> RetryDecision {
    if let Some(detail) = provider_error {
        let reason = if detail.marker.contains("HeaderTimeout")
            || detail.marker.contains("headers timed out")
        {
            RetryReason::ProviderHeaderTimeout
        } else {
            RetryReason::ProviderError
        };
        return RetryDecision {
            retryable: true,
            reason: Some(reason),
            detail: format!(
                "provider failure detected in {}: {}",
                detail.source, detail.marker
            ),
        };
    }
    if supervised.termination == TerminationKind::TimeoutKilled {
        return RetryDecision {
            retryable: true,
            reason: Some(RetryReason::TruncatedStream),
            detail: "the wall timeout cut the stream mid-turn".to_string(),
        };
    }
    RetryDecision::none(format!(
        "no retry: termination={}",
        supervised.termination.as_str()
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// SHA-256 (bare hex) of a file, read in bounded chunks.
pub fn file_sha256(path: &Path) -> Result<String, RealHarnessError> {
    let mut file = File::open(path).map_err(|error| RealHarnessError::BinaryUnreadable {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| RealHarnessError::BinaryUnreadable {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_binary(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("opencode-stub");
        std::fs::write(&path, body).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod");
        }
        path
    }

    fn unique_dir(prefix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-realharness-{prefix}-{}-{id}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("clear stale temp dir");
        }
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn pinned_constants_match_the_probe() {
        assert_eq!(OPENCODE_PINNED_VERSION, "1.18.27");
        assert_eq!(OPENCODE_PINNED_SHA256.len(), 64);
        assert_eq!(
            OPENCODE_PINNED_SHA256_DIGEST,
            format!("sha256:{OPENCODE_PINNED_SHA256}")
        );
        assert_eq!(DEFAULT_REAL_MODEL, "opencode/big-pickle");
    }

    #[test]
    fn digest_gate_fails_closed_on_a_foreign_binary() {
        let dir = unique_dir("gate");
        let binary = stub_binary(&dir, "#!/bin/sh\nexit 0\n");
        let config = RealHarnessConfig::new(&binary);
        let error = RealHarness::open(config).expect_err("a foreign binary must be refused");
        assert_eq!(error.kind(), RealHarnessErrorKind::BinaryDigestMismatch);
        assert!(matches!(
            error,
            RealHarnessError::BinaryDigestMismatch { .. }
        ));
        // The test-double seam accepts the stub when the caller states the
        // expectation explicitly.
        let digest = file_sha256(&binary).expect("digest");
        let harness = RealHarness::open_with_expected_digest(
            RealHarnessConfig::new(&binary),
            &digest,
            "stub",
        )
        .expect("test double opens");
        assert_eq!(harness.identity().binary_sha256, format!("sha256:{digest}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn digest_gate_fails_closed_on_a_missing_binary() {
        let error = RealHarness::open(RealHarnessConfig::new("/nonexistent/opencode-xyz"))
            .expect_err("a missing binary must be refused");
        assert_eq!(error.kind(), RealHarnessErrorKind::BinaryUnreadable);
    }

    #[test]
    fn argv_is_the_frozen_run_shape() {
        let dir = unique_dir("argv");
        let binary = stub_binary(&dir, "#!/bin/sh\nexit 0\n");
        let digest = file_sha256(&binary).expect("digest");
        let mut config = RealHarnessConfig::new(&binary);
        config.extra_args = vec!["--pure".to_string()];
        let harness = RealHarness::open_with_expected_digest(config, &digest, "stub")
            .expect("test double opens");
        let argv = harness.argv("install ripgrep using Root");
        assert_eq!(
            argv,
            vec![
                binary.to_string_lossy().into_owned(),
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--model".to_string(),
                DEFAULT_REAL_MODEL.to_string(),
                "--pure".to_string(),
                "install ripgrep using Root".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provider_markers_are_detected_and_classified() {
        let detail = ProviderErrorDetail {
            marker: "ProviderHeaderTimeoutError".to_string(),
            source: "opencode-log".to_string(),
            excerpt: "...".to_string(),
        };
        let supervised = SupervisedResult {
            termination: TerminationKind::Completed,
            exit_code: Some(0),
            timed_out: false,
            killed: false,
            wall_ms: 314_923,
            events: Vec::new(),
            dropped_lines: Vec::new(),
            raw_bytes_sha256: "sha256:0".to_string(),
            raw_stdout: b"{\"type\":\"text\"}\n".to_vec(),
            raw_stderr: Vec::new(),
            stdout_truncated: false,
            residue: crate::supervisor::ResidueReport {
                cleanup_ok: true,
                leftover_killed: 0,
                note: "test".to_string(),
            },
        };
        let decision = decide_retry(&supervised, Some(&detail));
        assert!(decision.retryable);
        assert_eq!(decision.reason, Some(RetryReason::ProviderHeaderTimeout));

        let final_decision = decide_retry(&supervised, None);
        assert!(!final_decision.retryable);
        assert!(final_decision.detail.contains("no retry"));
    }

    #[test]
    fn truncated_streams_are_retryable_and_completed_runs_are_not() {
        let mut supervised = SupervisedResult {
            termination: TerminationKind::TimeoutKilled,
            exit_code: None,
            timed_out: true,
            killed: true,
            wall_ms: 900_000,
            events: Vec::new(),
            dropped_lines: Vec::new(),
            raw_bytes_sha256: "sha256:0".to_string(),
            raw_stdout: Vec::new(),
            raw_stderr: Vec::new(),
            stdout_truncated: true,
            residue: crate::supervisor::ResidueReport {
                cleanup_ok: true,
                leftover_killed: 0,
                note: "test".to_string(),
            },
        };
        let decision = decide_retry(&supervised, None);
        assert!(decision.retryable);
        assert_eq!(decision.reason, Some(RetryReason::TruncatedStream));

        supervised.termination = TerminationKind::Completed;
        supervised.timed_out = false;
        supervised.killed = false;
        assert!(!decide_retry(&supervised, None).retryable);

        supervised.termination = TerminationKind::OutputCapExceeded;
        assert!(
            !decide_retry(&supervised, None).retryable,
            "an output-cap kill is final"
        );

        supervised.termination = TerminationKind::Denied;
        assert!(!decide_retry(&supervised, None).retryable);
    }
}
