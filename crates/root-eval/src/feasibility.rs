//! Milestone 5 feasibility-matrix driver (deterministic fake agents).
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
//! - **harness** — the documented feasibility-only [`FEASIBILITY_HARNESS_NAME`]
//!   lock, version [`M5_FAKE_HARNESS_VERSION`], `binary_sha256` =
//!   `m5-fake-agent.v1` combined digest over the per-task fake-agent script
//!   bytes (sorted by task id);
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
//!    the prompt is written to `<trial>/prompt.txt`; a pre-agent tree
//!    snapshot is captured.
//! 3. **Execution.** The fake agent script runs under [`run_supervised`] with
//!    the trial allowlist environment plus `TRIAL_FAKE_AGENT_PROMPT` /
//!    `TRIAL_FAKE_AGENT_CONDITION`, cwd = the trial repository, wall timeout
//!    from the record's `budgets.wall_time_seconds_max`, output cap
//!    [`M5_OUTPUT_CAP_BYTES`].
//! 4. **Verification.** A post-agent snapshot and diff are captured, the
//!    fake-tool audit log is parsed, every `[verifier.commands]` entry runs
//!    through [`run_verifier`] (PATH = fake-bin + stub-bin + host PATH last),
//!    and [`evaluate_acceptance`] folds all evidence into an
//!    [`AssertionReport`].
//! 5. **Outcome mapping.** See below.
//! 6. **Evidence.** Raw stdout, raw stderr, the audit log, the diff text, the
//!    canonical assertion-report JSON, and every verifier's stdout/stderr are
//!    redacted ([`crate::redact_bytes`], pre-persistence per M3) and stored in
//!    the content-addressed artifact store; [`seal_manifest`] binds them with
//!    the execution digest and the captured [`RepoDiff::diff_sha256`].
//! 7. **Store rows.** One `trials` row, one `executions` row, one
//!    verification row for the built-in acceptance engine
//!    (`verifier_id = "builtin:acceptance"`, verdict = `all_primary_passed`,
//!    `exit_codes_json = []`, `stdout_sha256` = SHA-256 of the canonical
//!    assertion-report JSON), one verification row per external verifier
//!    outcome, the sealed evidence manifest, and the outcome class.
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
//! # Process-spawning sites
//!
//! This module is the third documented exception to the crate's no-spawn
//! rule: bounded `git` children (fixture `init`/`add`/`commit`/`rev-parse`,
//! `env_clear` + fixed identity) plus everything the materializer,
//! supervisor, and verifier already spawn. Nothing here reads the ambient
//! environment except `PATH` (for `git` and the host verifier PATH tail).
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
    capture_tree_snapshot, diff_snapshots, evaluate_acceptance, parse_audit_log, AssertionReport,
    TrialEvidence,
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
use crate::materializer::{allowed_env, materialize_trial_with_fake_tools, trial_stub_bin_dir};
use crate::plan::{
    compute_plan_hash, BenchmarkTaskRef, ConditionSpec, ExperimentSpec, HarnessLock,
    RepositoryLock, EXPERIMENT_SPEC_SCHEMA_VERSION, FEASIBILITY_HARNESS_NAME,
    GENERATED_COMMIT_PREFIX,
};
use crate::report::{build_feasibility_report, ReportVerdict};
use crate::schedule::{generate_schedule, PlannedTrial, ScheduleSpec, ScheduleTaskRef};
use crate::schema::{Fixture, Prompt, TaskRecord};
use crate::seal::tree_digest_v1;
use crate::store::{EvalStore, ExecutionRow, ExperimentRow, TrialRow, TrialState, VerificationRow};
use crate::supervisor::{run_supervised, SupervisorConfig, TerminationKind};
use crate::taxonomy::{classify_trial, TrialOutcomeClass};
use crate::trial::{TrialLayout, TrialSpec};
use crate::verifier::{
    run_verifier, tree_fingerprint, VerifierOutcome, VerifierSpec, VerifierStep, VerifierVerdict,
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
}

impl FeasibilityConfig {
    /// Build a config with the recorded defaults (`R = 3`, seed `881204`,
    /// marker treatment skill).
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
    agent_path: PathBuf,
    agent_sha256: String,
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
}

/// One processed trial: its summary and materialized root.
struct TrialOutcome {
    summary: TrialSummary,
    root: PathBuf,
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
    let plan = build_experiment_spec(config, &tasks, &manifests)?;
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
    };
    let contamination_clean = contamination.canary_clean
        && contamination.roots_distinct
        && contamination.roots_pairwise_disjoint
        && contamination.roots_contained
        && contamination.roots_outside_store
        && contamination.residue_clean;

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
                 residue_clean={}",
                summary_path.display(),
                summary.contamination.canary_clean,
                summary.contamination.roots_distinct,
                summary.contamination.roots_pairwise_disjoint,
                summary.contamination.roots_contained,
                summary.contamination.roots_outside_store,
                summary.contamination.residue_clean,
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
        let agent_path = config.agents_dir.join(format!("{stem}.sh"));
        let agent_bytes = fs::read(&agent_path).map_err(|error| FeasibilityError::Input {
            path: agent_path.clone(),
            message: format!("fake agent script: {error}"),
        })?;
        tasks.push(LoadedTask {
            record,
            agent_path,
            agent_sha256: sha256_digest(&agent_bytes),
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

/// Build the pinned experiment spec whose hash identifies the run.
fn build_experiment_spec(
    config: &FeasibilityConfig,
    tasks: &[LoadedTask],
    manifests: &ConditionManifests,
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
    let agent_digest = combined_entries_digest(
        "m5-fake-agent.v1",
        &tasks
            .iter()
            .map(|task| (task.record.task_id.clone(), task.agent_sha256.clone()))
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
            name: FEASIBILITY_HARNESS_NAME.to_string(),
            version: M5_FAKE_HARNESS_VERSION.to_string(),
            binary_sha256: agent_digest,
        },
        repository: RepositoryLock {
            commit: format!("{GENERATED_COMMIT_PREFIX}{tree_digest}"),
            tree_sha256: tree_digest,
        },
        created_at: M5_FIXED_CREATED_AT.to_string(),
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
    let layout = materialize_trial_with_fake_tools(&trial_spec, context.trials_base, &bindings)?;

    // 3. Prompt file and pre-agent snapshot.
    let prompt_path = layout.root.join("prompt.txt");
    fs::write(&prompt_path, &task.prompt_bytes).map_err(|error| io_error(&prompt_path, error))?;
    let pre = capture_tree_snapshot(&layout.repo_dir)?;

    // 4. Supervised fake-agent execution.
    let mut env: Vec<(String, String)> = allowed_env(&layout).into_iter().collect();
    env.push((
        "TRIAL_FAKE_AGENT_PROMPT".to_string(),
        prompt_path.to_string_lossy().into_owned(),
    ));
    env.push((
        "TRIAL_FAKE_AGENT_CONDITION".to_string(),
        planned.condition_id.clone(),
    ));
    let supervisor_config = SupervisorConfig::new(
        u64::from(record.budgets.wall_time_seconds_max).saturating_mul(1000),
        M5_OUTPUT_CAP_BYTES,
    );
    let started_at_ms = unix_millis();
    let supervised = run_supervised(
        &[
            "/bin/sh".to_string(),
            task.agent_path.to_string_lossy().into_owned(),
        ],
        &env,
        &layout.repo_dir,
        &supervisor_config,
    );
    let finished_at_ms = unix_millis();

    // 5. Post-agent snapshot, audit log, verifiers, acceptance.
    let post = capture_tree_snapshot(&layout.repo_dir)?;
    let changes = diff_snapshots(&pre, &post);
    let audit_bytes =
        fs::read(&layout.audit_log).map_err(|error| io_error(&layout.audit_log, error))?;
    let audit = parse_audit_log(&audit_bytes)?;
    let verifier_env = verifier_environment(&layout);
    let mut outcomes: Vec<VerifierOutcome> = Vec::new();
    if let Some(verifier) = &record.verifier {
        for command in &verifier.commands {
            let mut spec = VerifierSpec::new(
                command.id.clone(),
                vec![VerifierStep {
                    argv: command.argv.clone(),
                    expected_exit: command.expected_exit,
                }],
            );
            spec.timeout_ms = u64::from(command.timeout_seconds).saturating_mul(1000);
            outcomes.push(run_verifier(&spec, &layout.repo_dir, &verifier_env));
        }
    }
    let final_message = supervised
        .events
        .iter()
        .rev()
        .find_map(|event| event.text.as_deref());
    let builtin_started = Instant::now();
    let builtin_pre = tree_fingerprint(&layout.repo_dir).unwrap_or_default();
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
    let builtin_post = tree_fingerprint(&layout.repo_dir).unwrap_or_default();
    let builtin_wall_ms = u64::try_from(builtin_started.elapsed().as_millis()).unwrap_or(u64::MAX);

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
    let taxonomy = classify_trial(&supervised.termination, verifier_overlay);
    let outcome_class = match taxonomy {
        TrialOutcomeClass::CompletedPassed | TrialOutcomeClass::CompletedFailed => {
            if assertion_report.all_primary_passed {
                TrialOutcomeClass::CompletedPassed
            } else {
                TrialOutcomeClass::CompletedFailed
            }
        }
        other => other,
    };
    let invalid_reason = outcome_class
        .is_invalid()
        .then(|| invalid_reason_for(&taxonomy, supervised.termination, &outcomes));

    // 7. Repo diff record for the seal.
    let diff = capture_diff(&layout.repo_dir, &DiffCaps::default())?;

    // 8. Redacted artifacts.
    let assertion_json = assertion_report_json(trial_id, &assertion_report)?;
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
        termination: supervised.termination,
        exit_code: supervised.exit_code,
        wall_ms: supervised.wall_ms,
        started_at_ms,
        finished_at_ms,
        requested_model: M5_REQUESTED_MODEL.to_string(),
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
    if canary.clean {
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
            termination: supervised.termination.as_str().to_string(),
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
        },
        root: layout.root.clone(),
    })
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

/// The stricter verifier environment: fake-bin + stub-bin + host `PATH` last.
fn verifier_environment(layout: &TrialLayout) -> Vec<(String, String)> {
    let mut path = layout.fake_bin_dir.to_string_lossy().into_owned();
    if let Some(stub_bin) = trial_stub_bin_dir(layout) {
        path.push(':');
        path.push_str(&stub_bin.to_string_lossy());
    }
    path.push(':');
    path.push_str(&std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()));
    vec![("PATH".to_string(), path)]
}

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
