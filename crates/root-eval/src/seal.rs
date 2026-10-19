//! Draft sealing for benchmark task records (Milestone 5).
//!
//! Milestone 1 introduced [`TaskDraft`] — the structural layer that admits
//! `sha256:PENDING` sentinels and a missing `[integrity]` table. This module
//! closes the loop: [`seal_draft_at`] derives every digest that is a pure
//! function of the record's own declared bytes, refuses any sentinel that
//! cannot be derived, computes the canonical `integrity.content_sha256`
//! (§2.10), and returns the sealed TOML document. The result is guaranteed to
//! parse through [`TaskRecord::parse_str`] — the sealing tool re-parses it
//! before returning, so a sealed record is always evaluation-ready.
//!
//! # Derivations
//!
//! | Field | Rule |
//! |---|---|
//! | `prompt.file_sha256` (`kind = "file"`) | SHA-256 of the prompt file bytes, read relative to the record directory |
//! | `fixture.files[].content_sha256` | SHA-256 of that entry's `content` bytes |
//! | `fixture.tree_sha256` (`kind = "generated"`) | [`tree_digest_v1`] over the declared files (see the schema note below) |
//! | `environment.fake_tools[].script_sha256` | [`crate::fake_tools::script_sha256`] of the declared profile |
//! | `integrity.content_sha256` | canonical hash of the document minus `[integrity]` (§2.10) |
//!
//! `integrity.hash_domain` is set to the constant `task-record.v1`. Absent
//! optional digests stay absent. Every remaining `sha256:PENDING` in a
//! required digest position (a `repo_commit` fixture's declared tree hash, an
//! `artifact_property` `sha256` value, a verifier `stdout_sha256`/
//! `stderr_sha256`) is refused with [`SealError::NonDerivable`] naming **all**
//! such fields.
//!
//! # Schema note on `fixture.tree_sha256`
//!
//! v1 declares the tree hash only for `repo_commit` fixtures; for `generated`
//! fixtures the tree identity is *derived*, never stored (§2.4), and the
//! parser refuses the field on a generated fixture. The sealer still
//! implements the derivation branch for completeness — see
//! `derive_fixture_digests` — but a draft cannot reach it through
//! [`TaskDraft::parse_str`]; [`tree_digest_v1`] is primarily the public rule
//! used by materializers and verifiers to compute the same identity check.
//!
//! # Determinism
//!
//! Sealing is a pure function of the draft document plus the record directory:
//! two runs produce byte-identical output. Comments and original formatting
//! are already gone after TOML parsing, and the emitted document is the
//! parsed document re-serialized in TOML key order with one trailing newline.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;
use toml::map::Map;
use toml::Value;

use crate::canonical::canonical_record;
use crate::digest::{is_pending_sentinel, sha256_digest};
use crate::fake_tools::{script_sha256, ProfileId, ABSENT_PROFILE_TOKEN};
use crate::parse::collect_pending_digests;
use crate::schema::FixtureFile;
use crate::validate::is_contained_relative_path;
use crate::{TaskDraft, TaskRecord, TASK_HASH_DOMAIN};

/// Machine-matchable classification for every [`SealError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SealErrorKind {
    /// One or more required digests cannot be derived from the record.
    NonDerivable,
    /// The draft document is not a well-formed task-record document.
    InvalidDraft,
    /// A file-based prompt is unsafe or unreadable.
    PromptFile,
    /// Canonical serialization or hashing failed.
    Canonical,
    /// The sealed document failed strict re-parsing (fail-closed guard).
    NotParseable,
}

/// Errors produced while sealing a task draft.
///
/// Re-exported from the crate root as [`crate::DraftSealError`] because
/// [`crate::SealError`] already names the evidence-sealing error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SealError {
    #[error("cannot derive required digest field(s): {}", fields.join(", "))]
    NonDerivable { fields: Vec<String> },

    #[error("invalid draft field `{field}`: {reason}")]
    InvalidDraft { field: String, reason: String },

    #[error("prompt file `{path}` cannot be hashed: {reason}")]
    PromptFile { path: PathBuf, reason: String },

    #[error("canonical hashing failed: {message}")]
    Canonical { message: String },

    #[error("sealed document does not parse as a task record: {message}")]
    NotParseable { message: String },
}

impl SealError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> SealErrorKind {
        match self {
            Self::NonDerivable { .. } => SealErrorKind::NonDerivable,
            Self::InvalidDraft { .. } => SealErrorKind::InvalidDraft,
            Self::PromptFile { .. } => SealErrorKind::PromptFile,
            Self::Canonical { .. } => SealErrorKind::Canonical,
            Self::NotParseable { .. } => SealErrorKind::NotParseable,
        }
    }
}

/// Seal a draft, resolving a file-based prompt relative to the current
/// directory.
///
/// Equivalent to `seal_draft_at(draft, Path::new("."))`; drafts whose prompt
/// is file-based should prefer [`seal_draft_at`] with the record's directory.
pub fn seal_draft(draft: &TaskDraft) -> Result<String, SealError> {
    seal_draft_at(draft, Path::new("."))
}

/// Seal a draft whose prompt file paths resolve against `record_dir`.
///
/// Replaces every derivable `sha256:PENDING` digest, refuses any remaining
/// required sentinel with [`SealError::NonDerivable`] (naming every such
/// field), computes `integrity.content_sha256` over the document minus
/// `[integrity]`, sets `integrity.hash_domain = "task-record.v1"`, and
/// returns the sealed TOML string.
pub fn seal_draft_at(draft: &TaskDraft, record_dir: &Path) -> Result<String, SealError> {
    let mut document = draft.document().clone();
    derive_digests(&mut document, record_dir)?;

    let table = document.as_table().ok_or_else(|| SealError::InvalidDraft {
        field: "<document>".to_string(),
        reason: "expected a TOML table".to_string(),
    })?;
    let remaining: Vec<String> = collect_pending_digests(table)
        .into_iter()
        // `[integrity]` is derived below, not a non-derivable field.
        .filter(|field| field != "integrity.content_sha256")
        .collect();
    if !remaining.is_empty() {
        return Err(SealError::NonDerivable { fields: remaining });
    }

    let canonical = canonical_record(&document).map_err(|error| SealError::Canonical {
        message: error.to_string(),
    })?;
    let content_sha256 = canonical.sha256().to_string();

    let mut integrity = Map::new();
    integrity.insert("content_sha256".to_string(), Value::String(content_sha256));
    integrity.insert(
        "hash_domain".to_string(),
        Value::String(TASK_HASH_DOMAIN.to_string()),
    );
    let table = document
        .as_table_mut()
        .ok_or_else(|| SealError::InvalidDraft {
            field: "<document>".to_string(),
            reason: "expected a TOML table".to_string(),
        })?;
    table.insert("integrity".to_string(), Value::Table(integrity));

    let mut sealed = toml::to_string_pretty(&document).map_err(|error| SealError::Canonical {
        message: format!("sealed serialization failed: {error}"),
    })?;
    while sealed.ends_with('\n') {
        sealed.pop();
    }
    sealed.push('\n');

    // Fail closed: a sealed record must always be evaluation-ready.
    TaskRecord::parse_str(&sealed).map_err(|error| SealError::NotParseable {
        message: error.to_string(),
    })?;
    Ok(sealed)
}

/// The tree-digest rule v1 for `generated` fixtures (§2.4).
///
/// For each file in lexicographic `path` order, hash-concatenate `path`, a NUL
/// byte, the byte length of `content` (decimal), a NUL byte, the declared
/// `content_sha256`, and a NUL byte; digest the concatenation. Declaration
/// order does not matter — the function sorts by `path` itself — and the
/// declared digests are used as-is, so callers sealing a draft should fill
/// `content_sha256` first (which [`seal_draft_at`] does).
pub fn tree_digest_v1(files: &[FixtureFile]) -> String {
    let mut ordered: Vec<&FixtureFile> = files.iter().collect();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut buffer: Vec<u8> = Vec::new();
    for file in ordered {
        buffer.extend_from_slice(file.path.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(file.content.len().to_string().as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(file.content_sha256.as_bytes());
        buffer.push(0);
    }
    sha256_digest(&buffer)
}

// ---------------------------------------------------------------------------
// Derivations
// ---------------------------------------------------------------------------

fn derive_digests(document: &mut Value, record_dir: &Path) -> Result<(), SealError> {
    let table = document
        .as_table_mut()
        .ok_or_else(|| SealError::InvalidDraft {
            field: "<document>".to_string(),
            reason: "expected a TOML table".to_string(),
        })?;
    derive_prompt_digest(table, record_dir)?;
    derive_fixture_digests(table)?;
    derive_fake_tool_digests(table)?;
    Ok(())
}

fn derive_prompt_digest(
    table: &mut Map<String, Value>,
    record_dir: &Path,
) -> Result<(), SealError> {
    let Some(prompt) = table.get_mut("prompt").and_then(Value::as_table_mut) else {
        return Ok(());
    };
    if prompt.get("kind").and_then(Value::as_str) != Some("file") {
        return Ok(());
    }
    if !prompt
        .get("file_sha256")
        .and_then(Value::as_str)
        .is_some_and(is_pending_sentinel)
    {
        return Ok(());
    }
    let file =
        prompt
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| SealError::InvalidDraft {
                field: "prompt.file".to_string(),
                reason: "file-based prompt is missing its `file` path".to_string(),
            })?;
    if !is_contained_relative_path(file) {
        return Err(SealError::PromptFile {
            path: PathBuf::from(file),
            reason: "prompt.file must be a relative path without `..` segments".to_string(),
        });
    }
    let path = record_dir.join(file);
    let bytes = fs::read(&path).map_err(|error| SealError::PromptFile {
        path: path.clone(),
        reason: error.to_string(),
    })?;
    prompt.insert(
        "file_sha256".to_string(),
        Value::String(sha256_digest(&bytes)),
    );
    Ok(())
}

fn derive_fixture_digests(table: &mut Map<String, Value>) -> Result<(), SealError> {
    let Some(fixture) = table.get_mut("fixture").and_then(Value::as_table_mut) else {
        return Ok(());
    };
    if fixture.get("kind").and_then(Value::as_str) != Some("generated") {
        return Ok(());
    }
    if let Some(files) = fixture.get_mut("files").and_then(Value::as_array_mut) {
        for (index, entry) in files.iter_mut().enumerate() {
            let Some(file) = entry.as_table_mut() else {
                continue;
            };
            if !file
                .get("content_sha256")
                .and_then(Value::as_str)
                .is_some_and(is_pending_sentinel)
            {
                continue;
            }
            let content = file.get("content").and_then(Value::as_str).ok_or_else(|| {
                SealError::InvalidDraft {
                    field: format!("fixture.files[{index}].content"),
                    reason: "generated fixture file is missing its `content` body".to_string(),
                }
            })?;
            let digest = sha256_digest(content.as_bytes());
            file.insert("content_sha256".to_string(), Value::String(digest));
        }
    }
    // Defensive branch (see the module docs): v1 forbids a declared
    // `tree_sha256` on generated fixtures, so `TaskDraft` cannot carry one;
    // if a future draft layer admits it, the documented rule v1 digest is
    // derived here. The final re-parse guard still fails closed if the sealed
    // document were to become schema-invalid.
    if fixture
        .get("tree_sha256")
        .and_then(Value::as_str)
        .is_some_and(is_pending_sentinel)
    {
        let files = declared_fixture_files(fixture.get("files"));
        fixture.insert(
            "tree_sha256".to_string(),
            Value::String(tree_digest_v1(&files)),
        );
    }
    Ok(())
}

fn derive_fake_tool_digests(table: &mut Map<String, Value>) -> Result<(), SealError> {
    let Some(environment) = table.get_mut("environment").and_then(Value::as_table_mut) else {
        return Ok(());
    };
    let Some(tools) = environment
        .get_mut("fake_tools")
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for (index, entry) in tools.iter_mut().enumerate() {
        let Some(tool) = entry.as_table_mut() else {
            continue;
        };
        let profile = tool
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if profile == ABSENT_PROFILE_TOKEN {
            // `absent` must not carry a digest; the draft layer enforces it.
            continue;
        }
        if !tool
            .get("script_sha256")
            .and_then(Value::as_str)
            .is_some_and(is_pending_sentinel)
        {
            continue;
        }
        let id = ProfileId::from_token(&profile).ok_or_else(|| SealError::InvalidDraft {
            field: format!("environment.fake_tools[{index}].profile"),
            reason: format!("unknown fake-tool profile `{profile}`"),
        })?;
        tool.insert(
            "script_sha256".to_string(),
            Value::String(script_sha256(id)),
        );
    }
    Ok(())
}

fn declared_fixture_files(value: Option<&Value>) -> Vec<FixtureFile> {
    let Some(array) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|entry| entry.as_table())
        .map(|file| FixtureFile {
            path: file
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content: file
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content_sha256: file
                .get("content_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_file(path: &str, content: &str) -> FixtureFile {
        FixtureFile {
            path: path.to_string(),
            content: content.to_string(),
            content_sha256: sha256_digest(content.as_bytes()),
        }
    }

    #[test]
    fn tree_digest_is_order_independent() {
        let a = fixture_file("a.txt", "alpha\n");
        let b = fixture_file("b.txt", "beta\n");
        assert_eq!(
            tree_digest_v1(&[a.clone(), b.clone()]),
            tree_digest_v1(&[b, a])
        );
    }

    #[test]
    fn tree_digest_changes_with_every_component() {
        let base = fixture_file("a.txt", "alpha\n");
        let digest = tree_digest_v1(std::slice::from_ref(&base));

        let renamed = FixtureFile {
            path: "b.txt".to_string(),
            ..base.clone()
        };
        assert_ne!(digest, tree_digest_v1(std::slice::from_ref(&renamed)));

        let other_content = fixture_file("a.txt", "alpha!\n");
        assert_ne!(digest, tree_digest_v1(std::slice::from_ref(&other_content)));

        let stale_digest = FixtureFile {
            content_sha256: sha256_digest(b"other"),
            ..base
        };
        assert_ne!(digest, tree_digest_v1(std::slice::from_ref(&stale_digest)));
    }

    #[test]
    fn tree_digest_of_no_files_is_sha256_of_empty_input() {
        assert_eq!(tree_digest_v1(&[]), sha256_digest(b""));
    }
}
