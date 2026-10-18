//! Umbrella error type for the Phase B evaluation surface.
//!
//! Phase A has its own [`crate::TaskError`] taxonomy for task-record parsing.
//! Phase B adds three narrower taxonomies — [`ContextError`] (effective-context
//! resolution), [`DeltaError`] (condition-overlay enforcement), and the
//! plan-hash errors collected here — and this enum lets callers that compose
//! them (resolve → hash → plan) bubble a single error type.
//!
//! The variant set is deliberately small: plan-input validation, canonical
//! serialization failures, and transparent wrappers for the two sub-module
//! errors.

use thiserror::Error;

use crate::condition::DeltaError;
use crate::context::ContextError;

/// Machine-matchable classification for every [`EvalError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvalErrorKind {
    /// `ExperimentSpec::schema_version` is outside the supported range.
    SchemaVersion,
    /// A required string field was empty.
    EmptyField,
    /// The harness lock names a harness other than the pinned `opencode`.
    UnsupportedHarness,
    /// A digest field is not `sha256:<64 lowercase hex>`.
    DigestInvalid,
    /// Two benchmark task refs share `(task_id, version)`.
    DuplicateTaskRef,
    /// Two conditions share an `id`.
    DuplicateConditionId,
    /// A recomputed plan hash differs from the expected value.
    PlanHashMismatch,
    /// Canonical serialization of a hash projection failed.
    Canonicalization,
    /// Effective-context resolution failed; the source is a [`ContextError`].
    Context,
    /// Condition-delta verification failed; the source is a [`DeltaError`].
    Delta,
}

/// Errors produced while validating, hashing, or verifying an experiment plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvalError {
    #[error(
        "experiment schema version {found} is not supported (this crate writes version {supported})"
    )]
    SchemaVersion { found: u32, supported: u32 },

    #[error("`{field}` must not be empty")]
    EmptyField { field: String },

    #[error("harness `{name}` is not supported (this milestone pins `opencode`)")]
    UnsupportedHarness { name: String },

    #[error("invalid digest in `{field}`: expected sha256:<64 lowercase hex>, found `{value}`")]
    DigestInvalid { field: String, value: String },

    #[error("duplicate benchmark task reference `{task_id}` version {version}")]
    DuplicateTaskRef { task_id: String, version: u64 },

    #[error("duplicate condition id `{id}`")]
    DuplicateConditionId { id: String },

    #[error("plan hash mismatch: expected {expected}, found {found}")]
    PlanHashMismatch { expected: String, found: String },

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },

    #[error(transparent)]
    Context(#[from] ContextError),

    #[error(transparent)]
    Delta(#[from] DeltaError),
}

impl EvalError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> EvalErrorKind {
        match self {
            Self::SchemaVersion { .. } => EvalErrorKind::SchemaVersion,
            Self::EmptyField { .. } => EvalErrorKind::EmptyField,
            Self::UnsupportedHarness { .. } => EvalErrorKind::UnsupportedHarness,
            Self::DigestInvalid { .. } => EvalErrorKind::DigestInvalid,
            Self::DuplicateTaskRef { .. } => EvalErrorKind::DuplicateTaskRef,
            Self::DuplicateConditionId { .. } => EvalErrorKind::DuplicateConditionId,
            Self::PlanHashMismatch { .. } => EvalErrorKind::PlanHashMismatch,
            Self::Canonicalization { .. } => EvalErrorKind::Canonicalization,
            Self::Context(_) => EvalErrorKind::Context,
            Self::Delta(_) => EvalErrorKind::Delta,
        }
    }

    /// The offending field path, when the error is field-scoped.
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::SchemaVersion { .. } => Some("schema_version"),
            Self::EmptyField { field } | Self::DigestInvalid { field, .. } => Some(field),
            Self::UnsupportedHarness { .. }
            | Self::DuplicateTaskRef { .. }
            | Self::DuplicateConditionId { .. }
            | Self::PlanHashMismatch { .. }
            | Self::Canonicalization { .. }
            | Self::Context(_)
            | Self::Delta(_) => None,
        }
    }
}
