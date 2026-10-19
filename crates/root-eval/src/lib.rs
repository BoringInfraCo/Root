//! Strict parser for Root benchmark task records.
//!
//! **Milestone 1, Phase A scope.** This crate implements the immutable
//! benchmark task record from `Docs/Eval/benchmarks/TASK-SPEC.md` (schema
//! `task-record.v1`): strict TOML parsing, exact-key-set validation at every
//! nesting level, semantic rule enforcement, the draft `sha256:PENDING`
//! refusal, and the canonical content hash. It is library-only: there is no
//! binary and no CLI surface.
//!
//! Phase B extends the crate with the experiment-side primitives for
//! Milestone 1 (the audit's "immutable spec and effective config snapshot"):
//!
//! - `context` — a hermetic OpenCode `1.18.27` effective-context resolver
//!   (contract §4): instruction files, discovered skills, recorded discovery
//!   errors/warnings, and a canonical root-relative manifest hash;
//! - `condition` — declared condition overlays and the A/B audit check that
//!   two manifests differ *only* by the declared paths and digests;
//! - `plan` — the minimal [`ExperimentSpec`] and its canonical plan hash
//!   (task refs sorted by `(task_id, version)`, conditions by `id`; no
//!   environment, clock, or filesystem input).
//!
//! Phase B never reads the process environment, never spawns a process, and
//! never assumes `$HOME`: every root and every `OPENCODE_DISABLE_*` /
//! `OPENCODE_CONFIG_DIR` gate is an explicit input.
//!
//! Milestone 2, Track A adds isolated trial materialization (`trial`,
//! `materializer`, `isolation`): fresh per-trial directories, a detached
//! repository checkout, project skill files for the treatment condition,
//! hermetic harness config, offline fake tools, and an allowlisted
//! environment. `materialize_trial` and [`capture_diff`] are the crate's
//! documented process-spawning exceptions: bounded `git` child processes only
//! (checkout and verification for the former, read-only change capture for the
//! latter), and nothing else.
//!
//! Milestone 3, Phase A adds the evidence and verification layer:
//!
//! - `evidence` — the content-addressed artifact store (`<evals_root>/
//!   artifacts/sha256/<first2>/<hex>`), deterministic pre-persistence
//!   redaction, and the sealed [`EvidenceManifest`] whose seal covers every
//!   manifest field and whose artifacts are re-verifiable against the store;
//! - `diff` — bounded `git status`/`git diff` change capture
//!   ([`RepoDiff`]) for the scored trial checkout;
//! - `verifier` — [`run_verifier`], which executes predeclared verifier
//!   commands through the supervisor between two [`tree_fingerprint`]s so a
//!   verifier that mutates the scored tree can never pass unnoticed;
//! - `taxonomy` — [`classify_trial`], the audit's failure/missing-data policy
//!   as a machine-matchable [`TrialOutcomeClass`].
//!
//! Milestone 3, Phase B adds the local evaluation store (`store`): the
//! separate versioned SQLite database at `<evals_root>/<experiment_id>/
//! state.db` whose schema makes every execution/trial/config/task/revision/
//! verifier relationship queryable, with a typed [`StoreError`] taxonomy, a
//! read-only mode, and an integrity check that re-verifies every stored
//! evidence manifest against the content-addressed artifact store.
//!
//! The store takes an explicit evals root; nothing in this crate touches the
//! Root directory.
//!
//! Resolving a `(task_id, version)` against a benchmark family and sealing
//! drafts remain family/plan concerns; this crate provides the record-level
//! primitives they consume.
//!
//! # Parsing model
//!
//! 1. Parse TOML into a value tree.
//! 2. Peek and gate `schema_version` (newer versions are refused, never
//!    partially read).
//! 3. Reject floats anywhere, then validate the exact key set for every table
//!    and array element. Unknown fields are errors, not warnings.
//! 4. Refuse `sha256:PENDING` in digest fields as [`TaskError::SentinelRefusal`],
//!    distinct from a malformed digest.
//! 5. Deserialize into `#[serde(deny_unknown_fields)]` shapes as a second
//!    layer, then apply the semantic rules (variants, paths, naming, budgets,
//!    acceptance dispatch/references, fixture self-checks).
//! 6. Compute the canonical hash over the parsed document minus `[integrity]`
//!    and require it to equal `integrity.content_sha256`.
//!
//! Parsing is entirely in memory and never touches the Root directory or the
//! filesystem.
//!
//! # Drafts
//!
//! [`TaskDraft`] exposes the structural layer used by authoring tooling: it
//! validates schema version, key sets, types, enums, and required fields, but
//! permits a missing `[integrity]` table and reports pending digest fields
//! instead of refusing them. [`TaskRecord::parse_str`] is the evaluation entry
//! point and refuses any draft.
//!
//! # Deviations from TASK-SPEC v1
//!
//! Rules that could not be taken literally or that strictly belong to a later
//! milestone, implemented as the closest faithful semantics:
//!
//! 1. **`u64` bounds.** `fixture.max_tree_bytes` and `budgets.tokens_max` are
//!    `u64` in the spec, but TOML integers are `i64`; values above `i64::MAX`
//!    are unrepresentable and surface as TOML syntax errors.
//! 2. **Regex subset.** Spec §4.3.7 leaves the engine unpinned, so
//!    `regex`/`pattern` fields are validated for type, non-emptiness, and
//!    control characters only. Engine-level syntax validation belongs to the
//!    Phase B runner.
//! 3. **Prompt lint.** §2.3.1 rules 1–4 are review-enforced prose; only the
//!    §2.3.1 rule 5 size limit (4096 bytes after trailing-newline
//!    normalization) is mechanical, per open question §4.3.3.
//! 4. **Fixture fetch heuristic.** §2.4.2 rule 2 ("plausible fetch of a live
//!    external resource ... that the *trial* would need") is a
//!    runtime-interpretation rule; it is not a parser refusal, because the
//!    spec's own Example 3 ships `curl ... | sh` text as adversarial content.
//! 5. **Sentinel scan sites.** `sha256:PENDING` is refused in every digest
//!    field; the literal may still appear inside prompt/fixture text, where it
//!    is content rather than a digest placeholder.
//! 6. **Canonical emitter.** Key sorting, comment removal, and the digest
//!    domain follow §2.10 exactly; exact escaping and array indentation are
//!    delegated to `toml` 0.8's pretty emitter and pinned by the required
//!    parse/re-serialize fixed-point check rather than a hand-rolled writer.
//! 7. **Line endings.** Carriage returns are refused in prompt and fixture
//!    content, derived from the deterministic LF-endings requirement (§2.4.2).
//! 8. **Conservative paths.** Besides absolute paths and `..`, backslashes and
//!    `:` are refused to rule out Windows-absolute and drive-relative forms.
//! 9. **Evidence explicitness.** §2.7.7 requires `evidence` on every
//!    acceptance entry; the per-kind "default" annotations are not
//!    materialized.
//! 10. **Token registry.** `environment.allowed_tools` is grammar-checked
//!     (`[a-z][a-z0-9_-]*`); validation against the pinned contract profile's
//!     registry waits on the Milestone-0 freeze (open question §4.3.1).
//!
//! Runner- and plan-level rules — verifier reliability probes (§2.8.1),
//! family-level `(task_id, version)` uniqueness (§2.10.2), budget dominance
//! versus `ExperimentSpec` (§2.6), termination totality (§2.9), and the
//! fake-tool side-channel contract (§2.5.2) — are Phase B/M2 concerns and are
//! intentionally not implemented here.

mod canonical;
mod canonical_json;
mod condition;
mod context;
mod diff;
mod digest;
mod error;
mod eval_error;
mod events;
mod evidence;
mod isolation;
mod materializer;
mod parse;
mod plan;
mod raw;
mod runner;
mod schema;
mod store;
mod supervisor;
mod taxonomy;
mod trial;
mod validate;
mod verifier;

pub use condition::{
    apply_overlay, verify_declared_delta, ConditionOverlay, DeltaError, DeltaErrorKind,
    DeltaExpectation, OverlayAction, OverlayEntry,
};
pub use context::{
    resolve_manifest, ContextError, ContextErrorKind, ContextRoots, DiscoveryError,
    DiscoveryErrorKind, DiscoveryFlags, DiscoveryWarning, DiscoveryWarningKind,
    EffectiveContextManifest, InstructionRole, InstructionSource, ResolvedInstruction,
    ResolvedSkill, SkillSource, BUILTIN_SKILL_NAMES, CONTEXT_HASH_DOMAIN, MAX_CONTEXT_FILE_BYTES,
    MAX_SCAN_DEPTH,
};
pub use diff::{
    capture_diff, DiffCaps, DiffError, DiffErrorKind, RepoDiff, DEFAULT_DIFF_TIMEOUT_MS,
    DIFF_HASH_DOMAIN, MAX_DIFF_BYTES,
};
pub use digest::{
    is_canonical_sha256_digest, is_pending_sentinel, sha256_digest, DIGEST_PREFIX, PENDING_SENTINEL,
};
pub use error::{ErrorKind, TaskError};
pub use eval_error::{EvalError, EvalErrorKind};
pub use events::{
    parse_ndjson_line, EventParseError, EventParseErrorKind, NormalizedEvent, StepTokens,
    StreamError, ToolCall, ASK_REJECT_MESSAGE, MODEL_OBSERVED_LIMITATION, SOURCE_ERROR,
    SOURCE_STEP_FINISH, SOURCE_STEP_START, SOURCE_TEXT, SOURCE_TOOL_USE,
};
pub use evidence::{
    normalized_events_digest, redact_bytes, seal_manifest, seal_projection, verify_artifacts,
    verify_seal, ArtifactRef, ArtifactStore, EvidenceError, EvidenceErrorKind, EvidenceManifest,
    ExecutionDigest, RedactionEvent, SealError, SealErrorKind, SealInputs, VerifierDigest,
    ARTIFACT_SHARD_DIR, ARTIFACT_STORE_DIR, EVIDENCE_MAX_SUPPORTED_VERSION,
    EVIDENCE_SCHEMA_VERSION, JSON_SECRET_KEYS, MAX_ARTIFACT_BYTES, MAX_ARTIFACT_LOGICAL_NAME_LEN,
    NORMALIZED_EVENTS_HASH_DOMAIN, REDACTION_JSON_SECRET_KEY, REDACTION_MARKER_PREFIX,
    REDACTION_SECRET_KEY_ASSIGNMENT, SEAL_HASH_DOMAIN,
};
pub use isolation::{
    check_no_leak, cleanup_trial, residue_scan, verify_isolation, IsolationError,
    IsolationErrorKind, Residue,
};
pub use materializer::{
    allowed_env, materialize_trial, scrub_env, secret_pattern_hit, MaterializerError,
    MaterializerErrorKind,
};
pub use plan::{
    compute_plan_hash, verify_plan, BenchmarkTaskRef, ConditionSpec, ExperimentSpec, HarnessLock,
    RepositoryLock, EXPERIMENT_HASH_DOMAIN, EXPERIMENT_SPEC_MAX_SUPPORTED_VERSION,
    EXPERIMENT_SPEC_SCHEMA_VERSION, PINNED_HARNESS_NAME,
};
pub use runner::{run_trial, TrialRunRecord, TrialRunRequest, UsageTotals};
pub use schema::*;
pub use store::{
    ArtifactRow, EvalStore, EvidenceRecord, ExecutionRow, ExperimentRow, ExperimentSummary,
    StoreError, StoreErrorKind, StoreIntegrityCheck, StoreIntegrityFailure, StoreIntegrityReport,
    TrialFull, TrialRow, TrialState, VerificationRow, EVAL_STORE_FILE_NAME,
    EVAL_STORE_MAX_SUPPORTED_VERSION, EVAL_STORE_SCHEMA_VERSION, MAX_EVAL_STORE_ID_LEN,
};
pub use supervisor::{
    cancel_flag, run_supervised, CancelFlag, DroppedLine, ResidueReport, SupervisedResult,
    SupervisorConfig, TerminationKind, DEFAULT_KILL_GRACE_MS,
};
pub use taxonomy::{classify_trial, TrialOutcomeClass};
pub use trial::{
    validate_trial_id, TrialError, TrialErrorKind, TrialLayout, TrialSpec, MAX_TRIAL_ID_LEN,
};
pub use validate::{is_contained_relative_path, is_relative_glob, is_task_identifier};
pub use verifier::{
    run_verifier, tree_fingerprint, VerifierError, VerifierErrorKind, VerifierOutcome,
    VerifierSpec, VerifierStep, VerifierVerdict, DEFAULT_VERIFIER_OUTPUT_CAP_BYTES,
    DEFAULT_VERIFIER_TIMEOUT_MS, MAX_FINGERPRINT_DEPTH, MAX_FINGERPRINT_FILE_BYTES,
    TREE_FINGERPRINT_DOMAIN,
};

/// Task-format schema version emitted and accepted by this crate (§2.2).
pub const TASK_SPEC_SCHEMA_VERSION: u32 = 1;

/// Highest task-format schema version this crate can read.
///
/// Mirrors `ROOT_LOCK_MAX_SUPPORTED_VERSION`: a record declaring a newer
/// version is refused, never read with a lossy shape.
pub const TASK_SPEC_MAX_SUPPORTED_VERSION: u32 = 1;

/// Lowest supported task-format schema version. v1 parses only `1`.
pub const TASK_SPEC_MIN_SUPPORTED_VERSION: u32 = 1;

/// Constant `integrity.hash_domain` value (§2.10).
pub const TASK_HASH_DOMAIN: &str = "task-record.v1";

impl TaskRecord {
    /// Strictly parse and validate a task record for evaluation use.
    ///
    /// Returns [`TaskError::SentinelRefusal`] for draft records carrying
    /// `sha256:PENDING`, and [`TaskError::SchemaVersion`] for versions outside
    /// the supported range.
    pub fn parse_str(content: &str) -> Result<Self, TaskError> {
        let (raw, document) = parse::parse_evaluated(content)?;
        validate::build_record(raw, &document)
    }

    /// Strictly parse and validate UTF-8 task-record bytes.
    ///
    /// Invalid UTF-8 is reported as [`TaskError::NonUtf8`] rather than being
    /// lossily decoded.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self, TaskError> {
        let content = std::str::from_utf8(bytes).map_err(|_| TaskError::NonUtf8)?;
        Self::parse_str(content)
    }

    /// Canonical `sha256:<64 lowercase hex>` digest of the parsed record.
    ///
    /// Computed over the canonical projection that excludes `[integrity]`;
    /// equal by construction to [`TaskRecord::integrity`]'s
    /// `content_sha256`.
    pub fn canonical_hash(&self) -> &str {
        self.canonical.sha256()
    }

    /// Canonical bytes whose SHA-256 is [`TaskRecord::canonical_hash`].
    ///
    /// Those bytes omit the `[integrity]` table, per the canonical-hash rule
    /// (§2.10).
    pub fn canonical_bytes(&self) -> &[u8] {
        self.canonical.bytes()
    }

    /// The stable `(task_id, version)` identity pair.
    pub fn identity(&self) -> TaskIdentity {
        TaskIdentity {
            task_id: self.task_id.clone(),
            version: self.version,
        }
    }

    /// The sealed content digest, identical to the canonical hash.
    pub fn content_sha256(&self) -> &str {
        &self.integrity.content_sha256
    }
}

/// Structurally valid task document that may still be an unsealed draft.
///
/// A draft has passed the TOML/schema/key-set/type/enum layer, but digest
/// sentinels may be present and `[integrity]` may be absent. This is the
/// authoring and sealing entry point; evaluation code must use
/// [`TaskRecord::parse_str`].
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDraft {
    document: toml::Value,
    schema_version: u32,
    pending_digest_fields: Vec<String>,
    canonical: canonical::CanonicalRecord,
}

impl TaskDraft {
    /// Parse a draft at the structural layer.
    pub fn parse_str(content: &str) -> Result<Self, TaskError> {
        let (document, schema_version, pending_digest_fields) = parse::parse_draft(content)?;
        let canonical = canonical::canonical_record(&document)?;
        Ok(Self {
            document,
            schema_version,
            pending_digest_fields,
            canonical,
        })
    }

    /// Parse draft bytes as UTF-8.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self, TaskError> {
        let content = std::str::from_utf8(bytes).map_err(|_| TaskError::NonUtf8)?;
        Self::parse_str(content)
    }

    /// The parsed TOML document (comments and original formatting are gone).
    pub fn document(&self) -> &toml::Value {
        &self.document
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Digest fields still holding `sha256:PENDING`, in schema-field order.
    pub fn pending_digest_fields(&self) -> &[String] {
        &self.pending_digest_fields
    }

    /// True when no digest sentinel remains.
    pub fn is_sealed(&self) -> bool {
        self.pending_digest_fields.is_empty()
    }

    /// Canonical digest over the draft projection (integrity excluded).
    ///
    /// The digest excludes `[integrity]`, so a draft that only lacks a correct
    /// content hash already has its final canonical hash. Sealing replaces
    /// sentinels and writes this value into `integrity.content_sha256`.
    pub fn canonical_hash(&self) -> &str {
        self.canonical.sha256()
    }

    /// Canonical bytes whose SHA-256 is [`TaskDraft::canonical_hash`].
    pub fn canonical_bytes(&self) -> &[u8] {
        self.canonical.bytes()
    }
}
