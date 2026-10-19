//! Evidence sealing, the content-addressed artifact store, and redaction
//! (Milestone 3, Phase A).
//!
//! # Artifact store
//!
//! [`ArtifactStore`] keeps every evidence artifact under
//! `<evals_root>/artifacts/sha256/<first2>/<hex>` (the audit's
//! `$ROOT_DIR/evals/artifacts/sha256/...` layout). Writes are atomic — a temp
//! file in the destination shard plus `rename` — and every `put` re-reads the
//! stored bytes and checks their hash before returning, so a store that cannot
//! read back what it wrote fails closed. Reads ([`ArtifactStore::get`]) and
//! [`ArtifactStore::verify`] hash-check on read: tampering with artifact bytes
//! is detected, never silently accepted.
//!
//! # Redaction
//!
//! [`redact_bytes`] applies two deterministic, documented pattern families to
//! raw evidence bytes **before persistence** and reports per-pattern counts as
//! [`RedactionEvent`]s:
//!
//! 1. `secret-key-assignment` — a value assigned to a key matching the trial
//!    materializer's blocklist ([`crate::secret_pattern_hit`]: `AWS_*`,
//!    `GITHUB_*`, `OPENAI_*`, `ANTHROPIC_*`, `*_TOKEN`, `*_SECRET`, `*_KEY`),
//!    in either `KEY=value` or `"KEY": "value"` form;
//! 2. `json-secret-key` — a quoted JSON string value under one of the
//!    `auth.json`-style credential keys in [`JSON_SECRET_KEYS`] (`apiKey`,
//!    `token`, `secret`, `password`, and their common case/snake variants).
//!
//! Values are replaced with `[REDACTED:<pattern>]`; already-redacted values
//! are left alone, so redaction is idempotent. This is a **heuristic**, not a
//! secret manager: it does not scan for credential-shaped tokens outside these
//! patterns and must not be treated as a guarantee.
//!
//! # Sealed manifest
//!
//! [`seal_manifest`] builds an [`EvidenceManifest`] and computes
//! [`EvidenceManifest::seal_sha256`] over the canonical JSON projection of
//! every field except the seal itself (canonical key ordering via
//! [`crate::canonical_json`], hashing via [`crate::sha256_digest`]).
//! [`verify_seal`] recomputes and compares, so tampering with **any** manifest
//! field — including `sealed_at_ms` and artifact ordering — fails;
//! [`verify_artifacts`] re-verifies every referenced artifact against the
//! store, so tampering with artifact bytes fails too.
//!
//! The projection is deliberately float-free (floating-point numbers are not
//! canonical JSON), so verifier/execution data enters it as strings, integers,
//! and booleans only.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::digest::{is_canonical_sha256_digest, sha256_digest, DIGEST_PREFIX};
use crate::events::NormalizedEvent;
use crate::materializer::secret_pattern_hit;
use crate::supervisor::{SupervisedResult, TerminationKind};
use crate::verifier::{VerifierOutcome, VerifierVerdict};

/// Evidence-manifest schema version emitted and accepted by this crate.
pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Highest evidence-manifest schema version this crate can read.
pub const EVIDENCE_MAX_SUPPORTED_VERSION: u32 = 1;

/// Directory (under the evals root) holding the content-addressed store.
pub const ARTIFACT_STORE_DIR: &str = "artifacts";

/// Algorithm shard directory inside the artifact store.
pub const ARTIFACT_SHARD_DIR: &str = "sha256";

/// Largest single artifact this store accepts, in bytes.
///
/// Bounded evidence: raw streams, diffs, and verifier outputs are small; a
/// larger artifact is a bug or an attack, not evidence. Callers with a
/// different budget use [`ArtifactStore::with_max_artifact_bytes`].
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// Longest accepted artifact logical name, in bytes.
pub const MAX_ARTIFACT_LOGICAL_NAME_LEN: usize = 128;

/// Hash domain mixed into the seal projection.
pub const SEAL_HASH_DOMAIN: &str = "evidence-manifest.v1";

/// Hash domain mixed into the normalized-events digest projection.
pub const NORMALIZED_EVENTS_HASH_DOMAIN: &str = "normalized-events.v1";

/// Redaction pattern: a value assigned to a blocklisted secret key.
pub const REDACTION_SECRET_KEY_ASSIGNMENT: &str = "secret-key-assignment";

/// Redaction pattern: a quoted JSON value under an `auth.json`-style key.
pub const REDACTION_JSON_SECRET_KEY: &str = "json-secret-key";

/// Prefix of every redaction marker; also the idempotence sentinel.
pub const REDACTION_MARKER_PREFIX: &str = "[REDACTED:";

/// Lower-cased JSON keys whose quoted string values are redacted.
///
/// Case-insensitive: `"apiKey"` and `"apikey"` both match. This set is
/// intentionally small and documented; it is a heuristic, not a credential
/// registry.
pub const JSON_SECRET_KEYS: &[&str] = &[
    "apikey",
    "api_key",
    "token",
    "access_token",
    "accesstoken",
    "refresh_token",
    "id_token",
    "sessiontoken",
    "secret",
    "client_secret",
    "clientsecret",
    "secret_key",
    "secretaccesskey",
    "privatekey",
    "password",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Artifact store
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`EvidenceError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceErrorKind {
    /// A digest is not `sha256:<64 lowercase hex>`.
    InvalidDigest,
    /// An artifact logical name is empty or outside the allowed grammar.
    InvalidLogicalName,
    /// A single artifact exceeds the store's size cap.
    ArtifactTooLarge,
    /// The requested artifact is not present in the store.
    MissingArtifact,
    /// Stored bytes hash to something other than their content address.
    HashMismatch,
    /// A reference's declared byte count differs from the stored file.
    SizeMismatch,
    /// An I/O operation failed.
    Io,
    /// Canonical serialization of a hash projection failed.
    Canonicalization,
}

/// Errors produced by the artifact store and the evidence hash projections.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvidenceError {
    #[error("invalid digest in `{field}`: expected sha256:<64 lowercase hex>, found `{value}`")]
    InvalidDigest { field: String, value: String },

    #[error(
        "invalid artifact logical name `{name}`: expected 1-{max} characters of [A-Za-z0-9._:-]"
    )]
    InvalidLogicalName { name: String, max: usize },

    #[error("artifact is {bytes} bytes, over the {cap}-byte single-artifact cap")]
    ArtifactTooLarge { bytes: u64, cap: u64 },

    #[error("artifact `{sha256}` is missing at `{path}`")]
    MissingArtifact { sha256: String, path: PathBuf },

    #[error("artifact hash mismatch: expected {expected}, found {found}")]
    HashMismatch { expected: String, found: String },

    #[error(
        "artifact size mismatch: reference declares {expected} bytes, stored file has {found}"
    )]
    SizeMismatch { expected: u64, found: u64 },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },
}

impl EvidenceError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> EvidenceErrorKind {
        match self {
            Self::InvalidDigest { .. } => EvidenceErrorKind::InvalidDigest,
            Self::InvalidLogicalName { .. } => EvidenceErrorKind::InvalidLogicalName,
            Self::ArtifactTooLarge { .. } => EvidenceErrorKind::ArtifactTooLarge,
            Self::MissingArtifact { .. } => EvidenceErrorKind::MissingArtifact,
            Self::HashMismatch { .. } => EvidenceErrorKind::HashMismatch,
            Self::SizeMismatch { .. } => EvidenceErrorKind::SizeMismatch,
            Self::Io { .. } => EvidenceErrorKind::Io,
            Self::Canonicalization { .. } => EvidenceErrorKind::Canonicalization,
        }
    }
}

/// A content-addressed reference to one stored artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Caller-chosen role label (`stdout`, `diff`, `verifier-stderr`, ...).
    /// Not part of the content address; only `sha256` and `bytes` identify the
    /// stored bytes.
    pub logical_name: String,
    /// Canonical `sha256:<64 lowercase hex>` content address.
    pub sha256: String,
    /// Exact stored byte count.
    pub bytes: u64,
}

/// Content-addressed evidence store rooted at
/// `<evals_root>/artifacts/sha256/`.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    max_artifact_bytes: u64,
}

impl ArtifactStore {
    /// Open (creating on demand) the store under `evals_root`.
    pub fn new(evals_root: &Path) -> Self {
        Self {
            root: evals_root.join(ARTIFACT_STORE_DIR).join(ARTIFACT_SHARD_DIR),
            max_artifact_bytes: MAX_ARTIFACT_BYTES,
        }
    }

    /// Open a store with a caller-supplied single-artifact cap.
    ///
    /// Exists so tests and experiments with tighter budgets can exercise the
    /// cap without allocating [`MAX_ARTIFACT_BYTES`].
    pub fn with_max_artifact_bytes(evals_root: &Path, max_artifact_bytes: u64) -> Self {
        Self {
            root: evals_root.join(ARTIFACT_STORE_DIR).join(ARTIFACT_SHARD_DIR),
            max_artifact_bytes,
        }
    }

    /// The store root (`<evals_root>/artifacts/sha256`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The single-artifact byte cap in force for this store.
    pub fn max_artifact_bytes(&self) -> u64 {
        self.max_artifact_bytes
    }

    /// The on-disk path for a content address.
    pub fn path_for(&self, sha256: &str) -> Result<PathBuf, EvidenceError> {
        let hex = require_digest("sha256", sha256)?;
        Ok(self.root.join(&hex[..2]).join(hex))
    }

    /// Store `bytes` under its SHA-256 and return the reference.
    ///
    /// Atomic (temp file in the destination shard + `rename`, fsync
    /// best-effort) and hash-verified on completion: the stored bytes are
    /// read back and re-hashed, so a store that cannot read back what it just
    /// wrote fails closed. Storing the same content twice yields the same
    /// content address and path; the logical name is caller metadata only.
    pub fn put(&self, logical_name: &str, bytes: &[u8]) -> Result<ArtifactRef, EvidenceError> {
        validate_logical_name(logical_name)?;
        if bytes.len() as u64 > self.max_artifact_bytes {
            return Err(EvidenceError::ArtifactTooLarge {
                bytes: bytes.len() as u64,
                cap: self.max_artifact_bytes,
            });
        }
        let digest = sha256_digest(bytes);
        let hex = digest
            .strip_prefix(DIGEST_PREFIX)
            .expect("sha256_digest always carries the prefix");
        let shard_dir = self.root.join(&hex[..2]);
        let target = shard_dir.join(hex);

        if !target.exists() {
            fs::create_dir_all(&shard_dir).map_err(|error| io_error(&shard_dir, error))?;
            set_mode(&shard_dir, 0o700)?;
            let temp_path = shard_dir.join(format!(
                ".tmp-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            {
                let mut file =
                    fs::File::create(&temp_path).map_err(|error| io_error(&temp_path, error))?;
                file.write_all(bytes)
                    .map_err(|error| io_error(&temp_path, error))?;
                // Best effort: the rename is the atomicity boundary; a failed
                // fsync degrades durability, never correctness.
                let _ = file.sync_all();
            }
            set_mode(&temp_path, 0o600)?;
            match fs::rename(&temp_path, &target) {
                Ok(()) => {}
                Err(error) => {
                    let _ = fs::remove_file(&temp_path);
                    // A concurrent writer may have published the same content
                    // address first; that is success, not failure.
                    if !target.exists() {
                        return Err(io_error(&target, error));
                    }
                }
            }
        }

        // Never trust a content-addressed path without checking the bytes it
        // currently holds.
        let stored = fs::read(&target).map_err(|error| io_error(&target, error))?;
        let found = sha256_digest(&stored);
        if found != digest {
            return Err(EvidenceError::HashMismatch {
                expected: digest,
                found,
            });
        }
        Ok(ArtifactRef {
            logical_name: logical_name.to_string(),
            sha256: digest,
            bytes: bytes.len() as u64,
        })
    }

    /// Read an artifact by content address, hash-verified on read.
    pub fn get(&self, sha256: &str) -> Result<Vec<u8>, EvidenceError> {
        let path = self.path_for(sha256)?;
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                EvidenceError::MissingArtifact {
                    sha256: sha256.to_string(),
                    path: path.clone(),
                }
            } else {
                io_error(&path, error)
            }
        })?;
        let found = sha256_digest(&bytes);
        if found != sha256 {
            return Err(EvidenceError::HashMismatch {
                expected: sha256.to_string(),
                found,
            });
        }
        Ok(bytes)
    }

    /// Verify one reference: the artifact exists, its bytes hash to the
    /// declared address, and its length matches the declared size.
    pub fn verify(&self, reference: &ArtifactRef) -> Result<(), EvidenceError> {
        let bytes = self.get(&reference.sha256)?;
        if bytes.len() as u64 != reference.bytes {
            return Err(EvidenceError::SizeMismatch {
                expected: reference.bytes,
                found: bytes.len() as u64,
            });
        }
        Ok(())
    }
}

fn validate_logical_name(name: &str) -> Result<(), EvidenceError> {
    let valid = !name.is_empty()
        && name.len() <= MAX_ARTIFACT_LOGICAL_NAME_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(EvidenceError::InvalidLogicalName {
            name: name.to_string(),
            max: MAX_ARTIFACT_LOGICAL_NAME_LEN,
        })
    }
}

fn io_error(path: &Path, error: std::io::Error) -> EvidenceError {
    EvidenceError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), EvidenceError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), EvidenceError> {
    Ok(())
}

/// Validate a digest and return its bare lowercase hex.
fn require_digest<'a>(field: &str, value: &'a str) -> Result<&'a str, EvidenceError> {
    if !is_canonical_sha256_digest(value) {
        return Err(EvidenceError::InvalidDigest {
            field: field.to_string(),
            value: value.to_string(),
        });
    }
    Ok(&value[DIGEST_PREFIX.len()..])
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// One redaction pattern family and how many values it replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionEvent {
    /// Pattern family identifier ([`REDACTION_SECRET_KEY_ASSIGNMENT`] or
    /// [`REDACTION_JSON_SECRET_KEY`]).
    pub pattern: String,
    /// Number of values replaced by this pattern.
    pub count: u64,
}

/// Redact secret-shaped values from evidence bytes before persistence.
///
/// Deterministic and idempotent; see the module docs for the two documented
/// pattern families and their limits. Returns the redacted bytes and one
/// [`RedactionEvent`] per pattern that matched (sorted by pattern name).
pub fn redact_bytes(bytes: &[u8]) -> (Vec<u8>, Vec<RedactionEvent>) {
    let mut output = Vec::with_capacity(bytes.len());
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut index = 0usize;
    while index < bytes.len() {
        match match_secret(bytes, index) {
            Some(found) => {
                output.extend_from_slice(&bytes[index..found.value_start]);
                output.extend_from_slice(REDACTION_MARKER_PREFIX.as_bytes());
                output.extend_from_slice(found.pattern.as_bytes());
                output.push(b']');
                *counts.entry(found.pattern).or_insert(0) += 1;
                index = found.value_end;
            }
            None => {
                output.push(bytes[index]);
                index += 1;
            }
        }
    }
    let events = counts
        .into_iter()
        .map(|(pattern, count)| RedactionEvent {
            pattern: pattern.to_string(),
            count,
        })
        .collect();
    (output, events)
}

struct SecretMatch {
    value_start: usize,
    value_end: usize,
    pattern: &'static str,
}

fn match_secret(bytes: &[u8], index: usize) -> Option<SecretMatch> {
    match bytes.get(index) {
        Some(b'"') => match_json_secret(bytes, index),
        Some(byte) if is_key_start(*byte) => match_env_secret(bytes, index),
        _ => None,
    }
}

fn is_key_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_value_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(byte, b'"' | b'\'' | b',' | b';' | b'&' | b'}' | b']' | b')')
}

fn already_redacted(value: &[u8]) -> bool {
    value.starts_with(REDACTION_MARKER_PREFIX.as_bytes())
}

/// `KEY=value` / `KEY='value'` / `KEY="value"` where `KEY` matches the
/// materializer blocklist.
fn match_env_secret(bytes: &[u8], index: usize) -> Option<SecretMatch> {
    // The key must start on a word boundary: `ITHUB_TOKEN` inside
    // `GITHUB_TOKEN` is not a key.
    if index > 0 && is_key_char(bytes[index - 1]) {
        return None;
    }
    let mut cursor = index;
    while cursor < bytes.len() && is_key_char(bytes[cursor]) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'=') {
        return None;
    }
    let key = std::str::from_utf8(&bytes[index..cursor]).ok()?;
    if !secret_pattern_hit(key) {
        return None;
    }

    let (value_start, value_end) = match bytes.get(cursor + 1) {
        Some(quote @ (b'"' | b'\'')) => {
            let quote = *quote;
            let value_start = cursor + 2;
            let mut probe = value_start;
            let mut end = None;
            while probe < bytes.len() {
                let byte = bytes[probe];
                if quote == b'"' && byte == b'\\' {
                    probe += 2;
                    continue;
                }
                if byte == quote {
                    end = Some(probe);
                    break;
                }
                probe += 1;
            }
            (value_start, end?)
        }
        Some(_) => {
            let value_start = cursor + 1;
            let mut probe = value_start;
            while probe < bytes.len() && !is_value_delimiter(bytes[probe]) {
                probe += 1;
            }
            (value_start, probe)
        }
        None => return None,
    };
    if value_start >= value_end || already_redacted(&bytes[value_start..value_end]) {
        return None;
    }
    Some(SecretMatch {
        value_start,
        value_end,
        pattern: REDACTION_SECRET_KEY_ASSIGNMENT,
    })
}

/// `"key": "value"` where `key` matches the blocklist or [`JSON_SECRET_KEYS`].
fn match_json_secret(bytes: &[u8], index: usize) -> Option<SecretMatch> {
    let key_start = index + 1;
    let mut cursor = key_start;
    while cursor < bytes.len() && bytes[cursor] != b'"' {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        return None;
    }
    let key = std::str::from_utf8(&bytes[key_start..cursor]).ok()?;

    let mut probe = cursor + 1;
    while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
        probe += 1;
    }
    if bytes.get(probe) != Some(&b':') {
        return None;
    }
    probe += 1;
    while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
        probe += 1;
    }
    if bytes.get(probe) != Some(&b'"') {
        return None;
    }
    let value_start = probe + 1;
    let mut scan = value_start;
    let mut end = None;
    while scan < bytes.len() {
        let byte = bytes[scan];
        if byte == b'\\' {
            scan += 2;
            continue;
        }
        if byte == b'"' {
            end = Some(scan);
            break;
        }
        scan += 1;
    }
    let value_end = end?;
    if value_start >= value_end || already_redacted(&bytes[value_start..value_end]) {
        return None;
    }
    let pattern = if secret_pattern_hit(key) {
        REDACTION_SECRET_KEY_ASSIGNMENT
    } else if JSON_SECRET_KEYS
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
    {
        REDACTION_JSON_SECRET_KEY
    } else {
        return None;
    };
    Some(SecretMatch {
        value_start,
        value_end,
        pattern,
    })
}

// ---------------------------------------------------------------------------
// Execution and verifier digests
// ---------------------------------------------------------------------------

/// Deterministic digest of a normalized event stream.
///
/// Projects every normalized field into canonical JSON, with `cost` carried as
/// its shortest round-trip decimal string (floating-point numbers are not
/// canonical JSON), mixed with [`NORMALIZED_EVENTS_HASH_DOMAIN`]. The digest
/// covers stream order, session identity, tool payloads, tokens, and the
/// per-line raw hashes.
pub fn normalized_events_digest(events: &[NormalizedEvent]) -> Result<String, EvidenceError> {
    let projected: Vec<Value> = events
        .iter()
        .map(|event| {
            json!({
                "sequence": event.sequence,
                "source_event_type": event.source_event_type,
                "session_id": event.session_id,
                "timestamp_ms": event.timestamp_ms,
                "model_requested": event.model_requested,
                "model_observed": event.model_observed,
                "tool": event.tool.as_ref().map(|tool| json!({
                    "name": tool.name,
                    "call_id": tool.call_id,
                    "input_json": tool.input_json,
                    "output_str": tool.output_str,
                    "status": tool.status,
                    "error": tool.error,
                })),
                "text": event.text,
                "tokens": event.tokens.map(|tokens| json!({
                    "input": tokens.input,
                    "output": tokens.output,
                    "reasoning": tokens.reasoning,
                    "cache_read": tokens.cache_read,
                    "cache_write": tokens.cache_write,
                })),
                "cost": event.cost.map(|cost| cost.to_string()),
                "error": event.error.as_ref().map(|error| json!({
                    "name": error.name,
                    "data_json": error.data_json,
                })),
                "raw_line_hash": event.raw_line_hash,
            })
        })
        .collect();
    let projection = json!({
        "domain": NORMALIZED_EVENTS_HASH_DOMAIN,
        "events": projected,
    });
    let bytes = canonical_json_bytes(&projection)
        .map_err(|message| EvidenceError::Canonicalization { message })?;
    Ok(sha256_digest(&bytes))
}

/// Sealed execution digest: raw stream hash, normalized-event hash, terminal
/// classification, and exit code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionDigest {
    /// `sha256:<hex>` of the exact raw stdout bytes.
    pub raw_sha256: String,
    /// `sha256:<hex>` of the normalized event stream
    /// ([`normalized_events_digest`]).
    pub normalized_events_sha256: String,
    /// Supervisor terminal classification (never re-interpreted).
    pub termination: TerminationKind,
    /// Process exit code (`None` when signaled or never spawned).
    pub exit_code: Option<i32>,
}

impl ExecutionDigest {
    /// Build the digest from a supervised result.
    pub fn from_supervised(supervised: &SupervisedResult) -> Result<Self, EvidenceError> {
        Ok(Self {
            raw_sha256: supervised.raw_bytes_sha256.clone(),
            normalized_events_sha256: normalized_events_digest(&supervised.events)?,
            termination: supervised.termination,
            exit_code: supervised.exit_code,
        })
    }
}

/// Sealed verifier digest recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierDigest {
    /// Verifier identity.
    pub verifier_id: String,
    /// Machine-matchable verdict.
    pub outcome: VerifierVerdict,
    /// `sha256:<hex>` of the retained verifier stdout bytes.
    pub stdout_sha256: String,
    /// `sha256:<hex>` of the retained verifier stderr bytes.
    pub stderr_sha256: String,
    /// True when the verifier mutated the scored tree.
    pub mutation_detected: bool,
}

impl From<&VerifierOutcome> for VerifierDigest {
    fn from(outcome: &VerifierOutcome) -> Self {
        Self {
            verifier_id: outcome.verifier_id.clone(),
            outcome: outcome.verdict,
            stdout_sha256: outcome.stdout_sha256.clone(),
            stderr_sha256: outcome.stderr_sha256.clone(),
            mutation_detected: outcome.mutation_detected,
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed manifest
// ---------------------------------------------------------------------------

/// The sealed per-trial evidence manifest.
///
/// `seal_sha256` is computed over the canonical projection of every other
/// field; see [`seal_manifest`] and [`verify_seal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceManifest {
    /// Manifest schema version ([`EVIDENCE_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Owning experiment identifier.
    pub experiment_id: String,
    /// Trial identifier.
    pub trial_id: String,
    /// Canonical experiment plan hash the trial was generated from.
    pub plan_hash: String,
    /// Sealed task-record content digest.
    pub task_content_sha256: String,
    /// Effective-context manifest hash materialized for this trial.
    pub manifest_sha256: String,
    /// Sealed execution digest.
    pub execution: ExecutionDigest,
    /// Digest of the captured repository change record
    /// ([`crate::RepoDiff::diff_sha256`]).
    pub repo_diff_sha256: String,
    /// Sealed verifier digest (`None` when no verifier ran).
    pub verifier: Option<VerifierDigest>,
    /// Artifacts this manifest references, in the caller's declared order
    /// (order is part of the seal).
    pub artifacts: Vec<ArtifactRef>,
    /// Redaction events applied before persistence, sorted by pattern.
    pub redactions: Vec<RedactionEvent>,
    /// Caller-supplied seal time, Unix milliseconds.
    pub sealed_at_ms: u64,
    /// `sha256:<hex>` over the canonical projection of every field above.
    pub seal_sha256: String,
}

/// Inputs for [`seal_manifest`]; `sealed_at_ms` is an explicit input so seals
/// are reproducible in tests.
#[derive(Debug, Clone)]
pub struct SealInputs<'a> {
    /// Owning experiment identifier (non-empty).
    pub experiment_id: &'a str,
    /// Trial identifier (non-empty).
    pub trial_id: &'a str,
    /// Canonical plan hash.
    pub plan_hash: &'a str,
    /// Sealed task-record content digest.
    pub task_content_sha256: &'a str,
    /// Effective-context manifest hash.
    pub manifest_sha256: &'a str,
    /// Sealed execution digest.
    pub execution: &'a ExecutionDigest,
    /// Repository change-record digest.
    pub repo_diff_sha256: &'a str,
    /// Optional verifier digest.
    pub verifier: Option<&'a VerifierDigest>,
    /// Artifact references, in declared order.
    pub artifacts: &'a [ArtifactRef],
    /// Redaction events.
    pub redactions: &'a [RedactionEvent],
    /// Seal time, Unix milliseconds.
    pub sealed_at_ms: u64,
}

/// Machine-matchable classification for every [`SealError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SealErrorKind {
    /// The manifest declares an unsupported schema version.
    UnsupportedSchema,
    /// A required identity field is empty.
    EmptyField,
    /// A digest field is not `sha256:<64 lowercase hex>`.
    InvalidDigest,
    /// An artifact reference is malformed or duplicated.
    InvalidArtifact,
    /// A redaction event is malformed.
    InvalidRedaction,
    /// Canonical serialization of the seal projection failed.
    Canonicalization,
    /// The recomputed seal does not match the declared `seal_sha256`.
    SealMismatch,
    /// A referenced artifact is missing from the store.
    ArtifactMissing,
    /// A referenced artifact's stored bytes hash differently.
    ArtifactHashMismatch,
    /// A referenced artifact's stored size differs.
    ArtifactSizeMismatch,
    /// A referenced artifact's digest field is malformed.
    ArtifactInvalidDigest,
    /// A referenced artifact could not be read.
    ArtifactIo,
}

/// Errors produced while sealing or verifying an [`EvidenceManifest`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SealError {
    #[error(
        "evidence schema version {found} is not supported (this crate writes version {supported})"
    )]
    UnsupportedSchema { found: u32, supported: u32 },

    #[error("`{field}` must not be empty")]
    EmptyField { field: String },

    #[error("invalid digest in `{field}`: expected sha256:<64 lowercase hex>, found `{value}`")]
    InvalidDigest { field: String, value: String },

    #[error("invalid artifact reference `{logical_name}`: {reason}")]
    InvalidArtifact {
        logical_name: String,
        reason: String,
    },

    #[error("invalid redaction event `{pattern}`: {reason}")]
    InvalidRedaction { pattern: String, reason: String },

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },

    #[error("evidence seal mismatch: manifest declares {expected}, recomputed {found}")]
    SealMismatch { expected: String, found: String },

    #[error("artifact `{logical_name}` ({sha256}) is missing from the store")]
    ArtifactMissing {
        logical_name: String,
        sha256: String,
    },

    #[error("artifact `{logical_name}` hash mismatch: manifest declares {expected}, store holds {found}")]
    ArtifactHashMismatch {
        logical_name: String,
        expected: String,
        found: String,
    },

    #[error(
        "artifact `{logical_name}` size mismatch: manifest declares {expected} bytes, store holds {found}"
    )]
    ArtifactSizeMismatch {
        logical_name: String,
        expected: u64,
        found: u64,
    },

    #[error("artifact `{logical_name}` has an invalid digest `{value}`")]
    ArtifactInvalidDigest { logical_name: String, value: String },

    #[error("artifact `{logical_name}` could not be read: {message}")]
    ArtifactIo {
        logical_name: String,
        message: String,
    },
}

impl SealError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> SealErrorKind {
        match self {
            Self::UnsupportedSchema { .. } => SealErrorKind::UnsupportedSchema,
            Self::EmptyField { .. } => SealErrorKind::EmptyField,
            Self::InvalidDigest { .. } => SealErrorKind::InvalidDigest,
            Self::InvalidArtifact { .. } => SealErrorKind::InvalidArtifact,
            Self::InvalidRedaction { .. } => SealErrorKind::InvalidRedaction,
            Self::Canonicalization { .. } => SealErrorKind::Canonicalization,
            Self::SealMismatch { .. } => SealErrorKind::SealMismatch,
            Self::ArtifactMissing { .. } => SealErrorKind::ArtifactMissing,
            Self::ArtifactHashMismatch { .. } => SealErrorKind::ArtifactHashMismatch,
            Self::ArtifactSizeMismatch { .. } => SealErrorKind::ArtifactSizeMismatch,
            Self::ArtifactInvalidDigest { .. } => SealErrorKind::ArtifactInvalidDigest,
            Self::ArtifactIo { .. } => SealErrorKind::ArtifactIo,
        }
    }
}

/// Build a manifest and compute its seal.
///
/// Deterministic: the same inputs always produce the same `seal_sha256`.
/// Fail-closed validation rejects empty identities, malformed digests,
/// duplicate artifact logical names, and malformed redaction events before any
/// hashing happens.
pub fn seal_manifest(inputs: SealInputs<'_>) -> Result<EvidenceManifest, SealError> {
    validate_seal_inputs(&inputs)?;
    let manifest = EvidenceManifest {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        experiment_id: inputs.experiment_id.to_string(),
        trial_id: inputs.trial_id.to_string(),
        plan_hash: inputs.plan_hash.to_string(),
        task_content_sha256: inputs.task_content_sha256.to_string(),
        manifest_sha256: inputs.manifest_sha256.to_string(),
        execution: inputs.execution.clone(),
        repo_diff_sha256: inputs.repo_diff_sha256.to_string(),
        verifier: inputs.verifier.cloned(),
        artifacts: inputs.artifacts.to_vec(),
        redactions: inputs.redactions.to_vec(),
        sealed_at_ms: inputs.sealed_at_ms,
        seal_sha256: String::new(),
    };
    let bytes = seal_projection(&manifest)?;
    Ok(EvidenceManifest {
        seal_sha256: sha256_digest(&bytes),
        ..manifest
    })
}

/// Recompute the seal and compare it with the declared `seal_sha256`.
///
/// Fails with [`SealError::SealMismatch`] for **any** mutated field — the
/// projection covers every field except the seal itself, including
/// `sealed_at_ms` and the declared artifact order.
pub fn verify_seal(manifest: &EvidenceManifest) -> Result<(), SealError> {
    if manifest.schema_version != EVIDENCE_SCHEMA_VERSION {
        return Err(SealError::UnsupportedSchema {
            found: manifest.schema_version,
            supported: EVIDENCE_SCHEMA_VERSION,
        });
    }
    let bytes = seal_projection(manifest)?;
    let found = sha256_digest(&bytes);
    if found != manifest.seal_sha256 {
        return Err(SealError::SealMismatch {
            expected: manifest.seal_sha256.clone(),
            found,
        });
    }
    Ok(())
}

/// Re-verify every artifact a manifest references against the store.
///
/// Each reference is hash-checked on read and size-checked against the
/// declared byte count, so tampering with artifact bytes (same length or not)
/// fails verification.
pub fn verify_artifacts(
    manifest: &EvidenceManifest,
    store: &ArtifactStore,
) -> Result<(), SealError> {
    for reference in &manifest.artifacts {
        store
            .verify(reference)
            .map_err(|error| map_artifact_error(reference, error))?;
    }
    Ok(())
}

/// The canonical JSON projection whose SHA-256 is the seal.
///
/// Exposed for the eval store and for audit tooling; it never includes
/// `seal_sha256`.
pub fn seal_projection(manifest: &EvidenceManifest) -> Result<Vec<u8>, SealError> {
    let projection = json!({
        "domain": SEAL_HASH_DOMAIN,
        "schema_version": manifest.schema_version,
        "experiment_id": manifest.experiment_id,
        "trial_id": manifest.trial_id,
        "plan_hash": manifest.plan_hash,
        "task_content_sha256": manifest.task_content_sha256,
        "manifest_sha256": manifest.manifest_sha256,
        "execution": {
            "raw_sha256": manifest.execution.raw_sha256,
            "normalized_events_sha256": manifest.execution.normalized_events_sha256,
            "termination": manifest.execution.termination.as_str(),
            "exit_code": manifest.execution.exit_code,
        },
        "repo_diff_sha256": manifest.repo_diff_sha256,
        "verifier": manifest.verifier.as_ref().map(|verifier| json!({
            "verifier_id": verifier.verifier_id,
            "outcome": verifier.outcome.as_str(),
            "stdout_sha256": verifier.stdout_sha256,
            "stderr_sha256": verifier.stderr_sha256,
            "mutation_detected": verifier.mutation_detected,
        })),
        "artifacts": manifest.artifacts.iter().map(|artifact| json!({
            "logical_name": artifact.logical_name,
            "sha256": artifact.sha256,
            "bytes": artifact.bytes,
        })).collect::<Vec<_>>(),
        "redactions": manifest.redactions.iter().map(|redaction| json!({
            "pattern": redaction.pattern,
            "count": redaction.count,
        })).collect::<Vec<_>>(),
        "sealed_at_ms": manifest.sealed_at_ms,
    });
    canonical_json_bytes(&projection).map_err(|message| SealError::Canonicalization { message })
}

fn validate_seal_inputs(inputs: &SealInputs<'_>) -> Result<(), SealError> {
    require_non_empty("experiment_id", inputs.experiment_id)?;
    require_non_empty("trial_id", inputs.trial_id)?;
    require_canonical("plan_hash", inputs.plan_hash)?;
    require_canonical("task_content_sha256", inputs.task_content_sha256)?;
    require_canonical("manifest_sha256", inputs.manifest_sha256)?;
    require_canonical("execution.raw_sha256", &inputs.execution.raw_sha256)?;
    require_canonical(
        "execution.normalized_events_sha256",
        &inputs.execution.normalized_events_sha256,
    )?;
    require_canonical("repo_diff_sha256", inputs.repo_diff_sha256)?;
    if let Some(verifier) = inputs.verifier {
        require_non_empty("verifier.verifier_id", &verifier.verifier_id)?;
        require_canonical("verifier.stdout_sha256", &verifier.stdout_sha256)?;
        require_canonical("verifier.stderr_sha256", &verifier.stderr_sha256)?;
    }
    let mut names: HashSet<&str> = HashSet::new();
    for artifact in inputs.artifacts {
        if artifact.logical_name.is_empty() {
            return Err(SealError::InvalidArtifact {
                logical_name: artifact.logical_name.clone(),
                reason: "logical name must not be empty".to_string(),
            });
        }
        if !names.insert(artifact.logical_name.as_str()) {
            return Err(SealError::InvalidArtifact {
                logical_name: artifact.logical_name.clone(),
                reason: "duplicate logical name".to_string(),
            });
        }
        if !is_canonical_sha256_digest(&artifact.sha256) {
            return Err(SealError::InvalidArtifact {
                logical_name: artifact.logical_name.clone(),
                reason: format!("invalid digest `{}`", artifact.sha256),
            });
        }
    }
    for redaction in inputs.redactions {
        if redaction.pattern.is_empty() {
            return Err(SealError::InvalidRedaction {
                pattern: redaction.pattern.clone(),
                reason: "pattern must not be empty".to_string(),
            });
        }
    }
    Ok(())
}

fn require_non_empty(field: &str, value: &str) -> Result<(), SealError> {
    if value.is_empty() {
        Err(SealError::EmptyField {
            field: field.to_string(),
        })
    } else {
        Ok(())
    }
}

fn require_canonical(field: &str, value: &str) -> Result<(), SealError> {
    if is_canonical_sha256_digest(value) {
        Ok(())
    } else {
        Err(SealError::InvalidDigest {
            field: field.to_string(),
            value: value.to_string(),
        })
    }
}

fn map_artifact_error(reference: &ArtifactRef, error: EvidenceError) -> SealError {
    let logical_name = reference.logical_name.clone();
    match error {
        EvidenceError::InvalidDigest { value, .. } => SealError::ArtifactInvalidDigest {
            logical_name,
            value,
        },
        EvidenceError::MissingArtifact { sha256, .. } => SealError::ArtifactMissing {
            logical_name,
            sha256,
        },
        EvidenceError::HashMismatch { expected, found } => SealError::ArtifactHashMismatch {
            logical_name,
            expected,
            found,
        },
        EvidenceError::SizeMismatch { expected, found } => SealError::ArtifactSizeMismatch {
            logical_name,
            expected,
            found,
        },
        other => SealError::ArtifactIo {
            logical_name,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::TerminationKind;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(prefix: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "root-eval-m3-evidence-{}-{}-{}",
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

    fn digest(seed: &str) -> String {
        sha256_digest(seed.as_bytes())
    }

    fn sample_artifact_refs() -> (ArtifactRef, ArtifactRef) {
        (
            ArtifactRef {
                logical_name: "stdout".to_string(),
                sha256: digest("stdout bytes"),
                bytes: 12,
            },
            ArtifactRef {
                logical_name: "stderr".to_string(),
                sha256: digest("stderr bytes"),
                bytes: 12,
            },
        )
    }

    struct Sample {
        experiment_id: String,
        trial_id: String,
        plan_hash: String,
        task_content_sha256: String,
        manifest_sha256: String,
        execution: ExecutionDigest,
        repo_diff_sha256: String,
        verifier: Option<VerifierDigest>,
        artifacts: Vec<ArtifactRef>,
        redactions: Vec<RedactionEvent>,
        sealed_at_ms: u64,
    }

    impl Sample {
        fn new() -> Self {
            let (stdout, stderr) = sample_artifact_refs();
            Self {
                experiment_id: "eval-001".to_string(),
                trial_id: "trial-0001".to_string(),
                plan_hash: digest("plan"),
                task_content_sha256: digest("task"),
                manifest_sha256: digest("context"),
                execution: ExecutionDigest {
                    raw_sha256: digest("raw stdout"),
                    normalized_events_sha256: digest("normalized events"),
                    termination: TerminationKind::Completed,
                    exit_code: Some(0),
                },
                repo_diff_sha256: digest("diff"),
                verifier: Some(VerifierDigest {
                    verifier_id: "run-suite".to_string(),
                    outcome: VerifierVerdict::Pass,
                    stdout_sha256: digest("verifier stdout"),
                    stderr_sha256: digest("verifier stderr"),
                    mutation_detected: false,
                }),
                artifacts: vec![stdout, stderr],
                redactions: vec![RedactionEvent {
                    pattern: REDACTION_JSON_SECRET_KEY.to_string(),
                    count: 1,
                }],
                sealed_at_ms: 1_700_000_000_000,
            }
        }

        fn inputs(&self) -> SealInputs<'_> {
            SealInputs {
                experiment_id: &self.experiment_id,
                trial_id: &self.trial_id,
                plan_hash: &self.plan_hash,
                task_content_sha256: &self.task_content_sha256,
                manifest_sha256: &self.manifest_sha256,
                execution: &self.execution,
                repo_diff_sha256: &self.repo_diff_sha256,
                verifier: self.verifier.as_ref(),
                artifacts: &self.artifacts,
                redactions: &self.redactions,
                sealed_at_ms: self.sealed_at_ms,
            }
        }
    }

    fn sealed(sample: &Sample) -> EvidenceManifest {
        seal_manifest(sample.inputs()).expect("sample seals")
    }

    // ------------------------------------------------------------------
    // Content-addressed store
    // ------------------------------------------------------------------

    #[test]
    fn cas_put_get_round_trip_dedupe_and_layout() {
        let root = unique_dir("cas");
        let store = ArtifactStore::new(&root);
        let reference = store.put("stdout", b"hello cas\n").expect("put");
        assert_eq!(reference.sha256, sha256_digest(b"hello cas\n"));
        assert_eq!(reference.bytes, 10);
        assert_eq!(reference.logical_name, "stdout");
        assert_eq!(store.get(&reference.sha256).expect("get"), b"hello cas\n");
        store.verify(&reference).expect("verify");

        let hex = reference.sha256.strip_prefix(DIGEST_PREFIX).expect("hex");
        let path = store.path_for(&reference.sha256).expect("path");
        assert_eq!(
            path,
            root.join("artifacts")
                .join("sha256")
                .join(&hex[..2])
                .join(hex),
            "content-addressed layout"
        );

        // Duplicate content under another logical name: same address, same path.
        let again = store.put("other-name", b"hello cas\n").expect("put again");
        assert_eq!(again.sha256, reference.sha256);
        assert_eq!(again.bytes, reference.bytes);
        assert_eq!(
            store.path_for(&again.sha256).expect("path"),
            path,
            "duplicate puts share one file"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cas_get_and_verify_detect_tampered_bytes() {
        let root = unique_dir("cas-tamper");
        let store = ArtifactStore::new(&root);
        let reference = store.put("stdout", b"raw stdout\n").expect("put");
        let path = store.path_for(&reference.sha256).expect("path");

        // Same-length tamper: only the hash can catch it.
        fs::write(&path, b"TAMPERED!!\n").expect("tamper");
        assert_eq!(
            store.get(&reference.sha256).expect_err("get").kind(),
            EvidenceErrorKind::HashMismatch
        );
        assert_eq!(
            store.verify(&reference).expect_err("verify").kind(),
            EvidenceErrorKind::HashMismatch
        );

        // Truncation: still a hash mismatch on read.
        fs::write(&path, b"raw").expect("truncate");
        assert_eq!(
            store.verify(&reference).expect_err("verify").kind(),
            EvidenceErrorKind::HashMismatch
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cas_verify_rejects_wrong_ref_and_missing_artifact() {
        let root = unique_dir("cas-refs");
        let store = ArtifactStore::new(&root);
        let reference = store.put("stdout", b"payload\n").expect("put");

        let wrong_size = ArtifactRef {
            logical_name: "stdout".to_string(),
            sha256: reference.sha256.clone(),
            bytes: reference.bytes + 1,
        };
        assert_eq!(
            store.verify(&wrong_size).expect_err("size").kind(),
            EvidenceErrorKind::SizeMismatch
        );

        let missing = ArtifactRef {
            logical_name: "stdout".to_string(),
            sha256: digest("never stored"),
            bytes: 1,
        };
        assert_eq!(
            store.verify(&missing).expect_err("missing").kind(),
            EvidenceErrorKind::MissingArtifact
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cas_rejects_oversized_artifacts_and_invalid_names_and_digests() {
        let root = unique_dir("cas-caps");
        let small = ArtifactStore::with_max_artifact_bytes(&root, 4);
        assert_eq!(
            small.put("stdout", b"12345").expect_err("oversized").kind(),
            EvidenceErrorKind::ArtifactTooLarge
        );
        assert_eq!(
            small.put("bad/name", b"1234").expect_err("name").kind(),
            EvidenceErrorKind::InvalidLogicalName
        );
        assert_eq!(
            small.put("", b"1234").expect_err("empty name").kind(),
            EvidenceErrorKind::InvalidLogicalName
        );
        assert_eq!(
            small.get("nope").expect_err("digest").kind(),
            EvidenceErrorKind::InvalidDigest
        );
        let _ = fs::remove_dir_all(&root);
    }

    // ------------------------------------------------------------------
    // Redaction
    // ------------------------------------------------------------------

    #[test]
    fn redacts_blocklisted_assignments_with_counts() {
        let input = b"AWS_SECRET_ACCESS_KEY=abcd1234\n\
GITHUB_TOKEN=ghp_xyz\n\
PATH=/usr/bin\n\
OPENAI_API_KEY=\"sk-quoted\"\n\
CI_JOB_TOKEN='single-quoted'\n";
        let (output, events) = redact_bytes(input);
        let text = String::from_utf8(output.clone()).expect("utf-8");
        assert_eq!(
            text,
            "AWS_SECRET_ACCESS_KEY=[REDACTED:secret-key-assignment]\n\
GITHUB_TOKEN=[REDACTED:secret-key-assignment]\n\
PATH=/usr/bin\n\
OPENAI_API_KEY=\"[REDACTED:secret-key-assignment]\"\n\
CI_JOB_TOKEN='[REDACTED:secret-key-assignment]'\n"
        );
        assert_eq!(
            events,
            vec![RedactionEvent {
                pattern: REDACTION_SECRET_KEY_ASSIGNMENT.to_string(),
                count: 4,
            }]
        );
        assert!(!text.contains("abcd1234"));
        assert!(!text.contains("ghp_xyz"));
        assert!(!text.contains("sk-quoted"));
    }

    #[test]
    fn redacts_json_credential_values_with_counts() {
        let input = br#"{"apiKey": "sk-live-123", "token":"tok_abc", "secret": "shh", "password": "hunter2", "name": "root", "nested": {"accessToken": "at-1"}}"#;
        let (output, events) = redact_bytes(input);
        let text = String::from_utf8(output).expect("utf-8");
        assert_eq!(
            text,
            r#"{"apiKey": "[REDACTED:json-secret-key]", "token":"[REDACTED:json-secret-key]", "secret": "[REDACTED:json-secret-key]", "password": "[REDACTED:json-secret-key]", "name": "root", "nested": {"accessToken": "[REDACTED:json-secret-key]"}}"#
        );
        assert_eq!(
            events,
            vec![RedactionEvent {
                pattern: REDACTION_JSON_SECRET_KEY.to_string(),
                count: 5,
            }]
        );
        assert!(!text.contains("sk-live-123"));
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn json_keys_matching_the_blocklist_use_the_assignment_family() {
        let input = br#"{"AWS_SECRET_ACCESS_KEY": "abc", "GITHUB_TOKEN": "def"}"#;
        let (output, events) = redact_bytes(input);
        assert_eq!(
            String::from_utf8(output).expect("utf-8"),
            r#"{"AWS_SECRET_ACCESS_KEY": "[REDACTED:secret-key-assignment]", "GITHUB_TOKEN": "[REDACTED:secret-key-assignment]"}"#
        );
        assert_eq!(
            events,
            vec![RedactionEvent {
                pattern: REDACTION_SECRET_KEY_ASSIGNMENT.to_string(),
                count: 2,
            }]
        );
    }

    #[test]
    fn clean_bytes_are_unchanged() {
        let input = b"hello world\nPATH=/usr/bin\n{\"name\": \"root\", \"count\": 3}\n";
        let (output, events) = redact_bytes(input);
        assert_eq!(output, input);
        assert!(events.is_empty());
    }

    #[test]
    fn redaction_is_idempotent() {
        let input = b"GITHUB_TOKEN=ghp_x\n{\"password\": \"hunter2\"}\n";
        let (once, first_events) = redact_bytes(input);
        assert_eq!(first_events.len(), 2);
        let (twice, second_events) = redact_bytes(&once);
        assert_eq!(once, twice, "redaction must be idempotent");
        assert!(second_events.is_empty(), "no second-pass matches");
        assert!(!String::from_utf8_lossy(&twice).contains("hunter2"));
    }

    #[test]
    fn unterminated_and_empty_values_are_left_alone() {
        // An unterminated quote is not a match; an empty value has nothing to
        // redact.
        let input = b"GITHUB_TOKEN=\"unterminated\nGITHUB_TOKEN=\n";
        let (output, events) = redact_bytes(input);
        assert_eq!(output, input);
        assert!(events.is_empty());
    }

    // ------------------------------------------------------------------
    // Sealing
    // ------------------------------------------------------------------

    #[test]
    fn seal_is_deterministic_and_verifies() {
        let sample = Sample::new();
        let first = sealed(&sample);
        let second = sealed(&sample);
        assert_eq!(first.seal_sha256, second.seal_sha256);
        assert!(first.seal_sha256.starts_with("sha256:"));
        assert_eq!(first.schema_version, EVIDENCE_SCHEMA_VERSION);
        verify_seal(&first).expect("seal verifies");

        let mut later = sample;
        later.sealed_at_ms += 1;
        let third = sealed(&later);
        assert_ne!(third.seal_sha256, first.seal_sha256);
    }

    #[test]
    fn mutating_any_manifest_field_fails_seal_verification() {
        type Mutator = Box<dyn Fn(&mut EvidenceManifest)>;
        let mutators: Vec<(&str, Mutator)> = vec![
            (
                "experiment_id",
                Box::new(|m| m.experiment_id = "eval-002".to_string()),
            ),
            (
                "trial_id",
                Box::new(|m| m.trial_id = "trial-9999".to_string()),
            ),
            (
                "plan_hash",
                Box::new(|m| m.plan_hash = digest("other plan")),
            ),
            (
                "task_content_sha256",
                Box::new(|m| m.task_content_sha256 = digest("other task")),
            ),
            (
                "manifest_sha256",
                Box::new(|m| m.manifest_sha256 = digest("other context")),
            ),
            (
                "execution.raw_sha256",
                Box::new(|m| m.execution.raw_sha256 = digest("other raw")),
            ),
            (
                "execution.normalized_events_sha256",
                Box::new(|m| m.execution.normalized_events_sha256 = digest("other events")),
            ),
            (
                "execution.termination",
                Box::new(|m| m.execution.termination = TerminationKind::TimeoutKilled),
            ),
            (
                "execution.exit_code",
                Box::new(|m| m.execution.exit_code = Some(1)),
            ),
            (
                "repo_diff_sha256",
                Box::new(|m| m.repo_diff_sha256 = digest("other diff")),
            ),
            (
                "verifier.verifier_id",
                Box::new(|m| {
                    if let Some(verifier) = m.verifier.as_mut() {
                        verifier.verifier_id = "other".to_string();
                    }
                }),
            ),
            (
                "verifier.outcome",
                Box::new(|m| {
                    if let Some(verifier) = m.verifier.as_mut() {
                        verifier.outcome = VerifierVerdict::Fail;
                    }
                }),
            ),
            (
                "verifier.stdout_sha256",
                Box::new(|m| {
                    if let Some(verifier) = m.verifier.as_mut() {
                        verifier.stdout_sha256 = digest("other verifier stdout");
                    }
                }),
            ),
            (
                "verifier.mutation_detected",
                Box::new(|m| {
                    if let Some(verifier) = m.verifier.as_mut() {
                        verifier.mutation_detected = true;
                    }
                }),
            ),
            ("verifier.presence", Box::new(|m| m.verifier = None)),
            ("artifacts.reorder", Box::new(|m| m.artifacts.reverse())),
            (
                "artifacts.logical_name",
                Box::new(|m| m.artifacts[0].logical_name = "renamed".to_string()),
            ),
            (
                "artifacts.sha256",
                Box::new(|m| m.artifacts[0].sha256 = digest("other artifact")),
            ),
            ("artifacts.bytes", Box::new(|m| m.artifacts[0].bytes += 1)),
            (
                "artifacts.count",
                Box::new(|m| {
                    m.artifacts.pop();
                }),
            ),
            (
                "redactions",
                Box::new(|m| {
                    m.redactions.push(RedactionEvent {
                        pattern: REDACTION_SECRET_KEY_ASSIGNMENT.to_string(),
                        count: 2,
                    })
                }),
            ),
            ("redactions.count", Box::new(|m| m.redactions[0].count += 1)),
            ("sealed_at_ms", Box::new(|m| m.sealed_at_ms += 1)),
            (
                "seal_sha256",
                Box::new(|m| m.seal_sha256 = digest("forged seal")),
            ),
        ];

        for (field, mutate) in mutators {
            let mut manifest = sealed(&Sample::new());
            mutate(&mut manifest);
            let error = verify_seal(&manifest)
                .expect_err(&format!("mutating `{field}` must fail the seal"));
            assert_eq!(
                error.kind(),
                SealErrorKind::SealMismatch,
                "field `{field}` must fail with a seal mismatch, got {error}"
            );
        }
    }

    #[test]
    fn seal_rejects_malformed_inputs() {
        let sample = Sample::new();

        let mut bad = sample.inputs();
        bad.trial_id = "";
        assert_eq!(
            seal_manifest(bad).expect_err("empty trial id").kind(),
            SealErrorKind::EmptyField
        );

        let mut bad = sample.inputs();
        bad.plan_hash = "nope";
        assert_eq!(
            seal_manifest(bad).expect_err("bad digest").kind(),
            SealErrorKind::InvalidDigest
        );

        let mut execution = sample.execution.clone();
        execution.normalized_events_sha256 = "sha256:UPPER".to_string();
        let mut bad = sample.inputs();
        bad.execution = &execution;
        assert_eq!(
            seal_manifest(bad).expect_err("bad digest").kind(),
            SealErrorKind::InvalidDigest
        );

        let mut duplicate = sample.artifacts.clone();
        duplicate[1].logical_name = duplicate[0].logical_name.clone();
        let mut bad = sample.inputs();
        bad.artifacts = &duplicate;
        assert_eq!(
            seal_manifest(bad).expect_err("duplicate name").kind(),
            SealErrorKind::InvalidArtifact
        );

        let empty_pattern = [RedactionEvent {
            pattern: String::new(),
            count: 1,
        }];
        let mut bad = sample.inputs();
        bad.redactions = &empty_pattern;
        assert_eq!(
            seal_manifest(bad).expect_err("empty pattern").kind(),
            SealErrorKind::InvalidRedaction
        );
    }

    #[test]
    fn verify_seal_rejects_unsupported_schema_before_hashing() {
        let mut manifest = sealed(&Sample::new());
        manifest.schema_version = EVIDENCE_MAX_SUPPORTED_VERSION + 1;
        assert_eq!(
            verify_seal(&manifest).expect_err("schema").kind(),
            SealErrorKind::UnsupportedSchema
        );
    }

    #[test]
    fn artifact_verification_detects_tampering_and_removal() {
        let root = unique_dir("seal-artifacts");
        let store = ArtifactStore::new(&root);
        let stdout = store.put("stdout", b"raw stdout\n").expect("put stdout");
        let stderr = store.put("stderr", b"raw stderr\n").expect("put stderr");

        let mut sample = Sample::new();
        sample.artifacts = vec![stdout.clone(), stderr.clone()];
        let manifest = sealed(&sample);
        verify_seal(&manifest).expect("seal verifies");
        verify_artifacts(&manifest, &store).expect("artifacts verify");

        // Same-length tamper of one artifact.
        let stdout_path = store.path_for(&stdout.sha256).expect("path");
        fs::write(&stdout_path, b"TAMPERED!!\n").expect("tamper");
        assert_eq!(
            verify_artifacts(&manifest, &store)
                .expect_err("tamper")
                .kind(),
            SealErrorKind::ArtifactHashMismatch
        );

        // Removal of the other artifact (checked against a manifest that
        // references only it, so the earlier tamper cannot mask the result).
        let mut only_stderr = Sample::new();
        only_stderr.artifacts = vec![stderr.clone()];
        let stderr_manifest = sealed(&only_stderr);
        let stderr_path = store.path_for(&stderr.sha256).expect("path");
        fs::remove_file(&stderr_path).expect("remove");
        assert_eq!(
            verify_artifacts(&stderr_manifest, &store)
                .expect_err("missing")
                .kind(),
            SealErrorKind::ArtifactMissing
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn execution_digest_hashes_normalized_events_deterministically() {
        let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../Docs/Eval/milestone-0/fixtures/golden/01-no-tool/stdout.ndjson");
        let raw = fs::read_to_string(&golden).expect("read golden");
        let mut events = Vec::new();
        for (sequence, line) in raw.lines().enumerate() {
            events.push(
                crate::events::parse_ndjson_line(line, sequence as u64).expect("parse golden line"),
            );
        }
        let first = normalized_events_digest(&events).expect("digest");
        let second = normalized_events_digest(&events).expect("digest again");
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));

        // Any event change changes the digest.
        let mut changed = events.clone();
        changed[0].session_id = "ses_other".to_string();
        assert_ne!(normalized_events_digest(&changed).expect("digest"), first);

        // Order is part of the projection.
        let mut reordered = events;
        reordered.reverse();
        assert_ne!(normalized_events_digest(&reordered).expect("digest"), first);
    }
}
