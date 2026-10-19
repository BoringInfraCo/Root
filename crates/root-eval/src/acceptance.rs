//! M5 acceptance-assertion evaluator (TASK-SPEC §2.7).
//!
//! [`evaluate_acceptance`] interprets a parsed [`TaskRecord`]'s ordered
//! `acceptance` list against one trial's evidence ([`TrialEvidence`]) and
//! returns a deterministic [`AssertionReport`] whose result order is the
//! record order. This is the built-in declarative layer of §2.8; the external
//! verifier-command layer lives in [`crate::verifier`] and `test_pass` is the
//! bridge between the two.
//!
//! # Evidence inputs
//!
//! - [`AuditLine`] + [`parse_audit_log`] — the strict side-channel contract
//!   (§2.5.2): each line is compact JSON with exactly the five keys `seq`,
//!   `tool`, `argv`, `exit`, `profile`, and `seq` starts at 1 and increments
//!   by exactly one per line.
//! - [`TreeSnapshot`] + [`capture_tree_snapshot`] + [`diff_snapshots`] — the
//!   scored tree before/after the trial (regular files only, `.git` excluded,
//!   symlinks refused, sorted `/`-separated relative paths).
//! - [`read_tree_file`] — bounded content reader for `artifact_property`.
//! - [`TrialEvidence`] — the per-trial bundle the driver builds from the
//!   materializer/runner/verifier outputs.
//!
//! # Semantics (TASK-SPEC §2.7)
//!
//! - **`command_sequence`** (§2.7.1): every step must match, in order, on the
//!   selected channel. `allow_interleaved = true` (default) means a
//!   subsequence over *all* invocations of the channel; `false` means the
//!   steps must be consecutive. `ordinal = "first"` requires the step's
//!   matched occurrence to be the first invocation matching *any* step of
//!   this sequence (see "Decisions" below). `forbid_after_last_step = true`
//!   requires no invocation at all after the last matched step.
//! - **`prohibited_absent`** (§2.7.2): passes iff no invocation on the
//!   channel matches any pattern (substring or anchor-free regex search,
//!   with an optional tool-name filter).
//! - **`artifact_property`** (§2.7.3): one property (`exists`, `absent`,
//!   `sha256`, `contains`, `matches_regex`) of one path in the trial tree, or
//!   of the fixed `fake-tool-audit.jsonl` log.
//! - **`response_fact`** (§2.7.4): substring/regex facts over the final
//!   message under normalizer `final-message.v1`.
//! - **`test_pass`** (§2.7.5): binds one `[verifier]` command id to its
//!   [`VerifierOutcome`]: expected exit code, declared stdout/stderr hashes,
//!   and the no-mutation postcondition.
//! - **`diff_allowlist`** (§2.7.6): every changed/added/deleted path must
//!   match one of the fnmatch-style globs; an empty list asserts an unchanged
//!   tree.
//!
//! # Normalizers
//!
//! **`commandline.v1`** ([`commandline_v1`]). For an argv-form invocation the
//! effective command line is the argv joined with single spaces. The shell
//! `-c` form (`sh`/`bash`/`zsh` + `-c`) and the common wrappers (`git`,
//! `npm`, `npx`) are normalized recursively so that nested forms are pinned
//! by the same rule; the inner command string is treated literally and never
//! re-tokenized. Consequence: a recorder that keeps the inner command as one
//! argv element and a recorder that splits it into several words normalize to
//! the same effective line. `exact` steps compare string equality against
//! their own argv joined with single spaces; `pattern` steps run an
//! anchor-free search with the Rust `regex` crate (no backreferences, no
//! lookahead by construction).
//!
//! The audit channel records a shim invocation as `(tool, argv)`. If `argv[0]`
//! does not equal `tool` (an args-only recording), `tool` is prepended before
//! normalization — the "tool-prefix recovery" rule — so full-argv and
//! args-only recordings match the same steps.
//!
//! **`final-message.v1`** (§2.7.4). With `strip_code_fences = true` (the
//! default), every fenced block opened by a line whose first non-space
//! characters are three or more backticks or tildes is replaced by a single
//! `[code]` marker; an unclosed fence runs to the end of the message.
//! Matching is case-sensitive and performed on the normalized text.
//!
//! **fnmatch subset** (for `diff_allowlist`). `*` matches any sequence of
//! characters (including `/`, Python-`fnmatch` style), `?` matches exactly one
//! character, and `[...]` is a character class with optional leading `!` or
//! `^` negation, `a-z` ranges, and a literal `]` when first. An unterminated
//! `[` is a literal `[`. Backslashes and `:` are refused by the task-record
//! parser, so no escape syntax is defined.
//!
//! **Harness-event invocations.** Each [`NormalizedEvent`] carrying a
//! [`ToolCall`] contributes one invocation; events without a tool are
//! ignored. The tool name is `ToolCall.name`. For `bash` calls the command
//! text is the `state.input.command` string when present (used literally —
//! the event side is not re-tokenized, so `sh -c` unwrapping and wrapper
//! recursion apply to argv-form audit invocations only); otherwise the
//! canonical `ToolCall.input_json` string is used. `prohibited_absent` scans
//! the same command text on both channels.
//!
//! # Totality
//!
//! Missing evidence for an assertion's channel (no audit log, no final
//! message, no tree root, no matching verifier outcome, no tree diff) fails
//! that assertion with a clear `detail`; the report stays total. Malformed
//! evidence (bad JSONL, unreadable or symlinked tree, invalid regex in the
//! record) is an [`AcceptanceError`] — an invalid trial, never a silent
//! assertion failure.
//!
//! # Decisions and deviations
//!
//! 1. **`ordinal = "first"` scope.** §2.7.1 says "the first invocation
//!    matching *any* of the task's fake-tool or command stream". This
//!    evaluator scopes "any" to the steps of the *same* sequence and to
//!    invocations that actually match one of those steps: interleaved
//!    invocations matching no step are noise for the ordinal check (they are
//!    still noise for the subsequence scan). A stricter literal reading —
//!    "the first invocation recorded on the channel at all" — would also
//!    reject unrelated invocations before the step; that reading is flagged
//!    for review rather than silently chosen.
//! 2. **Empty lists.** `expect_substrings`/`forbid_substrings`/`allow_paths`
//!    with zero entries are vacuous (all-pass / none-forbidden / unchanged
//!    tree). An `expect_any_of` with zero groups (or a zero-entry group)
//!    cannot satisfy "at least one group has ≥ 1 substring present" and
//!    therefore fails.
//! 3. **`audit_log_bytes`.** [`TrialEvidence`] additionally carries the raw
//!    audit-log bytes so `artifact_property` with `source = "fake-tool-audit"`
//!    can evaluate `sha256`/`contains`/`matches_regex` against the log *file*
//!    bytes (§2.7.3). `exists`/`absent` are answered from the parsed log when
//!    the raw bytes are absent.
//! 4. **`verifiers` slice.** [`TrialEvidence`] carries a slice of verifier
//!    outcomes rather than a single one: one `[verifier.commands]` entry maps
//!    to one `VerifierSpec`/`VerifierOutcome` identity (§2.8), and a record
//!    may bind several `test_pass` assertions to several ids. The bound
//!    command's exit code is the outcome's first exit code (one outcome per
//!    record command).
//! 5. **Unreadable tree paths.** A symlink anywhere in a captured tree is a
//!    typed error (fail closed, mirroring [`crate::tree_fingerprint`]); a
//!    directory at an `artifact_property` path is treated as "no file", not
//!    as an error.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::digest::sha256_digest;
use crate::events::{NormalizedEvent, ToolCall};
use crate::schema::{
    Acceptance, AcceptanceKind, AcceptancePayload, ArtifactProperty, ArtifactPropertyName,
    ArtifactSource, CommandSequence, CommandStep, DiffAllowlist, EvidenceChannel, Ordinal,
    PatternMatch, ProhibitedAbsent, ResponseFact, StepMatcher, TaskRecord, TestPass,
};
use crate::validate::is_contained_relative_path;
use crate::verifier::VerifierOutcome;

/// Fixed file name of the trial fake-tool audit log (§2.5.2).
pub const FAKE_TOOL_AUDIT_FILE_NAME: &str = "fake-tool-audit.jsonl";

/// The exact key set every audit line carries, in contract order (§2.5.2).
pub const AUDIT_LINE_KEYS: [&str; 5] = ["seq", "tool", "argv", "exit", "profile"];

/// Largest regular file the tree walker and the artifact reader will load.
pub const MAX_ARTIFACT_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Deepest directory level [`capture_tree_snapshot`] will descend.
pub const MAX_SNAPSHOT_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`AcceptanceError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcceptanceErrorKind {
    /// The audit log bytes are not valid UTF-8.
    AuditLogNotUtf8,
    /// An audit line is empty.
    AuditLineEmpty,
    /// An audit line carries leading or trailing whitespace.
    AuditLineWhitespace,
    /// An audit line is not valid JSON.
    AuditLineInvalidJson,
    /// An audit line is not a JSON object.
    AuditLineNotObject,
    /// An audit line carries a key outside the five-key contract.
    AuditUnknownKey,
    /// An audit line is missing a contract key.
    AuditMissingKey,
    /// An audit line field has the wrong JSON type.
    AuditFieldType,
    /// An audit line's `seq` is not the expected position.
    AuditSequence,
    /// The snapshot root does not exist or is not a directory.
    TreeNotADirectory,
    /// A tree I/O operation failed.
    TreeIo,
    /// A symlink was encountered while capturing or reading a tree.
    TreeSymlinkRefused,
    /// A non-regular entry (fifo, socket, device) was encountered.
    TreeUnsupportedEntry,
    /// The tree is deeper than [`MAX_SNAPSHOT_DEPTH`].
    TreeTooDeep,
    /// A regular file exceeds [`MAX_ARTIFACT_FILE_BYTES`].
    TreeFileTooLarge,
    /// A tree path is not a contained relative `/`-separated path.
    TreePathInvalid,
    /// A record regex (command step, prohibition, or artifact property) does
    /// not compile.
    RegexInvalid,
    /// An `artifact_property` requiring a value declares none.
    PropertyValueMissing,
    /// A `test_pass` references a verifier id the record does not declare.
    VerifierNotDeclared,
}

/// Errors produced while parsing trial evidence or evaluating assertions.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AcceptanceError {
    #[error("audit log is not valid UTF-8")]
    AuditLogNotUtf8,

    #[error("audit line {line}: empty line")]
    AuditLineEmpty { line: u64 },

    #[error("audit line {line}: leading or trailing whitespace is not allowed in compact JSONL")]
    AuditLineWhitespace { line: u64 },

    #[error("audit line {line}: invalid JSON: {message}")]
    AuditLineInvalidJson { line: u64, message: String },

    #[error("audit line {line}: expected a JSON object")]
    AuditLineNotObject { line: u64 },

    #[error(
        "audit line {line}: unknown key `{key}` (the side-channel contract allows exactly \
         seq, tool, argv, exit, profile)"
    )]
    AuditUnknownKey { line: u64, key: String },

    #[error("audit line {line}: missing key `{key}`")]
    AuditMissingKey { line: u64, key: String },

    #[error("audit line {line}: field `{field}` must be {expected}")]
    AuditFieldType {
        line: u64,
        field: String,
        expected: String,
    },

    #[error(
        "audit line {line}: seq must be {expected} (strictly increasing from 1 without gaps), \
         found {found}"
    )]
    AuditSequence {
        line: u64,
        expected: u64,
        found: u64,
    },

    #[error("tree root `{path}` does not exist or is not a directory")]
    TreeNotADirectory { path: PathBuf },

    #[error("i/o error for `{path}`: {message}")]
    TreeIo { path: PathBuf, message: String },

    #[error("symlink refused in tree: `{path}`")]
    TreeSymlinkRefused { path: PathBuf },

    #[error("unsupported tree entry `{path}`: only regular files and directories are walked")]
    TreeUnsupportedEntry { path: PathBuf },

    #[error("tree `{path}` is deeper than the {MAX_SNAPSHOT_DEPTH}-level snapshot limit")]
    TreeTooDeep { path: PathBuf },

    #[error("file `{path}` is {bytes} bytes, over the {cap}-byte artifact cap")]
    TreeFileTooLarge { path: PathBuf, bytes: u64, cap: u64 },

    #[error("path `{path}` is not a contained relative `/`-separated path: {reason}")]
    TreePathInvalid { path: String, reason: String },

    #[error("invalid regex in `{field}`: {message} (pattern `{pattern}`)")]
    RegexInvalid {
        field: String,
        pattern: String,
        message: String,
    },

    #[error("`{field}` requires a value for property `{property}`")]
    PropertyValueMissing { field: String, property: String },

    #[error("record declares no verifier command `{verifier_id}`")]
    VerifierNotDeclared { verifier_id: String },
}

impl AcceptanceError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> AcceptanceErrorKind {
        match self {
            Self::AuditLogNotUtf8 => AcceptanceErrorKind::AuditLogNotUtf8,
            Self::AuditLineEmpty { .. } => AcceptanceErrorKind::AuditLineEmpty,
            Self::AuditLineWhitespace { .. } => AcceptanceErrorKind::AuditLineWhitespace,
            Self::AuditLineInvalidJson { .. } => AcceptanceErrorKind::AuditLineInvalidJson,
            Self::AuditLineNotObject { .. } => AcceptanceErrorKind::AuditLineNotObject,
            Self::AuditUnknownKey { .. } => AcceptanceErrorKind::AuditUnknownKey,
            Self::AuditMissingKey { .. } => AcceptanceErrorKind::AuditMissingKey,
            Self::AuditFieldType { .. } => AcceptanceErrorKind::AuditFieldType,
            Self::AuditSequence { .. } => AcceptanceErrorKind::AuditSequence,
            Self::TreeNotADirectory { .. } => AcceptanceErrorKind::TreeNotADirectory,
            Self::TreeIo { .. } => AcceptanceErrorKind::TreeIo,
            Self::TreeSymlinkRefused { .. } => AcceptanceErrorKind::TreeSymlinkRefused,
            Self::TreeUnsupportedEntry { .. } => AcceptanceErrorKind::TreeUnsupportedEntry,
            Self::TreeTooDeep { .. } => AcceptanceErrorKind::TreeTooDeep,
            Self::TreeFileTooLarge { .. } => AcceptanceErrorKind::TreeFileTooLarge,
            Self::TreePathInvalid { .. } => AcceptanceErrorKind::TreePathInvalid,
            Self::RegexInvalid { .. } => AcceptanceErrorKind::RegexInvalid,
            Self::PropertyValueMissing { .. } => AcceptanceErrorKind::PropertyValueMissing,
            Self::VerifierNotDeclared { .. } => AcceptanceErrorKind::VerifierNotDeclared,
        }
    }
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

/// One parsed line of the trial fake-tool audit log (§2.5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLine {
    /// Monotonic invocation sequence, starting at 1.
    pub seq: u64,
    /// Provides name of the invoked shim.
    pub tool: String,
    /// Invocation argv as recorded by the shim.
    pub argv: Vec<String>,
    /// Process exit code the shim produced.
    pub exit: i32,
    /// Registered profile id that produced the line.
    pub profile: String,
}

/// Strictly parse a compact-JSONL fake-tool audit log.
///
/// Rules (the §2.5.2 side-channel contract, enforced strictly):
///
/// - the bytes are UTF-8; each line is compact JSON with **exactly** the keys
///   [`AUDIT_LINE_KEYS`] — unknown keys, missing keys, and wrong JSON types
///   are typed errors;
/// - `seq` starts at 1 and increments by exactly one per line (no gaps, no
///   reordering);
/// - an empty input is a valid empty log (`Ok(vec![])`);
/// - a single trailing `\n` is tolerated (JSONL convention); every other
///   empty line, leading/trailing whitespace on a line, and CRLF line endings
///   are refused (strict compact JSONL).
pub fn parse_audit_log(bytes: &[u8]) -> Result<Vec<AuditLine>, AcceptanceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| AcceptanceError::AuditLogNotUtf8)?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut parsed = Vec::with_capacity(lines.len());
    for (index, raw) in lines.iter().enumerate() {
        let line = index as u64 + 1;
        if raw.is_empty() {
            return Err(AcceptanceError::AuditLineEmpty { line });
        }
        if raw.trim() != *raw {
            return Err(AcceptanceError::AuditLineWhitespace { line });
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|error| AcceptanceError::AuditLineInvalidJson {
                line,
                message: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or(AcceptanceError::AuditLineNotObject { line })?;

        for key in object.keys() {
            if !AUDIT_LINE_KEYS.contains(&key.as_str()) {
                return Err(AcceptanceError::AuditUnknownKey {
                    line,
                    key: key.clone(),
                });
            }
        }
        for key in AUDIT_LINE_KEYS {
            if !object.contains_key(key) {
                return Err(AcceptanceError::AuditMissingKey {
                    line,
                    key: key.to_string(),
                });
            }
        }

        let field_type = |field: &str, expected: &str| AcceptanceError::AuditFieldType {
            line,
            field: field.to_string(),
            expected: expected.to_string(),
        };
        let seq = object
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| field_type("seq", "a non-negative integer"))?;
        let tool = object
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| field_type("tool", "a string"))?
            .to_string();
        let argv_values = object
            .get("argv")
            .and_then(Value::as_array)
            .ok_or_else(|| field_type("argv", "an array of strings"))?;
        let mut argv = Vec::with_capacity(argv_values.len());
        for (arg_index, value) in argv_values.iter().enumerate() {
            let arg = value
                .as_str()
                .ok_or_else(|| field_type(&format!("argv[{arg_index}]"), "a string"))?;
            argv.push(arg.to_string());
        }
        let exit = object
            .get("exit")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| field_type("exit", "a 32-bit integer"))?;
        let profile = object
            .get("profile")
            .and_then(Value::as_str)
            .ok_or_else(|| field_type("profile", "a string"))?
            .to_string();

        if seq != line {
            return Err(AcceptanceError::AuditSequence {
                line,
                expected: line,
                found: seq,
            });
        }
        parsed.push(AuditLine {
            seq,
            tool,
            argv,
            exit,
            profile,
        });
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Tree snapshots
// ---------------------------------------------------------------------------

/// One regular file captured in a [`TreeSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    /// `sha256:<hex>` of the file bytes.
    pub sha256: String,
    /// File size in bytes.
    pub bytes_len: u64,
}

/// Content snapshot of a repository tree: every regular file (`.git`
/// excluded), keyed by sorted `/`-separated relative path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeSnapshot {
    pub files: BTreeMap<String, TreeFile>,
}

/// How a path changed between two [`TreeSnapshot`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeChangeKind {
    Added,
    Modified,
    Deleted,
}

impl TreeChangeKind {
    /// Stable snake-case token for reports and evidence manifests.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// One changed path between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChange {
    pub path: String,
    pub kind: TreeChangeKind,
}

/// Walk `root` and capture every regular file's digest and size.
///
/// `.git` entries never contribute; symlinks, non-regular entries, files over
/// [`MAX_ARTIFACT_FILE_BYTES`], and trees deeper than [`MAX_SNAPSHOT_DEPTH`]
/// are typed errors (fail closed, mirroring [`crate::tree_fingerprint`]).
/// Paths are sorted `/`-separated relative paths.
pub fn capture_tree_snapshot(root: &Path) -> Result<TreeSnapshot, AcceptanceError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|error| map_root_metadata_error(root, error))?;
    if metadata.file_type().is_symlink() {
        return Err(AcceptanceError::TreeSymlinkRefused {
            path: root.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(AcceptanceError::TreeNotADirectory {
            path: root.to_path_buf(),
        });
    }
    let mut files = BTreeMap::new();
    walk_snapshot(root, root, 0, &mut files)?;
    Ok(TreeSnapshot { files })
}

fn map_root_metadata_error(root: &Path, error: std::io::Error) -> AcceptanceError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AcceptanceError::TreeNotADirectory {
            path: root.to_path_buf(),
        }
    } else {
        AcceptanceError::TreeIo {
            path: root.to_path_buf(),
            message: error.to_string(),
        }
    }
}

fn walk_snapshot(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut BTreeMap<String, TreeFile>,
) -> Result<(), AcceptanceError> {
    if depth > MAX_SNAPSHOT_DEPTH {
        return Err(AcceptanceError::TreeTooDeep {
            path: dir.to_path_buf(),
        });
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| AcceptanceError::TreeIo {
        path: dir.to_path_buf(),
        message: error.to_string(),
    })? {
        let entry = entry.map_err(|error| AcceptanceError::TreeIo {
            path: dir.to_path_buf(),
            message: error.to_string(),
        })?;
        entries.push(entry.path());
    }
    entries.sort();

    for path in entries {
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| AcceptanceError::TreeIo {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(AcceptanceError::TreeSymlinkRefused { path });
        }
        if metadata.is_dir() {
            walk_snapshot(root, &path, depth + 1, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(AcceptanceError::TreeUnsupportedEntry { path });
        }
        if metadata.len() > MAX_ARTIFACT_FILE_BYTES {
            return Err(AcceptanceError::TreeFileTooLarge {
                path,
                bytes: metadata.len(),
                cap: MAX_ARTIFACT_FILE_BYTES,
            });
        }
        let bytes = fs::read(&path).map_err(|error| AcceptanceError::TreeIo {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let relative = relative_slash_path(root, &path)?;
        files.insert(
            relative,
            TreeFile {
                sha256: sha256_digest(&bytes),
                bytes_len: bytes.len() as u64,
            },
        );
    }
    Ok(())
}

fn relative_slash_path(root: &Path, path: &Path) -> Result<String, AcceptanceError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AcceptanceError::TreePathInvalid {
            path: path.to_string_lossy().into_owned(),
            reason: "path escapes the snapshot root".to_string(),
        })?;
    let mut parts: Vec<&str> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                parts.push(
                    part.to_str()
                        .ok_or_else(|| AcceptanceError::TreePathInvalid {
                            path: path.to_string_lossy().into_owned(),
                            reason: "path is not valid UTF-8".to_string(),
                        })?,
                );
            }
            other => {
                return Err(AcceptanceError::TreePathInvalid {
                    path: other.as_os_str().to_string_lossy().into_owned(),
                    reason: "expected only normal path components".to_string(),
                });
            }
        }
    }
    Ok(parts.join("/"))
}

/// Diff two snapshots into changed paths, sorted by path.
///
/// A path present in both snapshots with different digests is `Modified`; a
/// path only in the post snapshot is `Added`; only in the pre snapshot,
/// `Deleted`. The byte length is informational and never decides a diff.
pub fn diff_snapshots(pre: &TreeSnapshot, post: &TreeSnapshot) -> Vec<TreeChange> {
    let mut paths: BTreeSet<&str> = pre.files.keys().map(String::as_str).collect();
    paths.extend(post.files.keys().map(String::as_str));
    let mut changes = Vec::new();
    for path in paths {
        match (pre.files.get(path), post.files.get(path)) {
            (None, Some(_)) => changes.push(TreeChange {
                path: path.to_string(),
                kind: TreeChangeKind::Added,
            }),
            (Some(_), None) => changes.push(TreeChange {
                path: path.to_string(),
                kind: TreeChangeKind::Deleted,
            }),
            (Some(before), Some(after)) if before.sha256 != after.sha256 => {
                changes.push(TreeChange {
                    path: path.to_string(),
                    kind: TreeChangeKind::Modified,
                })
            }
            _ => {}
        }
    }
    changes
}

/// Read one regular file inside `root`, bounded by [`MAX_ARTIFACT_FILE_BYTES`].
///
/// `rel_path` must be a contained relative `/`-separated path. Returns
/// `Ok(None)` when no regular file exists at the path (missing, or a
/// directory); a symlink, a non-regular entry, or an oversized file is a
/// typed error rather than a silent absence.
pub fn read_tree_file(root: &Path, rel_path: &str) -> Result<Option<Vec<u8>>, AcceptanceError> {
    if !is_contained_relative_path(rel_path) {
        return Err(AcceptanceError::TreePathInvalid {
            path: rel_path.to_string(),
            reason: "expected a relative, `/`-separated path without `..` segments".to_string(),
        });
    }
    let path = root.join(rel_path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AcceptanceError::TreeIo {
                path,
                message: error.to_string(),
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(AcceptanceError::TreeSymlinkRefused { path });
    }
    if metadata.is_dir() {
        return Ok(None);
    }
    if !metadata.is_file() {
        return Err(AcceptanceError::TreeUnsupportedEntry { path });
    }
    if metadata.len() > MAX_ARTIFACT_FILE_BYTES {
        return Err(AcceptanceError::TreeFileTooLarge {
            path,
            bytes: metadata.len(),
            cap: MAX_ARTIFACT_FILE_BYTES,
        });
    }
    fs::read(&path)
        .map(Some)
        .map_err(|error| AcceptanceError::TreeIo {
            path,
            message: error.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Evidence bundle
// ---------------------------------------------------------------------------

/// The per-trial evidence bundle the acceptance evaluator consumes.
///
/// The driver wave builds this from the materializer/runner/verifier outputs.
/// Missing channels are `None`/empty; the evaluator fails the affected
/// assertions with a clear detail instead of erroring (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub struct TrialEvidence<'a> {
    /// Parsed fake-tool audit log; `Some(&[])` is a present but empty log.
    pub audit_log: Option<&'a [AuditLine]>,
    /// Raw audit-log bytes for `artifact_property` content checks (§2.7.3).
    pub audit_log_bytes: Option<&'a [u8]>,
    /// Normalized harness events; only tool-carrying events contribute.
    pub events: &'a [NormalizedEvent],
    /// The agent's final user-visible message.
    pub final_message: Option<&'a str>,
    /// Verifier outcomes, one per record `[verifier.commands]` id.
    pub verifiers: &'a [VerifierOutcome],
    /// Root of the scored trial tree for `artifact_property`.
    pub tree_root: Option<&'a Path>,
    /// Tree snapshot taken before the trial.
    pub pre_snapshot: Option<&'a TreeSnapshot>,
    /// Tree snapshot taken after the trial.
    pub post_snapshot: Option<&'a TreeSnapshot>,
    /// Precomputed tree diff; preferred over deriving one from snapshots.
    pub tree_changes: Option<&'a [TreeChange]>,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// One evaluated acceptance assertion, in record order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionResult {
    /// The record assertion id.
    pub id: String,
    /// The assertion kind token (`command_sequence`, ...).
    pub kind: String,
    /// Whether the record marks this assertion primary.
    pub primary: bool,
    /// Whether the assertion passed.
    pub passed: bool,
    /// Deterministic human-readable explanation (what matched / why not).
    pub detail: String,
}

/// The full assertion report for one trial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionReport {
    /// One result per record assertion, in record order.
    pub results: Vec<AssertionResult>,
    /// True when every assertion passed.
    pub all_passed: bool,
    /// True when every primary assertion passed.
    pub all_primary_passed: bool,
}

impl AssertionReport {
    /// Ids of the failing assertions, in record order.
    pub fn failed_ids(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|result| !result.passed)
            .map(|result| result.id.as_str())
            .collect()
    }
}

/// Evaluate every acceptance assertion of `record` against `evidence`.
///
/// Result order is record order. Missing evidence fails the affected
/// assertions (never errors); malformed evidence is an [`AcceptanceError`].
pub fn evaluate_acceptance(
    record: &TaskRecord,
    evidence: &TrialEvidence<'_>,
) -> Result<AssertionReport, AcceptanceError> {
    let mut results = Vec::with_capacity(record.acceptance.len());
    for assertion in &record.acceptance {
        let check = evaluate_assertion(record, assertion, evidence)?;
        results.push(AssertionResult {
            id: assertion.id.clone(),
            kind: kind_token(assertion.kind).to_string(),
            primary: assertion.primary,
            passed: check.passed,
            detail: check.detail,
        });
    }
    let all_passed = results.iter().all(|result| result.passed);
    let all_primary_passed = results
        .iter()
        .filter(|result| result.primary)
        .all(|result| result.passed);
    Ok(AssertionReport {
        results,
        all_passed,
        all_primary_passed,
    })
}

/// Outcome of one assertion evaluation before it is folded into a result.
struct Check {
    passed: bool,
    detail: String,
}

impl Check {
    fn pass(detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(detail: impl Into<String>) -> Self {
        Self {
            passed: false,
            detail: detail.into(),
        }
    }
}

fn evaluate_assertion(
    record: &TaskRecord,
    assertion: &Acceptance,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    match &assertion.payload {
        AcceptancePayload::CommandSequence(sequence) => {
            evaluate_command_sequence(assertion, sequence, evidence)
        }
        AcceptancePayload::ProhibitedAbsent(payload) => {
            evaluate_prohibited_absent(assertion, payload, evidence)
        }
        AcceptancePayload::ArtifactProperty(payload) => {
            evaluate_artifact_property(assertion, payload, evidence)
        }
        AcceptancePayload::ResponseFact(payload) => {
            evaluate_response_fact(assertion, payload, evidence)
        }
        AcceptancePayload::TestPass(payload) => evaluate_test_pass(payload, evidence, record),
        AcceptancePayload::DiffAllowlist(payload) => evaluate_diff_allowlist(payload, evidence),
    }
}

fn kind_token(kind: AcceptanceKind) -> &'static str {
    match kind {
        AcceptanceKind::CommandSequence => "command_sequence",
        AcceptanceKind::ProhibitedAbsent => "prohibited_absent",
        AcceptanceKind::ArtifactProperty => "artifact_property",
        AcceptanceKind::ResponseFact => "response_fact",
        AcceptanceKind::TestPass => "test_pass",
        AcceptanceKind::DiffAllowlist => "diff_allowlist",
    }
}

fn evidence_token(channel: EvidenceChannel) -> &'static str {
    match channel {
        EvidenceChannel::FakeToolAudit => "fake-tool-audit",
        EvidenceChannel::HarnessEvents => "harness-events",
        EvidenceChannel::FinalMessage => "final-message",
        EvidenceChannel::VerifierRun => "verifier-run",
        EvidenceChannel::Tree => "tree",
    }
}

// ---------------------------------------------------------------------------
// Invocation view (both channels)
// ---------------------------------------------------------------------------

/// One invocation in the common channel view.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    /// Shim name (audit) or harness tool name (events).
    tool: String,
    /// Effective command line used by `command_sequence` matching.
    command: String,
}

impl Invocation {
    /// The searchable text used by `prohibited_absent` (same as `command`:
    /// the audit side carries the effective command line, the event side the
    /// derived command string / input JSON).
    fn text(&self) -> &str {
        &self.command
    }
}

/// Invocations for a command/prohibition assertion's channel.
///
/// `None` means the channel evidence is missing (no audit log). A present but
/// empty channel yields `Some(vec![])`: `prohibited_absent` passes vacuously
/// and `command_sequence` fails with a step-match detail.
fn channel_invocations(
    channel: EvidenceChannel,
    evidence: &TrialEvidence<'_>,
) -> Option<Vec<Invocation>> {
    match channel {
        EvidenceChannel::FakeToolAudit => evidence.audit_log.map(audit_invocations),
        EvidenceChannel::HarnessEvents => Some(event_invocations(evidence.events)),
        _ => None,
    }
}

fn audit_invocations(lines: &[AuditLine]) -> Vec<Invocation> {
    lines
        .iter()
        .map(|line| {
            // Tool-prefix recovery: accept both full-argv recordings
            // (`argv[0] == tool`) and args-only recordings.
            let mut argv = line.argv.clone();
            if argv.first().map(String::as_str) != Some(line.tool.as_str()) {
                argv.insert(0, line.tool.clone());
            }
            Invocation {
                tool: line.tool.clone(),
                command: commandline_v1(&argv),
            }
        })
        .collect()
}

fn event_invocations(events: &[NormalizedEvent]) -> Vec<Invocation> {
    events
        .iter()
        .filter_map(|event| event.tool.as_ref())
        .map(|call| Invocation {
            tool: call.name.clone(),
            command: event_command(call),
        })
        .collect()
}

/// Derive the event-side command text for one tool call.
///
/// `bash` calls carry the shell command line in `state.input.command`; the
/// string is used literally (the event side is never re-tokenized). Every
/// other tool (and a `bash` call without a string `command`) uses the
/// canonical `ToolCall.input_json` string.
fn event_command(call: &ToolCall) -> String {
    if call.name == "bash" {
        if let Ok(Value::Object(input)) = serde_json::from_str::<Value>(&call.input_json) {
            if let Some(Value::String(command)) = input.get("command") {
                return command.clone();
            }
        }
    }
    call.input_json.clone()
}

/// Normalizer `commandline.v1` (§2.7.1).
///
/// For a direct exec this is the argv joined with single spaces. Shell `-c`
/// forms (`sh -c X`, `bash -c X`, `zsh -c X`) and the common wrappers (`git`,
/// `npm`, `npx`) apply the rule recursively, treating the inner command
/// string literally. For argv-form input the recursive rules are a fixed
/// point of plain joining; they are implemented explicitly so nested forms
/// are pinned by the same literal rules and never re-tokenized.
pub fn commandline_v1<S: AsRef<str>>(argv: &[S]) -> String {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    let Some(head) = argv.first().copied() else {
        return String::new();
    };

    if matches!(head, "sh" | "bash" | "zsh") && argv.get(1) == Some(&"-c") && argv.len() >= 3 {
        // Outer head + the literal inner command string; any further argv
        // elements are the shell's positional parameters.
        let mut parts = vec![head, "-c", argv[2]];
        parts.extend_from_slice(&argv[3..]);
        return parts.join(" ");
    }

    if matches!(head, "git" | "npm" | "npx") {
        if let Some((prefix, wrapped)) = split_wrapper(&argv) {
            let inner = commandline_v1(wrapped);
            if inner.is_empty() {
                return prefix.join(" ");
            }
            return format!("{} {}", prefix.join(" "), inner);
        }
    }

    argv.join(" ")
}

/// Split a wrapper invocation into `(prefix, wrapped command)`.
///
/// - `npx <command...>` wraps `argv[1..]`;
/// - `npm exec <command...>` / `npm x <command...>` wrap `argv[2..]`;
/// - `git -c <value> <command...>` / `git -C <path> <command...>` wrap
///   `argv[3..]`.
///
/// Any other shape is not a wrapped command and normalizes as a plain join.
fn split_wrapper<'a>(argv: &'a [&'a str]) -> Option<(&'a [&'a str], &'a [&'a str])> {
    match argv.first().copied()? {
        "npx" if argv.len() > 1 => Some((&argv[..1], &argv[1..])),
        "npm" => match argv.get(1).copied() {
            Some("exec") | Some("x") if argv.len() > 2 => Some((&argv[..2], &argv[2..])),
            _ => None,
        },
        "git" => match argv.get(1).copied() {
            Some("-c") | Some("-C") if argv.len() > 3 => Some((&argv[..3], &argv[3..])),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// command_sequence (§2.7.1)
// ---------------------------------------------------------------------------

enum CompiledMatcher {
    Exact(String),
    Pattern(Regex),
}

impl CompiledMatcher {
    fn matches(&self, command: &str) -> bool {
        match self {
            Self::Exact(text) => command == text,
            Self::Pattern(regex) => regex.is_match(command),
        }
    }
}

fn describe_step(step: &CommandStep) -> String {
    match &step.matcher {
        StepMatcher::Exact { argv } => format!("argv `{}`", argv.join(" ")),
        StepMatcher::Pattern { regex } => format!("pattern `{regex}`"),
    }
}

fn evaluate_command_sequence(
    assertion: &Acceptance,
    sequence: &CommandSequence,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let channel = evidence_token(assertion.evidence);
    let Some(invocations) = channel_invocations(assertion.evidence, evidence) else {
        return Ok(Check::fail(format!(
            "missing `{channel}` evidence: no audit log provided"
        )));
    };
    if sequence.steps.is_empty() {
        return Ok(Check::pass("no steps declared"));
    }
    if invocations.is_empty() {
        return Ok(Check::fail(format!(
            "no invocations recorded on the `{channel}` channel; {} step(s) required",
            sequence.steps.len()
        )));
    }

    let mut matchers = Vec::with_capacity(sequence.steps.len());
    for (index, step) in sequence.steps.iter().enumerate() {
        matchers.push(compile_step_matcher(assertion, index, step)?);
    }

    let matched_indices = if sequence.allow_interleaved {
        subsequence_scan(&matchers, &invocations)
    } else {
        match consecutive_scan(&matchers, &invocations) {
            Some(indices) => indices,
            None => {
                return Ok(Check::fail(format!(
                    "allow_interleaved = false: no consecutive run of {} invocation(s) matches \
                     the steps on the `{channel}` channel",
                    sequence.steps.len()
                )));
            }
        }
    };
    if matched_indices.len() != sequence.steps.len() {
        let matched_count = matched_indices.len();
        return Ok(Check::fail(format!(
            "step {} of {} did not match any invocation on the `{channel}` channel (expected {})",
            matched_count + 1,
            sequence.steps.len(),
            describe_step(&sequence.steps[matched_count])
        )));
    }

    // ordinal = "first": the step's matched occurrence must be the first
    // invocation matching any step of this sequence (noise that matches no
    // step is ignored; see the module docs).
    for (step_index, step) in sequence.steps.iter().enumerate() {
        if step.ordinal != Ordinal::First {
            continue;
        }
        let matched_index = matched_indices[step_index];
        let earliest_any = invocations.iter().position(|invocation| {
            matchers
                .iter()
                .any(|matcher| matcher.matches(&invocation.command))
        });
        if let Some(earliest) = earliest_any {
            if earliest != matched_index {
                return Ok(Check::fail(format!(
                    "step {} has ordinal = \"first\", but invocation {} matched a required step \
                     before its own match at invocation {}",
                    step_index + 1,
                    earliest + 1,
                    matched_index + 1
                )));
            }
        }
    }

    if sequence.forbid_after_last_step {
        let last = matched_indices[matched_indices.len() - 1];
        if last + 1 != invocations.len() {
            return Ok(Check::fail(format!(
                "forbid_after_last_step = true: {} invocation(s) recorded after the last matched step",
                invocations.len() - last - 1
            )));
        }
    }

    Ok(Check::pass(format!(
        "matched {} step(s) in order on the `{channel}` channel",
        sequence.steps.len()
    )))
}

fn compile_step_matcher(
    assertion: &Acceptance,
    index: usize,
    step: &CommandStep,
) -> Result<CompiledMatcher, AcceptanceError> {
    match &step.matcher {
        StepMatcher::Exact { argv } => Ok(CompiledMatcher::Exact(argv.join(" "))),
        StepMatcher::Pattern { regex } => {
            let field = format!("acceptance[{}].steps[{}].regex", assertion.id, index);
            Ok(CompiledMatcher::Pattern(compile_regex(&field, regex)?))
        }
    }
}

fn compile_regex(field: &str, pattern: &str) -> Result<Regex, AcceptanceError> {
    Regex::new(pattern).map_err(|error| AcceptanceError::RegexInvalid {
        field: field.to_string(),
        pattern: pattern.to_string(),
        message: error.to_string(),
    })
}

/// Greedy subsequence scan: returns the matched invocation index per step,
/// advancing on the next required step. A shorter result means the remaining
/// steps did not match.
fn subsequence_scan(matchers: &[CompiledMatcher], invocations: &[Invocation]) -> Vec<usize> {
    let mut matched = Vec::with_capacity(matchers.len());
    let mut step_index = 0;
    for (index, invocation) in invocations.iter().enumerate() {
        if step_index >= matchers.len() {
            break;
        }
        if matchers[step_index].matches(&invocation.command) {
            matched.push(index);
            step_index += 1;
        }
    }
    matched
}

/// Earliest contiguous run of invocations matching the steps in order.
fn consecutive_scan(
    matchers: &[CompiledMatcher],
    invocations: &[Invocation],
) -> Option<Vec<usize>> {
    if matchers.len() > invocations.len() {
        return None;
    }
    for start in 0..=(invocations.len() - matchers.len()) {
        let matched: Vec<usize> = (0..matchers.len()).map(|offset| start + offset).collect();
        if matchers
            .iter()
            .zip(&matched)
            .all(|(matcher, index)| matcher.matches(&invocations[*index].command))
        {
            return Some(matched);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// prohibited_absent (§2.7.2)
// ---------------------------------------------------------------------------

fn evaluate_prohibited_absent(
    assertion: &Acceptance,
    payload: &ProhibitedAbsent,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let channel = evidence_token(assertion.evidence);
    let Some(invocations) = channel_invocations(assertion.evidence, evidence) else {
        return Ok(Check::fail(format!(
            "missing `{channel}` evidence: no audit log provided"
        )));
    };

    let mut compiled: Vec<Option<Regex>> = Vec::with_capacity(payload.patterns.len());
    for (index, pattern) in payload.patterns.iter().enumerate() {
        if pattern.match_kind == PatternMatch::Regex {
            let field = format!("acceptance[{}].patterns[{}].pattern", assertion.id, index);
            compiled.push(Some(compile_regex(&field, &pattern.pattern)?));
        } else {
            compiled.push(None);
        }
    }

    for (invocation_index, invocation) in invocations.iter().enumerate() {
        for (pattern_index, pattern) in payload.patterns.iter().enumerate() {
            if pattern
                .tool
                .as_deref()
                .is_some_and(|tool| tool != invocation.tool)
            {
                continue;
            }
            let hit = match &compiled[pattern_index] {
                Some(regex) => regex.is_match(invocation.text()),
                None => invocation.text().contains(&pattern.pattern),
            };
            if hit {
                return Ok(Check::fail(format!(
                    "invocation {} (`{}`, tool `{}`) matched prohibited pattern {} `{}`",
                    invocation_index + 1,
                    invocation.command,
                    invocation.tool,
                    pattern_index + 1,
                    pattern.pattern
                )));
            }
        }
    }

    Ok(Check::pass(format!(
        "no invocation matched {} prohibited pattern(s) on the `{channel}` channel",
        payload.patterns.len()
    )))
}

// ---------------------------------------------------------------------------
// artifact_property (§2.7.3)
// ---------------------------------------------------------------------------

fn evaluate_artifact_property(
    assertion: &Acceptance,
    payload: &ArtifactProperty,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    match payload.source {
        ArtifactSource::Tree => evaluate_tree_artifact(assertion, payload, evidence),
        ArtifactSource::FakeToolAudit => evaluate_audit_artifact(assertion, payload, evidence),
    }
}

fn evaluate_tree_artifact(
    assertion: &Acceptance,
    payload: &ArtifactProperty,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let Some(root) = evidence.tree_root else {
        return Ok(Check::fail(format!(
            "missing `tree` evidence: no tree root provided for `{}`",
            payload.path
        )));
    };
    let path = &payload.path;
    match payload.property {
        ArtifactPropertyName::Exists => {
            let present = read_tree_file(root, path)?.is_some();
            Ok(if present {
                Check::pass(format!("`{path}` exists in the trial tree"))
            } else {
                Check::fail(format!("`{path}` does not exist in the trial tree"))
            })
        }
        ArtifactPropertyName::Absent => {
            let present = read_tree_file(root, path)?.is_some();
            Ok(if present {
                Check::fail(format!(
                    "`{path}` exists in the trial tree but must be absent"
                ))
            } else {
                Check::pass(format!("`{path}` is absent from the trial tree"))
            })
        }
        ArtifactPropertyName::Sha256 => {
            let expected = artifact_value(assertion, payload)?;
            let Some(bytes) = read_tree_file(root, path)? else {
                return Ok(Check::fail(format!(
                    "`{path}` is absent; sha256 cannot be checked"
                )));
            };
            let actual = sha256_digest(&bytes);
            Ok(if actual == expected {
                Check::pass(format!("`{path}` hashes to {actual}"))
            } else {
                Check::fail(format!("`{path}` hashes to {actual}, expected {expected}"))
            })
        }
        ArtifactPropertyName::Contains => {
            let needle = artifact_value(assertion, payload)?;
            let Some(bytes) = read_tree_file(root, path)? else {
                return Ok(Check::fail(format!(
                    "`{path}` is absent; contains cannot be checked"
                )));
            };
            let found = bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes());
            Ok(if found {
                Check::pass(format!("`{path}` contains `{needle}`"))
            } else {
                Check::fail(format!("`{path}` does not contain `{needle}`"))
            })
        }
        ArtifactPropertyName::MatchesRegex => {
            let pattern = artifact_value(assertion, payload)?;
            let field = format!("acceptance[{}].value", assertion.id);
            let regex = compile_regex(&field, pattern)?;
            let Some(bytes) = read_tree_file(root, path)? else {
                return Ok(Check::fail(format!(
                    "`{path}` is absent; matches_regex cannot be checked"
                )));
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                return Ok(Check::fail(format!(
                    "`{path}` is not valid UTF-8; matches_regex cannot be evaluated"
                )));
            };
            Ok(if regex.is_match(text) {
                Check::pass(format!("`{path}` matches `{pattern}`"))
            } else {
                Check::fail(format!("`{path}` does not match `{pattern}`"))
            })
        }
    }
}

fn evaluate_audit_artifact(
    assertion: &Acceptance,
    payload: &ArtifactProperty,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let path = &payload.path;
    let present = evidence.audit_log.is_some() || evidence.audit_log_bytes.is_some();
    match payload.property {
        ArtifactPropertyName::Exists => Ok(if present {
            Check::pass(format!("`{path}` exists (audit log present)"))
        } else {
            Check::fail(format!("`{path}` is absent: no audit log evidence"))
        }),
        ArtifactPropertyName::Absent => Ok(if present {
            Check::fail(format!("`{path}` exists but must be absent"))
        } else {
            Check::pass(format!("`{path}` is absent"))
        }),
        ArtifactPropertyName::Sha256
        | ArtifactPropertyName::Contains
        | ArtifactPropertyName::MatchesRegex => {
            let Some(bytes) = evidence.audit_log_bytes else {
                return Ok(Check::fail(format!(
                    "missing raw audit-log bytes: `{path}` content properties require the \
                     audit file bytes"
                )));
            };
            match payload.property {
                ArtifactPropertyName::Sha256 => {
                    let expected = artifact_value(assertion, payload)?;
                    let actual = sha256_digest(bytes);
                    Ok(if actual == expected {
                        Check::pass(format!("`{path}` hashes to {actual}"))
                    } else {
                        Check::fail(format!("`{path}` hashes to {actual}, expected {expected}"))
                    })
                }
                ArtifactPropertyName::Contains => {
                    let needle = artifact_value(assertion, payload)?;
                    let found = bytes
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes());
                    Ok(if found {
                        Check::pass(format!("`{path}` contains `{needle}`"))
                    } else {
                        Check::fail(format!("`{path}` does not contain `{needle}`"))
                    })
                }
                ArtifactPropertyName::MatchesRegex => {
                    let pattern = artifact_value(assertion, payload)?;
                    let field = format!("acceptance[{}].value", assertion.id);
                    let regex = compile_regex(&field, pattern)?;
                    let Ok(text) = std::str::from_utf8(bytes) else {
                        return Ok(Check::fail(format!(
                            "`{path}` is not valid UTF-8; matches_regex cannot be evaluated"
                        )));
                    };
                    Ok(if regex.is_match(text) {
                        Check::pass(format!("`{path}` matches `{pattern}`"))
                    } else {
                        Check::fail(format!("`{path}` does not match `{pattern}`"))
                    })
                }
                ArtifactPropertyName::Exists | ArtifactPropertyName::Absent => unreachable!(),
            }
        }
    }
}

/// The declared `value` for a property that requires one.
///
/// The record parser guarantees `Some` for `sha256`/`contains`/
/// `matches_regex`; a missing or empty value is a typed record defect.
fn artifact_value<'a>(
    assertion: &Acceptance,
    payload: &'a ArtifactProperty,
) -> Result<&'a str, AcceptanceError> {
    match payload.value.as_deref() {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(AcceptanceError::PropertyValueMissing {
            field: format!("acceptance[{}].value", assertion.id),
            property: format!("{:?}", payload.property).to_lowercase(),
        }),
    }
}

// ---------------------------------------------------------------------------
// response_fact (§2.7.4)
// ---------------------------------------------------------------------------

fn evaluate_response_fact(
    assertion: &Acceptance,
    payload: &ResponseFact,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let Some(message) = evidence.final_message else {
        return Ok(Check::fail(
            "missing `final-message` evidence: no final message provided",
        ));
    };
    let normalized = if payload.strip_code_fences {
        strip_code_fences(message)
    } else {
        message.to_string()
    };

    let mut failures: Vec<String> = Vec::new();
    if let Some(substrings) = &payload.expect_substrings {
        for (index, substring) in substrings.iter().enumerate() {
            if !normalized.contains(substring) {
                failures.push(format!(
                    "expected substring {} `{substring}` is not present",
                    index + 1
                ));
            }
        }
    }
    if let Some(groups) = &payload.expect_any_of {
        let any_group = groups
            .iter()
            .any(|group| group.iter().any(|substring| normalized.contains(substring)));
        if !any_group {
            failures.push(format!(
                "no expect_any_of group matched ({} group(s) declared)",
                groups.len()
            ));
        }
    }
    if let Some(substrings) = &payload.forbid_substrings {
        for substring in substrings {
            if normalized.contains(substring) {
                failures.push(format!("forbidden substring `{substring}` is present"));
            }
        }
    }
    if let Some(pattern) = &payload.forbid_regex {
        let field = format!("acceptance[{}].forbid_regex", assertion.id);
        let regex = compile_regex(&field, pattern)?;
        if regex.is_match(&normalized) {
            failures.push(format!("forbidden regex `{pattern}` matched"));
        }
    }

    if failures.is_empty() {
        Ok(Check::pass("final message satisfies all declared facts"))
    } else {
        Ok(Check::fail(failures.join("; ")))
    }
}

/// Normalizer `final-message.v1`: replace fenced code blocks with `[code]`.
///
/// A fence opens on a line whose first non-space characters are three or more
/// backticks or tildes and closes on a line consisting only of at least as
/// many of the same character. An unclosed fence runs to the end of the
/// message.
fn strip_code_fences(message: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in message.split('\n') {
        match fence {
            None => match fence_open(line) {
                Some(marker) => {
                    fence = Some(marker);
                    output.push("[code]");
                }
                None => output.push(line),
            },
            Some((marker, length)) => {
                if fence_close(line, marker, length) {
                    fence = None;
                }
            }
        }
    }
    output.join("\n")
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    if length >= 3 {
        Some((marker, length))
    } else {
        None
    }
}

fn fence_close(line: &str, marker: char, length: usize) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c == marker) && trimmed.chars().count() >= length
}

// ---------------------------------------------------------------------------
// test_pass (§2.7.5)
// ---------------------------------------------------------------------------

fn evaluate_test_pass(
    payload: &TestPass,
    evidence: &TrialEvidence<'_>,
    record: &TaskRecord,
) -> Result<Check, AcceptanceError> {
    let command = record
        .verifier
        .as_ref()
        .and_then(|verifier| {
            verifier
                .commands
                .iter()
                .find(|command| command.id == payload.verifier_id)
        })
        .ok_or_else(|| AcceptanceError::VerifierNotDeclared {
            verifier_id: payload.verifier_id.clone(),
        })?;

    let Some(outcome) = evidence
        .verifiers
        .iter()
        .find(|outcome| outcome.verifier_id == payload.verifier_id)
    else {
        let available: Vec<&str> = evidence
            .verifiers
            .iter()
            .map(|outcome| outcome.verifier_id.as_str())
            .collect();
        let available = if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        };
        return Ok(Check::fail(format!(
            "missing `verifier-run` evidence: no verifier outcome for `{}` (available: {available})",
            payload.verifier_id
        )));
    };

    let mut failures: Vec<String> = Vec::new();
    match outcome.exit_codes.first() {
        Some(Some(exit)) if *exit == command.expected_exit => {}
        Some(Some(exit)) => failures.push(format!(
            "exit code {exit} does not equal the declared expected_exit {}",
            command.expected_exit
        )),
        Some(None) => failures.push("the verifier command produced no exit code".to_string()),
        None => failures.push("the verifier outcome carries no exit codes".to_string()),
    }
    if let Some(expected) = &command.stdout_sha256 {
        if &outcome.stdout_sha256 != expected {
            failures.push(format!(
                "stdout sha256 {} does not equal the declared {expected}",
                outcome.stdout_sha256
            ));
        }
    }
    if let Some(expected) = &command.stderr_sha256 {
        if &outcome.stderr_sha256 != expected {
            failures.push(format!(
                "stderr sha256 {} does not equal the declared {expected}",
                outcome.stderr_sha256
            ));
        }
    }
    if outcome.mutation_detected {
        failures
            .push("the verifier mutated the scored tree (mutation_detected = true)".to_string());
    }

    if failures.is_empty() {
        Ok(Check::pass(format!(
            "verifier `{}` produced the expected outcome",
            payload.verifier_id
        )))
    } else {
        Ok(Check::fail(failures.join("; ")))
    }
}

// ---------------------------------------------------------------------------
// diff_allowlist (§2.7.6)
// ---------------------------------------------------------------------------

fn evaluate_diff_allowlist(
    payload: &DiffAllowlist,
    evidence: &TrialEvidence<'_>,
) -> Result<Check, AcceptanceError> {
    let changes: Vec<TreeChange> = match evidence.tree_changes {
        Some(changes) => changes.to_vec(),
        None => match (evidence.pre_snapshot, evidence.post_snapshot) {
            (Some(pre), Some(post)) => diff_snapshots(pre, post),
            _ => {
                return Ok(Check::fail(
                    "missing `tree` evidence: provide `tree_changes` or both pre/post tree \
                     snapshots",
                ));
            }
        },
    };

    if payload.allow_paths.is_empty() {
        return Ok(if changes.is_empty() {
            Check::pass("tree unchanged (empty allowlist)")
        } else {
            Check::fail(format!(
                "empty allowlist asserts an unchanged tree, but {} change(s) were recorded: {}",
                changes.len(),
                describe_changes(&changes)
            ))
        });
    }

    let mut matchers = Vec::with_capacity(payload.allow_paths.len());
    for (index, glob) in payload.allow_paths.iter().enumerate() {
        let field = format!("acceptance.allow_paths[{index}]");
        matchers.push(compile_glob(&field, glob)?);
    }
    let unlisted: Vec<TreeChange> = changes
        .iter()
        .filter(|change| {
            !matchers
                .iter()
                .any(|matcher| matcher.is_match(&change.path))
        })
        .cloned()
        .collect();

    if unlisted.is_empty() {
        Ok(Check::pass(format!(
            "{} change(s) all match the allowlist",
            changes.len()
        )))
    } else {
        Ok(Check::fail(format!(
            "{} change(s) outside the allowlist: {}",
            unlisted.len(),
            describe_changes(&unlisted)
        )))
    }
}

fn describe_changes(changes: &[TreeChange]) -> String {
    const MAX_LISTED: usize = 5;
    let mut parts: Vec<String> = changes
        .iter()
        .take(MAX_LISTED)
        .map(|change| format!("{} ({})", change.path, change.kind.as_str()))
        .collect();
    if changes.len() > MAX_LISTED {
        parts.push(format!("and {} more", changes.len() - MAX_LISTED));
    }
    parts.join(", ")
}

fn compile_glob(field: &str, glob: &str) -> Result<Regex, AcceptanceError> {
    let pattern = glob_to_regex(glob);
    Regex::new(&pattern).map_err(|error| AcceptanceError::RegexInvalid {
        field: field.to_string(),
        pattern: glob.to_string(),
        message: error.to_string(),
    })
}

/// Translate the documented fnmatch subset into an anchored regex.
fn glob_to_regex(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut output = String::from("^(?:");
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' => {
                output.push_str(".*");
                index += 1;
            }
            '?' => {
                output.push('.');
                index += 1;
            }
            '[' => {
                let mut cursor = index + 1;
                let negated = matches!(chars.get(cursor), Some('!') | Some('^'));
                if negated {
                    cursor += 1;
                }
                let mut members: Vec<char> = Vec::new();
                if chars.get(cursor) == Some(&']') {
                    members.push(']');
                    cursor += 1;
                }
                let mut closed = false;
                while let Some(character) = chars.get(cursor) {
                    if *character == ']' {
                        closed = true;
                        break;
                    }
                    members.push(*character);
                    cursor += 1;
                }
                if !closed {
                    // Unterminated class: a literal `[`.
                    output.push_str("\\[");
                    index += 1;
                    continue;
                }
                if members.is_empty() {
                    // `[!]` / `[^]`: a class that can never match.
                    output.push_str("[^\\s\\S]");
                } else {
                    output.push('[');
                    if negated {
                        output.push('^');
                    }
                    for (member_index, member) in members.iter().enumerate() {
                        match member {
                            '\\' | '[' | '&' => {
                                output.push('\\');
                                output.push(*member);
                            }
                            '^' => output.push_str("\\^"),
                            ']' => output.push_str("\\]"),
                            '-' => {
                                if member_index == 0 || member_index + 1 == members.len() {
                                    output.push_str("\\-");
                                } else {
                                    output.push('-');
                                }
                            }
                            other => output.push(*other),
                        }
                    }
                    output.push(']');
                }
                index = cursor + 1;
            }
            character => {
                push_regex_escaped(&mut output, character);
                index += 1;
            }
        }
    }
    output.push_str(")$");
    output
}

fn push_regex_escaped(output: &mut String, character: char) {
    if matches!(
        character,
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
    ) {
        output.push('\\');
    }
    output.push(character);
}

// ---------------------------------------------------------------------------
// Tests for private helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commandline_direct_exec_is_joined_argv() {
        assert_eq!(
            commandline_v1(&["root", "doctor", "--json"]),
            "root doctor --json"
        );
        assert_eq!(commandline_v1::<&str>(&[]), "");
    }

    #[test]
    fn commandline_shell_c_concatenates_literally() {
        assert_eq!(
            commandline_v1(&["sh", "-c", "root doctor --json"]),
            "sh -c root doctor --json"
        );
        // A split inner command normalizes to the same effective line.
        assert_eq!(
            commandline_v1(&["sh", "-c", "root", "doctor", "--json"]),
            "sh -c root doctor --json"
        );
        assert_eq!(
            commandline_v1(&["bash", "-c", "echo hi"]),
            "bash -c echo hi"
        );
        assert_eq!(commandline_v1(&["zsh", "-c", "echo hi"]), "zsh -c echo hi");
    }

    #[test]
    fn commandline_wrappers_chain_recursively() {
        assert_eq!(
            commandline_v1(&["npx", "root", "doctor"]),
            "npx root doctor"
        );
        assert_eq!(
            commandline_v1(&["npm", "exec", "--", "root", "doctor"]),
            "npm exec -- root doctor"
        );
        assert_eq!(
            commandline_v1(&["git", "-C", "repo", "sh", "-c", "root doctor --json"]),
            "git -C repo sh -c root doctor --json"
        );
        assert_eq!(commandline_v1(&["git", "status"]), "git status");
    }

    #[test]
    fn glob_subset_matches_as_documented() {
        assert!(compile_glob("g", "*").unwrap().is_match("a/b/c"));
        assert!(compile_glob("g", "test/*")
            .unwrap()
            .is_match("test/sub/file"));
        assert!(compile_glob("g", "src/*.rs")
            .unwrap()
            .is_match("src/main.rs"));
        assert!(!compile_glob("g", "src/*.rs")
            .unwrap()
            .is_match("src/main.md"));
        assert!(compile_glob("g", "a?c").unwrap().is_match("abc"));
        assert!(!compile_glob("g", "a?c").unwrap().is_match("ac"));
        assert!(compile_glob("g", "[ab]x").unwrap().is_match("ax"));
        assert!(compile_glob("g", "[!ab]x").unwrap().is_match("cx"));
        assert!(!compile_glob("g", "[!ab]x").unwrap().is_match("ax"));
        assert!(compile_glob("g", "a-z").unwrap().is_match("a-z"));
        assert!(compile_glob("g", "a[b").unwrap().is_match("a[b"));
        assert!(!compile_glob("g", "[!]x").unwrap().is_match("x"));
        assert!(compile_glob("g", "[a-c]x").unwrap().is_match("bx"));
        // `[]x` is an unterminated class (literal `[]x`), fnmatch style;
        // a literal `]` as the first member needs `[]]x`.
        assert!(compile_glob("g", "[]x").unwrap().is_match("[]x"));
        assert!(compile_glob("g", "[]]x").unwrap().is_match("]x"));
    }

    #[test]
    fn fences_are_replaced_by_one_marker() {
        let message = "before\n```json\n{\"claim\": \"installed\"}\n```\nafter";
        assert_eq!(strip_code_fences(message), "before\n[code]\nafter");
        let tilde = "~~~\nsecret\n~~~\nvisible";
        assert_eq!(strip_code_fences(tilde), "[code]\nvisible");
        // Unclosed fence runs to the end of the message.
        assert_eq!(strip_code_fences("kept\n```\ndropped"), "kept\n[code]");
        // Inline single backticks are not fences.
        assert_eq!(strip_code_fences("a `b` c"), "a `b` c");
    }
}
