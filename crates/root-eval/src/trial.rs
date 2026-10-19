//! Trial identity and per-trial filesystem layout (Milestone 2, Track A).
//!
//! A [`TrialSpec`] binds one `(task_id, version, condition_id)` cell of an
//! experiment plan to the exact inputs the materializer must lay down:
//! the resolved [`EffectiveContextManifest`], the declared [`ConditionOverlay`],
//! the plan hash, and the repository lock. A [`TrialLayout`] is the directory
//! tree the materializer creates for exactly one trial.
//!
//! This module performs no I/O, spawns no processes, and reads no environment:
//! it only validates identity strings and describes paths.

use std::path::PathBuf;

use thiserror::Error;

use crate::condition::ConditionOverlay;
use crate::context::EffectiveContextManifest;
use crate::digest::is_canonical_sha256_digest;
use crate::plan::RepositoryLock;
use crate::validate::is_task_identifier;

/// Longest accepted [`TrialSpec::trial_id`], in bytes.
pub const MAX_TRIAL_ID_LEN: usize = 64;

/// Machine-matchable classification for every [`TrialError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrialErrorKind {
    /// `trial_id` is empty, too long, or outside `[a-z0-9-]`.
    InvalidTrialId,
    /// A required identity field is empty or malformed.
    InvalidField,
    /// A digest field is not `sha256:<64 lowercase hex>`.
    DigestInvalid,
}

/// Errors produced while validating a [`TrialSpec`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TrialError {
    #[error("invalid trial_id `{value}`: expected 1-{max} characters of [a-z0-9-]")]
    InvalidTrialId { value: String, max: usize },

    #[error("invalid `{field}`: {reason}")]
    InvalidField { field: String, reason: String },

    #[error("invalid digest in `{field}`: expected sha256:<64 lowercase hex>, found `{value}`")]
    DigestInvalid { field: String, value: String },
}

impl TrialError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> TrialErrorKind {
        match self {
            Self::InvalidTrialId { .. } => TrialErrorKind::InvalidTrialId,
            Self::InvalidField { .. } => TrialErrorKind::InvalidField,
            Self::DigestInvalid { .. } => TrialErrorKind::DigestInvalid,
        }
    }
}

/// One trial cell: the exact inputs the materializer must lay down.
///
/// Treatment-ness lives entirely in `manifest` + `overlay` (a treatment
/// manifest carries the declared skill files; a baseline manifest carries
/// none). The materializer never sniffs `condition_id` strings.
#[derive(Debug, Clone)]
pub struct TrialSpec {
    /// Per-trial directory name: `1..=64` chars of `[a-z0-9-]`.
    pub trial_id: String,
    /// Owning experiment identifier (non-empty).
    pub experiment_id: String,
    /// Benchmark task identifier (task-record `[a-z][a-z0-9._-]{0,63}` grammar).
    pub task_id: String,
    /// Benchmark task version.
    pub task_version: u64,
    /// Condition identifier (for example `baseline` / `treatment`).
    pub condition_id: String,
    /// Resolved effective context whose `project:` skill files are written
    /// into the trial repository.
    pub manifest: EffectiveContextManifest,
    /// Declared file overlay; empty for the baseline condition.
    pub overlay: ConditionOverlay,
    /// Canonical plan hash this trial was generated from.
    pub plan_hash: String,
    /// Repository lock: the commit to check out and the tree digest.
    pub repository: RepositoryLock,
    /// Checkout source: a git repository directory, or a plain file tree
    /// that is copied verbatim (see the materializer docs).
    pub worktree_source: PathBuf,
}

impl TrialSpec {
    /// Fail-closed validation of every identity, digest, and path field.
    pub fn validate(&self) -> Result<(), TrialError> {
        validate_trial_id(&self.trial_id)?;
        require_non_empty(&self.experiment_id, "experiment_id")?;
        if !is_task_identifier(&self.task_id) {
            return Err(TrialError::InvalidField {
                field: "task_id".to_string(),
                reason: "expected [a-z][a-z0-9._-]{0,63}".to_string(),
            });
        }
        require_non_empty(&self.condition_id, "condition_id")?;
        if !is_canonical_sha256_digest(&self.plan_hash) {
            return Err(TrialError::DigestInvalid {
                field: "plan_hash".to_string(),
                value: self.plan_hash.clone(),
            });
        }
        require_non_empty(&self.repository.commit, "repository.commit")?;
        if !is_canonical_sha256_digest(&self.repository.tree_sha256) {
            return Err(TrialError::DigestInvalid {
                field: "repository.tree_sha256".to_string(),
                value: self.repository.tree_sha256.clone(),
            });
        }
        if self.worktree_source.as_os_str().is_empty() {
            return Err(TrialError::InvalidField {
                field: "worktree_source".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        Ok(())
    }
}

/// Validate a trial directory name: `1..=64` chars of `[a-z0-9-]`.
pub fn validate_trial_id(value: &str) -> Result<(), TrialError> {
    let bytes = value.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= MAX_TRIAL_ID_LEN
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if ok {
        Ok(())
    } else {
        Err(TrialError::InvalidTrialId {
            value: value.to_string(),
            max: MAX_TRIAL_ID_LEN,
        })
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<(), TrialError> {
    if value.is_empty() {
        Err(TrialError::InvalidField {
            field: field.to_string(),
            reason: "must not be empty".to_string(),
        })
    } else {
        Ok(())
    }
}

/// The per-trial directory tree created by the materializer.
///
/// ```text
/// <root>/
///   repo/                 detached checkout (trial cwd / project root)
///   home/                 trial $HOME
///   xdg-config/           trial $XDG_CONFIG_HOME
///   xdg-data/             trial $XDG_DATA_HOME
///   xdg-cache/            trial $XDG_CACHE_HOME
///   xdg-state/            trial $XDG_STATE_HOME
///   tmp/                  trial $TMPDIR
///   fake-bin/             trial-local fake tools (first on $PATH)
///   trial.env             allowlist env file (shell-sourceable)
///   audit.log             fake-tool audit trail ($TRIAL_AUDIT_LOG)
///   pids.txt              supervised-process marker (`none`; Track A spawns
///                         nothing long-lived)
///   residue-manifest.txt  every path created by materialization
/// ```
#[derive(Debug, Clone)]
pub struct TrialLayout {
    /// `<base_dir>/<trial_id>`; every other path lives under this root.
    pub root: PathBuf,
    /// Detached repository checkout (the trial working directory).
    pub repo_dir: PathBuf,
    /// Trial `$HOME`.
    pub home_dir: PathBuf,
    /// Trial `$XDG_CONFIG_HOME`.
    pub xdg_config_home: PathBuf,
    /// Trial `$XDG_DATA_HOME`.
    pub xdg_data_home: PathBuf,
    /// Trial `$XDG_CACHE_HOME`.
    pub xdg_cache_home: PathBuf,
    /// Trial `$XDG_STATE_HOME`.
    pub xdg_state_home: PathBuf,
    /// Trial `$TMPDIR`.
    pub tmpdir: PathBuf,
    /// Trial-local fake tools directory (sole `$PATH` entry).
    pub fake_bin_dir: PathBuf,
    /// Shell-sourceable allowlist environment file.
    pub env_file: PathBuf,
    /// Fake-tool audit log (`$TRIAL_AUDIT_LOG`).
    pub audit_log: PathBuf,
    /// Process-marker file for the residue scan.
    pub pids_file: PathBuf,
    /// Residue manifest listing every created path.
    pub residue_manifest_file: PathBuf,
    /// Every path created by materialization, in creation order.
    pub created_paths: Vec<PathBuf>,
}

impl TrialLayout {
    /// True when `path` (lexically) lives under the trial root.
    pub fn contains(&self, path: &std::path::Path) -> bool {
        path.starts_with(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: &str) -> String {
        format!("sha256:{}", root_lockfile::compute_sha256(seed.as_bytes()))
    }

    #[test]
    fn trial_id_grammar() {
        assert_eq!(validate_trial_id("trial-001"), Ok(()));
        assert_eq!(
            validate_trial_id("a"),
            Ok(()),
            "single characters are valid"
        );
        assert_eq!(
            validate_trial_id(&"a".repeat(MAX_TRIAL_ID_LEN)),
            Ok(()),
            "exactly 64 chars are valid"
        );
        for bad in [
            "",
            "Trial-001",
            "trial_001",
            "trial 001",
            "trial/001",
            "../escape",
            &"a".repeat(MAX_TRIAL_ID_LEN + 1),
        ] {
            assert_eq!(
                validate_trial_id(bad).map_err(|error| error.kind()),
                Err(TrialErrorKind::InvalidTrialId),
                "trial_id `{bad}` must be refused"
            );
        }
    }

    #[test]
    fn empty_identity_fields_are_refused() {
        // Build a spec-shaped value without needing a real manifest: only
        // the identity checks run before any manifest use, so an empty
        // experiment_id must fail first. (Full-spec tests live in the
        // integration suite, which constructs manifests from fixtures.)
        let err = TrialError::InvalidField {
            field: "experiment_id".to_string(),
            reason: "must not be empty".to_string(),
        };
        assert_eq!(err.kind(), TrialErrorKind::InvalidField);
        assert_eq!(
            TrialError::DigestInvalid {
                field: "plan_hash".to_string(),
                value: "nope".to_string(),
            }
            .kind(),
            TrialErrorKind::DigestInvalid
        );
        // The digest helper used by validation accepts only canonical digests.
        assert!(!is_canonical_sha256_digest("nope"));
        assert!(is_canonical_sha256_digest(&digest("x")));
    }
}
