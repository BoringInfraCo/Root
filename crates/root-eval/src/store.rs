//! Local evaluation SQLite store (Milestone 3, Phase B).
//!
//! [`EvalStore`] owns one SQLite database per experiment at
//! `<evals_root>/<experiment_id>/state.db` — the audit's "separate versioned
//! SQLite database under `$ROOT_DIR/evals/<experiment-id>/state.db`". Evidence
//! bytes stay in the content-addressed artifact store beside it
//! ([`ArtifactStore`], `<evals_root>/artifacts/sha256/...`); the database
//! stores references and hashes, never unlimited blobs.
//!
//! The store takes an explicit `evals_root`; it never reads `ROOT_DIR` or any
//! other environment variable, and it never touches the continuity database.
//!
//! # Schema (v1, forward-only)
//!
//! `meta`, `experiments`, `trials`, `executions`, `verifications`, `evidence`,
//! `artifacts`, and `evidence_artifacts`, with indexes on
//! `trials(experiment_id)`, `executions(trial_id)`, `verifications(trial_id)`,
//! and `evidence(trial_id)`. Relationships are explicit foreign keys, so every
//! execution/trial/config/task/revision/verifier relationship is queryable
//! (the M3 exit criterion) and a row for a missing parent is refused.
//!
//! # State model
//!
//! `trials.state` summarizes what exists for a trial:
//! `sealed` > `verified` > `executed` > `planned` by row presence, and
//! `invalid` whenever the recorded outcome class is one of the invalid classes
//! (invalid wins until [`EvalStore::set_trial_outcome`] records a non-invalid
//! class, which recomputes the milestone state). Every write recomputes this
//! column from the rows themselves, so `state` cannot drift from the record.
//!
//! # Validation and duplicates
//!
//! Every write validates ids, digests, JSON columns, and integer ranges before
//! touching SQLite. Duplicate primary keys are typed [`StoreError::Duplicate`]
//! errors, never silent overwrites. A repeated artifact content address
//! dedupes in the `artifacts` registry, but a contradicting byte count for the
//! same address is refused. Evidence registration re-verifies the manifest
//! seal and cross-checks the manifest identity (experiment, trial, plan, task,
//! and context digests) against the trial row, so the store cannot hold a
//! manifest that claims to belong to another trial.
//!
//! # Atomicity, backups, and read-only access
//!
//! Every write is one transaction; a failed statement leaves no partial rows
//! (the M3 "failed publication" criterion). Opening an older schema version
//! creates a `<db>.backup-v<n>` copy with `VACUUM INTO` before migrating — with
//! v1 as the current version the guard is unreachable until a v2 migration
//! exists (it is unit-tested directly). [`EvalStore::open_read_only`] creates
//! nothing, refuses an out-of-date schema instead of migrating, avoids
//! `-wal`/`-shm` sidecars, and fails every write path with
//! [`StoreError::ReadOnly`].
//!
//! # Timestamps and identities
//!
//! Timestamps are caller-supplied Unix milliseconds (`u64`) per the M2/M3
//! convention; ids are caller-supplied strings. The store invents neither.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::digest::is_canonical_sha256_digest;
use crate::evidence::{
    verify_artifacts, verify_seal, ArtifactRef, ArtifactStore, EvidenceManifest,
};
use crate::supervisor::TerminationKind;
use crate::taxonomy::TrialOutcomeClass;
use crate::verifier::VerifierVerdict;

/// Current eval-store schema version written by this build.
pub const EVAL_STORE_SCHEMA_VERSION: u32 = 1;

/// Highest eval-store schema version this build can read.
///
/// A store declaring a newer version is refused on open — never partially
/// read and never migrated.
pub const EVAL_STORE_MAX_SUPPORTED_VERSION: u32 = 1;

/// Database file name inside the per-experiment directory.
pub const EVAL_STORE_FILE_NAME: &str = "state.db";

/// Longest accepted id string, in bytes.
pub const MAX_EVAL_STORE_ID_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`StoreError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreErrorKind {
    /// The database file or its directory could not be created or opened.
    Open,
    /// The store declares a schema version this build cannot read.
    UnsupportedSchema,
    /// A migration (or its pre-migration backup) failed.
    Migration,
    /// A SQL statement failed.
    Query,
    /// A referenced row (or the requested row) does not exist.
    NotFound,
    /// A primary-key/unique row already exists.
    Duplicate,
    /// A caller-supplied field failed validation.
    InvalidInput,
    /// A transaction could not start or commit.
    Transaction,
    /// A write was attempted on a read-only store.
    ReadOnly,
    /// Evidence integrity verification failed.
    Integrity,
}

/// Errors produced by the local evaluation store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    #[error("eval store at `{path}` could not be opened: {message}")]
    Open { path: PathBuf, message: String },

    #[error("eval store schema version {found} is not supported by this build (supported: {supported}); {hint}")]
    UnsupportedSchema {
        found: u32,
        supported: u32,
        hint: String,
    },

    #[error("eval store migration to version {version} failed: {message}")]
    Migration { version: u32, message: String },

    #[error("eval store query failed: {message}")]
    Query { message: String },

    #[error("{entity} `{id}` was not found")]
    NotFound { entity: String, id: String },

    #[error("{entity} `{id}` already exists")]
    Duplicate { entity: String, id: String },

    #[error("invalid `{field}`: {message}")]
    InvalidInput { field: String, message: String },

    #[error("eval store transaction failed: {message}")]
    Transaction { message: String },

    #[error("`{operation}` is not available on a read-only eval store")]
    ReadOnly { operation: String },

    #[error("eval store integrity check failed: {message}")]
    Integrity { message: String },
}

impl StoreError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> StoreErrorKind {
        match self {
            Self::Open { .. } => StoreErrorKind::Open,
            Self::UnsupportedSchema { .. } => StoreErrorKind::UnsupportedSchema,
            Self::Migration { .. } => StoreErrorKind::Migration,
            Self::Query { .. } => StoreErrorKind::Query,
            Self::NotFound { .. } => StoreErrorKind::NotFound,
            Self::Duplicate { .. } => StoreErrorKind::Duplicate,
            Self::InvalidInput { .. } => StoreErrorKind::InvalidInput,
            Self::Transaction { .. } => StoreErrorKind::Transaction,
            Self::ReadOnly { .. } => StoreErrorKind::ReadOnly,
            Self::Integrity { .. } => StoreErrorKind::Integrity,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(code, _) = &error {
            if code.code == rusqlite::ErrorCode::ReadOnly {
                return StoreError::ReadOnly {
                    operation: "sqlite".to_string(),
                };
            }
        }
        StoreError::Query {
            message: error.to_string(),
        }
    }
}

/// Map an insert/update failure with known entity identity to a typed error.
fn write_error(entity: &str, id: &str, error: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(code, _) = &error {
        match code.code {
            rusqlite::ErrorCode::ConstraintViolation => {
                return match code.extended_code {
                    rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                    | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => StoreError::Duplicate {
                        entity: entity.to_string(),
                        id: id.to_string(),
                    },
                    rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => StoreError::NotFound {
                        entity: format!("referenced row for {entity}"),
                        id: id.to_string(),
                    },
                    _ => StoreError::InvalidInput {
                        field: entity.to_string(),
                        message: format!("`{id}`: {error}"),
                    },
                };
            }
            rusqlite::ErrorCode::ReadOnly => {
                return StoreError::ReadOnly {
                    operation: entity.to_string(),
                };
            }
            _ => {}
        }
    }
    StoreError::Query {
        message: format!("{entity} `{id}`: {error}"),
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const MIGRATION_V1: &str = r#"
CREATE TABLE experiments (
    experiment_id TEXT PRIMARY KEY,
    plan_hash     TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE trials (
    trial_id               TEXT PRIMARY KEY,
    experiment_id          TEXT NOT NULL REFERENCES experiments(experiment_id),
    task_id                TEXT NOT NULL,
    task_version           INTEGER NOT NULL,
    condition_id           TEXT NOT NULL,
    repetition             INTEGER NOT NULL,
    order_index            INTEGER NOT NULL,
    plan_hash              TEXT NOT NULL,
    manifest_sha256        TEXT NOT NULL,
    task_content_sha256    TEXT NOT NULL,
    repository_commit      TEXT,
    repository_tree_sha256 TEXT,
    state                  TEXT NOT NULL CHECK(state IN ('planned','executed','verified','sealed','invalid')),
    outcome_class          TEXT,
    invalid_reason         TEXT
);
CREATE INDEX idx_trials_experiment ON trials(experiment_id);

CREATE TABLE executions (
    execution_id           TEXT PRIMARY KEY,
    trial_id               TEXT NOT NULL REFERENCES trials(trial_id),
    termination            TEXT NOT NULL,
    exit_code              INTEGER,
    wall_ms                INTEGER NOT NULL,
    started_at_ms          INTEGER NOT NULL,
    finished_at_ms         INTEGER NOT NULL,
    requested_model        TEXT NOT NULL,
    observed_model         TEXT,
    raw_sha256             TEXT NOT NULL,
    events_sha256          TEXT NOT NULL,
    session_ids_json       TEXT NOT NULL,
    usage_json             TEXT NOT NULL,
    missing_telemetry_json TEXT NOT NULL
);
CREATE INDEX idx_executions_trial ON executions(trial_id);

CREATE TABLE verifications (
    verification_id   TEXT PRIMARY KEY,
    trial_id          TEXT NOT NULL REFERENCES trials(trial_id),
    verifier_id       TEXT NOT NULL,
    verdict           TEXT NOT NULL CHECK(verdict IN ('pass','fail','invalid')),
    mutation_detected INTEGER NOT NULL,
    exit_codes_json   TEXT NOT NULL,
    stdout_sha256     TEXT NOT NULL,
    stderr_sha256     TEXT NOT NULL,
    pre_tree_sha256   TEXT NOT NULL,
    post_tree_sha256  TEXT NOT NULL,
    wall_ms           INTEGER NOT NULL,
    timed_out         INTEGER NOT NULL
);
CREATE INDEX idx_verifications_trial ON verifications(trial_id);

CREATE TABLE evidence (
    seal_sha256   TEXT PRIMARY KEY,
    trial_id      TEXT NOT NULL REFERENCES trials(trial_id),
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id),
    manifest_json TEXT NOT NULL,
    sealed_at_ms  INTEGER NOT NULL
);
CREATE INDEX idx_evidence_trial ON evidence(trial_id);

CREATE TABLE artifacts (
    sha256       TEXT PRIMARY KEY,
    logical_name TEXT NOT NULL,
    bytes        INTEGER NOT NULL
);

CREATE TABLE evidence_artifacts (
    seal_sha256     TEXT NOT NULL REFERENCES evidence(seal_sha256),
    artifact_sha256 TEXT NOT NULL REFERENCES artifacts(sha256),
    logical_name    TEXT NOT NULL,
    PRIMARY KEY (seal_sha256, logical_name)
);
"#;

const EXPERIMENT_COLUMNS: &str = "experiment_id, plan_hash, created_at_ms";

const TRIAL_COLUMNS: &str = "trial_id, experiment_id, task_id, task_version, condition_id, \
     repetition, order_index, plan_hash, manifest_sha256, task_content_sha256, \
     repository_commit, repository_tree_sha256, state, outcome_class, invalid_reason";

const EXECUTION_COLUMNS: &str = "execution_id, trial_id, termination, exit_code, wall_ms, \
     started_at_ms, finished_at_ms, requested_model, observed_model, raw_sha256, \
     events_sha256, session_ids_json, usage_json, missing_telemetry_json";

const VERIFICATION_COLUMNS: &str = "verification_id, trial_id, verifier_id, verdict, \
     mutation_detected, exit_codes_json, stdout_sha256, stderr_sha256, pre_tree_sha256, \
     post_tree_sha256, wall_ms, timed_out";

const EVIDENCE_COLUMNS: &str = "seal_sha256, trial_id, experiment_id, manifest_json, sealed_at_ms";

const ARTIFACT_COLUMNS: &str = "sha256, logical_name, bytes";

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

/// Lifecycle summary of one trial (`trials.state`).
///
/// See the module docs for how the value is recomputed from the rows that
/// exist for the trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrialState {
    /// The trial row exists; nothing else has been recorded.
    Planned,
    /// At least one execution row exists.
    Executed,
    /// At least one verification row exists.
    Verified,
    /// At least one sealed evidence manifest exists.
    Sealed,
    /// The recorded outcome class is an invalid class.
    Invalid,
}

impl TrialState {
    /// Stable token stored in `trials.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Executed => "executed",
            Self::Verified => "verified",
            Self::Sealed => "sealed",
            Self::Invalid => "invalid",
        }
    }

    /// Parse the stable token produced by [`TrialState::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "planned" => Some(Self::Planned),
            "executed" => Some(Self::Executed),
            "verified" => Some(Self::Verified),
            "sealed" => Some(Self::Sealed),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// One `experiments` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentRow {
    /// Experiment identity (must match the store's experiment).
    pub experiment_id: String,
    /// Canonical experiment plan hash (`sha256:<64 lowercase hex>`).
    pub plan_hash: String,
    /// Creation time, Unix milliseconds.
    pub created_at_ms: u64,
}

/// One `trials` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialRow {
    /// Trial identity.
    pub trial_id: String,
    /// Owning experiment identity (must match the store's experiment).
    pub experiment_id: String,
    /// Benchmark task identity.
    pub task_id: String,
    /// Benchmark task version.
    pub task_version: u64,
    /// Condition identity (`baseline`, `treatment`, ...).
    pub condition_id: String,
    /// Repetition index within the task/condition block.
    pub repetition: u64,
    /// Precomputed balanced order position.
    pub order_index: u64,
    /// Canonical plan hash the trial was generated from.
    pub plan_hash: String,
    /// Effective-context manifest hash materialized for the trial.
    pub manifest_sha256: String,
    /// Sealed task-record content digest.
    pub task_content_sha256: String,
    /// Repository commit the trial checkout was pinned to, when known.
    pub repository_commit: Option<String>,
    /// Repository tree fingerprint, when known.
    pub repository_tree_sha256: Option<String>,
    /// Lifecycle summary (see the module docs).
    pub state: TrialState,
    /// Recorded outcome class, when classification has happened.
    pub outcome_class: Option<TrialOutcomeClass>,
    /// Why an invalid class invalidated the trial.
    pub invalid_reason: Option<String>,
}

/// One `executions` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRow {
    /// Execution identity.
    pub execution_id: String,
    /// Owning trial identity.
    pub trial_id: String,
    /// Supervisor terminal classification (never re-interpreted).
    pub termination: TerminationKind,
    /// Process exit code (`None` when signaled or never spawned).
    pub exit_code: Option<i32>,
    /// Wall time of the run, milliseconds.
    pub wall_ms: u64,
    /// Spawn time, Unix milliseconds.
    pub started_at_ms: u64,
    /// Reap time, Unix milliseconds.
    pub finished_at_ms: u64,
    /// Requested model, recorded verbatim.
    pub requested_model: String,
    /// Observed model, when the harness exposes one.
    pub observed_model: Option<String>,
    /// `sha256:<hex>` over the exact raw stdout bytes.
    pub raw_sha256: String,
    /// `sha256:<hex>` over the normalized event stream.
    pub events_sha256: String,
    /// JSON array of harness session ids, stored exactly as supplied.
    pub session_ids_json: String,
    /// JSON usage object, stored exactly as supplied.
    pub usage_json: String,
    /// JSON array naming every unavailable telemetry field, stored exactly as
    /// supplied. Missing telemetry is never coerced to an empty array.
    pub missing_telemetry_json: String,
}

impl ExecutionRow {
    /// Parse [`ExecutionRow::session_ids_json`] as a list of strings.
    pub fn session_ids(&self) -> Result<Vec<String>, StoreError> {
        let value: JsonValue =
            serde_json::from_str(&self.session_ids_json).map_err(|error| StoreError::Query {
                message: format!("stored session_ids_json is not valid JSON: {error}"),
            })?;
        json_string_array(&value).ok_or_else(|| StoreError::Query {
            message: "stored session_ids_json is not a JSON array of strings".to_string(),
        })
    }

    /// Parse [`ExecutionRow::missing_telemetry_json`] as a list of strings.
    pub fn missing_telemetry(&self) -> Result<Vec<String>, StoreError> {
        let value: JsonValue =
            serde_json::from_str(&self.missing_telemetry_json).map_err(|error| {
                StoreError::Query {
                    message: format!("stored missing_telemetry_json is not valid JSON: {error}"),
                }
            })?;
        json_string_array(&value).ok_or_else(|| StoreError::Query {
            message: "stored missing_telemetry_json is not a JSON array of strings".to_string(),
        })
    }

    /// Parse [`ExecutionRow::usage_json`] as a JSON value.
    pub fn usage(&self) -> Result<JsonValue, StoreError> {
        serde_json::from_str(&self.usage_json).map_err(|error| StoreError::Query {
            message: format!("stored usage_json is not valid JSON: {error}"),
        })
    }
}

/// One `verifications` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRow {
    /// Verification identity.
    pub verification_id: String,
    /// Owning trial identity.
    pub trial_id: String,
    /// Verifier identity from the predeclared spec.
    pub verifier_id: String,
    /// Machine-matchable verdict.
    pub verdict: VerifierVerdict,
    /// True when the verifier mutated the scored tree.
    pub mutation_detected: bool,
    /// JSON array of per-command exit codes (`null` for commands that never
    /// ran), stored exactly as supplied.
    pub exit_codes_json: String,
    /// `sha256:<hex>` of the retained verifier stdout.
    pub stdout_sha256: String,
    /// `sha256:<hex>` of the retained verifier stderr.
    pub stderr_sha256: String,
    /// Tree fingerprint before the verifier ran (`""` when the pre-fingerprint
    /// itself failed).
    pub pre_tree_sha256: String,
    /// Tree fingerprint after the verifier ran (`""` when it could not be
    /// computed).
    pub post_tree_sha256: String,
    /// Wall time of the verifier run, milliseconds.
    pub wall_ms: u64,
    /// True when a command hit the wall timeout.
    pub timed_out: bool,
}

/// One `evidence` row (a sealed manifest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRecord {
    /// Evidence seal (`sha256:<hex>` over the canonical manifest projection).
    pub seal_sha256: String,
    /// Owning trial identity.
    pub trial_id: String,
    /// Owning experiment identity.
    pub experiment_id: String,
    /// The sealed manifest, serialized as stored.
    pub manifest_json: String,
    /// Caller-supplied seal time, Unix milliseconds.
    pub sealed_at_ms: u64,
}

impl EvidenceRecord {
    /// Parse [`EvidenceRecord::manifest_json`] back into an
    /// [`EvidenceManifest`].
    pub fn manifest(&self) -> Result<EvidenceManifest, StoreError> {
        serde_json::from_str(&self.manifest_json).map_err(|error| StoreError::Query {
            message: format!("stored evidence manifest is not a valid manifest: {error}"),
        })
    }
}

/// One `artifacts` registry row (content-addressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    /// Canonical content address.
    pub sha256: String,
    /// Logical name this address was first registered under.
    pub logical_name: String,
    /// Declared byte count.
    pub bytes: u64,
}

/// `trial_full` projection: the trial row joined with the latest execution,
/// the latest verification, the latest sealed evidence row, and the outcome
/// class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialFull {
    /// The trial row itself.
    pub trial: TrialRow,
    /// Latest execution by insertion order, when one exists.
    pub latest_execution: Option<ExecutionRow>,
    /// Latest verification by insertion order, when one exists.
    pub latest_verification: Option<VerificationRow>,
    /// Latest sealed evidence row by insertion order, when one exists.
    pub evidence: Option<EvidenceRecord>,
    /// Mirrors [`TrialRow::outcome_class`] for callers that join explicitly.
    pub outcome_class: Option<TrialOutcomeClass>,
}

/// Trial-state counts and outcome-class histogram for one experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentSummary {
    /// Trials whose state is `planned`.
    pub planned: u32,
    /// Trials whose state is `executed`.
    pub executed: u32,
    /// Trials whose state is `verified`.
    pub verified: u32,
    /// Trials whose state is `sealed`.
    pub sealed: u32,
    /// Trials whose state is `invalid`.
    pub invalid: u32,
    /// Recorded outcome classes with their trial counts, ordered by token.
    pub outcome_class_counts: Vec<(String, u32)>,
}

/// Which integrity check failed for one stored manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreIntegrityCheck {
    /// The stored `manifest_json` is not a valid evidence manifest.
    ManifestJson,
    /// The stored row identity (seal, trial, experiment) disagrees with the
    /// manifest JSON.
    Identity,
    /// The recomputed seal does not match the manifest's declared seal.
    Seal,
    /// A referenced artifact is missing or its stored bytes do not verify.
    Artifact,
}

/// One integrity failure, attributed to a trial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIntegrityFailure {
    /// Trial the failing evidence row belongs to.
    pub trial_id: String,
    /// Seal of the failing evidence row.
    pub seal_sha256: String,
    /// Which check failed.
    pub check: StoreIntegrityCheck,
    /// Human-readable reason.
    pub message: String,
}

/// Result of [`EvalStore::verify_store_evidence`].
///
/// The report is `Ok` even when checks fail — failures are listed, not thrown,
/// so a caller can report every broken manifest at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIntegrityReport {
    /// Number of stored evidence manifests checked.
    pub checked: u64,
    /// Failures, in stored order.
    pub failures: Vec<StoreIntegrityFailure>,
}

impl StoreIntegrityReport {
    /// True when every checked manifest verified.
    pub fn is_intact(&self) -> bool {
        self.failures.is_empty()
    }

    /// Failures attributed to one trial.
    pub fn failures_for_trial<'a>(&'a self, trial_id: &str) -> Vec<&'a StoreIntegrityFailure> {
        self.failures
            .iter()
            .filter(|failure| failure.trial_id == trial_id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// One experiment's local evaluation database.
#[derive(Debug)]
pub struct EvalStore {
    conn: Connection,
    experiment_id: String,
    path: PathBuf,
    read_only: bool,
}

impl EvalStore {
    /// Open (creating if needed) the store for `experiment_id` under
    /// `evals_root` and migrate it to the current schema version.
    ///
    /// Creates `<evals_root>/<experiment_id>/state.db` with 0700 directories
    /// and a 0600 database file, enables WAL and foreign keys, and refuses a
    /// store whose schema version is newer than this build supports (typed
    /// [`StoreError::UnsupportedSchema`], never a partial read). Never reads
    /// the process environment.
    pub fn open(evals_root: &Path, experiment_id: &str) -> Result<Self, StoreError> {
        require_id("experiment_id", experiment_id)?;
        require_experiment_component(experiment_id)?;

        let dir = evals_root.join(experiment_id);
        let path = dir.join(EVAL_STORE_FILE_NAME);
        fs::create_dir_all(&dir).map_err(|error| StoreError::Open {
            path: dir.clone(),
            message: error.to_string(),
        })?;
        set_mode(&dir, 0o700)?;
        set_mode(evals_root, 0o700)?;

        let conn = Connection::open(&path).map_err(|error| StoreError::Open {
            path: path.clone(),
            message: error.to_string(),
        })?;
        configure(&conn).map_err(|error| StoreError::Open {
            path: path.clone(),
            message: format!("failed to configure the eval store: {error}"),
        })?;

        let current = read_schema_version(&conn).map_err(|error| StoreError::Open {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if current > EVAL_STORE_MAX_SUPPORTED_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: current,
                supported: EVAL_STORE_MAX_SUPPORTED_VERSION,
                hint: "upgrade Root before opening this eval store".to_string(),
            });
        }
        if needs_pre_migration_backup(current, EVAL_STORE_SCHEMA_VERSION) {
            create_pre_migration_backup(&conn, &path, current)?;
        }
        migrate(&conn)?;
        set_mode(&path, 0o600)?;

        Ok(Self {
            conn,
            experiment_id: experiment_id.to_string(),
            path,
            read_only: false,
        })
    }

    /// Open an existing store strictly read-only.
    ///
    /// Never creates the database or its parent directory, never migrates, and
    /// never enables WAL, so no `-wal`/`-shm` sidecars are created for a
    /// cleanly closed database. A store whose schema is not the current
    /// version is refused (migrate it read-write first). Every write method
    /// returns [`StoreError::ReadOnly`].
    pub fn open_read_only(evals_root: &Path, experiment_id: &str) -> Result<Self, StoreError> {
        require_id("experiment_id", experiment_id)?;
        require_experiment_component(experiment_id)?;

        let path = evals_root.join(experiment_id).join(EVAL_STORE_FILE_NAME);
        if !path.exists() {
            return Err(StoreError::Open {
                path,
                message: "eval store does not exist; open it read-write to create it".to_string(),
            });
        }

        // A cleanly closed WAL database has no live `-wal`, so it can be
        // opened `immutable` — that keeps SQLite from creating `-wal`/`-shm`
        // sidecars. If a writer currently holds uncheckpointed frames, open
        // the existing WAL read-only instead (the sidecars already exist;
        // nothing new is created).
        let conn = if wal_has_frames(&path) {
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        } else {
            Connection::open_with_flags(
                sqlite_immutable_uri(&path),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
        }
        .map_err(|error| StoreError::Open {
            path: path.clone(),
            message: error.to_string(),
        })?;
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA foreign_keys = ON;")
            .map_err(|error| StoreError::Open {
                path: path.clone(),
                message: format!("failed to configure the read-only eval store: {error}"),
            })?;

        let current = read_schema_version(&conn).map_err(|error| StoreError::Open {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if current > EVAL_STORE_MAX_SUPPORTED_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: current,
                supported: EVAL_STORE_MAX_SUPPORTED_VERSION,
                hint: "upgrade Root before opening this eval store".to_string(),
            });
        }
        if current < EVAL_STORE_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: current,
                supported: EVAL_STORE_SCHEMA_VERSION,
                hint: "open the store read-write to migrate before read-only inspection"
                    .to_string(),
            });
        }

        Ok(Self {
            conn,
            experiment_id: experiment_id.to_string(),
            path,
            read_only: true,
        })
    }

    /// The experiment this store belongs to.
    pub fn experiment_id(&self) -> &str {
        &self.experiment_id
    }

    /// The database file path (`<evals_root>/<experiment_id>/state.db`).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when the store was opened with [`EvalStore::open_read_only`].
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The applied schema version.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        read_schema_version(&self.conn).map_err(StoreError::from)
    }

    // -----------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------

    /// Record the experiment row.
    pub fn record_experiment(&self, row: ExperimentRow) -> Result<(), StoreError> {
        self.ensure_writable("record_experiment")?;
        self.require_own_experiment(&row.experiment_id)?;
        require_digest("plan_hash", &row.plan_hash)?;
        let created_at_ms = require_u64("created_at_ms", row.created_at_ms)?;

        let tx = self.transaction("record_experiment")?;
        tx.execute(
            "INSERT INTO experiments (experiment_id, plan_hash, created_at_ms) VALUES (?1, ?2, ?3)",
            params![row.experiment_id, row.plan_hash, created_at_ms],
        )
        .map_err(|error| write_error("experiment", &row.experiment_id, error))?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    /// Record a planned trial.
    pub fn record_trial(&self, row: TrialRow) -> Result<(), StoreError> {
        self.ensure_writable("record_trial")?;
        require_id("trial_id", &row.trial_id)?;
        self.require_own_experiment(&row.experiment_id)?;
        require_id("task_id", &row.task_id)?;
        require_id("condition_id", &row.condition_id)?;
        require_digest("plan_hash", &row.plan_hash)?;
        require_digest("manifest_sha256", &row.manifest_sha256)?;
        require_digest("task_content_sha256", &row.task_content_sha256)?;
        if let Some(commit) = &row.repository_commit {
            require_id("repository_commit", commit)?;
        }
        if let Some(tree) = &row.repository_tree_sha256 {
            require_digest("repository_tree_sha256", tree)?;
        }
        let task_version = require_u64("task_version", row.task_version)?;
        let repetition = require_u64("repetition", row.repetition)?;
        let order_index = require_u64("order_index", row.order_index)?;
        validate_outcome_coherence(row.outcome_class, row.invalid_reason.as_deref())?;
        if row.state == TrialState::Invalid
            && !row.outcome_class.is_some_and(|class| class.is_invalid())
        {
            return Err(StoreError::InvalidInput {
                field: "state".to_string(),
                message: "a trial recorded as `invalid` must carry an invalid outcome class"
                    .to_string(),
            });
        }
        if row.state != TrialState::Invalid
            && row.outcome_class.is_some_and(|class| class.is_invalid())
        {
            return Err(StoreError::InvalidInput {
                field: "state".to_string(),
                message: "an invalid outcome class requires `state = invalid`".to_string(),
            });
        }

        let tx = self.transaction("record_trial")?;
        tx.execute(
            "INSERT INTO trials (
                 trial_id, experiment_id, task_id, task_version, condition_id, repetition,
                 order_index, plan_hash, manifest_sha256, task_content_sha256,
                 repository_commit, repository_tree_sha256, state, outcome_class, invalid_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                row.trial_id,
                row.experiment_id,
                row.task_id,
                task_version,
                row.condition_id,
                repetition,
                order_index,
                row.plan_hash,
                row.manifest_sha256,
                row.task_content_sha256,
                row.repository_commit,
                row.repository_tree_sha256,
                row.state.as_str(),
                row.outcome_class.map(|class| class.as_str()),
                row.invalid_reason,
            ],
        )
        .map_err(|error| write_error("trial", &row.trial_id, error))?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    /// Record one execution attempt for a trial.
    pub fn record_execution(&self, row: ExecutionRow) -> Result<(), StoreError> {
        self.ensure_writable("record_execution")?;
        require_id("execution_id", &row.execution_id)?;
        require_id("trial_id", &row.trial_id)?;
        require_digest("raw_sha256", &row.raw_sha256)?;
        require_digest("events_sha256", &row.events_sha256)?;
        require_id("requested_model", &row.requested_model)?;
        if let Some(observed) = &row.observed_model {
            require_id("observed_model", observed)?;
        }
        let wall_ms = require_u64("wall_ms", row.wall_ms)?;
        let started_at_ms = require_u64("started_at_ms", row.started_at_ms)?;
        let finished_at_ms = require_u64("finished_at_ms", row.finished_at_ms)?;
        if row.finished_at_ms < row.started_at_ms {
            return Err(StoreError::InvalidInput {
                field: "finished_at_ms".to_string(),
                message: format!(
                    "finished_at_ms {} is before started_at_ms {}",
                    row.finished_at_ms, row.started_at_ms
                ),
            });
        }
        require_json_string_array("session_ids_json", &row.session_ids_json)?;
        require_json("usage_json", &row.usage_json)?;
        require_json_string_array("missing_telemetry_json", &row.missing_telemetry_json)?;
        self.require_trial(&row.trial_id)?;

        let tx = self.transaction("record_execution")?;
        tx.execute(
            "INSERT INTO executions (
                 execution_id, trial_id, termination, exit_code, wall_ms, started_at_ms,
                 finished_at_ms, requested_model, observed_model, raw_sha256, events_sha256,
                 session_ids_json, usage_json, missing_telemetry_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                row.execution_id,
                row.trial_id,
                row.termination.as_str(),
                row.exit_code,
                wall_ms,
                started_at_ms,
                finished_at_ms,
                row.requested_model,
                row.observed_model,
                row.raw_sha256,
                row.events_sha256,
                row.session_ids_json,
                row.usage_json,
                row.missing_telemetry_json,
            ],
        )
        .map_err(|error| write_error("execution", &row.execution_id, error))?;
        refresh_trial_state(&tx, &row.trial_id)?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    /// Record one verification result for a trial.
    pub fn record_verification(&self, row: VerificationRow) -> Result<(), StoreError> {
        self.ensure_writable("record_verification")?;
        require_id("verification_id", &row.verification_id)?;
        require_id("trial_id", &row.trial_id)?;
        require_id("verifier_id", &row.verifier_id)?;
        require_digest("stdout_sha256", &row.stdout_sha256)?;
        require_digest("stderr_sha256", &row.stderr_sha256)?;
        require_digest_or_empty("pre_tree_sha256", &row.pre_tree_sha256)?;
        require_digest_or_empty("post_tree_sha256", &row.post_tree_sha256)?;
        require_json_exit_codes("exit_codes_json", &row.exit_codes_json)?;
        let wall_ms = require_u64("wall_ms", row.wall_ms)?;
        self.require_trial(&row.trial_id)?;

        let tx = self.transaction("record_verification")?;
        tx.execute(
            "INSERT INTO verifications (
                 verification_id, trial_id, verifier_id, verdict, mutation_detected,
                 exit_codes_json, stdout_sha256, stderr_sha256, pre_tree_sha256,
                 post_tree_sha256, wall_ms, timed_out
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                row.verification_id,
                row.trial_id,
                row.verifier_id,
                row.verdict.as_str(),
                i64::from(row.mutation_detected),
                row.exit_codes_json,
                row.stdout_sha256,
                row.stderr_sha256,
                row.pre_tree_sha256,
                row.post_tree_sha256,
                wall_ms,
                i64::from(row.timed_out),
            ],
        )
        .map_err(|error| write_error("verification", &row.verification_id, error))?;
        refresh_trial_state(&tx, &row.trial_id)?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    /// Record a sealed evidence manifest and register its artifacts.
    ///
    /// Re-verifies the manifest seal, validates every identity and digest,
    /// cross-checks the manifest against the trial row, then — in one
    /// transaction — inserts the `evidence` row, dedupes the `artifacts`
    /// registry, and writes the `evidence_artifacts` join rows. Artifact bytes
    /// are not touched here; [`EvalStore::verify_store_evidence`] re-verifies
    /// them against the [`ArtifactStore`].
    pub fn record_evidence(&self, manifest: &EvidenceManifest) -> Result<(), StoreError> {
        self.ensure_writable("record_evidence")?;
        verify_seal(manifest).map_err(|error| StoreError::Integrity {
            message: format!("evidence manifest seal does not verify: {error}"),
        })?;
        self.require_own_experiment(&manifest.experiment_id)?;
        require_id("trial_id", &manifest.trial_id)?;
        require_digest("plan_hash", &manifest.plan_hash)?;
        require_digest("task_content_sha256", &manifest.task_content_sha256)?;
        require_digest("manifest_sha256", &manifest.manifest_sha256)?;
        require_digest("execution.raw_sha256", &manifest.execution.raw_sha256)?;
        require_digest(
            "execution.normalized_events_sha256",
            &manifest.execution.normalized_events_sha256,
        )?;
        require_digest("repo_diff_sha256", &manifest.repo_diff_sha256)?;
        if let Some(verifier) = &manifest.verifier {
            require_id("verifier.verifier_id", &verifier.verifier_id)?;
            require_digest("verifier.stdout_sha256", &verifier.stdout_sha256)?;
            require_digest("verifier.stderr_sha256", &verifier.stderr_sha256)?;
        }
        for reference in &manifest.artifacts {
            require_id("artifacts.logical_name", &reference.logical_name)?;
            require_digest("artifacts.sha256", &reference.sha256)?;
            require_u64("artifacts.bytes", reference.bytes)?;
        }

        let trial = self.trial(&manifest.trial_id)?;
        require_same(
            "experiment_id",
            &trial.experiment_id,
            &manifest.experiment_id,
        )?;
        require_same("plan_hash", &trial.plan_hash, &manifest.plan_hash)?;
        require_same(
            "task_content_sha256",
            &trial.task_content_sha256,
            &manifest.task_content_sha256,
        )?;
        require_same(
            "manifest_sha256",
            &trial.manifest_sha256,
            &manifest.manifest_sha256,
        )?;

        let sealed_at_ms = require_u64("sealed_at_ms", manifest.sealed_at_ms)?;
        let manifest_json =
            serde_json::to_string(manifest).map_err(|error| StoreError::InvalidInput {
                field: "manifest_json".to_string(),
                message: format!("evidence manifest serialization failed: {error}"),
            })?;

        let tx = self.transaction("record_evidence")?;
        tx.execute(
            "INSERT INTO evidence (seal_sha256, trial_id, experiment_id, manifest_json, sealed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                manifest.seal_sha256,
                manifest.trial_id,
                manifest.experiment_id,
                manifest_json,
                sealed_at_ms,
            ],
        )
        .map_err(|error| write_error("evidence", &manifest.seal_sha256, error))?;
        for reference in &manifest.artifacts {
            register_artifact(&tx, reference)?;
            tx.execute(
                "INSERT INTO evidence_artifacts (seal_sha256, artifact_sha256, logical_name)
                 VALUES (?1, ?2, ?3)",
                params![
                    manifest.seal_sha256,
                    reference.sha256,
                    reference.logical_name
                ],
            )
            .map_err(|error| write_error("evidence_artifacts", &reference.logical_name, error))?;
        }
        refresh_trial_state(&tx, &manifest.trial_id)?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    /// Record the trial's outcome class (and invalid reason).
    ///
    /// An invalid class requires a non-empty reason and sets `state =
    /// invalid`; a non-invalid class must not carry a reason and recomputes
    /// the milestone state (`sealed` > `verified` > `executed` > `planned`)
    /// from the rows that exist, so a retried trial recovers from `invalid`.
    pub fn set_trial_outcome(
        &self,
        trial_id: &str,
        outcome_class: TrialOutcomeClass,
        invalid_reason: Option<String>,
    ) -> Result<(), StoreError> {
        self.ensure_writable("set_trial_outcome")?;
        require_id("trial_id", trial_id)?;
        validate_outcome_coherence(Some(outcome_class), invalid_reason.as_deref())?;
        self.require_trial(trial_id)?;

        let tx = self.transaction("set_trial_outcome")?;
        tx.execute(
            "UPDATE trials SET outcome_class = ?1, invalid_reason = ?2 WHERE trial_id = ?3",
            params![outcome_class.as_str(), invalid_reason, trial_id],
        )
        .map_err(|error| write_error("trial", trial_id, error))?;
        refresh_trial_state(&tx, trial_id)?;
        tx.commit().map_err(transaction_error)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------

    /// Read one experiment row.
    pub fn experiment(&self, experiment_id: &str) -> Result<ExperimentRow, StoreError> {
        require_id("experiment_id", experiment_id)?;
        self.conn
            .query_row(
                &format!("SELECT {EXPERIMENT_COLUMNS} FROM experiments WHERE experiment_id = ?1"),
                params![experiment_id],
                row_to_experiment,
            )
            .optional()
            .map_err(StoreError::from)?
            .ok_or_else(|| StoreError::NotFound {
                entity: "experiment".to_string(),
                id: experiment_id.to_string(),
            })
    }

    /// List one experiment's trials in balanced order (`order_index`, then
    /// `trial_id`). An unknown experiment yields an empty list.
    pub fn list_trials(&self, experiment_id: &str) -> Result<Vec<TrialRow>, StoreError> {
        require_id("experiment_id", experiment_id)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {TRIAL_COLUMNS} FROM trials WHERE experiment_id = ?1
                 ORDER BY order_index ASC, trial_id ASC"
            ))
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![experiment_id], row_to_trial)
            .map_err(StoreError::from)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    /// Read one trial row.
    pub fn trial(&self, trial_id: &str) -> Result<TrialRow, StoreError> {
        require_id("trial_id", trial_id)?;
        self.conn
            .query_row(
                &format!("SELECT {TRIAL_COLUMNS} FROM trials WHERE trial_id = ?1"),
                params![trial_id],
                row_to_trial,
            )
            .optional()
            .map_err(StoreError::from)?
            .ok_or_else(|| StoreError::NotFound {
                entity: "trial".to_string(),
                id: trial_id.to_string(),
            })
    }

    /// The full trial projection: trial + latest execution + latest
    /// verification + latest sealed evidence + outcome class.
    ///
    /// "Latest" is the most recently inserted row for that trial (rowid
    /// order), so retries and re-seals are unambiguous.
    pub fn trial_full(&self, trial_id: &str) -> Result<TrialFull, StoreError> {
        let trial = self.trial(trial_id)?;
        let latest_execution = self
            .conn
            .query_row(
                &format!(
                    "SELECT {EXECUTION_COLUMNS} FROM executions WHERE trial_id = ?1
                     ORDER BY rowid DESC LIMIT 1"
                ),
                params![trial_id],
                row_to_execution,
            )
            .optional()
            .map_err(StoreError::from)?;
        let latest_verification = self
            .conn
            .query_row(
                &format!(
                    "SELECT {VERIFICATION_COLUMNS} FROM verifications WHERE trial_id = ?1
                     ORDER BY rowid DESC LIMIT 1"
                ),
                params![trial_id],
                row_to_verification,
            )
            .optional()
            .map_err(StoreError::from)?;
        let evidence = self
            .conn
            .query_row(
                &format!(
                    "SELECT {EVIDENCE_COLUMNS} FROM evidence WHERE trial_id = ?1
                     ORDER BY rowid DESC LIMIT 1"
                ),
                params![trial_id],
                row_to_evidence,
            )
            .optional()
            .map_err(StoreError::from)?;
        Ok(TrialFull {
            outcome_class: trial.outcome_class,
            trial,
            latest_execution,
            latest_verification,
            evidence,
        })
    }

    /// Every execution attempt for a trial, in insertion order.
    pub fn executions_for_trial(&self, trial_id: &str) -> Result<Vec<ExecutionRow>, StoreError> {
        require_id("trial_id", trial_id)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {EXECUTION_COLUMNS} FROM executions WHERE trial_id = ?1 ORDER BY rowid ASC"
            ))
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![trial_id], row_to_execution)
            .map_err(StoreError::from)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    /// Every verification for a trial, in insertion order.
    pub fn verifications_for_trial(
        &self,
        trial_id: &str,
    ) -> Result<Vec<VerificationRow>, StoreError> {
        require_id("trial_id", trial_id)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {VERIFICATION_COLUMNS} FROM verifications WHERE trial_id = ?1
                 ORDER BY rowid ASC"
            ))
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![trial_id], row_to_verification)
            .map_err(StoreError::from)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    /// Every sealed evidence row for a trial, in insertion order.
    pub fn evidence_for_trial(&self, trial_id: &str) -> Result<Vec<EvidenceRecord>, StoreError> {
        require_id("trial_id", trial_id)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {EVIDENCE_COLUMNS} FROM evidence WHERE trial_id = ?1 ORDER BY rowid ASC"
            ))
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![trial_id], row_to_evidence)
            .map_err(StoreError::from)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StoreError::from)
    }

    /// Read one sealed evidence row by seal.
    pub fn evidence(&self, seal_sha256: &str) -> Result<EvidenceRecord, StoreError> {
        require_digest("seal_sha256", seal_sha256)?;
        self.conn
            .query_row(
                &format!("SELECT {EVIDENCE_COLUMNS} FROM evidence WHERE seal_sha256 = ?1"),
                params![seal_sha256],
                row_to_evidence,
            )
            .optional()
            .map_err(StoreError::from)?
            .ok_or_else(|| StoreError::NotFound {
                entity: "evidence".to_string(),
                id: seal_sha256.to_string(),
            })
    }

    /// Read one artifact registry row by content address.
    pub fn artifact(&self, sha256: &str) -> Result<ArtifactRow, StoreError> {
        require_digest("sha256", sha256)?;
        self.conn
            .query_row(
                &format!("SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE sha256 = ?1"),
                params![sha256],
                row_to_artifact,
            )
            .optional()
            .map_err(StoreError::from)?
            .ok_or_else(|| StoreError::NotFound {
                entity: "artifact".to_string(),
                id: sha256.to_string(),
            })
    }

    /// Trial-state counts and the outcome-class histogram for one experiment.
    ///
    /// An unknown experiment is [`StoreError::NotFound`].
    pub fn experiment_summary(&self, experiment_id: &str) -> Result<ExperimentSummary, StoreError> {
        self.experiment(experiment_id)?;

        let mut summary = ExperimentSummary {
            planned: 0,
            executed: 0,
            verified: 0,
            sealed: 0,
            invalid: 0,
            outcome_class_counts: Vec::new(),
        };
        let mut stmt = self
            .conn
            .prepare("SELECT state, COUNT(*) FROM trials WHERE experiment_id = ?1 GROUP BY state")
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![experiment_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(StoreError::from)?;
        for row in rows {
            let (token, count) = row.map_err(StoreError::from)?;
            let state = TrialState::from_token(&token).ok_or_else(|| StoreError::Query {
                message: format!("unknown trial state `{token}` in the store"),
            })?;
            match state {
                TrialState::Planned => summary.planned = count,
                TrialState::Executed => summary.executed = count,
                TrialState::Verified => summary.verified = count,
                TrialState::Sealed => summary.sealed = count,
                TrialState::Invalid => summary.invalid = count,
            }
        }

        let mut stmt = self
            .conn
            .prepare(
                "SELECT outcome_class, COUNT(*) FROM trials
                 WHERE experiment_id = ?1 AND outcome_class IS NOT NULL
                 GROUP BY outcome_class ORDER BY outcome_class ASC",
            )
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map(params![experiment_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(StoreError::from)?;
        for row in rows {
            let (token, count) = row.map_err(StoreError::from)?;
            if TrialOutcomeClass::from_token(&token).is_none() {
                return Err(StoreError::Query {
                    message: format!("unknown outcome class `{token}` in the store"),
                });
            }
            summary.outcome_class_counts.push((token, count));
        }
        Ok(summary)
    }

    // -----------------------------------------------------------------
    // Integrity
    // -----------------------------------------------------------------

    /// Re-verify every stored evidence manifest against the artifact store.
    ///
    /// For each `evidence` row: parse the manifest JSON, re-check its seal
    /// ([`verify_seal`]), re-check the stored row identity, and re-verify
    /// every referenced artifact's bytes ([`verify_artifacts`]). Failures are
    /// reported per trial in [`StoreIntegrityReport::failures`]; the result is
    /// `Err` only when the store itself cannot be queried.
    pub fn verify_store_evidence(
        &self,
        store: &ArtifactStore,
    ) -> Result<StoreIntegrityReport, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {EVIDENCE_COLUMNS} FROM evidence ORDER BY rowid ASC"
            ))
            .map_err(StoreError::from)?;
        let rows = stmt
            .query_map([], row_to_evidence)
            .map_err(StoreError::from)?;

        let mut report = StoreIntegrityReport {
            checked: 0,
            failures: Vec::new(),
        };
        for row in rows {
            let record = row.map_err(StoreError::from)?;
            report.checked += 1;

            let manifest = match record.manifest() {
                Ok(manifest) => manifest,
                Err(error) => {
                    report.failures.push(StoreIntegrityFailure {
                        trial_id: record.trial_id,
                        seal_sha256: record.seal_sha256,
                        check: StoreIntegrityCheck::ManifestJson,
                        message: error.to_string(),
                    });
                    continue;
                }
            };

            let mut push = |check: StoreIntegrityCheck, message: String| {
                report.failures.push(StoreIntegrityFailure {
                    trial_id: record.trial_id.clone(),
                    seal_sha256: record.seal_sha256.clone(),
                    check,
                    message,
                });
            };
            if manifest.seal_sha256 != record.seal_sha256 {
                push(
                    StoreIntegrityCheck::Identity,
                    format!(
                        "stored seal column is {} but the manifest declares {}",
                        record.seal_sha256, manifest.seal_sha256
                    ),
                );
            }
            if manifest.trial_id != record.trial_id {
                push(
                    StoreIntegrityCheck::Identity,
                    format!(
                        "stored trial_id is {} but the manifest names {}",
                        record.trial_id, manifest.trial_id
                    ),
                );
            }
            if manifest.experiment_id != record.experiment_id {
                push(
                    StoreIntegrityCheck::Identity,
                    format!(
                        "stored experiment_id is {} but the manifest names {}",
                        record.experiment_id, manifest.experiment_id
                    ),
                );
            }
            if let Err(error) = verify_seal(&manifest) {
                push(StoreIntegrityCheck::Seal, error.to_string());
            }
            if let Err(error) = verify_artifacts(&manifest, store) {
                push(StoreIntegrityCheck::Artifact, error.to_string());
            }
        }
        Ok(report)
    }

    // -----------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------

    fn ensure_writable(&self, operation: &str) -> Result<(), StoreError> {
        if self.read_only {
            return Err(StoreError::ReadOnly {
                operation: operation.to_string(),
            });
        }
        Ok(())
    }

    fn require_own_experiment(&self, experiment_id: &str) -> Result<(), StoreError> {
        require_id("experiment_id", experiment_id)?;
        if experiment_id != self.experiment_id {
            return Err(StoreError::InvalidInput {
                field: "experiment_id".to_string(),
                message: format!(
                    "row names experiment `{experiment_id}`, but this store belongs to `{}`",
                    self.experiment_id
                ),
            });
        }
        Ok(())
    }

    fn require_trial(&self, trial_id: &str) -> Result<TrialRow, StoreError> {
        self.trial(trial_id)
    }

    fn transaction(&self, operation: &str) -> Result<rusqlite::Transaction<'_>, StoreError> {
        self.conn
            .unchecked_transaction()
            .map_err(|error| StoreError::Transaction {
                message: format!("{operation}: {error}"),
            })
    }
}

// ---------------------------------------------------------------------------
// State helpers
// ---------------------------------------------------------------------------

/// Recompute `trials.state` from the rows that exist and write it back.
///
/// `invalid` wins whenever the recorded outcome class is an invalid class;
/// otherwise the highest milestone (`sealed` > `verified` > `executed` >
/// `planned`) present for the trial.
fn refresh_trial_state(tx: &rusqlite::Transaction<'_>, trial_id: &str) -> Result<(), StoreError> {
    let state = recompute_state(tx, trial_id)?;
    tx.execute(
        "UPDATE trials SET state = ?1 WHERE trial_id = ?2",
        params![state.as_str(), trial_id],
    )
    .map_err(|error| write_error("trial", trial_id, error))?;
    Ok(())
}

fn recompute_state(
    tx: &rusqlite::Transaction<'_>,
    trial_id: &str,
) -> Result<TrialState, StoreError> {
    let outcome_token: Option<String> = tx
        .query_row(
            "SELECT outcome_class FROM trials WHERE trial_id = ?1",
            params![trial_id],
            |row| row.get(0),
        )
        .map_err(StoreError::from)?;
    if outcome_token
        .as_deref()
        .and_then(TrialOutcomeClass::from_token)
        .is_some_and(|class| class.is_invalid())
    {
        return Ok(TrialState::Invalid);
    }
    if row_exists(tx, "evidence", trial_id)? {
        return Ok(TrialState::Sealed);
    }
    if row_exists(tx, "verifications", trial_id)? {
        return Ok(TrialState::Verified);
    }
    if row_exists(tx, "executions", trial_id)? {
        return Ok(TrialState::Executed);
    }
    Ok(TrialState::Planned)
}

fn row_exists(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    trial_id: &str,
) -> Result<bool, StoreError> {
    // `table` is one of three hard-coded literals; never caller input.
    let count: i64 = tx
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE trial_id = ?1"),
            params![trial_id],
            |row| row.get(0),
        )
        .map_err(StoreError::from)?;
    Ok(count > 0)
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn require_id(field: &str, value: &str) -> Result<(), StoreError> {
    if value.is_empty() {
        return Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: "must not be empty".to_string(),
        });
    }
    if value.len() > MAX_EVAL_STORE_ID_LEN {
        return Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: format!("must be at most {MAX_EVAL_STORE_ID_LEN} bytes"),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: "must not contain control characters".to_string(),
        });
    }
    Ok(())
}

/// `experiment_id` becomes a path component; refuse anything else.
fn require_experiment_component(experiment_id: &str) -> Result<(), StoreError> {
    let valid = !experiment_id.is_empty()
        && experiment_id != "."
        && experiment_id != ".."
        && experiment_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidInput {
            field: "experiment_id".to_string(),
            message: "must be a single path component of [A-Za-z0-9._-]".to_string(),
        })
    }
}

fn require_digest(field: &str, value: &str) -> Result<(), StoreError> {
    if is_canonical_sha256_digest(value) {
        Ok(())
    } else {
        Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: format!("expected sha256:<64 lowercase hex>, found `{value}`"),
        })
    }
}

fn require_digest_or_empty(field: &str, value: &str) -> Result<(), StoreError> {
    if value.is_empty() {
        Ok(())
    } else {
        require_digest(field, value)
    }
}

/// Validate a `u64` field and return it as the SQLite `i64` parameter.
fn require_u64(field: &str, value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::InvalidInput {
        field: field.to_string(),
        message: format!("{value} does not fit the signed 64-bit storage range"),
    })
}

fn require_same(field: &str, stored: &str, manifest: &str) -> Result<(), StoreError> {
    if stored == manifest {
        Ok(())
    } else {
        Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: format!(
                "evidence manifest declares `{manifest}` but the trial row records `{stored}`"
            ),
        })
    }
}

fn validate_outcome_coherence(
    outcome_class: Option<TrialOutcomeClass>,
    invalid_reason: Option<&str>,
) -> Result<(), StoreError> {
    match (outcome_class, invalid_reason) {
        (Some(class), Some(reason)) if class.is_invalid() => {
            if reason.trim().is_empty() {
                return Err(StoreError::InvalidInput {
                    field: "invalid_reason".to_string(),
                    message: "an invalid outcome class requires a non-empty reason".to_string(),
                });
            }
            if reason.len() > MAX_EVAL_STORE_ID_LEN {
                return Err(StoreError::InvalidInput {
                    field: "invalid_reason".to_string(),
                    message: format!("must be at most {MAX_EVAL_STORE_ID_LEN} bytes"),
                });
            }
            Ok(())
        }
        (Some(class), None) if class.is_invalid() => Err(StoreError::InvalidInput {
            field: "invalid_reason".to_string(),
            message: "an invalid outcome class requires a non-empty reason".to_string(),
        }),
        (_, Some(_)) => Err(StoreError::InvalidInput {
            field: "invalid_reason".to_string(),
            message: "only an invalid outcome class may carry an invalid reason".to_string(),
        }),
        (_, None) => Ok(()),
    }
}

fn require_json(field: &str, json: &str) -> Result<(), StoreError> {
    serde_json::from_str::<JsonValue>(json)
        .map(|_| ())
        .map_err(|error| StoreError::InvalidInput {
            field: field.to_string(),
            message: format!("must be valid JSON: {error}"),
        })
}

fn require_json_string_array(field: &str, json: &str) -> Result<(), StoreError> {
    let value: JsonValue =
        serde_json::from_str(json).map_err(|error| StoreError::InvalidInput {
            field: field.to_string(),
            message: format!("must be valid JSON: {error}"),
        })?;
    if json_string_array(&value).is_some() {
        Ok(())
    } else {
        Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: "must be a JSON array of strings".to_string(),
        })
    }
}

fn json_string_array(value: &JsonValue) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn require_json_exit_codes(field: &str, json: &str) -> Result<(), StoreError> {
    let value: JsonValue =
        serde_json::from_str(json).map_err(|error| StoreError::InvalidInput {
            field: field.to_string(),
            message: format!("must be valid JSON: {error}"),
        })?;
    let valid = value.as_array().is_some_and(|items| {
        items
            .iter()
            .all(|item| item.is_null() || item.as_i64().is_some())
    });
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidInput {
            field: field.to_string(),
            message: "must be a JSON array of integers or null".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// SQLite helpers
// ---------------------------------------------------------------------------

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )
}

fn read_schema_version(conn: &Connection) -> rusqlite::Result<u32> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |row| row.get(0),
    )?;
    if present == 0 {
        return Ok(0);
    }
    conn.query_row(
        "SELECT COALESCE(MAX(schema_version), 0) FROM meta",
        [],
        |row| row.get(0),
    )
}

/// `<db>.backup-v<from>`, e.g. `state.db.backup-v1`.
fn backup_path(path: &Path, from: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".backup-v{from}"));
    PathBuf::from(name)
}

/// True when a pre-migration backup must be taken before migrating a store at
/// `current` toward `target`.
///
/// Version 0 means "no schema yet" (fresh or empty database) and is never
/// backed up; a version equal to or newer than the target is not a migration.
fn needs_pre_migration_backup(current: u32, target: u32) -> bool {
    current > 0 && current < target
}

/// Create a consistent pre-migration snapshot with `VACUUM INTO`.
///
/// A symlinked backup path is refused (fail closed). An existing regular-file
/// backup is left untouched. A fresh path is populated atomically by SQLite.
fn create_pre_migration_backup(
    conn: &Connection,
    path: &Path,
    from: u32,
) -> Result<(), StoreError> {
    let backup = backup_path(path, from);
    match fs::symlink_metadata(&backup) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(StoreError::Migration {
                version: from,
                message: format!(
                    "refusing to migrate: the pre-migration backup path {} is a symlink",
                    backup.display()
                ),
            });
        }
        Ok(meta) if meta.is_file() => {
            return Ok(());
        }
        Ok(_) => {
            return Err(StoreError::Migration {
                version: from,
                message: format!(
                    "refusing to migrate: the pre-migration backup path {} is not a regular file",
                    backup.display()
                ),
            });
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(StoreError::Open {
                path: backup,
                message: error.to_string(),
            });
        }
    }

    let escaped = backup.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .map_err(|error| StoreError::Migration {
            version: from,
            message: format!(
                "pre-migration backup at {} failed: {error}",
                backup.display()
            ),
        })?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (schema_version INTEGER NOT NULL);")
        .map_err(|error| StoreError::Migration {
            version: EVAL_STORE_SCHEMA_VERSION,
            message: format!("failed to initialize the eval store meta table: {error}"),
        })?;

    let current = read_schema_version(conn).map_err(|error| StoreError::Migration {
        version: EVAL_STORE_SCHEMA_VERSION,
        message: error.to_string(),
    })?;
    if current > EVAL_STORE_MAX_SUPPORTED_VERSION {
        return Err(StoreError::UnsupportedSchema {
            found: current,
            supported: EVAL_STORE_MAX_SUPPORTED_VERSION,
            hint: "upgrade Root before opening this eval store".to_string(),
        });
    }
    if current < 1 {
        apply_migration(conn, 1, MIGRATION_V1)?;
    }
    Ok(())
}

/// Apply one schema migration and its version marker atomically. If any
/// statement fails, the transaction rolls back and no partial tables or
/// version rows remain, so the migration can be retried.
fn apply_migration(conn: &Connection, version: u32, sql: &str) -> Result<(), StoreError> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| StoreError::Transaction {
            message: format!("failed to start eval store migration to version {version}: {error}"),
        })?;
    tx.execute_batch(sql)
        .map_err(|error| StoreError::Migration {
            version,
            message: error.to_string(),
        })?;
    tx.execute("DELETE FROM meta", [])
        .map_err(|error| StoreError::Migration {
            version,
            message: format!("failed to rewrite the eval store version marker: {error}"),
        })?;
    tx.execute(
        "INSERT INTO meta (schema_version) VALUES (?1)",
        params![version],
    )
    .map_err(|error| StoreError::Migration {
        version,
        message: format!("failed to write the eval store version marker: {error}"),
    })?;
    tx.commit().map_err(|error| StoreError::Transaction {
        message: format!("failed to commit eval store migration to version {version}: {error}"),
    })?;
    Ok(())
}

fn transaction_error(error: rusqlite::Error) -> StoreError {
    StoreError::Transaction {
        message: error.to_string(),
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!("-{suffix}"));
    PathBuf::from(name)
}

fn wal_has_frames(path: &Path) -> bool {
    fs::metadata(sidecar_path(path, "wal"))
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

/// Build a `file:` URI that opens `path` immutable, so SQLite never creates
/// WAL shared-memory sidecars. Characters outside the URI-unreserved set are
/// percent-encoded.
fn sqlite_immutable_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        let character = byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '/' | '-' | '_' | '.' | '~') {
            uri.push(character);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    uri
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| StoreError::Open {
        path: path.to_path_buf(),
        message: format!("failed to set permissions to {mode:o}: {error}"),
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), StoreError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn conversion_error(index: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(ErrorKind::InvalidData, message)),
    )
}

fn row_to_experiment(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExperimentRow> {
    Ok(ExperimentRow {
        experiment_id: row.get(0)?,
        plan_hash: row.get(1)?,
        created_at_ms: row.get(2)?,
    })
}

fn row_to_trial(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrialRow> {
    let state_token: String = row.get(12)?;
    let outcome_token: Option<String> = row.get(13)?;
    Ok(TrialRow {
        trial_id: row.get(0)?,
        experiment_id: row.get(1)?,
        task_id: row.get(2)?,
        task_version: row.get(3)?,
        condition_id: row.get(4)?,
        repetition: row.get(5)?,
        order_index: row.get(6)?,
        plan_hash: row.get(7)?,
        manifest_sha256: row.get(8)?,
        task_content_sha256: row.get(9)?,
        repository_commit: row.get(10)?,
        repository_tree_sha256: row.get(11)?,
        state: TrialState::from_token(&state_token)
            .ok_or_else(|| conversion_error(12, format!("unknown trial state `{state_token}`")))?,
        outcome_class: match outcome_token {
            Some(token) => {
                Some(TrialOutcomeClass::from_token(&token).ok_or_else(|| {
                    conversion_error(13, format!("unknown outcome class `{token}`"))
                })?)
            }
            None => None,
        },
        invalid_reason: row.get(14)?,
    })
}

fn row_to_execution(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExecutionRow> {
    let termination_token: String = row.get(2)?;
    Ok(ExecutionRow {
        execution_id: row.get(0)?,
        trial_id: row.get(1)?,
        termination: TerminationKind::from_token(&termination_token).ok_or_else(|| {
            conversion_error(2, format!("unknown termination `{termination_token}`"))
        })?,
        exit_code: row.get(3)?,
        wall_ms: row.get(4)?,
        started_at_ms: row.get(5)?,
        finished_at_ms: row.get(6)?,
        requested_model: row.get(7)?,
        observed_model: row.get(8)?,
        raw_sha256: row.get(9)?,
        events_sha256: row.get(10)?,
        session_ids_json: row.get(11)?,
        usage_json: row.get(12)?,
        missing_telemetry_json: row.get(13)?,
    })
}

fn row_to_verification(row: &rusqlite::Row<'_>) -> rusqlite::Result<VerificationRow> {
    let verdict_token: String = row.get(3)?;
    Ok(VerificationRow {
        verification_id: row.get(0)?,
        trial_id: row.get(1)?,
        verifier_id: row.get(2)?,
        verdict: VerifierVerdict::from_token(&verdict_token).ok_or_else(|| {
            conversion_error(3, format!("unknown verifier verdict `{verdict_token}`"))
        })?,
        mutation_detected: row.get::<_, i64>(4)? != 0,
        exit_codes_json: row.get(5)?,
        stdout_sha256: row.get(6)?,
        stderr_sha256: row.get(7)?,
        pre_tree_sha256: row.get(8)?,
        post_tree_sha256: row.get(9)?,
        wall_ms: row.get(10)?,
        timed_out: row.get::<_, i64>(11)? != 0,
    })
}

fn row_to_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRecord> {
    Ok(EvidenceRecord {
        seal_sha256: row.get(0)?,
        trial_id: row.get(1)?,
        experiment_id: row.get(2)?,
        manifest_json: row.get(3)?,
        sealed_at_ms: row.get(4)?,
    })
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        sha256: row.get(0)?,
        logical_name: row.get(1)?,
        bytes: row.get(2)?,
    })
}

/// Register one manifest artifact in the content-addressed registry.
///
/// The registry dedupes by content address. A repeated address with the same
/// byte count is a no-op; the same address with a different byte count is a
/// contradiction and is refused (inside the caller's transaction, so nothing
/// partial survives).
fn register_artifact(
    tx: &rusqlite::Transaction<'_>,
    reference: &ArtifactRef,
) -> Result<(), StoreError> {
    let existing: Option<u64> = tx
        .query_row(
            "SELECT bytes FROM artifacts WHERE sha256 = ?1",
            params![reference.sha256],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)?;
    match existing {
        Some(bytes) if bytes == reference.bytes => Ok(()),
        Some(bytes) => Err(StoreError::InvalidInput {
            field: "artifacts.bytes".to_string(),
            message: format!(
                "content address {} is registered with {bytes} bytes, but this manifest declares {} bytes",
                reference.sha256, reference.bytes
            ),
        }),
        None => {
            tx.execute(
                "INSERT INTO artifacts (sha256, logical_name, bytes) VALUES (?1, ?2, ?3)",
                params![reference.sha256, reference.logical_name, reference.bytes],
            )
            .map_err(|error| write_error("artifact", &reference.sha256, error))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_migration_backup_guard_truth_table() {
        // Version 0 is "no schema yet" and is never backed up.
        assert!(!needs_pre_migration_backup(0, 1));
        assert!(!needs_pre_migration_backup(0, 3));
        // A current version is not a migration.
        assert!(!needs_pre_migration_backup(1, 1));
        // Newer versions are refused elsewhere, never backed up.
        assert!(!needs_pre_migration_backup(2, 1));
        // Older positive versions are the only case that backs up.
        assert!(needs_pre_migration_backup(1, 2));
        assert!(needs_pre_migration_backup(2, 3));
    }

    #[test]
    fn experiment_id_must_be_a_safe_single_component() {
        assert!(require_experiment_component("eval-001").is_ok());
        assert!(require_experiment_component("eval_001.v2").is_ok());
        assert_eq!(
            require_experiment_component("..").unwrap_err().kind(),
            StoreErrorKind::InvalidInput
        );
        assert_eq!(
            require_experiment_component("a/b").unwrap_err().kind(),
            StoreErrorKind::InvalidInput
        );
        assert_eq!(
            require_experiment_component("a b").unwrap_err().kind(),
            StoreErrorKind::InvalidInput
        );
        assert_eq!(
            require_experiment_component("").unwrap_err().kind(),
            StoreErrorKind::InvalidInput
        );
    }

    #[test]
    fn digest_validation_accepts_canonical_and_empty_where_allowed() {
        assert!(require_digest("d", &crate::sha256_digest(b"x")).is_ok());
        assert!(require_digest("d", "sha256:PENDING").is_err());
        assert!(require_digest("d", "").is_err());
        assert!(require_digest_or_empty("d", "").is_ok());
        assert!(require_digest_or_empty("d", "nope").is_err());
    }
}
