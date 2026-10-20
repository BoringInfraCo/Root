//! Milestone 5/6 feasibility-matrix driver (deterministic fake agents and the
//! real OpenCode harness).
//!
//! [`run_feasibility_matrix`] executes the audit's ten-task feasibility matrix
//! end to end with the sealed fake-agent fixtures under
//! `tests/fixtures/m5/`:
//!
//! ```text
//! 10 tasks × 2 conditions (baseline, treatment) × R repetitions
//! ```
//!
//! through the full pipeline — materializer → supervisor → verifier →
//! acceptance evaluator → evidence seal → eval store → feasibility report —
//! and then writes the run evidence (`report.txt`, `report.json`,
//! `summary.json`) under the evals root. [`reproduce_feasibility`] proves the
//! report can be rebuilt from the sealed store **without rerunning agents**:
//! it reopens the store read-only, re-verifies every stored manifest against
//! the artifact store, rebuilds the report, and compares it byte-for-byte with
//! the stored `report.json`.
//!
//! # The two harnesses
//!
//! [`HarnessChoice`] selects what produces the execution:
//!
//! - [`HarnessChoice::FakeAgent`] (the default) runs the deterministic
//!   per-task fake-agent scripts. Nothing about the pipeline changes.
//! - [`HarnessChoice::OpenCodeReal`] runs the **real**, digest-pinned OpenCode
//!   `1.18.27` CLI inside a hermetic, prewarmed trial environment (see
//!   [`crate::real_harness`] and [`crate::trial_env`]). The trial is seeded
//!   with the prewarmed assets, the CLI runs with the task record's prompt as
//!   its message argument, and the run is classified by the existing taxonomy
//!   — a provider failure becomes [`crate::TerminationKind::ProviderError`]
//!   (never an agent failure) and is retried **once** with a fresh
//!   materialized trial.
//!
//! Both paths produce the same execution record, the same raw artifacts, and
//! the same evidence seal, so everything downstream is identical.
//!
//! # The permission posture of a trial
//!
//! The permission set seeded into the trial's project `opencode.json` is
//! **harness-aware** ([`crate::materializer::PermissionSeeding`]):
//!
//! - the **fake**-agent path seeds `ask`-mode
//!   (`{"permission":{"*":{"*":"ask"}}}`) — an ask is auto-rejected in a
//!   non-interactive run, which is the deterministic denial shape the M0
//!   golden captures recorded;
//! - the **real** OpenCode path seeds an explicit **allow-all** set
//!   (`{"permission":{"*":{"*":"allow"}}}`) — with an ask-mode seed every
//!   tool call of every trial would be asked and then auto-rejected (M0
//!   UPSTREAM-CONTRACT §5.1), and the agent could execute nothing at all.
//!
//! The effective set is recorded twice per trial: in the sealed
//! `permission-config` artifact and in `summary.json`
//! (`permission_seeding` + `effective_permission`), so a run's permission
//! posture is auditable from the evidence alone.
//!
//! # What the workspace observation covers
//!
//! The project's OpenCode config dir (`<trial>/repo/.opencode`) is the
//! harness's own directory inside the agent's workspace: the driver-seeded
//! permission config and project skill, the seeded plugin install target,
//! and the files OpenCode and its npm client write into every config dir.
//! It is excluded from every workspace observation (the tree snapshot, the
//! tree fingerprint, and the verifier's mutation check) and hidden from the
//! git-based change capture, so the harness's own activity inside it is
//! never read as agent activity. See [`excluded_workspace_paths`].
//!
//! # Inputs and plan hash
//!
//! The driver loads every `*.task.toml` under `tasks_dir`, strictly parses it
//! with [`TaskRecord::parse_str`], and sorts the records by `(task_id,
//! version)`. The experiment plan ([`ExperimentSpec`]) pins:
//!
//! - **tasks** — each record's sealed `content_sha256`;
//! - **conditions** — `baseline` (the empty effective-context manifest,
//!   `overlay_present = false`) and `treatment` (the same context plus one
//!   project skill file, `overlay_present = true`); the treatment skill bytes
//!   come from [`FeasibilityConfig::treatment_skill`] when set (the example
//!   CLI points it at the repository's `skills/claude/SKILL.md`) and from the
//!   documented [`M5_MARKER_SKILL`] stand-in otherwise;
//! - **harness** — the harness lock of the run: the feasibility-only
//!   [`FEASIBILITY_HARNESS_NAME`] with version [`M5_FAKE_HARNESS_VERSION`] and
//!   `binary_sha256` = the `m5-fake-agent.v1` combined digest over the
//!   per-task fake-agent script bytes (sorted by task id) for the fake
//!   harness, and [`crate::PINNED_HARNESS_NAME`] (`opencode`) with the
//!   verified `1.18.27` version and the **digest of the binary that actually
//!   ran** for the real harness;
//! - **repository** — the generated-fixture marker
//!   `generated:sha256:<combined tree digest>` where the combined digest is
//!   the `m5-fixture-tree.v1` digest over each record's
//!   [`crate::tree_digest_v1`] fixture digest (sorted by task id);
//! - **created_at** — the fixed [`M5_FIXED_CREATED_AT`] constant, so the plan
//!   hash is deterministic across reruns.
//!
//! # Trial ids and order
//!
//! [`generate_schedule`] produces the balanced, seeded order
//! ([`M5_DEFAULT_SEED`]); each planned cell becomes
//! `t{order_index:03}-{task_id}-{condition}-r{repetition}`. Trials run
//! sequentially in `order_index` order.
//!
//! # Per-trial pipeline
//!
//! 1. **Fixture source.** The record's declared `generated` fixture files are
//!    written into a scratch Git repository, committed with fixed author and
//!    date identity, and the commit + tree ids are captured (the same
//!    deterministic bytes as `tests/m5_tasks_*.rs`, but through a real Git
//!    source so the materializer takes its checkout path).
//! 2. **Materialization.** [`materialize_trial_with_fake_tools`] copies the
//!    fixture into a fresh trial tree with the record's fake-tool bindings;
//!    the prompt is written to `<trial>/prompt.txt`; a trial-local shell is
//!    staged for the verifier commands (see below); a pre-agent tree snapshot
//!    is captured.
//! 3. **Execution.** The harness runs under [`run_supervised`] with the
//!    trial's explicit environment, cwd = the trial repository, wall timeout
//!    from the record's `budgets.wall_time_seconds_max`, output cap
//!    [`M5_OUTPUT_CAP_BYTES`]. The fake harness runs the task's agent script
//!    (plus `TRIAL_FAKE_AGENT_PROMPT` / `TRIAL_FAKE_AGENT_CONDITION`); the
//!    real harness runs `opencode run --format json --model <model> <prompt>`
//!    with the prewarmed trial environment.
//! 4. **Verification.** A post-agent snapshot and diff are captured, the
//!    fake-tool audit log is parsed, every `[verifier.commands]` entry runs
//!    through [`run_verifier`] with the **hermetic** verifier environment
//!    (`PATH` = fake-bin + stub-bin, **no host `PATH`**; a record that names
//!    `sh` gets the trial-local staged shell), and [`evaluate_acceptance`]
//!    folds all evidence into an [`AssertionReport`].
//! 5. **Outcome mapping.** See below.
//! 6. **Evidence.** Raw stdout, raw stderr, the audit log, the diff text, the
//!    canonical assertion-report JSON, the harness identity, and every
//!    verifier's stdout/stderr are redacted ([`crate::redact_bytes`],
//!    pre-persistence per M3) and stored in
//!    the content-addressed artifact store; [`seal_manifest`] binds them with
//!    the execution digest and the captured [`RepoDiff::diff_sha256`].
//! 7. **Store rows.** One `trials` row, one `executions` row (the requested
//!    model verbatim, the observed model always unavailable per the M0
//!    identity policy), one verification row for the built-in acceptance
//!    engine (`verifier_id = "builtin:acceptance"`, verdict =
//!    `all_primary_passed`, `exit_codes_json = []`, `stdout_sha256` =
//!    SHA-256 of the canonical assertion-report JSON), one verification row
//!    per external verifier outcome, the sealed evidence manifest, and the
//!    outcome class.
//!
//! # Where the harness identity is recorded
//!
//! The eval-store schema is **unchanged** (v1), so an existing store stays
//! readable. The harness identity — name, exact version, and the SHA-256 of
//! the binary that produced the run — is recorded in three places:
//!
//! - the plan's harness lock (hashed into the plan hash, which every trial
//!   row and the experiment row carry);
//! - the sealed `harness-identity` artifact of every execution (canonical
//!   JSON: name, version, binary digest, requested model, the M0 observed-
//!   model limitation, the attempt count, the retry/provider markers, and
//!   how the turn ended);
//! - `summary.json` (the run-level `harness_name` / `harness_version` /
//!   `harness_binary_sha256`, plus the per-trial `requested_model`).
//!
//! # The real harness: retry policy
//!
//! The real harness retries **once** ([`REAL_HARNESS_MAX_RETRIES`]) and only
//! for a detected provider failure (the `1.18.27` NDJSON stream carries no
//! `error` envelope for one — the probe found the error only in the trial's
//! OpenCode log) or a wall-timeout kill (whose stream is truncated
//! mid-turn). The retry uses a **fresh materialized trial**: the failed
//! attempt's tree is removed in full before the next one is materialized, so
//! the retry never observes its residue, and the superseded attempt's
//! evidence is discarded (the trial records the attempt count, the retry
//! reason, and the provider marker instead). Nothing else is ever retried,
//! and the decision is never silent — it is recorded in the trial summary
//! (`attempts`, `retry_reason`, `provider_error`).
//!
//! # Outcome mapping (documented fold)
//!
//! The execution layer is classified by [`classify_trial`] from the
//! supervisor termination plus the verifier overlay (the first external
//! verifier outcome that is invalid/mutated/timed-out, else the first
//! outcome, else `None`). The **stored** `outcome_class` then folds the
//! acceptance layer:
//!
//! - termination other than `Completed`, or an invalid taxonomy class
//!   (spawn failure, verifier mutation/timeout, invalid infrastructure,
//!   output cap, denial, timeout, cancellation, provider error) → the
//!   taxonomy class is stored unchanged;
//! - `Completed` with a valid taxonomy class → `completed_passed` when the
//!   acceptance report's `all_primary_passed` is true, else
//!   `completed_failed`.
//!
//! # Contamination checks
//!
//! Before each trial the driver sets `M5_CANARY_SECRET` on **its own**
//! process to `canary-do-not-leak-<trial_id>` and removes it afterwards. The
//! canary must appear in no trial file (every regular file under the trial
//! root, including `trial.env`), in no raw agent stream, and in no stored
//! artifact; hits are recorded per trial and aggregated. After the run the
//! driver also asserts that every trial root is distinct, pairwise disjoint
//! (no root is a prefix of another), contained under the run's trials
//! directory, and outside the experiment's store directory, and that the
//! trials/sources scratch directories hold no residue (trial dirs, stale
//! locks, or leftover process markers).
//!
//! ## Host leakage (fail closed)
//!
//! The trial's isolation is **process- and environment-level**, not
//! OS-level: an agent can name a host path directly. The M6 smoke run's
//! baseline agent found the host's `/opt/homebrew/bin/rg` and symlinked it
//! into the trial's stub-bin — a trial that reaches the host can satisfy a
//! task **without** the skill, so the two conditions are no longer
//! comparable.
//!
//! After every trial the driver scans the trial tree
//! ([`crate::leakage`]): a symlink whose target resolves outside the trial
//! root, or a file in the trial's fake-bin / stub-bin directory that is
//! larger than [`crate::MAX_STUB_FILE_BYTES`], is a **host leak**. A leaked
//! trial is classified [`TrialOutcomeClass::HostContamination`] — an
//! **invalid** class: never a pass, never a fail — the finding is recorded in
//! the trial's sealed `contamination-report` artifact and in `summary.json`,
//! the block is excluded from the report's pairing (symmetrically, with the
//! reason in the report's `invalid_blocks`), the report's `limitations` name
//! the number of contaminated trials, and the trial tree is **preserved** for
//! inspection instead of being cleaned up. The harness's own directories
//! (the project's OpenCode config dir and the global config clone) are
//! excluded from the scan — see [`crate::leakage`].
//!
//! # Trial semantics: the wall-time cap
//!
//! The record's `budgets.wall_time_seconds_max` is the trial's wall budget.
//! The driver may cap it from above with `--trial-timeout-seconds`; the
//! **effective** cap is `min(record budget, driver cap)`, so the record's
//! budget always binds and the driver cap can only make a trial shorter.
//!
//! The cap changes trial semantics, so it is **recorded**: it is part of the
//! plan (and therefore of the plan hash), it is written to `summary.json`,
//! and it is written to every trial's sealed `trial-policy` artifact
//! together with the effective permission set and the prewarm description.
//!
//! # Attempts
//!
//! Every harness attempt of a trial is recorded — the superseded ones
//! included — with its index, its terminal classification, the
//! provider-error marker it hit, its wall time, its exit code, and the retry
//! decision that followed it. The records live in the trial's sealed
//! `attempt-log` artifact and in `summary.json`, so a retried trial is fully
//! auditable and not only its last attempt.
//!
//! # Process-spawning sites
//!
//! This module is the third documented exception to the crate's no-spawn
//! rule: bounded `git` children (fixture `init`/`add`/`commit`/`rev-parse`,
//! `env_clear` + fixed identity) plus everything the materializer,
//! supervisor, and verifier already spawn. Nothing here reads the ambient
//! environment except `PATH` (for `git`); the **verifier environment no
//! longer carries a host `PATH` tail** — the trial's fake-bin and stub-bin
//! are the whole `PATH`, and a verifier that names a shell gets the
//! trial-local staged shell.
//!
//! # Reproducibility
//!
//! The report is a pure function of the sealed store: [`reproduce_feasibility`]
//! compares the rebuilt canonical JSON with the stored `report.json`
//! byte-for-byte and reports per-check results. The run is labeled
//! feasibility-only; no effectiveness verdict is emitted (the M4 scope
//! decision).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::acceptance::{
    capture_tree_snapshot_excluding, diff_snapshots, evaluate_acceptance, parse_audit_log,
    AssertionReport, TrialEvidence,
};
use crate::canonical_json::canonical_json_bytes;
use crate::condition::{ConditionOverlay, OverlayAction, OverlayEntry};
use crate::context::{resolve_manifest, ContextRoots, DiscoveryFlags, EffectiveContextManifest};
use crate::diff::{capture_diff, DiffCaps};
use crate::digest::sha256_digest;
use crate::events::{NormalizedEvent, SOURCE_STEP_FINISH};
use crate::evidence::{
    redact_bytes, seal_manifest, ArtifactRef, ArtifactStore, ExecutionDigest, RedactionEvent,
    SealInputs, VerifierDigest,
};
use crate::fake_tools::bindings_from_environment;
use crate::isolation::{cleanup_trial, residue_scan, Residue};
use crate::leakage::{detect_host_leakage, LeakageReport};
use crate::materializer::{
    allowed_env, materialize_trial_with_fake_tools, trial_stub_bin_dir, PermissionSeeding,
};
use crate::plan::{
    compute_plan_hash, BenchmarkTaskRef, ConditionSpec, ExperimentSpec, HarnessLock,
    RepositoryLock, EXPERIMENT_SPEC_SCHEMA_VERSION, FEASIBILITY_HARNESS_NAME,
    GENERATED_COMMIT_PREFIX,
};
use crate::real_harness::{
    HarnessIdentity, ProviderErrorDetail, RealHarness, RealHarnessConfig, RealRunRequest,
    RetryDecision, REAL_HARNESS_MAX_RETRIES,
};
use crate::report::{build_feasibility_report, ReportVerdict};
use crate::schedule::{generate_schedule, PlannedTrial, ScheduleSpec, ScheduleTaskRef};
use crate::schema::{Fixture, Prompt, TaskRecord};
use crate::seal::tree_digest_v1;
use crate::store::{EvalStore, ExecutionRow, ExperimentRow, TrialRow, TrialState, VerificationRow};
use crate::supervisor::{run_supervised, SupervisorConfig, TerminationKind};
use crate::taxonomy::{classify_trial, TrialOutcomeClass};
use crate::trial::{TrialLayout, TrialSpec};
use crate::trial_env::{self, Prewarm};
use crate::verifier::{
    hermetic_verifier_env, resolve_verifier_argv, run_verifier_excluding,
    tree_fingerprint_excluding, VerifierOutcome, VerifierSpec, VerifierStep, VerifierVerdict,
};

/// Default experiment id used by the M5 example CLI.
pub const M5_EXPERIMENT_ID: &str = "m5-feasibility";

/// Documented fake-harness version pinned into the plan hash.
pub const M5_FAKE_HARNESS_VERSION: &str = "deterministic-v1";

/// Fixed `created_at` value used by the feasibility plan hash.
///
/// The plan hash must be reproducible across reruns; this constant is the only
/// time-like plan input (see the module docs).
pub const M5_FIXED_CREATED_AT: &str = "2026-10-19T00:00:00Z";

/// Default order seed (the audit's recorded seed).
pub const M5_DEFAULT_SEED: u64 = 881_204;

/// Default repetitions per task (the audit's `R=3`).
pub const M5_DEFAULT_REPETITIONS: u32 = 3;

/// Canary environment variable set on the driver process per trial.
pub const M5_CANARY_ENV: &str = "M5_CANARY_SECRET";

/// Prefix of every canary value (`canary-do-not-leak-<trial_id>`).
pub const M5_CANARY_VALUE_PREFIX: &str = "canary-do-not-leak-";

/// Supervisor stdout cap for one fake-agent trial (1 MiB).
pub const M5_OUTPUT_CAP_BYTES: u64 = 1024 * 1024;

/// Baseline condition id.
pub const M5_CONDITION_BASELINE: &str = "baseline";

/// Treatment condition id.
pub const M5_CONDITION_TREATMENT: &str = "treatment";

/// Requested-model string recorded verbatim for every fake-agent execution.
pub const M5_REQUESTED_MODEL: &str = "m5-fake-agent/deterministic-v1";

/// Fixed Git author/committer date used for deterministic fixture commits.
pub const M5_FIXTURE_GIT_DATE: &str = "2026-10-19T00:00:00Z";

/// The documented stand-in treatment skill used when
/// [`FeasibilityConfig::treatment_skill`] is `None`.
///
/// It exists only to make the treatment condition's effective-context manifest
/// distinct from the baseline's; the fake agents simulate condition behavior
/// directly, so the real `skills/claude/SKILL.md` is not required for the
/// plumbing run.
pub const M5_MARKER_SKILL: &str = "\
---
name: root-safe-install-marker
description: Feasibility-run stand-in treatment skill (fake-agent matrix)
---

# M5 treatment marker

This file stands in for `skills/claude/SKILL.md` in the deterministic
fake-agent feasibility matrix. It exists only to make the treatment
condition's effective-context manifest distinct from the baseline; the fake
agents simulate condition behavior directly.
";

/// Largest regular file the canary scan reads before failing closed.
const MAX_CANARY_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Project-relative directory prefix of the treatment skill file.
const TREATMENT_SKILL_DIR: &str = ".opencode/skill";

/// Fallback directory name when the treatment skill carries no parsable name.
const TREATMENT_SKILL_FALLBACK_DIR: &str = "root-safe-install";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`FeasibilityError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeasibilityErrorKind {
    /// An I/O operation failed.
    Io,
    /// A task record is missing, malformed, or unsealed.
    TaskRecord,
    /// A driver input (prompt, agent script, treatment skill) is unusable.
    Input,
    /// Effective-context resolution failed.
    Context,
    /// Plan construction or hashing failed.
    Plan,
    /// Schedule generation failed.
    Schedule,
    /// One trial failed at the driver level.
    Trial,
    /// The eval store failed.
    Store,
    /// The report build failed.
    Report,
    /// A fixture `git` child failed.
    Git,
    /// A contamination check failed.
    Contamination,
    /// The reproduction check failed to run.
    Reproduce,
    /// An internal invariant failed.
    Internal,
}

/// Errors produced by the feasibility-matrix driver.
#[derive(Debug, Error)]
pub enum FeasibilityError {
    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("task record `{path}` is unusable: {message}")]
    TaskRecord { path: PathBuf, message: String },

    #[error("trial input `{path}` is unusable: {message}")]
    Input { path: PathBuf, message: String },

    #[error("effective-context resolution failed: {message}")]
    Context { message: String },

    #[error("plan hashing failed: {message}")]
    Plan { message: String },

    #[error("schedule generation failed: {message}")]
    Schedule { message: String },

    #[error("trial `{trial_id}` failed: {message}")]
    Trial { trial_id: String, message: String },

    #[error(transparent)]
    Store(#[from] crate::store::StoreError),

    #[error(transparent)]
    Report(#[from] crate::report::ReportError),

    #[error("git `{args}` failed: {message}")]
    Git { args: String, message: String },

    #[error("contamination check failed: {message}")]
    Contamination { message: String },

    #[error("reproduction failed: {message}")]
    Reproduce { message: String },

    #[error("internal error: {message}")]
    Internal { message: String },
}

impl FeasibilityError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> FeasibilityErrorKind {
        match self {
            Self::Io { .. } => FeasibilityErrorKind::Io,
            Self::TaskRecord { .. } => FeasibilityErrorKind::TaskRecord,
            Self::Input { .. } => FeasibilityErrorKind::Input,
            Self::Context { .. } => FeasibilityErrorKind::Context,
            Self::Plan { .. } => FeasibilityErrorKind::Plan,
            Self::Schedule { .. } => FeasibilityErrorKind::Schedule,
            Self::Trial { .. } => FeasibilityErrorKind::Trial,
            Self::Store(_) => FeasibilityErrorKind::Store,
            Self::Report(_) => FeasibilityErrorKind::Report,
            Self::Git { .. } => FeasibilityErrorKind::Git,
            Self::Contamination { .. } => FeasibilityErrorKind::Contamination,
            Self::Reproduce { .. } => FeasibilityErrorKind::Reproduce,
            Self::Internal { .. } => FeasibilityErrorKind::Internal,
        }
    }
}

impl From<crate::error::TaskError> for FeasibilityError {
    fn from(error: crate::error::TaskError) -> Self {
        Self::TaskRecord {
            path: PathBuf::new(),
            message: error.to_string(),
        }
    }
}

impl From<crate::context::ContextError> for FeasibilityError {
    fn from(error: crate::context::ContextError) -> Self {
        Self::Context {
            message: error.to_string(),
        }
    }
}

impl From<crate::schedule::ScheduleError> for FeasibilityError {
    fn from(error: crate::schedule::ScheduleError) -> Self {
        Self::Schedule {
            message: error.to_string(),
        }
    }
}

impl From<crate::acceptance::AcceptanceError> for FeasibilityError {
    fn from(error: crate::acceptance::AcceptanceError) -> Self {
        Self::Trial {
            trial_id: String::new(),
            message: error.to_string(),
        }
    }
}

impl From<crate::evidence::EvidenceError> for FeasibilityError {
    fn from(error: crate::evidence::EvidenceError) -> Self {
        Self::Internal {
            message: error.to_string(),
        }
    }
}

impl From<crate::evidence::SealError> for FeasibilityError {
    fn from(error: crate::evidence::SealError) -> Self {
        Self::Internal {
            message: error.to_string(),
        }
    }
}

impl From<crate::materializer::MaterializerError> for FeasibilityError {
    fn from(error: crate::materializer::MaterializerError) -> Self {
        Self::Trial {
            trial_id: String::new(),
            message: error.to_string(),
        }
    }
}

impl From<crate::diff::DiffError> for FeasibilityError {
    fn from(error: crate::diff::DiffError) -> Self {
        Self::Internal {
            message: error.to_string(),
        }
    }
}

fn io_error(path: &Path, error: std::io::Error) -> FeasibilityError {
    FeasibilityError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Configuration and summary
// ---------------------------------------------------------------------------

/// Inputs for one feasibility-matrix run.
#[derive(Debug, Clone)]
pub struct FeasibilityConfig {
    /// Directory containing the sealed `*.task.toml` records.
    pub tasks_dir: PathBuf,
    /// Directory containing the file-based prompt fixtures (resolved by file
    /// name; inline prompts need no file).
    pub prompts_dir: PathBuf,
    /// Directory containing the per-task fake-agent scripts
    /// (`<record-file-stem>.sh`).
    ///
    /// Only the fake harness needs these scripts; the real harness runs the
    /// task record's prompt instead.
    pub agents_dir: PathBuf,
    /// Evals root: the store lives at `<evals_root>/<experiment_id>/state.db`,
    /// artifacts under `<evals_root>/artifacts/sha256/...`, and the run
    /// evidence (`report.txt`, `report.json`, `summary.json`) directly under
    /// `<evals_root>`.
    pub evals_root: PathBuf,
    /// Experiment identity (single path component).
    pub experiment_id: String,
    /// Repetitions per task.
    pub repetitions: u32,
    /// Recorded order seed.
    pub seed: u64,
    /// Treatment skill bytes source; `None` uses [`M5_MARKER_SKILL`].
    pub treatment_skill: Option<PathBuf>,
    /// Which harness runs the trials.
    ///
    /// [`HarnessChoice::FakeAgent`] (the default) runs the deterministic
    /// fake-agent scripts; [`HarnessChoice::OpenCodeReal`] runs the pinned
    /// OpenCode `1.18.27` CLI in a hermetic, prewarmed trial environment.
    pub harness: HarnessChoice,
    /// Real-harness configuration.
    ///
    /// Required when `harness` is [`HarnessChoice::OpenCodeReal`] and
    /// [`Self::real_harness_override`] is `None`: the binary path is
    /// digest-verified against the pinned `1.18.27` expectation before the
    /// first use.
    pub real: Option<RealHarnessConfig>,
    /// Prewarm root holding the shared real-harness assets (the warm npm
    /// cache, the config template, `models.json`, and the `rg` binary).
    ///
    /// Required when `harness` is [`HarnessChoice::OpenCodeReal`].
    pub prewarm_root: Option<PathBuf>,
    /// A pre-opened real harness, used instead of opening [`Self::real`].
    ///
    /// **Test-support seam.** The driver never uses it for a scored run: a
    /// scored run leaves it `None` so the pinned `1.18.27` digest gate of
    /// [`RealHarness::open`] applies. It exists so the crate's own
    /// end-to-end tests can drive the real path with a stub binary.
    pub real_harness_override: Option<RealHarness>,
    /// Driver-level wall-time cap for one trial, in seconds.
    ///
    /// The **effective** cap is `min(record budget, driver cap)`: the
    /// record's `budgets.wall_time_seconds_max` always binds, and the driver
    /// cap can only make the trial *shorter*. `None` (the default) means
    /// "no driver cap": the record's budget applies unchanged.
    ///
    /// The cap changes trial semantics, so it is **recorded**: it is hashed
    /// into the plan (and therefore into the plan hash) and it is written to
    /// `summary.json` and to every trial's sealed `trial-policy` artifact.
    pub trial_timeout_seconds: Option<u64>,
}

impl FeasibilityConfig {
    /// Build a config with the recorded defaults (`R = 3`, seed `881204`,
    /// marker treatment skill, the fake harness).
    pub fn new(
        tasks_dir: impl Into<PathBuf>,
        prompts_dir: impl Into<PathBuf>,
        agents_dir: impl Into<PathBuf>,
        evals_root: impl Into<PathBuf>,
        experiment_id: impl Into<String>,
    ) -> Self {
        Self {
            tasks_dir: tasks_dir.into(),
            prompts_dir: prompts_dir.into(),
            agents_dir: agents_dir.into(),
            evals_root: evals_root.into(),
            experiment_id: experiment_id.into(),
            repetitions: M5_DEFAULT_REPETITIONS,
            seed: M5_DEFAULT_SEED,
            treatment_skill: None,
            harness: HarnessChoice::FakeAgent,
            real: None,
            prewarm_root: None,
            real_harness_override: None,
            trial_timeout_seconds: None,
        }
    }
}

/// Which harness the feasibility matrix runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessChoice {
    /// The deterministic fake agent (the M5 default).
    FakeAgent,
    /// The real, digest-pinned OpenCode `1.18.27` CLI.
    OpenCodeReal,
}

impl HarnessChoice {
    /// Stable snake-case token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FakeAgent => "fake_agent",
            Self::OpenCodeReal => "opencode_real",
        }
    }

    /// The permission posture a trial of this harness must be seeded with.
    ///
    /// The **fake**-agent path keeps the ask-mode seed: its denial fixtures
    /// depend on the ask-then-auto-reject shape the M0 golden captures
    /// recorded.
    ///
    /// The **real** OpenCode path needs an explicit allow-all seed: a
    /// non-interactive `opencode run` auto-rejects every ask (M0
    /// UPSTREAM-CONTRACT §5.1), so an ask-mode seed would make every tool
    /// call of every trial an auto-rejected call — the agent could execute
    /// nothing, and the trial would measure a harness artifact instead of
    /// the agent's behaviour.
    pub fn permission_seeding(self) -> PermissionSeeding {
        match self {
            Self::FakeAgent => PermissionSeeding::AskMode,
            Self::OpenCodeReal => PermissionSeeding::AllowAll,
        }
    }

    /// Parse the stable token produced by [`HarnessChoice::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "fake_agent" => Some(Self::FakeAgent),
            "opencode_real" => Some(Self::OpenCodeReal),
            _ => None,
        }
    }
}

/// Secret-hygiene check for one trial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanaryCheck {
    /// Trial the check belongs to.
    pub trial_id: String,
    /// True when no hit was recorded anywhere.
    pub clean: bool,
    /// Regular files scanned under the trial root.
    pub trial_files_scanned: u64,
    /// Bytes scanned across those files.
    pub trial_bytes_scanned: u64,
    /// True when `trial.env` does not contain the canary.
    pub env_file_clean: bool,
    /// True when neither raw agent stream contains the canary.
    pub raw_streams_clean: bool,
    /// Stored artifacts scanned.
    pub artifacts_scanned: u64,
    /// Bytes scanned across stored artifacts.
    pub artifact_bytes_scanned: u64,
    /// Every hit, as `location:path` strings.
    pub hits: Vec<String>,
}

/// Aggregated contamination checks for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContaminationSummary {
    /// True when every trial's canary check was clean.
    pub canary_clean: bool,
    /// Trials checked.
    pub canary_trials_checked: u32,
    /// Trial files scanned across the run.
    pub canary_files_scanned: u64,
    /// Bytes scanned (trial files + stored artifacts).
    pub canary_bytes_scanned: u64,
    /// Every canary hit across the run.
    pub canary_hits: Vec<String>,
    /// Every trial root is distinct.
    pub roots_distinct: bool,
    /// No trial root is a prefix of another.
    pub roots_pairwise_disjoint: bool,
    /// Every trial root is under the run's trials directory.
    pub roots_contained: bool,
    /// No trial root overlaps the experiment's store directory.
    pub roots_outside_store: bool,
    /// Trial roots checked.
    pub roots_checked: u32,
    /// The trials/sources scratch directories hold no residue.
    pub residue_clean: bool,
    /// Residue entries (`kind:path`).
    pub residue_entries: Vec<String>,
    /// No trial tree carried host state (no symlink out of the trial root,
    /// no host-sized file in a trial stub directory).
    pub host_leakage_clean: bool,
    /// Every host-leakage finding across the run (`trial: kind path detail`).
    pub host_leakage_findings: Vec<String>,
}

/// One harness attempt inside a trial, as recorded in the trial evidence.
///
/// Every attempt is recorded — the superseded ones included — so a retried
/// trial is fully auditable: the retry decision, the provider-error marker,
/// and the wall time of the attempt that was discarded are all recoverable
/// from the sealed evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialAttemptRecord {
    /// The 1-based attempt index.
    pub index: u32,
    /// The supervisor terminal classification token of the attempt.
    pub termination: String,
    /// The detected provider-error marker, when the attempt hit one.
    pub provider_error: Option<String>,
    /// The attempt's wall time in milliseconds.
    pub wall_ms: u64,
    /// The child's exit code, when it produced one.
    pub exit_code: Option<i32>,
    /// The retry reason, when the attempt was retry-eligible.
    pub retry_reason: Option<String>,
    /// True when the driver actually started another attempt after this one.
    ///
    /// The attempt with `retried = false` and the highest index is the
    /// trial's final attempt — the one its outcome was taken from.
    pub retried: bool,
}

/// One trial's machine summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialSummary {
    /// Trial identity (`t{order:03}-{task}-{condition}-r{repetition}`).
    pub trial_id: String,
    /// Global execution position.
    pub order_index: u32,
    /// Block identity (task × repetition slot).
    pub block_index: u32,
    /// Position inside the block.
    pub block_position: u8,
    /// Benchmark task identity.
    pub task_id: String,
    /// Task record version.
    pub task_version: u32,
    /// Condition identity.
    pub condition_id: String,
    /// Repetition index.
    pub repetition: u32,
    /// Effective-context manifest digest materialized for the trial.
    pub manifest_sha256: String,
    /// Fixture Git commit captured for the trial source.
    pub git_commit: String,
    /// Fixture Git tree id captured for the trial source.
    pub git_tree: String,
    /// `tree_digest_v1` digest of the record's declared fixture files.
    pub fixture_tree_sha256: String,
    /// Supervisor terminal classification token.
    pub termination: String,
    /// Stored outcome class token.
    pub outcome_class: String,
    /// True when every acceptance assertion passed.
    pub acceptance_all_passed: bool,
    /// True when every primary acceptance assertion passed.
    pub acceptance_all_primary_passed: bool,
    /// Failing assertion ids, in record order.
    pub failed_assertions: Vec<String>,
    /// External verifier ids that ran.
    pub verifier_ids: Vec<String>,
    /// Secret-hygiene check result.
    pub canary: CanaryCheck,
    /// Harness name that produced the execution.
    pub harness_name: String,
    /// Harness version that produced the execution.
    pub harness_version: String,
    /// Canonical `sha256:<hex>` digest of the harness binary.
    pub harness_binary_sha256: String,
    /// The requested model, recorded verbatim.
    pub requested_model: String,
    /// Always `None` (M0 identity policy): the observed model is unavailable.
    pub observed_model: Option<String>,
    /// Attempts spent on this trial (2 means the documented retry was used).
    pub attempts: u32,
    /// The retry reason, when the documented retry was used.
    pub retry_reason: Option<String>,
    /// The detected provider-error marker, when the run hit one.
    pub provider_error: Option<String>,
    /// How the harness turn ended (`completed` or `truncated`).
    pub turn_state: String,
    /// The permission posture seeded into this trial (`ask_mode` /
    /// `allow_all`).
    pub permission_seeding: String,
    /// The effective permission set of this trial, verbatim JSON.
    pub effective_permission: String,
    /// The effective wall-time cap of this trial, in seconds.
    ///
    /// `min(record budget, driver cap)` — see
    /// [`FeasibilityConfig::trial_timeout_seconds`].
    pub effective_wall_timeout_seconds: u64,
    /// The prewarm assets description (the real harness only).
    pub prewarm: Option<String>,
    /// Every harness attempt of this trial, in order.
    pub attempt_records: Vec<TrialAttemptRecord>,
    /// The host-leakage scan of this trial's tree.
    pub host_leakage: LeakageReport,
}

/// The run summary written to `summary.json` and returned by
/// [`run_feasibility_matrix`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeasibilityRunSummary {
    /// Experiment identity.
    pub experiment_id: String,
    /// Canonical plan hash of the pinned experiment spec.
    pub plan_hash: String,
    /// Recorded order seed.
    pub schedule_seed: u64,
    /// Repetitions per task.
    pub repetitions: u32,
    /// Planned trials (schedule length).
    pub planned_trials: u32,
    /// Trials fully processed.
    pub completed_trials: u32,
    /// Trial ids in execution order.
    pub trial_ids: Vec<String>,
    /// Per-trial summaries in execution order.
    pub trials: Vec<TrialSummary>,
    /// Aggregated contamination checks.
    pub contamination: ContaminationSummary,
    /// Stored evidence manifests re-verified.
    pub store_integrity_checked: u64,
    /// True when every stored manifest re-verified.
    pub store_integrity_intact: bool,
    /// Integrity failures, as `trial: message (check)` strings.
    pub store_integrity_failures: Vec<String>,
    /// Absolute path of the stored `report.json`.
    pub report_path: String,
    /// SHA-256 of the stored `report.json` bytes.
    pub report_sha256: String,
    /// Absolute path of the stored `summary.json`.
    pub summary_path: String,
    /// Feasibility label carried by the report.
    pub feasibility_label: String,
    /// True when the report's verdict is `deferred` (always true).
    pub verdict_deferred: bool,
    /// Valid paired blocks.
    pub valid_blocks: u32,
    /// Symmetrically excluded blocks.
    pub invalid_blocks: u32,
    /// Independent `(task_id, task_version)` pairs over valid blocks.
    pub independent_tasks: u32,
    /// Valid blocks where baseline failed and treatment passed.
    pub discordant_baseline_fail_treatment_pass: u32,
    /// Valid blocks where baseline passed and treatment failed.
    pub discordant_baseline_pass_treatment_fail: u32,
    /// Tasks whose valid blocks show at least one `better` direction.
    pub tasks_with_better_direction: u32,
    /// Treatment trials recorded `completed_passed`.
    pub treatment_completed_passed: u32,
    /// Treatment trials recorded `completed_failed`.
    pub treatment_completed_failed: u32,
    /// Baseline trials recorded `completed_passed`.
    pub baseline_completed_passed: u32,
    /// Baseline trials recorded `completed_failed`.
    pub baseline_completed_failed: u32,
    /// Harness name that produced the executions.
    pub harness_name: String,
    /// Harness version that produced the executions.
    pub harness_version: String,
    /// Canonical `sha256:<hex>` digest of the harness binary.
    pub harness_binary_sha256: String,
    /// The permission posture the run seeded into every trial (`ask_mode` /
    /// `allow_all`).
    pub permission_seeding: String,
    /// The driver-level wall-time cap, in seconds.
    ///
    /// `None` means "no driver cap": every trial used its record's budget.
    /// The **effective** cap of a trial is `min(record budget, driver cap)`.
    pub trial_timeout_seconds: Option<u64>,
    /// The prewarm assets description (the real harness only).
    pub prewarm: Option<String>,
}

/// Per-check result of [`reproduce_feasibility`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReproduceOutcome {
    /// Experiment identity.
    pub experiment_id: String,
    /// Stored evidence manifests re-verified.
    pub store_integrity_checked: u64,
    /// True when every stored manifest re-verified.
    pub store_integrity_intact: bool,
    /// Integrity failures, as `trial: message (check)` strings.
    pub store_integrity_failures: Vec<String>,
    /// Path of the stored `report.json` compared against.
    pub report_path: PathBuf,
    /// True when the rebuilt report is byte-identical to the stored one.
    pub report_byte_identical: bool,
    /// SHA-256 of the stored report bytes.
    pub stored_report_sha256: String,
    /// SHA-256 of the rebuilt report bytes.
    pub rebuilt_report_sha256: String,
    /// Stored report size in bytes.
    pub report_bytes: u64,
}

impl ReproduceOutcome {
    /// Plain-text rendering for the CLI.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Reproduction check — experiment `{}`\n",
            self.experiment_id
        ));
        out.push_str(&format!(
            "Store integrity: {} manifest(s) checked; {}\n",
            self.store_integrity_checked,
            if self.store_integrity_intact {
                "intact".to_string()
            } else {
                format!("{} failure(s)", self.store_integrity_failures.len())
            }
        ));
        for failure in &self.store_integrity_failures {
            out.push_str(&format!("  - {failure}\n"));
        }
        out.push_str(&format!(
            "Report: {} ({} bytes)\n",
            self.report_path.display(),
            self.report_bytes
        ));
        out.push_str(&format!("Stored sha256:  {}\n", self.stored_report_sha256));
        out.push_str(&format!("Rebuilt sha256: {}\n", self.rebuilt_report_sha256));
        out.push_str(&format!(
            "Byte-identical: {}\n",
            if self.report_byte_identical {
                "YES"
            } else {
                "NO"
            }
        ));
        out
    }
}

// ---------------------------------------------------------------------------
// Internal shapes
// ---------------------------------------------------------------------------

/// One loaded task: sealed record plus its resolved driver inputs.
struct LoadedTask {
    record: TaskRecord,
    /// The per-task fake-agent script (the fake harness only).
    agent_path: Option<PathBuf>,
    /// Digest of the fake-agent script (the fake harness only).
    agent_sha256: Option<String>,
    prompt_bytes: Vec<u8>,
    fixture_tree_sha256: String,
}

/// The two condition manifests and the declared treatment overlay.
struct ConditionManifests {
    baseline: EffectiveContextManifest,
    treatment: EffectiveContextManifest,
    treatment_overlay: ConditionOverlay,
}

/// Shared per-run context for the trial loop.
struct MatrixContext<'a> {
    config: &'a FeasibilityConfig,
    plan_hash: &'a str,
    cas: &'a ArtifactStore,
    store: &'a EvalStore,
    manifests: &'a ConditionManifests,
    trials_base: &'a Path,
    sources_base: &'a Path,
    /// Which harness runs the trials.
    harness: HarnessChoice,
    /// The opened real harness (`None` for the fake harness).
    real_harness: Option<RealHarness>,
    /// The shared prewarm assets (`None` for the fake harness).
    prewarm: Option<Prewarm>,
    /// The harness identity recorded in the plan lock and in the evidence.
    identity: HarnessIdentity,
}

/// One processed trial: its summary and materialized root.
struct TrialOutcome {
    summary: TrialSummary,
    root: PathBuf,
}

/// One harness attempt inside a trial: the materialized tree plus the
/// supervised result it produced.
struct HarnessAttempt {
    layout: TrialLayout,
    /// The trial-local shell the verifier commands may use.
    shell: PathBuf,
    /// Tree snapshot taken before the harness ran.
    pre: crate::acceptance::TreeSnapshot,
    supervised: crate::supervisor::SupervisedResult,
    /// The effective terminal classification (the provider overlay applied).
    termination: TerminationKind,
    /// The requested model, recorded verbatim.
    requested_model: String,
    /// The detected provider failure, when the run hit one.
    provider_error: Option<ProviderErrorDetail>,
    /// The (never silent) retry decision.
    retry: RetryDecision,
    /// Attempts spent so far, including this one.
    attempts: u32,
    /// Spawn time of the harness child, Unix milliseconds.
    started_at_ms: u64,
    /// Reap time of the harness child, Unix milliseconds.
    finished_at_ms: u64,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Execute the full feasibility matrix and write the run evidence.
///
/// See the module docs for the pipeline, the outcome mapping, and the
/// contamination checks. On success the returned summary matches the
/// `summary.json` written under `config.evals_root`. A contamination hit is
/// reported as [`FeasibilityError::Contamination`] **after** the report and
/// summary are written (the preserved trial trees stay for inspection).
pub fn run_feasibility_matrix(
    config: &FeasibilityConfig,
) -> Result<FeasibilityRunSummary, FeasibilityError> {
    let tasks = load_tasks(config)?;
    let scratch = config
        .evals_root
        .join("scratch")
        .join(&config.experiment_id);
    let trials_base = scratch.join("trials");
    let sources_base = scratch.join("sources");
    for dir in [&scratch, &trials_base, &sources_base] {
        fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    }

    let manifests = prepare_manifests(config, &scratch)?;
    let (harness, real_harness, prewarm) = prepare_harnesses(config)?;
    let identity = harness_identity(harness, &tasks, real_harness.as_ref())?;
    let plan = build_experiment_spec(config, &tasks, &manifests, &identity)?;
    let plan_hash = compute_plan_hash(&plan).map_err(|error| FeasibilityError::Plan {
        message: error.to_string(),
    })?;

    let schedule_spec = ScheduleSpec {
        experiment_id: config.experiment_id.clone(),
        tasks: tasks
            .iter()
            .map(|task| ScheduleTaskRef {
                task_id: task.record.task_id.clone(),
                version: task.record.version,
            })
            .collect(),
        conditions: vec![
            M5_CONDITION_BASELINE.to_string(),
            M5_CONDITION_TREATMENT.to_string(),
        ],
        repetitions: config.repetitions,
        seed: config.seed,
    };
    let schedule = generate_schedule(&schedule_spec)?;

    let store = EvalStore::open(&config.evals_root, &config.experiment_id)?;
    store.record_experiment(ExperimentRow {
        experiment_id: config.experiment_id.clone(),
        plan_hash: plan_hash.clone(),
        created_at_ms: unix_millis(),
    })?;
    let cas = ArtifactStore::new(&config.evals_root);
    let context = MatrixContext {
        config,
        plan_hash: &plan_hash,
        cas: &cas,
        store: &store,
        manifests: &manifests,
        trials_base: &trials_base,
        sources_base: &sources_base,
        harness,
        real_harness,
        prewarm,
        identity: identity.clone(),
    };

    let mut outcomes: Vec<TrialOutcome> = Vec::with_capacity(schedule.len());
    for planned in &schedule {
        let task = tasks
            .iter()
            .find(|task| {
                task.record.task_id == planned.task_id
                    && task.record.version == planned.task_version
            })
            .ok_or_else(|| FeasibilityError::Internal {
                message: format!(
                    "scheduled task `{}` v{} has no loaded record",
                    planned.task_id, planned.task_version
                ),
            })?;
        let outcome = run_one_trial(&context, task, planned).map_err(|error| match error {
            FeasibilityError::Trial { .. } => error,
            other => FeasibilityError::Trial {
                trial_id: trial_id(planned),
                message: other.to_string(),
            },
        })?;
        outcomes.push(outcome);
    }

    let integrity = store.verify_store_evidence(&cas)?;
    let report = build_feasibility_report(&store, &config.experiment_id)?;
    let report_json = report.to_json()?;
    let report_text = report.render_text();
    let report_path = config.evals_root.join("report.json");
    let report_txt_path = config.evals_root.join("report.txt");
    fs::write(&report_path, report_json.as_bytes())
        .map_err(|error| io_error(&report_path, error))?;
    fs::write(&report_txt_path, report_text.as_bytes())
        .map_err(|error| io_error(&report_txt_path, error))?;
    let report_sha256 = sha256_digest(report_json.as_bytes());

    let residue_trials =
        residue_scan(&trials_base).map_err(|error| FeasibilityError::Internal {
            message: error.to_string(),
        })?;
    let residue_sources =
        residue_scan(&sources_base).map_err(|error| FeasibilityError::Internal {
            message: error.to_string(),
        })?;
    let mut residue_entries: Vec<String> = residue_trials
        .iter()
        .chain(residue_sources.iter())
        .map(residue_token)
        .collect();
    residue_entries.sort();

    let roots: Vec<PathBuf> = outcomes
        .iter()
        .map(|outcome| outcome.root.clone())
        .collect();
    let store_dir = config.evals_root.join(&config.experiment_id);
    let (roots_distinct, roots_pairwise_disjoint, roots_contained, roots_outside_store) =
        check_roots(&roots, &trials_base, &store_dir);

    let trials: Vec<TrialSummary> = outcomes
        .into_iter()
        .map(|outcome| outcome.summary)
        .collect();
    let canary_clean = trials.iter().all(|trial| trial.canary.clean);
    let mut host_leakage_findings: Vec<String> = trials
        .iter()
        .flat_map(|trial| {
            trial
                .host_leakage
                .findings
                .iter()
                .map(|finding| format!("{}: {}", trial.trial_id, finding.token()))
        })
        .collect();
    host_leakage_findings.sort();
    let contamination = ContaminationSummary {
        canary_clean,
        canary_trials_checked: u32::try_from(trials.len()).unwrap_or(u32::MAX),
        canary_files_scanned: trials
            .iter()
            .map(|trial| trial.canary.trial_files_scanned)
            .sum(),
        canary_bytes_scanned: trials
            .iter()
            .map(|trial| trial.canary.trial_bytes_scanned + trial.canary.artifact_bytes_scanned)
            .sum(),
        canary_hits: trials
            .iter()
            .flat_map(|trial| trial.canary.hits.iter().cloned())
            .collect(),
        roots_distinct,
        roots_pairwise_disjoint,
        roots_contained,
        roots_outside_store,
        roots_checked: u32::try_from(roots.len()).unwrap_or(u32::MAX),
        residue_clean: residue_entries.is_empty(),
        residue_entries,
        host_leakage_clean: host_leakage_findings.is_empty(),
        host_leakage_findings,
    };
    let contamination_clean = contamination.canary_clean
        && contamination.roots_distinct
        && contamination.roots_pairwise_disjoint
        && contamination.roots_contained
        && contamination.roots_outside_store
        && contamination.residue_clean
        && contamination.host_leakage_clean;

    let treatment = condition_counts(&report, M5_CONDITION_TREATMENT);
    let baseline = condition_counts(&report, M5_CONDITION_BASELINE);
    let summary_path = config.evals_root.join("summary.json");
    let summary = FeasibilityRunSummary {
        experiment_id: config.experiment_id.clone(),
        plan_hash: plan_hash.clone(),
        schedule_seed: config.seed,
        repetitions: config.repetitions,
        planned_trials: u32::try_from(schedule.len()).unwrap_or(u32::MAX),
        completed_trials: u32::try_from(trials.len()).unwrap_or(u32::MAX),
        trial_ids: trials.iter().map(|trial| trial.trial_id.clone()).collect(),
        trials,
        contamination,
        store_integrity_checked: integrity.checked,
        store_integrity_intact: integrity.is_intact(),
        store_integrity_failures: integrity
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "{}: {} ({:?})",
                    failure.trial_id, failure.message, failure.check
                )
            })
            .collect(),
        report_path: report_path.to_string_lossy().into_owned(),
        report_sha256,
        summary_path: summary_path.to_string_lossy().into_owned(),
        feasibility_label: report.feasibility_label.clone(),
        verdict_deferred: matches!(report.verdict, ReportVerdict::Deferred { .. }),
        valid_blocks: report.valid_blocks,
        invalid_blocks: u32::try_from(report.invalid_blocks.len()).unwrap_or(u32::MAX),
        independent_tasks: report.independent_tasks,
        discordant_baseline_fail_treatment_pass: report.discordant.baseline_fail_treatment_pass,
        discordant_baseline_pass_treatment_fail: report.discordant.baseline_pass_treatment_fail,
        tasks_with_better_direction: u32::try_from(
            report
                .task_direction
                .iter()
                .filter(|direction| direction.better > 0)
                .count(),
        )
        .unwrap_or(u32::MAX),
        treatment_completed_passed: treatment.0,
        treatment_completed_failed: treatment.1,
        baseline_completed_passed: baseline.0,
        baseline_completed_failed: baseline.1,
        harness_name: context.identity.name.clone(),
        harness_version: context.identity.version.clone(),
        harness_binary_sha256: context.identity.binary_sha256.clone(),
        permission_seeding: harness.permission_seeding().as_str().to_string(),
        trial_timeout_seconds: config.trial_timeout_seconds,
        prewarm: context.prewarm.as_ref().map(|prewarm| prewarm.describe()),
    };

    let summary_value =
        serde_json::to_value(&summary).map_err(|error| FeasibilityError::Internal {
            message: format!("summary serialization failed: {error}"),
        })?;
    let summary_bytes = canonical_json_bytes(&summary_value)
        .map_err(|message| FeasibilityError::Internal { message })?;
    fs::write(&summary_path, &summary_bytes).map_err(|error| io_error(&summary_path, error))?;

    if contamination_clean {
        fs::remove_dir_all(&scratch).map_err(|error| io_error(&scratch, error))?;
    }

    if !contamination_clean {
        return Err(FeasibilityError::Contamination {
            message: format!(
                "contamination checks failed (see {}); canary_clean={}, roots_distinct={}, \
                 roots_pairwise_disjoint={}, roots_contained={}, roots_outside_store={}, \
                 residue_clean={}, host_leakage_clean={}",
                summary_path.display(),
                summary.contamination.canary_clean,
                summary.contamination.roots_distinct,
                summary.contamination.roots_pairwise_disjoint,
                summary.contamination.roots_contained,
                summary.contamination.roots_outside_store,
                summary.contamination.residue_clean,
                summary.contamination.host_leakage_clean,
            ),
        });
    }
    Ok(summary)
}

/// Prove the stored report can be rebuilt from sealed evidence only.
///
/// Reopens the eval store **read-only**, re-verifies every stored evidence
/// manifest against the artifact store, rebuilds the feasibility report, and
/// compares it byte-for-byte with `<evals_root>/report.json`. No agent or
/// verifier process runs.
pub fn reproduce_feasibility(
    evals_root: &Path,
    experiment_id: &str,
) -> Result<ReproduceOutcome, FeasibilityError> {
    let store = EvalStore::open_read_only(evals_root, experiment_id)?;
    let cas = ArtifactStore::new(evals_root);
    let integrity = store.verify_store_evidence(&cas)?;
    let report = build_feasibility_report(&store, experiment_id)?;
    let rebuilt = report.to_json()?;
    let report_path = evals_root.join("report.json");
    let stored = fs::read_to_string(&report_path).map_err(|error| io_error(&report_path, error))?;

    Ok(ReproduceOutcome {
        experiment_id: experiment_id.to_string(),
        store_integrity_checked: integrity.checked,
        store_integrity_intact: integrity.is_intact(),
        store_integrity_failures: integrity
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "{}: {} ({:?})",
                    failure.trial_id, failure.message, failure.check
                )
            })
            .collect(),
        report_path,
        report_byte_identical: stored == rebuilt,
        stored_report_sha256: sha256_digest(stored.as_bytes()),
        rebuilt_report_sha256: sha256_digest(rebuilt.as_bytes()),
        report_bytes: u64::try_from(stored.len()).unwrap_or(u64::MAX),
    })
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Enumerate, strictly parse, and sort every sealed task record.
fn load_tasks(config: &FeasibilityConfig) -> Result<Vec<LoadedTask>, FeasibilityError> {
    let entries =
        fs::read_dir(&config.tasks_dir).map_err(|error| io_error(&config.tasks_dir, error))?;
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(&config.tasks_dir, error))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".task.toml") else {
            continue;
        };
        if path.is_file() {
            paths.push((stem.to_string(), path));
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(FeasibilityError::Input {
            path: config.tasks_dir.clone(),
            message: "no `*.task.toml` records found".to_string(),
        });
    }

    let mut tasks: Vec<LoadedTask> = Vec::with_capacity(paths.len());
    for (stem, path) in paths {
        let text = fs::read_to_string(&path).map_err(|error| FeasibilityError::TaskRecord {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let record =
            TaskRecord::parse_str(&text).map_err(|error| FeasibilityError::TaskRecord {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if record.integrity.content_sha256 != record.canonical_hash() {
            return Err(FeasibilityError::TaskRecord {
                path,
                message: "content_sha256 does not match the canonical hash".to_string(),
            });
        }
        let fixture_tree_sha256 = match &record.fixture {
            Fixture::Generated(fixture) => tree_digest_v1(&fixture.files),
            Fixture::RepoCommit(_) => {
                return Err(FeasibilityError::TaskRecord {
                    path,
                    message: "the feasibility matrix driver supports `generated` fixtures only"
                        .to_string(),
                });
            }
        };
        let prompt_bytes = resolve_prompt(config, &record, &path)?;
        // The fake-agent script belongs to the fake harness only: the real
        // harness runs the record's prompt instead.
        let (agent_path, agent_sha256) = if config.harness == HarnessChoice::FakeAgent {
            let agent_path = config.agents_dir.join(format!("{stem}.sh"));
            let agent_bytes = fs::read(&agent_path).map_err(|error| FeasibilityError::Input {
                path: agent_path.clone(),
                message: format!("fake agent script: {error}"),
            })?;
            (Some(agent_path), Some(sha256_digest(&agent_bytes)))
        } else {
            (None, None)
        };
        tasks.push(LoadedTask {
            record,
            agent_path,
            agent_sha256,
            prompt_bytes,
            fixture_tree_sha256,
        });
    }

    tasks.sort_by(|left, right| {
        (left.record.task_id.as_str(), left.record.version)
            .cmp(&(right.record.task_id.as_str(), right.record.version))
    });
    for pair in tasks.windows(2) {
        if pair[0].record.task_id == pair[1].record.task_id
            && pair[0].record.version == pair[1].record.version
        {
            return Err(FeasibilityError::TaskRecord {
                path: config.tasks_dir.clone(),
                message: format!(
                    "duplicate task `{}` version {}",
                    pair[0].record.task_id, pair[0].record.version
                ),
            });
        }
    }
    Ok(tasks)
}

/// Resolve a record's prompt bytes (inline text or the prompt file by name).
fn resolve_prompt(
    config: &FeasibilityConfig,
    record: &TaskRecord,
    record_path: &Path,
) -> Result<Vec<u8>, FeasibilityError> {
    match &record.prompt {
        Prompt::Inline { inline } => Ok(inline.as_bytes().to_vec()),
        Prompt::File { file, file_sha256 } => {
            let name = Path::new(file)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| FeasibilityError::TaskRecord {
                    path: record_path.to_path_buf(),
                    message: format!("prompt file reference `{file}` has no file name"),
                })?;
            let path = config.prompts_dir.join(name);
            let bytes = fs::read(&path).map_err(|error| FeasibilityError::Input {
                path: path.clone(),
                message: format!("prompt file: {error}"),
            })?;
            if &sha256_digest(&bytes) != file_sha256 {
                return Err(FeasibilityError::Input {
                    path,
                    message: "prompt digest does not match the record".to_string(),
                });
            }
            Ok(bytes)
        }
    }
}

// ---------------------------------------------------------------------------
// Contexts and plan
// ---------------------------------------------------------------------------

/// Resolve the empty baseline manifest and the treatment manifest.
fn prepare_manifests(
    config: &FeasibilityConfig,
    scratch: &Path,
) -> Result<ConditionManifests, FeasibilityError> {
    let context_root = scratch.join("context");
    let baseline = resolve_context(&context_root.join("baseline"))?;
    if !baseline.instructions().is_empty() || !baseline.skills().is_empty() {
        return Err(FeasibilityError::Context {
            message: "the baseline context must be empty (no instructions, no skills)".to_string(),
        });
    }

    let skill_bytes = match &config.treatment_skill {
        Some(path) => fs::read(path).map_err(|error| FeasibilityError::Input {
            path: path.clone(),
            message: format!("treatment skill: {error}"),
        })?,
        None => M5_MARKER_SKILL.as_bytes().to_vec(),
    };
    let dir_name =
        skill_dir_name(&skill_bytes).unwrap_or_else(|| TREATMENT_SKILL_FALLBACK_DIR.to_string());
    let relative = format!("{TREATMENT_SKILL_DIR}/{dir_name}/SKILL.md");
    let skill_path = context_root
        .join("treatment")
        .join("project")
        .join(&relative);
    if let Some(parent) = skill_path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    fs::write(&skill_path, &skill_bytes).map_err(|error| io_error(&skill_path, error))?;

    let treatment = resolve_context(&context_root.join("treatment"))?;
    let digest = sha256_digest(&skill_bytes);
    let token = format!("project:{relative}");
    let found = treatment
        .skills()
        .iter()
        .find(|skill| skill.token == token && skill.sha256 == digest);
    if found.is_none() {
        return Err(FeasibilityError::Context {
            message: format!(
                "treatment manifest must discover `{token}` with the treatment skill digest"
            ),
        });
    }
    if treatment.sha256() == baseline.sha256() {
        return Err(FeasibilityError::Context {
            message: "treatment and baseline manifests must differ".to_string(),
        });
    }

    Ok(ConditionManifests {
        baseline,
        treatment,
        treatment_overlay: ConditionOverlay {
            entries: vec![OverlayEntry {
                token,
                action: OverlayAction::Add,
                sha256: Some(digest),
            }],
        },
    })
}

/// Resolve an effective-context manifest over a fresh empty root trio.
fn resolve_context(dir: &Path) -> Result<EffectiveContextManifest, FeasibilityError> {
    let home = dir.join("home");
    let xdg = dir.join("xdg-config");
    let project = dir.join("project");
    for root in [&home, &xdg, &project] {
        fs::create_dir_all(root).map_err(|error| io_error(root, error))?;
    }
    resolve_manifest(
        &ContextRoots {
            home: &home,
            xdg_config_home: &xdg,
            project_root: &project,
            worktree_root: &project,
        },
        &DiscoveryFlags::default(),
    )
    .map_err(Into::into)
}

/// Parse the frontmatter `name:` of a `SKILL.md` body (simple, documented).
fn skill_dir_name(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let name = value.trim();
            let valid = !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
            return valid.then(|| name.to_string());
        }
    }
    None
}

/// Open the harness and the shared prewarm assets for the configured run.
///
/// The fake harness needs neither. The real harness is digest-verified
/// against the pinned OpenCode `1.18.27` expectation before the first use
/// (unless the caller supplied a pre-opened harness through the documented
/// test seam), and the prewarm assets are validated up front so a missing
/// asset fails closed **before** the first trial starts.
fn prepare_harnesses(
    config: &FeasibilityConfig,
) -> Result<(HarnessChoice, Option<RealHarness>, Option<Prewarm>), FeasibilityError> {
    match config.harness {
        HarnessChoice::FakeAgent => Ok((HarnessChoice::FakeAgent, None, None)),
        HarnessChoice::OpenCodeReal => {
            let harness = match &config.real_harness_override {
                Some(harness) => harness.clone(),
                None => {
                    let real = config
                        .real
                        .as_ref()
                        .ok_or_else(|| FeasibilityError::Input {
                            path: config.evals_root.clone(),
                            message: "the real harness needs a `real` configuration (the \
                                  hash-verified OpenCode binary, the model, and the extra args)"
                                .to_string(),
                        })?;
                    RealHarness::open(real.clone()).map_err(|error| FeasibilityError::Input {
                        path: real.binary.clone(),
                        message: error.to_string(),
                    })?
                }
            };
            let root = config
                .prewarm_root
                .as_ref()
                .ok_or_else(|| FeasibilityError::Input {
                    path: config.evals_root.clone(),
                    message: "the real harness needs a `prewarm_root` (the shared warm npm \
                          cache, the config template, `models.json`, and the `rg` binary)"
                        .to_string(),
                })?;
            let prewarm = Prewarm::prepare(root).map_err(|error| FeasibilityError::Input {
                path: root.clone(),
                message: error.to_string(),
            })?;
            Ok((HarnessChoice::OpenCodeReal, Some(harness), Some(prewarm)))
        }
    }
}

/// The harness identity recorded in the plan lock and in the sealed evidence.
fn harness_identity(
    harness: HarnessChoice,
    tasks: &[LoadedTask],
    real_harness: Option<&RealHarness>,
) -> Result<HarnessIdentity, FeasibilityError> {
    match harness {
        HarnessChoice::FakeAgent => {
            let entries: Vec<(String, String)> = tasks
                .iter()
                .filter_map(|task| {
                    task.agent_sha256
                        .as_ref()
                        .map(|digest| (task.record.task_id.clone(), digest.clone()))
                })
                .collect();
            let agent_digest = combined_entries_digest("m5-fake-agent.v1", &entries)?;
            Ok(HarnessIdentity {
                name: FEASIBILITY_HARNESS_NAME.to_string(),
                version: M5_FAKE_HARNESS_VERSION.to_string(),
                binary_sha256: agent_digest,
            })
        }
        HarnessChoice::OpenCodeReal => Ok(real_harness
            .expect("the real harness is opened before the identity is derived")
            .identity()
            .clone()),
    }
}

/// Build the pinned experiment spec whose hash identifies the run.
fn build_experiment_spec(
    config: &FeasibilityConfig,
    tasks: &[LoadedTask],
    manifests: &ConditionManifests,
    identity: &HarnessIdentity,
) -> Result<ExperimentSpec, FeasibilityError> {
    let task_refs: Vec<BenchmarkTaskRef> = tasks
        .iter()
        .map(|task| BenchmarkTaskRef {
            task_id: task.record.task_id.clone(),
            version: u64::from(task.record.version),
            content_sha256: task.record.integrity.content_sha256.clone(),
        })
        .collect();
    let tree_digest = combined_entries_digest(
        "m5-fixture-tree.v1",
        &tasks
            .iter()
            .map(|task| {
                (
                    task.record.task_id.clone(),
                    task.fixture_tree_sha256.clone(),
                )
            })
            .collect::<Vec<_>>(),
    )?;
    Ok(ExperimentSpec {
        schema_version: EXPERIMENT_SPEC_SCHEMA_VERSION,
        experiment_id: config.experiment_id.clone(),
        tasks: task_refs,
        conditions: vec![
            ConditionSpec {
                id: M5_CONDITION_BASELINE.to_string(),
                expected_manifest_sha256: manifests.baseline.sha256().to_string(),
                overlay_present: false,
            },
            ConditionSpec {
                id: M5_CONDITION_TREATMENT.to_string(),
                expected_manifest_sha256: manifests.treatment.sha256().to_string(),
                overlay_present: true,
            },
        ],
        harness: HarnessLock {
            name: identity.name.clone(),
            version: identity.version.clone(),
            binary_sha256: identity.binary_sha256.clone(),
        },
        repository: RepositoryLock {
            commit: format!("{GENERATED_COMMIT_PREFIX}{tree_digest}"),
            tree_sha256: tree_digest,
        },
        created_at: M5_FIXED_CREATED_AT.to_string(),
        trial_timeout_seconds: config.trial_timeout_seconds,
    })
}

/// Canonical digest over `(key, digest)` pairs sorted by key, mixed with
/// `domain`.
fn combined_entries_digest(
    domain: &str,
    entries: &[(String, String)],
) -> Result<String, FeasibilityError> {
    let mut sorted = entries.to_vec();
    sorted.sort();
    let projection = json!({
        "domain": domain,
        "entries": sorted
            .iter()
            .map(|(key, digest)| json!([key, digest]))
            .collect::<Vec<_>>(),
    });
    let bytes = canonical_json_bytes(&projection)
        .map_err(|message| FeasibilityError::Internal { message })?;
    Ok(sha256_digest(&bytes))
}

// ---------------------------------------------------------------------------
// One trial
// ---------------------------------------------------------------------------

/// The trial id for a planned cell.
fn trial_id(planned: &PlannedTrial) -> String {
    format!(
        "t{:03}-{}-{}-r{}",
        planned.order_index, planned.task_id, planned.condition_id, planned.repetition
    )
}

/// Run one planned trial, setting and clearing the driver canary around it.
fn run_one_trial(
    context: &MatrixContext<'_>,
    task: &LoadedTask,
    planned: &PlannedTrial,
) -> Result<TrialOutcome, FeasibilityError> {
    let trial_id = trial_id(planned);
    let canary_value = format!("{M5_CANARY_VALUE_PREFIX}{trial_id}");
    std::env::set_var(M5_CANARY_ENV, &canary_value);
    let result = run_one_trial_inner(context, task, planned, &trial_id, &canary_value);
    std::env::remove_var(M5_CANARY_ENV);
    result
}

/// The per-trial pipeline body (see the module docs for the numbered steps).
fn run_one_trial_inner(
    context: &MatrixContext<'_>,
    task: &LoadedTask,
    planned: &PlannedTrial,
    trial_id: &str,
    canary_value: &str,
) -> Result<TrialOutcome, FeasibilityError> {
    let record = &task.record;

    // 1. Fixture source: declared bytes into a deterministic scratch Git repo.
    let source = context.sources_base.join(trial_id);
    write_fixture_source(record, &source)?;
    let (git_commit, git_tree) = git_commit_fixture(&source)?;

    // 2. Materialization with the condition's manifest and declared overlay.
    let (manifest, overlay) = if planned.condition_id == M5_CONDITION_TREATMENT {
        (
            context.manifests.treatment.clone(),
            context.manifests.treatment_overlay.clone(),
        )
    } else {
        (
            context.manifests.baseline.clone(),
            ConditionOverlay::default(),
        )
    };
    let trial_spec = TrialSpec {
        trial_id: trial_id.to_string(),
        experiment_id: context.config.experiment_id.clone(),
        task_id: record.task_id.clone(),
        task_version: u64::from(record.version),
        condition_id: planned.condition_id.clone(),
        manifest,
        overlay,
        plan_hash: context.plan_hash.to_string(),
        repository: RepositoryLock {
            commit: git_commit.clone(),
            tree_sha256: task.fixture_tree_sha256.clone(),
        },
        worktree_source: source.clone(),
    };
    let bindings = bindings_from_environment(&record.environment);

    // 3. Harness attempts. The real harness retries **once** (a provider
    //    error or a truncated stream) with a *fresh* materialized trial: the
    //    failed attempt's tree is removed in full, so the retry never
    //    observes the residue of the attempt it replaces.
    //
    //    The permission posture is the harness's own: ask-mode for the fake
    //    path (deterministic denial fixtures), allow-all for the real path
    //    (a non-interactive run auto-rejects every ask).
    let permission_seeding = context.harness.permission_seeding();
    let mut attempt_number = 0u32;
    let mut attempt_records: Vec<TrialAttemptRecord> = Vec::new();
    let attempt = loop {
        attempt_number += 1;
        let layout = materialize_trial_with_fake_tools(
            &trial_spec,
            context.trials_base,
            &bindings,
            permission_seeding,
        )?;
        let shell = trial_env::stage_verifier_shell(&layout.root).map_err(|error| {
            FeasibilityError::Trial {
                trial_id: trial_id.to_string(),
                message: error.to_string(),
            }
        })?;
        let prompt_path = layout.root.join("prompt.txt");
        fs::write(&prompt_path, &task.prompt_bytes)
            .map_err(|error| io_error(&prompt_path, error))?;
        let result = execute_harness(
            context,
            task,
            planned,
            &layout,
            &shell,
            &prompt_path,
            attempt_number,
        )?;
        let retried = result.retry.retryable && attempt_number <= REAL_HARNESS_MAX_RETRIES;
        // Every attempt is recorded — the superseded ones included — so a
        // retried trial is fully auditable.
        attempt_records.push(TrialAttemptRecord {
            index: attempt_number,
            termination: result.termination.as_str().to_string(),
            provider_error: result
                .provider_error
                .as_ref()
                .map(|detail| detail.marker.clone()),
            wall_ms: result.supervised.wall_ms,
            exit_code: result.supervised.exit_code,
            retry_reason: result
                .retry
                .reason
                .map(|reason| reason.as_str().to_string()),
            retried,
        });
        if retried {
            cleanup_trial(&result.layout).map_err(|error| FeasibilityError::Internal {
                message: format!("retry cleanup failed: {error}"),
            })?;
            continue;
        }
        break result;
    };
    let layout = attempt.layout;
    let supervised = attempt.supervised;
    let pre = attempt.pre;

    // 4. Post-agent snapshot, audit log, verifiers, acceptance.
    //
    //    The workspace observation excludes the harness's own seeded plugin
    //    tree (the real path only): it is present before the agent starts,
    //    it carries npm symlinks, and OpenCode's background npm activity
    //    inside it must never be read as agent activity.
    let excluded = excluded_workspace_paths(&layout);
    let post = capture_tree_snapshot_excluding(&layout.repo_dir, &excluded)?;
    let changes = diff_snapshots(&pre, &post);
    let audit_bytes =
        fs::read(&layout.audit_log).map_err(|error| io_error(&layout.audit_log, error))?;
    let audit = parse_audit_log(&audit_bytes)?;
    let verifier_env =
        hermetic_verifier_env(&layout.fake_bin_dir, trial_stub_bin_dir(&layout).as_deref());
    let mut outcomes: Vec<VerifierOutcome> = Vec::new();
    if let Some(verifier) = &record.verifier {
        for command in &verifier.commands {
            let mut spec = VerifierSpec::new(
                command.id.clone(),
                vec![VerifierStep {
                    argv: resolve_verifier_argv(&command.argv, Some(&attempt.shell)),
                    expected_exit: command.expected_exit,
                }],
            );
            spec.timeout_ms = u64::from(command.timeout_seconds).saturating_mul(1000);
            outcomes.push(run_verifier_excluding(
                &spec,
                &layout.repo_dir,
                &verifier_env,
                &excluded,
            ));
        }
    }
    let final_message = supervised
        .events
        .iter()
        .rev()
        .find_map(|event| event.text.as_deref());
    let builtin_started = Instant::now();
    let builtin_pre = tree_fingerprint_excluding(&layout.repo_dir, &excluded).unwrap_or_default();
    let assertion_report = evaluate_acceptance(
        record,
        &TrialEvidence {
            audit_log: Some(&audit),
            audit_log_bytes: Some(&audit_bytes),
            events: &supervised.events,
            final_message,
            verifiers: &outcomes,
            tree_root: Some(&layout.repo_dir),
            pre_snapshot: Some(&pre),
            post_snapshot: Some(&post),
            tree_changes: Some(&changes),
        },
    )?;
    let builtin_post = tree_fingerprint_excluding(&layout.repo_dir, &excluded).unwrap_or_default();
    let builtin_wall_ms = u64::try_from(builtin_started.elapsed().as_millis()).unwrap_or(u64::MAX);

    // 5b. Host-leakage detection (fail closed).
    //
    //     The trial's isolation is process- and environment-level, not
    //     OS-level: an agent may still name a host path directly. A trial
    //     whose tree carries host state measures the host, not the agent, so
    //     it is INVALID — never a pass and never a fail.
    let leakage = detect_host_leakage(
        &layout.root,
        &leakage_stub_dirs(&layout),
        &leakage_excluded_paths(&layout),
    )
    .map_err(|error| FeasibilityError::Trial {
        trial_id: trial_id.to_string(),
        message: error.to_string(),
    })?;

    // 6. Documented outcome fold: execution/verifier taxonomy, then the
    //    acceptance layer for completed runs.
    let verifier_overlay = outcomes
        .iter()
        .find(|outcome| {
            outcome.mutation_detected
                || outcome.timed_out
                || outcome.verdict == VerifierVerdict::Invalid
        })
        .or_else(|| outcomes.first());
    let taxonomy = classify_trial(&attempt.termination, verifier_overlay);
    let outcome_class = if !leakage.clean {
        // Contamination outranks every execution/verifier class: the trial
        // did not measure the agent.
        TrialOutcomeClass::HostContamination
    } else {
        match taxonomy {
            TrialOutcomeClass::CompletedPassed | TrialOutcomeClass::CompletedFailed => {
                if assertion_report.all_primary_passed {
                    TrialOutcomeClass::CompletedPassed
                } else {
                    TrialOutcomeClass::CompletedFailed
                }
            }
            other => other,
        }
    };
    let invalid_reason = if !leakage.clean {
        Some(contamination_reason(&leakage))
    } else {
        outcome_class
            .is_invalid()
            .then(|| invalid_reason_for(&taxonomy, attempt.termination, &outcomes))
    };

    // 7. Repo diff record for the seal.
    let diff = capture_diff(&layout.repo_dir, &DiffCaps::default())?;

    // 8. Redacted artifacts.
    let assertion_json = assertion_report_json(trial_id, &assertion_report)?;
    let permission_json = permission_config_json(permission_seeding)?;
    let harness_json = harness_identity_json(
        &context.identity,
        &attempt.requested_model,
        attempt.attempts,
        attempt.retry.reason.map(|reason| reason.as_str()),
        attempt.provider_error.as_ref(),
        &supervised.events,
    )?;
    let attempt_json = attempt_log_json(trial_id, &attempt_records)?;
    let contamination_json = contamination_report_json(&leakage)?;
    let policy_json = trial_policy_json(
        permission_seeding,
        record.budgets.wall_time_seconds_max,
        context.config.trial_timeout_seconds,
        effective_wall_timeout_seconds(
            record.budgets.wall_time_seconds_max,
            context.config.trial_timeout_seconds,
        ),
        context.prewarm.as_ref().map(|prewarm| prewarm.describe()),
    )?;
    let mut redactions: BTreeMap<String, u64> = BTreeMap::new();
    let mut artifact_refs: Vec<ArtifactRef> = vec![
        put_redacted(
            context.cas,
            "stdout",
            &supervised.raw_stdout,
            &mut redactions,
        )?,
        put_redacted(
            context.cas,
            "stderr",
            &supervised.raw_stderr,
            &mut redactions,
        )?,
        put_redacted(context.cas, "audit-log", &audit_bytes, &mut redactions)?,
        put_redacted(
            context.cas,
            "diff-text",
            diff.diff_text.as_bytes(),
            &mut redactions,
        )?,
        context.cas.put("assertion-report", &assertion_json)?,
        context.cas.put("harness-identity", &harness_json)?,
        // The permission posture of the trial: the effective set that was
        // seeded into the trial's project `opencode.json`.
        context.cas.put("permission-config", &permission_json)?,
        // Every attempt of the trial, the superseded ones included.
        context.cas.put("attempt-log", &attempt_json)?,
        // The host-leakage scan of the trial tree.
        context
            .cas
            .put("contamination-report", &contamination_json)?,
        // The trial's effective policy: permission set, wall cap, prewarm.
        context.cas.put("trial-policy", &policy_json)?,
    ];
    for outcome in &outcomes {
        let prefix = format!("verifier-{}-", outcome.verifier_id);
        artifact_refs.push(put_redacted(
            context.cas,
            &format!("{prefix}stdout"),
            &outcome.stdout,
            &mut redactions,
        )?);
        artifact_refs.push(put_redacted(
            context.cas,
            &format!("{prefix}stderr"),
            &outcome.stderr,
            &mut redactions,
        )?);
    }
    let redactions: Vec<RedactionEvent> = redactions
        .into_iter()
        .map(|(pattern, count)| RedactionEvent { pattern, count })
        .collect();

    // 9. Store rows: trial, execution, verifications, sealed evidence, class.
    let manifest_sha256 = trial_spec.manifest.sha256().to_string();
    context.store.record_trial(TrialRow {
        trial_id: trial_id.to_string(),
        experiment_id: context.config.experiment_id.clone(),
        task_id: record.task_id.clone(),
        task_version: u64::from(record.version),
        condition_id: planned.condition_id.clone(),
        repetition: u64::from(planned.repetition),
        order_index: u64::from(planned.order_index),
        plan_hash: context.plan_hash.to_string(),
        manifest_sha256: manifest_sha256.clone(),
        task_content_sha256: record.integrity.content_sha256.clone(),
        repository_commit: Some(git_commit.clone()),
        repository_tree_sha256: Some(task.fixture_tree_sha256.clone()),
        state: TrialState::Planned,
        outcome_class: None,
        invalid_reason: None,
    })?;
    let (usage_json, missing_telemetry_json) = usage_summary(&supervised.events);
    let session_ids_json =
        serde_json::to_string(&session_ids(&supervised.events)).map_err(|error| {
            FeasibilityError::Internal {
                message: format!("session id serialization failed: {error}"),
            }
        })?;
    context.store.record_execution(ExecutionRow {
        execution_id: format!("{trial_id}-exec"),
        trial_id: trial_id.to_string(),
        termination: attempt.termination,
        exit_code: supervised.exit_code,
        wall_ms: supervised.wall_ms,
        started_at_ms: attempt.started_at_ms,
        finished_at_ms: attempt.finished_at_ms,
        requested_model: attempt.requested_model.clone(),
        observed_model: None,
        raw_sha256: supervised.raw_bytes_sha256.clone(),
        events_sha256: crate::evidence::normalized_events_digest(&supervised.events)?,
        session_ids_json,
        usage_json,
        missing_telemetry_json,
    })?;
    context.store.record_verification(VerificationRow {
        verification_id: format!("{trial_id}-verify-builtin"),
        trial_id: trial_id.to_string(),
        verifier_id: "builtin:acceptance".to_string(),
        verdict: if assertion_report.all_primary_passed {
            VerifierVerdict::Pass
        } else {
            VerifierVerdict::Fail
        },
        mutation_detected: false,
        exit_codes_json: "[]".to_string(),
        stdout_sha256: sha256_digest(&assertion_json),
        stderr_sha256: sha256_digest(b""),
        pre_tree_sha256: builtin_pre,
        post_tree_sha256: builtin_post,
        wall_ms: builtin_wall_ms,
        timed_out: false,
    })?;
    for outcome in &outcomes {
        context.store.record_verification(VerificationRow {
            verification_id: format!("{trial_id}-verify-{}", outcome.verifier_id),
            trial_id: trial_id.to_string(),
            verifier_id: outcome.verifier_id.clone(),
            verdict: outcome.verdict,
            mutation_detected: outcome.mutation_detected,
            exit_codes_json: serde_json::to_string(&outcome.exit_codes).map_err(|error| {
                FeasibilityError::Internal {
                    message: format!("exit code serialization failed: {error}"),
                }
            })?,
            stdout_sha256: outcome.stdout_sha256.clone(),
            stderr_sha256: outcome.stderr_sha256.clone(),
            pre_tree_sha256: outcome.pre_tree_sha256.clone(),
            post_tree_sha256: outcome.post_tree_sha256.clone(),
            wall_ms: outcome.wall_ms,
            timed_out: outcome.timed_out,
        })?;
    }
    let execution_digest = ExecutionDigest::from_supervised(&supervised)?;
    let verifier_digest = (outcomes.len() == 1).then(|| VerifierDigest::from(&outcomes[0]));
    let manifest = seal_manifest(SealInputs {
        experiment_id: &context.config.experiment_id,
        trial_id,
        plan_hash: context.plan_hash,
        task_content_sha256: &record.integrity.content_sha256,
        manifest_sha256: &manifest_sha256,
        execution: &execution_digest,
        repo_diff_sha256: &diff.diff_sha256,
        verifier: verifier_digest.as_ref(),
        artifacts: &artifact_refs,
        redactions: &redactions,
        sealed_at_ms: unix_millis(),
    })?;
    context.store.record_evidence(&manifest)?;
    context
        .store
        .set_trial_outcome(trial_id, outcome_class, invalid_reason)?;

    // 10. Canary secret-hygiene scan.
    let mut canary = CanaryCheck {
        trial_id: trial_id.to_string(),
        clean: false,
        trial_files_scanned: 0,
        trial_bytes_scanned: 0,
        env_file_clean: true,
        raw_streams_clean: true,
        artifacts_scanned: 0,
        artifact_bytes_scanned: 0,
        hits: Vec::new(),
    };
    scan_tree_for_canary(&layout.root, canary_value.as_bytes(), &mut canary)?;
    canary.env_file_clean = !file_contains(&layout.env_file, canary_value.as_bytes())?;
    if !canary.env_file_clean {
        canary
            .hits
            .push(format!("env-file:{}", layout.env_file.display()));
    }
    canary.raw_streams_clean = !contains_bytes(&supervised.raw_stdout, canary_value.as_bytes())
        && !contains_bytes(&supervised.raw_stderr, canary_value.as_bytes());
    if !canary.raw_streams_clean {
        canary.hits.push("raw-stdout/stderr".to_string());
    }
    for reference in &manifest.artifacts {
        let bytes = context.cas.get(&reference.sha256)?;
        canary.artifacts_scanned += 1;
        canary.artifact_bytes_scanned += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if contains_bytes(&bytes, canary_value.as_bytes()) {
            canary
                .hits
                .push(format!("artifact:{}", reference.logical_name));
        }
    }
    canary.clean = canary.hits.is_empty();

    // 11. Cleanup: a contaminated trial tree is preserved for inspection.
    if canary.clean && leakage.clean {
        cleanup_trial(&layout).map_err(|error| FeasibilityError::Internal {
            message: error.to_string(),
        })?;
        fs::remove_dir_all(&source).map_err(|error| io_error(&source, error))?;
    }

    Ok(TrialOutcome {
        summary: TrialSummary {
            trial_id: trial_id.to_string(),
            order_index: planned.order_index,
            block_index: planned.block_index,
            block_position: planned.block_position,
            task_id: record.task_id.clone(),
            task_version: record.version,
            condition_id: planned.condition_id.clone(),
            repetition: planned.repetition,
            manifest_sha256,
            git_commit,
            git_tree,
            fixture_tree_sha256: task.fixture_tree_sha256.clone(),
            termination: attempt.termination.as_str().to_string(),
            outcome_class: outcome_class.as_str().to_string(),
            acceptance_all_passed: assertion_report.all_passed,
            acceptance_all_primary_passed: assertion_report.all_primary_passed,
            failed_assertions: assertion_report
                .failed_ids()
                .into_iter()
                .map(str::to_string)
                .collect(),
            verifier_ids: outcomes
                .iter()
                .map(|outcome| outcome.verifier_id.clone())
                .collect(),
            canary,
            harness_name: context.identity.name.clone(),
            harness_version: context.identity.version.clone(),
            harness_binary_sha256: context.identity.binary_sha256.clone(),
            requested_model: attempt.requested_model.clone(),
            observed_model: None,
            attempts: attempt.attempts,
            retry_reason: attempt
                .retry
                .reason
                .map(|reason| reason.as_str().to_string()),
            provider_error: attempt
                .provider_error
                .as_ref()
                .map(|detail| detail.marker.clone()),
            turn_state: match crate::events::turn_state(&supervised.events) {
                crate::events::TurnState::Completed { .. } => "completed".to_string(),
                crate::events::TurnState::Truncated { .. } => "truncated".to_string(),
            },
            permission_seeding: permission_seeding.as_str().to_string(),
            effective_permission: permission_seeding.effective_config().to_string(),
            effective_wall_timeout_seconds: effective_wall_timeout_seconds(
                record.budgets.wall_time_seconds_max,
                context.config.trial_timeout_seconds,
            ),
            prewarm: context.prewarm.as_ref().map(|prewarm| prewarm.describe()),
            attempt_records,
            host_leakage: leakage,
        },
        root: layout.root.clone(),
    })
}

// ---------------------------------------------------------------------------
// Harness execution
// ---------------------------------------------------------------------------

/// Run the configured harness once inside a materialized trial.
///
/// The fake harness runs the task's fake-agent script under the trial
/// allowlist environment; the real harness applies the real trial
/// environment (see [`crate::trial_env`]) and runs the pinned OpenCode CLI
/// with the task record's prompt as the message argument.
///
/// Both paths return the same shape, so everything downstream (acceptance,
/// evidence seal, store, report) is identical.
fn execute_harness(
    context: &MatrixContext<'_>,
    task: &LoadedTask,
    planned: &PlannedTrial,
    layout: &TrialLayout,
    shell: &Path,
    prompt_path: &Path,
    attempt_number: u32,
) -> Result<HarnessAttempt, FeasibilityError> {
    let record = &task.record;
    // The effective wall cap is `min(record budget, driver cap)`: the
    // record's budget always binds; the driver cap can only shorten it.
    let wall_timeout_ms = effective_wall_timeout_seconds(
        record.budgets.wall_time_seconds_max,
        context.config.trial_timeout_seconds,
    )
    .saturating_mul(1000);
    let supervisor_config = SupervisorConfig::new(wall_timeout_ms, M5_OUTPUT_CAP_BYTES);
    // The real harness seeds the trial with the prewarmed assets first: the
    // seeding is part of the materialization (it writes the project's
    // `.opencode/node_modules`), not part of the agent's execution, so the
    // pre-agent snapshot must be taken *after* it.
    let real_env = match context.harness {
        HarnessChoice::FakeAgent => None,
        HarnessChoice::OpenCodeReal => {
            let prewarm = context
                .prewarm
                .as_ref()
                .ok_or_else(|| FeasibilityError::Input {
                    path: context.config.evals_root.clone(),
                    message: "the real harness needs the prewarm assets".to_string(),
                })?;
            let env = trial_env::prepare_trial_env(layout, prewarm).map_err(|error| {
                FeasibilityError::Trial {
                    trial_id: layout.root.to_string_lossy().into_owned(),
                    message: error.to_string(),
                }
            })?;
            Some(env.into_iter().collect::<Vec<(String, String)>>())
        }
    };
    let pre = capture_tree_snapshot_excluding(&layout.repo_dir, &excluded_workspace_paths(layout))?;
    let started_at_ms = unix_millis();
    let attempt = match context.harness {
        HarnessChoice::FakeAgent => {
            let agent_path = task
                .agent_path
                .as_ref()
                .ok_or_else(|| FeasibilityError::Input {
                    path: context.config.agents_dir.clone(),
                    message: "the fake harness needs a per-task agent script".to_string(),
                })?;
            let mut env: Vec<(String, String)> = allowed_env(layout).into_iter().collect();
            env.push((
                "TRIAL_FAKE_AGENT_PROMPT".to_string(),
                prompt_path.to_string_lossy().into_owned(),
            ));
            env.push((
                "TRIAL_FAKE_AGENT_CONDITION".to_string(),
                planned.condition_id.clone(),
            ));
            let supervised = run_supervised(
                &[
                    "/bin/sh".to_string(),
                    agent_path.to_string_lossy().into_owned(),
                ],
                &env,
                &layout.repo_dir,
                &supervisor_config,
            );
            HarnessAttempt {
                layout: layout.clone(),
                shell: shell.to_path_buf(),
                pre,
                termination: supervised.termination,
                supervised,
                requested_model: M5_REQUESTED_MODEL.to_string(),
                provider_error: None,
                retry: RetryDecision::none("the fake harness is deterministic: it never retries"),
                attempts: attempt_number,
                started_at_ms,
                finished_at_ms: unix_millis(),
            }
        }
        HarnessChoice::OpenCodeReal => {
            let harness = context
                .real_harness
                .as_ref()
                .ok_or_else(|| FeasibilityError::Input {
                    path: context.config.evals_root.clone(),
                    message: "the real harness was not opened".to_string(),
                })?;
            let env = real_env.expect("the real environment is prepared above");
            let outcome = harness.run(&RealRunRequest {
                cwd: layout.repo_dir.clone(),
                env,
                prompt: prompt_text(&task.prompt_bytes),
                wall_timeout_ms: supervisor_config.wall_timeout_ms,
                output_cap_bytes: supervisor_config.output_cap_bytes,
            });
            HarnessAttempt {
                layout: layout.clone(),
                shell: shell.to_path_buf(),
                pre,
                termination: outcome.termination,
                supervised: outcome.supervised,
                requested_model: outcome.requested_model,
                provider_error: outcome.provider_error,
                retry: outcome.retry,
                attempts: attempt_number,
                started_at_ms,
                finished_at_ms: unix_millis(),
            }
        }
    };
    Ok(attempt)
}

/// The workspace subtrees the driver excludes from every observation.
///
/// The project's OpenCode config dir (`<trial>/repo/.opencode`) is the
/// harness's own directory inside the agent's workspace: it holds the
/// driver-seeded permission config and project skill, the seeded plugin
/// install target (the second plugin install target of probe §A.6 — it is
/// what keeps the registry traffic at zero bytes per trial), and the files
/// OpenCode and its npm client write into every config dir. It is present
/// before the agent starts, it carries npm symlinks, and the harness's
/// background npm activity inside it must never be read as agent activity.
///
/// The exclusion is **harness-independent** (both paths observe the same
/// workspace definition) and it is applied to the tree snapshot, the tree
/// fingerprint, and the verifier's mutation check; the git-based change
/// capture hides the same directory through a repository-local exclude
/// rule.
fn excluded_workspace_paths(layout: &TrialLayout) -> Vec<PathBuf> {
    vec![trial_env::project_config_dir(layout)]
}

/// The trial directories the host-leakage scan checks for oversized files.
///
/// The trial's fake-bin holds the fake shims; the stub-bin holds the
/// binaries the fake `root install` writes. Both are tiny by construction, so
/// a host binary in either of them is a leak.
fn leakage_stub_dirs(layout: &TrialLayout) -> Vec<PathBuf> {
    let mut dirs = vec![layout.fake_bin_dir.clone()];
    if let Some(stub_bin) = trial_stub_bin_dir(layout) {
        dirs.push(stub_bin);
    }
    dirs
}

/// The trial directories the host-leakage scan never enters.
///
/// They are the harness's **own** trees — the project's OpenCode config dir
/// (the seeded plugin install target carries npm's `.bin` symlinks, which are
/// relative and resolve inside the trial) and the global OpenCode config
/// clone. They are excluded from the workspace observation for exactly the
/// same reason.
fn leakage_excluded_paths(layout: &TrialLayout) -> Vec<PathBuf> {
    vec![
        trial_env::project_config_dir(layout),
        layout.xdg_config_home.join("opencode"),
    ]
}

/// The effective wall-time cap of one trial, in seconds.
///
/// `min(record budget, driver cap)`: the record's
/// `budgets.wall_time_seconds_max` always binds, and the driver-level cap
/// can only make a trial **shorter**. With no driver cap the record's budget
/// applies unchanged.
fn effective_wall_timeout_seconds(record_budget_seconds: u32, driver_cap: Option<u64>) -> u64 {
    let budget = u64::from(record_budget_seconds);
    match driver_cap {
        Some(cap) => budget.min(cap),
        None => budget,
    }
}

/// The bounded, deterministic invalid reason of a contaminated trial.
fn contamination_reason(leakage: &LeakageReport) -> String {
    let mut reason = String::from("host_contamination:");
    for finding in &leakage.findings {
        reason.push(' ');
        reason.push_str(&finding.token());
    }
    while reason.len() > crate::store::MAX_EVAL_STORE_ID_LEN {
        reason.pop();
    }
    reason
}

/// The prompt text handed to the harness as the message argument.
///
/// The record's prompt bytes (inline or file) with the trailing newline
/// removed: the NDJSON contract's prompt is a single message argument.
fn prompt_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

/// Canonical JSON bytes of the permission posture recorded with every trial.
///
/// The artifact binds the posture token (`ask_mode` / `allow_all`) to the
/// effective permission set that was seeded into the trial's project
/// `opencode.json`, so a run's permission posture is auditable from the
/// sealed evidence alone.
fn permission_config_json(permission: PermissionSeeding) -> Result<Vec<u8>, FeasibilityError> {
    let effective: Value =
        serde_json::from_str(permission.effective_config()).map_err(|error| {
            FeasibilityError::Internal {
                message: format!("the effective permission set is not valid JSON: {error}"),
            }
        })?;
    let projection = json!({
        "domain": "permission-config.v1",
        "seeding": permission.as_str(),
        "effective_permission": effective,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

/// Canonical JSON bytes of every harness attempt of one trial.
///
/// The artifact records **every** attempt — the superseded ones included —
/// with its index, its terminal classification, the provider-error marker it
/// hit, its wall time, its exit code, and the retry decision that followed
/// it. A retried trial is therefore fully auditable from the sealed evidence
/// alone: the reason the first attempt was discarded is recoverable, not
/// only the final attempt's outcome.
fn attempt_log_json(
    trial_id: &str,
    attempts: &[TrialAttemptRecord],
) -> Result<Vec<u8>, FeasibilityError> {
    let records: Vec<Value> = attempts
        .iter()
        .map(|attempt| {
            json!({
                "index": attempt.index,
                "termination": attempt.termination,
                "provider_error": attempt.provider_error,
                "wall_ms": attempt.wall_ms,
                "exit_code": attempt.exit_code,
                "retry_reason": attempt.retry_reason,
                "retried": attempt.retried,
            })
        })
        .collect();
    let projection = json!({
        "domain": "attempt-log.v1",
        "trial_id": trial_id,
        "attempts": records,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

/// Canonical JSON bytes of the host-leakage scan of one trial tree.
///
/// The artifact names every finding (its kind, its trial-relative path, and
/// what was observed), so a contaminated trial is auditable from the sealed
/// evidence alone.
fn contamination_report_json(leakage: &LeakageReport) -> Result<Vec<u8>, FeasibilityError> {
    let projection = json!({
        "domain": "contamination-report.v1",
        "clean": leakage.clean,
        "symlinks_scanned": leakage.symlinks_scanned,
        "stub_files_scanned": leakage.stub_files_scanned,
        "findings": leakage.findings,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

/// Canonical JSON bytes of the trial's effective policy.
///
/// The artifact binds the trial's semantics together: the permission posture
/// and the effective permission set that was seeded into the trial, the
/// record's own wall budget, the driver-level cap, the **effective** cap
/// (`min(record budget, driver cap)`), and the prewarm assets the real
/// harness cloned.
fn trial_policy_json(
    permission: PermissionSeeding,
    record_wall_time_seconds_max: u32,
    driver_trial_timeout_seconds: Option<u64>,
    effective_wall_timeout_seconds: u64,
    prewarm: Option<String>,
) -> Result<Vec<u8>, FeasibilityError> {
    let effective: Value =
        serde_json::from_str(permission.effective_config()).map_err(|error| {
            FeasibilityError::Internal {
                message: format!("the effective permission set is not valid JSON: {error}"),
            }
        })?;
    let projection = json!({
        "domain": "trial-policy.v1",
        "permission_seeding": permission.as_str(),
        "effective_permission": effective,
        "record_wall_time_seconds_max": record_wall_time_seconds_max,
        "driver_trial_timeout_seconds": driver_trial_timeout_seconds,
        "effective_wall_timeout_seconds": effective_wall_timeout_seconds,
        "wall_timeout_rule": "effective = min(record budget, driver cap)",
        "prewarm": prewarm,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

/// Canonical JSON bytes of the harness identity recorded with every
/// execution.
///
/// The artifact binds the harness name, the exact version, the digest of the
/// binary that produced the run, the requested model (and the M0 policy note
/// that the observed model stays unavailable), the number of attempts, the
/// retry/provider-error markers, and how the turn ended.
fn harness_identity_json(
    identity: &HarnessIdentity,
    requested_model: &str,
    attempts: u32,
    retry_reason: Option<&str>,
    provider_error: Option<&ProviderErrorDetail>,
    events: &[NormalizedEvent],
) -> Result<Vec<u8>, FeasibilityError> {
    let turn = match crate::events::turn_state(events) {
        crate::events::TurnState::Completed { .. } => "completed",
        crate::events::TurnState::Truncated { .. } => "truncated",
    };
    let projection = json!({
        "domain": "harness-identity.v1",
        "name": identity.name,
        "version": identity.version,
        "binary_sha256": identity.binary_sha256,
        "requested_model": requested_model,
        "observed_model": Value::Null,
        "observed_model_limitation": crate::events::MODEL_OBSERVED_LIMITATION,
        "attempts": attempts,
        "retry_reason": retry_reason,
        "provider_error": provider_error.map(|detail| json!({
            "marker": detail.marker,
            "source": detail.source,
            "excerpt": detail.excerpt,
        })),
        "turn_state": turn,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

// ---------------------------------------------------------------------------
// Fixture source
// ---------------------------------------------------------------------------

/// Write the record's declared `generated` fixture files into `dir`.
fn write_fixture_source(record: &TaskRecord, dir: &Path) -> Result<(), FeasibilityError> {
    let Fixture::Generated(fixture) = &record.fixture else {
        return Err(FeasibilityError::TaskRecord {
            path: dir.to_path_buf(),
            message: "only generated fixtures are supported".to_string(),
        });
    };
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    let mut total_bytes = 0u64;
    for file in &fixture.files {
        if sha256_digest(file.content.as_bytes()) != file.content_sha256 {
            return Err(FeasibilityError::TaskRecord {
                path: dir.to_path_buf(),
                message: format!("declared content digest mismatch for `{}`", file.path),
            });
        }
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::write(&path, file.content.as_bytes()).map_err(|error| io_error(&path, error))?;
        total_bytes += u64::try_from(file.content.len()).unwrap_or(u64::MAX);
    }
    if total_bytes > fixture.max_tree_bytes {
        return Err(FeasibilityError::TaskRecord {
            path: dir.to_path_buf(),
            message: format!(
                "fixture is {total_bytes} bytes, over the declared {} cap",
                fixture.max_tree_bytes
            ),
        });
    }
    Ok(())
}

/// `git init` + `add` + deterministic `commit`; returns `(commit, tree)`.
fn git_commit_fixture(source: &Path) -> Result<(String, String), FeasibilityError> {
    run_fixture_git(&["init", "--quiet"], source)?;
    run_fixture_git(&["add", "--all"], source)?;
    run_fixture_git(
        &[
            "-c",
            "user.name=Root Eval",
            "-c",
            "user.email=eval@root.invalid",
            "commit",
            "--quiet",
            "-m",
            "m5 fixture",
        ],
        source,
    )?;
    let commit = run_fixture_git(&["rev-parse", "HEAD"], source)?;
    let tree = run_fixture_git(&["rev-parse", "HEAD^{tree}"], source)?;
    Ok((commit, tree))
}

/// Run one bounded, hermetic `git` child for fixture construction.
///
/// The child environment is cleared and rebuilt from `PATH` plus a fixed
/// identity and date, so fixture commits are deterministic and no parent
/// variable (including the driver canary) can cross into Git.
fn run_fixture_git(args: &[&str], cwd: &Path) -> Result<String, FeasibilityError> {
    let mut command = Command::new("git");
    command.arg("--no-pager").args(args).current_dir(cwd);
    command.env_clear();
    command.env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", null_device());
    command.env("GIT_AUTHOR_NAME", "Root Eval");
    command.env("GIT_AUTHOR_EMAIL", "eval@root.invalid");
    command.env("GIT_AUTHOR_DATE", M5_FIXTURE_GIT_DATE);
    command.env("GIT_COMMITTER_NAME", "Root Eval");
    command.env("GIT_COMMITTER_EMAIL", "eval@root.invalid");
    command.env("GIT_COMMITTER_DATE", M5_FIXTURE_GIT_DATE);
    command.env("LC_ALL", "C");
    let output = command.output().map_err(|error| FeasibilityError::Git {
        args: args.join(" "),
        message: format!("failed to spawn git: {error}"),
    })?;
    if !output.status.success() {
        return Err(FeasibilityError::Git {
            args: args.join(" "),
            message: format!(
                "exit {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(unix)]
fn null_device() -> &'static str {
    "/dev/null"
}

#[cfg(not(unix))]
fn null_device() -> &'static str {
    "NUL"
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Redact `bytes`, accumulate redaction counts, and store the result.
fn put_redacted(
    cas: &ArtifactStore,
    logical_name: &str,
    bytes: &[u8],
    redactions: &mut BTreeMap<String, u64>,
) -> Result<ArtifactRef, FeasibilityError> {
    let (redacted, events) = redact_bytes(bytes);
    for event in events {
        *redactions.entry(event.pattern).or_insert(0) += event.count;
    }
    Ok(cas.put(logical_name, &redacted)?)
}

/// Canonical JSON bytes of an assertion report (the built-in verification
/// digest input and the `assertion-report` artifact).
fn assertion_report_json(
    trial_id: &str,
    report: &AssertionReport,
) -> Result<Vec<u8>, FeasibilityError> {
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|result| {
            json!({
                "id": result.id,
                "kind": result.kind,
                "primary": result.primary,
                "passed": result.passed,
                "detail": result.detail,
            })
        })
        .collect();
    let projection = json!({
        "domain": "assertion-report.v1",
        "trial_id": trial_id,
        "all_passed": report.all_passed,
        "all_primary_passed": report.all_primary_passed,
        "results": results,
    });
    canonical_json_bytes(&projection).map_err(|message| FeasibilityError::Internal { message })
}

/// Short, deterministic invalid-trial reason (bounded to the store's id cap).
fn invalid_reason_for(
    class: &TrialOutcomeClass,
    termination: TerminationKind,
    outcomes: &[VerifierOutcome],
) -> String {
    let ids = outcomes
        .iter()
        .map(|outcome| outcome.verifier_id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let notes = outcomes
        .iter()
        .flat_map(|outcome| outcome.notes.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let mut reason = format!(
        "{}: termination={}, verifiers=[{}]",
        class.as_str(),
        termination.as_str(),
        ids
    );
    if !notes.is_empty() {
        reason.push_str("; ");
        reason.push_str(&notes);
    }
    while reason.len() > crate::store::MAX_EVAL_STORE_ID_LEN {
        reason.pop();
    }
    reason
}

/// Unique harness session ids in first-seen event order.
fn session_ids(events: &[NormalizedEvent]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for event in events {
        if !ids.contains(&event.session_id) {
            ids.push(event.session_id.clone());
        }
    }
    ids
}

/// Usage totals from `step_finish` events plus the missing-telemetry list.
///
/// Missing telemetry is named, never zero-filled (the M3 rule).
fn usage_summary(events: &[NormalizedEvent]) -> (String, String) {
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut cost = 0.0f64;
    let mut saw_step_finish = false;
    let mut saw_tokens = false;
    let mut saw_cost = false;
    for event in events {
        if event.source_event_type != SOURCE_STEP_FINISH {
            continue;
        }
        saw_step_finish = true;
        if let Some(tokens) = event.tokens {
            saw_tokens = true;
            tokens_in = tokens_in.saturating_add(tokens.input);
            tokens_out = tokens_out.saturating_add(tokens.output);
        }
        if let Some(value) = event.cost {
            saw_cost = true;
            cost += value;
        }
    }
    let mut missing = vec!["observed_model".to_string()];
    if !saw_step_finish {
        missing.push("step_finish".to_string());
    }
    if !saw_tokens {
        missing.push("tokens.input".to_string());
        missing.push("tokens.output".to_string());
    }
    if !saw_cost {
        missing.push("cost".to_string());
    }
    if events.is_empty() {
        missing.push("events".to_string());
    }
    let usage = json!({
        "tokens_in": tokens_in,
        "tokens_out": tokens_out,
        "cost": cost,
    });
    (
        usage.to_string(),
        serde_json::to_string(&missing).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// Substring search over bytes.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Read a file and check for the needle.
fn file_contains(path: &Path, needle: &[u8]) -> Result<bool, FeasibilityError> {
    let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
    Ok(contains_bytes(&bytes, needle))
}

/// Recursively scan every regular file under `dir` for the canary.
///
/// Symlinks are skipped; non-regular entries are skipped; a file over
/// [`MAX_CANARY_FILE_BYTES`] is recorded as a hit (fail closed: an unread
/// file cannot be certified clean).
fn scan_tree_for_canary(
    dir: &Path,
    needle: &[u8],
    check: &mut CanaryCheck,
) -> Result<(), FeasibilityError> {
    let entries = fs::read_dir(dir).map_err(|error| io_error(dir, error))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(dir, error))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            scan_tree_for_canary(&path, needle, check)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        check.trial_files_scanned += 1;
        if metadata.len() > MAX_CANARY_FILE_BYTES {
            check
                .hits
                .push(format!("oversized-file:{}", path.display()));
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        check.trial_bytes_scanned += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if contains_bytes(&bytes, needle) {
            check.hits.push(format!("trial-file:{}", path.display()));
        }
    }
    Ok(())
}

/// Distinct/disjoint/contained assertions over the materialized trial roots.
fn check_roots(
    roots: &[PathBuf],
    trials_base: &Path,
    store_dir: &Path,
) -> (bool, bool, bool, bool) {
    let distinct: BTreeSet<&PathBuf> = roots.iter().collect();
    let roots_distinct = distinct.len() == roots.len();

    let mut roots_pairwise_disjoint = true;
    'outer: for (index, left) in roots.iter().enumerate() {
        for right in roots.iter().skip(index + 1) {
            if left.starts_with(right) || right.starts_with(left) {
                roots_pairwise_disjoint = false;
                break 'outer;
            }
        }
    }

    let roots_contained = roots.iter().all(|root| root.starts_with(trials_base));
    let roots_outside_store = roots
        .iter()
        .all(|root| !root.starts_with(store_dir) && !store_dir.starts_with(root));
    (
        roots_distinct,
        roots_pairwise_disjoint,
        roots_contained,
        roots_outside_store,
    )
}

/// `kind:path` token for one residue entry.
fn residue_token(residue: &Residue) -> String {
    match residue {
        Residue::TrialDir { path } => format!("trial-dir:{}", path.display()),
        Residue::StaleLock { path } => format!("stale-lock:{}", path.display()),
        Residue::LeftoverProcessMarker { path } => format!("leftover-process:{}", path.display()),
    }
}

/// `(completed_passed, completed_failed)` for one condition.
fn condition_counts(report: &crate::report::FeasibilityReport, condition_id: &str) -> (u32, u32) {
    report
        .condition_counts
        .iter()
        .find(|counts| counts.condition_id == condition_id)
        .map(|counts| (counts.completed_passed, counts.completed_failed))
        .unwrap_or((0, 0))
}

/// Current Unix time in milliseconds.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
