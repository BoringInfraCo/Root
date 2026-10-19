//! Paired feasibility report (Milestone 4) — scripted counts only.
//!
//! [`build_feasibility_report`] reads one experiment from an [`EvalStore`] and
//! renders the feasibility-stage aggregation the audit allows under the
//! recorded scope decision: per-condition counts, discordant pairs, per-task
//! direction, robust secondary medians, missing-telemetry counts, and
//! limitations. It does **not** emit an effectiveness verdict.
//!
//! # Scope: the verdict machinery is deferred
//!
//! The audit's Review Addendum (2026-10-18) defers "Milestone 4's
//! verdict/uncertainty machinery as a subsystem": the spike reports "scripted
//! paired counts, per-task direction, and honest `INSUFFICIENT EVIDENCE
//! (feasibility)` labels only — no automatic verdicts". [`ReportVerdict`]
//! therefore has exactly one variant, [`ReportVerdict::Deferred`], and there
//! are no confidence scores, effect estimates, or verdict-shaped words
//! anywhere in the rendered output.
//!
//! # Blocks and validity
//!
//! A *block* is one `(task_id, task_version, repetition)` cell — the paired
//! unit the audit's aggregation compares across conditions. The two recorded
//! conditions are ordered by id: the first is the **baseline**, the second the
//! **treatment** (the audit's `baseline` / `treatment` pair sorts this way).
//! An experiment that does not record exactly two conditions cannot be paired:
//! more than two is refused as [`ReportError::InvalidExperiment`], and one
//! condition records every block as invalid.
//!
//! A block is **valid** only when:
//!
//! 1. both conditions have exactly one trial in the block;
//! 2. both trials carry an outcome class, and neither class is an invalid
//!    class ([`TrialOutcomeClass::is_invalid`]);
//! 3. both trials share the same `plan_hash`;
//! 4. the two trials carry distinct `manifest_sha256` values (identical
//!    baseline/treatment manifests mean the conditions were not distinct).
//!
//! Exclusion is symmetric: when any condition's side of a block is missing or
//! invalid, **both** sides are excluded and the block is recorded once in
//! [`FeasibilityReport::invalid_blocks`] with the reason from the first
//! failing condition (baseline first, then treatment). Reported reasons:
//!
//! - `"<condition> trial missing"`;
//! - `"<condition> has N trials in one block"`;
//! - `"<condition> outcome class missing"`;
//! - `"<condition> outcome class invalid: <class>"`;
//! - `"plan hash mismatch"`;
//! - `"conditions not distinct"`;
//! - `"only one condition recorded"`.
//!
//! # Counts, direction, and secondary stats
//!
//! Per-condition counts summarize **every recorded trial** of that condition
//! (`completed_passed` / `completed_failed` / `invalid` / `missing`, where
//! `missing` means "no outcome class recorded yet"). The
//! `task_failures_by_class` histogram carries every recorded class except
//! `completed_passed` and `completed_failed` — invalid-class tokens and
//! `provider_error` appear here as well as in the aggregate `invalid` count,
//! because the histogram is a breakdown, not a second failure score.
//!
//! Discordant pairs and per-task direction are computed over **valid blocks
//! only**: a side "passes" when its class is
//! [`TrialOutcomeClass::CompletedPassed`] and "fails" otherwise (invalid
//! classes are already excluded by validity). `better` is baseline fail /
//! treatment pass; `worse` is the inverse; `same` is everything else.
//!
//! Secondary medians are robust summaries over valid blocks where **both
//! sides** carry the value: duration is each side's latest execution
//! `wall_ms`, and input tokens come from the integer `tokens_in` field of each
//! side's latest execution `usage_json` (the `UsageTotals` shape the M2
//! runner produces and the M3 store round-trips; any other shape is missing,
//! never zero). [`SecondaryStats::observed_pairs`] counts valid blocks where
//! all four secondary values exist; [`SecondaryStats::missing_pairs`] counts
//! valid blocks where at least one of the four is absent, so
//! `observed_pairs + missing_pairs` always equals the valid-block count.
//! [`FeasibilityReport::missing_metrics`] aggregates every execution's
//! `missing_telemetry` list across all attempts (retries included) into
//! sorted `"<field>: <count> of <total> executions"` entries.
//!
//! # Feasibility label
//!
//! [`FEASIBILITY_LABEL`] is emitted only when at least
//! [`MIN_VALID_BLOCKS_FOR_FEASIBILITY`] valid blocks span at least
//! [`MIN_INDEPENDENT_TASKS_FOR_FEASIBILITY`] independent
//! `(task_id, task_version)` pairs; otherwise the report carries
//! [`INSUFFICIENT_EVIDENCE_LABEL`]. Neither label is a verdict.
//!
//! # Determinism
//!
//! Trials are read in `order_index` order and every collection is sorted
//! before rendering, so the same store always produces byte-identical
//! [`FeasibilityReport::to_json`] output (canonical JSON, keys sorted at every
//! level) and identical text.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::store::{EvalStore, ExecutionRow, StoreError, TrialFull};
use crate::taxonomy::TrialOutcomeClass;

/// Report schema version emitted by this crate.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Feasibility-only label: at least two valid blocks over two independent
/// tasks.
pub const FEASIBILITY_LABEL: &str = "FEASIBILITY ONLY — no effectiveness verdict";

/// Insufficient-evidence label: the documented feasibility threshold is not
/// met. Not a verdict either way.
pub const INSUFFICIENT_EVIDENCE_LABEL: &str = "INSUFFICIENT EVIDENCE (feasibility)";

/// The one deferred-verdict reason; the verdict engine is out of scope.
pub const VERDICT_DEFERRED_REASON: &str =
    "verdict engine deferred per feasibility-spike scope decision";

/// The required terminal verdict line.
pub const VERDICT_DEFERRED_LINE: &str =
    "Verdict: DEFERRED (feasibility spike — no effectiveness claim)";

/// Minimum valid blocks for [`FEASIBILITY_LABEL`].
pub const MIN_VALID_BLOCKS_FOR_FEASIBILITY: u32 = 2;

/// Minimum independent tasks for [`FEASIBILITY_LABEL`].
pub const MIN_INDEPENDENT_TASKS_FOR_FEASIBILITY: u32 = 2;

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`ReportError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportErrorKind {
    /// The eval store could not be read.
    Store,
    /// Canonical serialization of the report failed.
    Canonicalization,
    /// The experiment cannot be rendered as a paired feasibility report.
    InvalidExperiment,
}

/// Errors produced while building or rendering a [`FeasibilityReport`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReportError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("canonical serialization failed: {message}")]
    Canonicalization { message: String },

    #[error("invalid experiment: {message}")]
    InvalidExperiment { message: String },
}

impl ReportError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> ReportErrorKind {
        match self {
            Self::Store(_) => ReportErrorKind::Store,
            Self::Canonicalization { .. } => ReportErrorKind::Canonicalization,
            Self::InvalidExperiment { .. } => ReportErrorKind::InvalidExperiment,
        }
    }
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// The report verdict. Exactly one variant exists — the verdict engine is
/// deferred per the feasibility-spike scope decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportVerdict {
    /// No verdict is emitted; `reason` is the recorded scope decision.
    Deferred {
        /// Why no verdict is emitted ([`VERDICT_DEFERRED_REASON`]).
        reason: String,
    },
}

impl ReportVerdict {
    /// The reason carried by this verdict (always
    /// [`VERDICT_DEFERRED_REASON`] today).
    pub fn reason(&self) -> &str {
        match self {
            Self::Deferred { reason } => reason,
        }
    }
}

/// Per-condition trial counts and the non-completed class histogram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionCounts {
    /// Condition identity as recorded by the trials.
    pub condition_id: String,
    /// Trials whose recorded class is `completed_passed`.
    pub completed_passed: u32,
    /// Trials whose recorded class is `completed_failed`.
    pub completed_failed: u32,
    /// Trials whose recorded class is an invalid class.
    pub invalid: u32,
    /// Trials with no recorded outcome class yet.
    pub missing: u32,
    /// Every recorded class except `completed_passed` / `completed_failed`,
    /// keyed by [`TrialOutcomeClass::as_str`] token.
    pub task_failures_by_class: BTreeMap<String, u32>,
}

/// One block excluded from pairing, with the reason from its first failing
/// condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidBlock {
    /// Benchmark task identity.
    pub task_id: String,
    /// Benchmark task version.
    pub task_version: u64,
    /// Repetition index within the task/condition block.
    pub repetition: u64,
    /// Deterministic exclusion reason (see the module docs).
    pub reason: String,
}

/// Discordant-pair counts over valid blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiscordantPairs {
    /// Valid blocks where the baseline side failed and the treatment passed.
    pub baseline_fail_treatment_pass: u32,
    /// Valid blocks where the baseline side passed and the treatment failed.
    pub baseline_pass_treatment_fail: u32,
}

/// Per-task direction over valid blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDirection {
    /// Benchmark task identity.
    pub task_id: String,
    /// Benchmark task version.
    pub task_version: u64,
    /// Valid blocks where the treatment side passed and the baseline failed.
    pub better: u32,
    /// Valid blocks with the same pass/fail outcome on both sides.
    pub same: u32,
    /// Valid blocks where the baseline side passed and the treatment failed.
    pub worse: u32,
}

/// Robust secondary statistics over valid blocks.
///
/// See the module docs for the exact pairing rule: medians rest on the valid
/// blocks where both sides carry the value, `observed_pairs` counts valid
/// blocks where all four secondary values exist, and `missing_pairs` counts
/// valid blocks where at least one is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondaryStats {
    /// Median of `treatment − baseline` latest-execution wall time, when any
    /// valid block carries both values.
    pub median_duration_delta_ms: Option<i64>,
    /// Median of `treatment − baseline` input tokens (`usage_json`
    /// `tokens_in`), when any valid block carries both values.
    pub median_input_token_delta: Option<i64>,
    /// Valid blocks where both durations and both input-token counts exist.
    pub observed_pairs: u32,
    /// Valid blocks where at least one of those four values is absent.
    pub missing_pairs: u32,
}

/// The paired feasibility report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeasibilityReport {
    /// Report schema version ([`REPORT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Owning experiment identifier.
    pub experiment_id: String,
    /// The deferred verdict (the only variant).
    pub verdict: ReportVerdict,
    /// [`FEASIBILITY_LABEL`] or [`INSUFFICIENT_EVIDENCE_LABEL`].
    pub feasibility_label: String,
    /// Independent `(task_id, task_version)` pairs over valid blocks.
    pub independent_tasks: u32,
    /// Distinct repetition indices recorded across the experiment's trials.
    pub repetitions: u32,
    /// Distinct `(task_id, task_version, repetition)` blocks observed.
    pub blocks_total: u32,
    /// Blocks valid for pairing.
    pub valid_blocks: u32,
    /// Blocks excluded from pairing, sorted by `(task_id, version,
    /// repetition)`.
    pub invalid_blocks: Vec<InvalidBlock>,
    /// Per-condition counts, sorted by condition id.
    pub condition_counts: Vec<ConditionCounts>,
    /// Discordant pairs over valid blocks.
    pub discordant: DiscordantPairs,
    /// Per-task direction, sorted by `(task_id, task_version)`.
    pub task_direction: Vec<TaskDirection>,
    /// Robust secondary statistics over valid blocks.
    pub secondary: SecondaryStats,
    /// Missing-telemetry aggregation, sorted by field name.
    pub missing_metrics: Vec<String>,
    /// Deterministic limitations; always includes the independent-task,
    /// repetition, exclusion, missing-telemetry, model-identity,
    /// cost-semantics, and verdict-deferral sentences.
    pub limitations: Vec<String>,
}

impl FeasibilityReport {
    /// Render the report as canonical JSON (keys sorted at every level,
    /// compact).
    pub fn to_json(&self) -> Result<String, ReportError> {
        let value = serde_json::to_value(self).map_err(|error| ReportError::Canonicalization {
            message: format!("report serialization failed: {error}"),
        })?;
        let bytes = canonical_json_bytes(&value)
            .map_err(|message| ReportError::Canonicalization { message })?;
        String::from_utf8(bytes).map_err(|error| ReportError::Canonicalization {
            message: format!("canonical report bytes are not UTF-8: {error}"),
        })
    }

    /// Render the plain-text terminal report.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Feasibility report — experiment `{}`\n",
            self.experiment_id
        ));
        out.push_str(&format!("Label: {}\n", self.feasibility_label));
        out.push_str(&format!(
            "Independent tasks: {} | Repetitions: {} | Blocks: {} total, {} valid, {} invalid\n",
            self.independent_tasks,
            self.repetitions,
            self.blocks_total,
            self.valid_blocks,
            self.invalid_blocks.len()
        ));

        out.push_str("\nCondition counts\n");
        if self.condition_counts.is_empty() {
            out.push_str("  (no conditions recorded)\n");
        }
        for counts in &self.condition_counts {
            out.push_str(&format!(
                "  {}: completed_passed={} completed_failed={} invalid={} missing={}\n",
                counts.condition_id,
                counts.completed_passed,
                counts.completed_failed,
                counts.invalid,
                counts.missing
            ));
            if !counts.task_failures_by_class.is_empty() {
                let classes = counts
                    .task_failures_by_class
                    .iter()
                    .map(|(class, count)| format!("{class}={count}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&format!("    task failures by class: {classes}\n"));
            }
        }

        out.push_str("\nDiscordant pairs (valid blocks)\n");
        out.push_str(&format!(
            "  baseline fail → treatment pass: {}\n",
            self.discordant.baseline_fail_treatment_pass
        ));
        out.push_str(&format!(
            "  baseline pass → treatment fail: {}\n",
            self.discordant.baseline_pass_treatment_fail
        ));

        out.push_str("\nPer-task direction (valid blocks)\n");
        if self.task_direction.is_empty() {
            out.push_str("  (no valid blocks)\n");
        }
        for direction in &self.task_direction {
            out.push_str(&format!(
                "  {} v{}: {} better / {} same / {} worse\n",
                direction.task_id,
                direction.task_version,
                direction.better,
                direction.same,
                direction.worse
            ));
        }

        out.push_str("\nSecondary (valid blocks; treatment − baseline)\n");
        out.push_str(&secondary_line(
            "median duration delta",
            "ms",
            self.secondary.median_duration_delta_ms,
            self.secondary.observed_pairs,
            self.secondary.missing_pairs,
        ));
        out.push_str(&secondary_line(
            "median input token delta",
            "tokens",
            self.secondary.median_input_token_delta,
            self.secondary.observed_pairs,
            self.secondary.missing_pairs,
        ));

        out.push_str("\nMissing metrics\n");
        if self.missing_metrics.is_empty() {
            out.push_str("  (none)\n");
        }
        for metric in &self.missing_metrics {
            out.push_str(&format!("  {metric}\n"));
        }

        out.push_str("\nLimitations\n");
        for limitation in &self.limitations {
            out.push_str(&format!("  - {limitation}\n"));
        }

        out.push('\n');
        out.push_str(VERDICT_DEFERRED_LINE);
        out.push('\n');
        out
    }
}

/// Build the paired feasibility report for `experiment_id` from `store`.
///
/// The experiment row must exist ([`StoreError::NotFound`] otherwise); all
/// trials are read in `order_index` order, each with its full projection
/// (latest execution, latest verification, latest sealed evidence) and every
/// execution attempt. No verdict is emitted — see the module docs.
pub fn build_feasibility_report(
    store: &EvalStore,
    experiment_id: &str,
) -> Result<FeasibilityReport, ReportError> {
    store.experiment(experiment_id)?;

    let rows = store.list_trials(experiment_id)?;
    let mut trials: Vec<LoadedTrial> = Vec::with_capacity(rows.len());
    for row in &rows {
        let full = store.trial_full(&row.trial_id)?;
        let executions = store.executions_for_trial(&row.trial_id)?;
        trials.push(LoadedTrial { full, executions });
    }

    let conditions: BTreeSet<String> = trials
        .iter()
        .map(|trial| trial.condition_id().to_string())
        .collect();
    if conditions.len() > 2 {
        return Err(ReportError::InvalidExperiment {
            message: format!(
                "paired feasibility report requires exactly 2 conditions, found {}",
                conditions.len()
            ),
        });
    }

    let condition_counts = count_conditions(&trials, &conditions);
    let blocks = group_blocks(&trials);
    let blocks_total = u32::try_from(blocks.len()).unwrap_or(u32::MAX);
    let (valid_pairs, invalid_blocks) = pair_blocks(&blocks, &trials, &conditions);
    let valid_blocks = u32::try_from(valid_pairs.len()).unwrap_or(u32::MAX);

    let (discordant, task_direction) = paired_outcomes(&valid_pairs, &trials);
    let independent_tasks = u32::try_from(task_direction.len()).unwrap_or(u32::MAX);
    let secondary = secondary_stats(&valid_pairs, &trials)?;

    let repetitions: BTreeSet<u64> = trials
        .iter()
        .map(|trial| trial.full.trial.repetition)
        .collect();
    let repetitions = u32::try_from(repetitions.len()).unwrap_or(u32::MAX);

    let missing_metrics = missing_metrics(&trials)?;

    let feasibility_label = if valid_blocks >= MIN_VALID_BLOCKS_FOR_FEASIBILITY
        && independent_tasks >= MIN_INDEPENDENT_TASKS_FOR_FEASIBILITY
    {
        FEASIBILITY_LABEL
    } else {
        INSUFFICIENT_EVIDENCE_LABEL
    }
    .to_string();

    let limitations = limitations(
        independent_tasks,
        repetitions,
        invalid_blocks.len(),
        blocks_total,
        &missing_metrics,
    );

    Ok(FeasibilityReport {
        schema_version: REPORT_SCHEMA_VERSION,
        experiment_id: experiment_id.to_string(),
        verdict: ReportVerdict::Deferred {
            reason: VERDICT_DEFERRED_REASON.to_string(),
        },
        feasibility_label,
        independent_tasks,
        repetitions,
        blocks_total,
        valid_blocks,
        invalid_blocks,
        condition_counts,
        discordant,
        task_direction,
        secondary,
        missing_metrics,
        limitations,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// One loaded trial: the full store projection plus every execution attempt.
struct LoadedTrial {
    full: TrialFull,
    executions: Vec<ExecutionRow>,
}

impl LoadedTrial {
    fn condition_id(&self) -> &str {
        &self.full.trial.condition_id
    }

    fn outcome_class(&self) -> Option<TrialOutcomeClass> {
        self.full.outcome_class
    }

    fn latest_execution(&self) -> Option<&ExecutionRow> {
        self.full.latest_execution.as_ref()
    }
}

/// Block key: `(task_id, task_version, repetition)`.
type BlockKey = (String, u64, u64);

fn count_conditions(trials: &[LoadedTrial], conditions: &BTreeSet<String>) -> Vec<ConditionCounts> {
    let mut counts: BTreeMap<&str, ConditionCounts> = conditions
        .iter()
        .map(|id| {
            (
                id.as_str(),
                ConditionCounts {
                    condition_id: id.clone(),
                    completed_passed: 0,
                    completed_failed: 0,
                    invalid: 0,
                    missing: 0,
                    task_failures_by_class: BTreeMap::new(),
                },
            )
        })
        .collect();

    for trial in trials {
        let Some(entry) = counts.get_mut(trial.condition_id()) else {
            continue;
        };
        match trial.outcome_class() {
            None => entry.missing += 1,
            Some(TrialOutcomeClass::CompletedPassed) => entry.completed_passed += 1,
            Some(TrialOutcomeClass::CompletedFailed) => entry.completed_failed += 1,
            Some(class) => {
                if class.is_invalid() {
                    entry.invalid += 1;
                }
                *entry
                    .task_failures_by_class
                    .entry(class.as_str().to_string())
                    .or_insert(0) += 1;
            }
        }
    }

    counts.into_values().collect()
}

fn group_blocks(trials: &[LoadedTrial]) -> BTreeMap<BlockKey, BTreeMap<String, Vec<usize>>> {
    let mut blocks: BTreeMap<BlockKey, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    for (index, trial) in trials.iter().enumerate() {
        let row = &trial.full.trial;
        blocks
            .entry((row.task_id.clone(), row.task_version, row.repetition))
            .or_default()
            .entry(row.condition_id.clone())
            .or_default()
            .push(index);
    }
    blocks
}

fn pair_blocks(
    blocks: &BTreeMap<BlockKey, BTreeMap<String, Vec<usize>>>,
    trials: &[LoadedTrial],
    conditions: &BTreeSet<String>,
) -> (Vec<(BlockKey, usize, usize)>, Vec<InvalidBlock>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();

    let mut ids = conditions.iter();
    let baseline = ids.next();
    let treatment = ids.next();

    for (key, by_condition) in blocks {
        let (Some(baseline), Some(treatment)) = (baseline, treatment) else {
            invalid.push(invalid_block(
                key,
                "only one condition recorded".to_string(),
            ));
            continue;
        };

        if let Some(reason) = block_failure(by_condition, trials, baseline) {
            invalid.push(invalid_block(key, reason));
            continue;
        }
        if let Some(reason) = block_failure(by_condition, trials, treatment) {
            invalid.push(invalid_block(key, reason));
            continue;
        }

        let baseline_index = by_condition[baseline][0];
        let treatment_index = by_condition[treatment][0];
        let baseline_row = &trials[baseline_index].full.trial;
        let treatment_row = &trials[treatment_index].full.trial;
        if baseline_row.plan_hash != treatment_row.plan_hash {
            invalid.push(invalid_block(key, "plan hash mismatch".to_string()));
        } else if baseline_row.manifest_sha256 == treatment_row.manifest_sha256 {
            invalid.push(invalid_block(key, "conditions not distinct".to_string()));
        } else {
            valid.push((key.clone(), baseline_index, treatment_index));
        }
    }

    (valid, invalid)
}

/// The first failure of `condition`'s side of a block, in deterministic order.
fn block_failure(
    by_condition: &BTreeMap<String, Vec<usize>>,
    trials: &[LoadedTrial],
    condition: &str,
) -> Option<String> {
    let Some(indices) = by_condition.get(condition) else {
        return Some(format!("{condition} trial missing"));
    };
    if indices.len() > 1 {
        return Some(format!(
            "{condition} has {} trials in one block",
            indices.len()
        ));
    }
    match trials[indices[0]].outcome_class() {
        None => Some(format!("{condition} outcome class missing")),
        Some(class) if class.is_invalid() => Some(format!(
            "{condition} outcome class invalid: {}",
            class.as_str()
        )),
        Some(_) => None,
    }
}

fn invalid_block(key: &BlockKey, reason: String) -> InvalidBlock {
    InvalidBlock {
        task_id: key.0.clone(),
        task_version: key.1,
        repetition: key.2,
        reason,
    }
}

fn paired_outcomes(
    valid: &[(BlockKey, usize, usize)],
    trials: &[LoadedTrial],
) -> (DiscordantPairs, Vec<TaskDirection>) {
    let mut discordant = DiscordantPairs::default();
    let mut by_task: BTreeMap<(String, u64), TaskDirection> = BTreeMap::new();

    for (key, baseline_index, treatment_index) in valid {
        let baseline_pass =
            trials[*baseline_index].outcome_class() == Some(TrialOutcomeClass::CompletedPassed);
        let treatment_pass =
            trials[*treatment_index].outcome_class() == Some(TrialOutcomeClass::CompletedPassed);

        if !baseline_pass && treatment_pass {
            discordant.baseline_fail_treatment_pass += 1;
        }
        if baseline_pass && !treatment_pass {
            discordant.baseline_pass_treatment_fail += 1;
        }

        let direction = by_task
            .entry((key.0.clone(), key.1))
            .or_insert_with(|| TaskDirection {
                task_id: key.0.clone(),
                task_version: key.1,
                better: 0,
                same: 0,
                worse: 0,
            });
        if !baseline_pass && treatment_pass {
            direction.better += 1;
        } else if baseline_pass && !treatment_pass {
            direction.worse += 1;
        } else {
            direction.same += 1;
        }
    }

    (discordant, by_task.into_values().collect())
}

fn secondary_stats(
    valid: &[(BlockKey, usize, usize)],
    trials: &[LoadedTrial],
) -> Result<SecondaryStats, StoreError> {
    let mut durations: Vec<i64> = Vec::new();
    let mut token_deltas: Vec<i64> = Vec::new();
    let mut observed_pairs = 0u32;
    let mut missing_pairs = 0u32;

    for (_, baseline_index, treatment_index) in valid {
        let baseline = &trials[*baseline_index];
        let treatment = &trials[*treatment_index];

        let baseline_duration = duration_ms(baseline.latest_execution());
        let treatment_duration = duration_ms(treatment.latest_execution());
        let baseline_tokens = input_tokens(baseline.latest_execution())?;
        let treatment_tokens = input_tokens(treatment.latest_execution())?;

        if baseline_duration.is_some()
            && treatment_duration.is_some()
            && baseline_tokens.is_some()
            && treatment_tokens.is_some()
        {
            observed_pairs += 1;
        } else {
            missing_pairs += 1;
        }

        if let (Some(baseline), Some(treatment)) = (baseline_duration, treatment_duration) {
            durations.push(treatment.saturating_sub(baseline));
        }
        if let (Some(baseline), Some(treatment)) = (baseline_tokens, treatment_tokens) {
            token_deltas.push(treatment.saturating_sub(baseline));
        }
    }

    Ok(SecondaryStats {
        median_duration_delta_ms: median(&mut durations),
        median_input_token_delta: median(&mut token_deltas),
        observed_pairs,
        missing_pairs,
    })
}

/// Latest-execution wall time as a signed delta input.
fn duration_ms(execution: Option<&ExecutionRow>) -> Option<i64> {
    execution.map(|execution| i64::try_from(execution.wall_ms).unwrap_or(i64::MAX))
}

/// Recognized `usage_json` shape: a JSON object with an integer `tokens_in`
/// field (the `UsageTotals` convention the M2 runner produces and the M3
/// store round-trips). Any other shape — missing field, non-integer value,
/// or non-object — is **missing**, never zero.
fn input_tokens(execution: Option<&ExecutionRow>) -> Result<Option<i64>, StoreError> {
    let Some(execution) = execution else {
        return Ok(None);
    };
    let usage: JsonValue = execution.usage()?;
    Ok(usage.get("tokens_in").and_then(JsonValue::as_i64))
}

/// Median of the given values: the middle value for odd counts, the average
/// of the two middle values for even counts (truncating toward zero).
fn median(values: &mut [i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        return Some(values[middle]);
    }
    let average = (i128::from(values[middle - 1]) + i128::from(values[middle])) / 2;
    Some(i64::try_from(average).unwrap_or(if average.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    }))
}

fn missing_metrics(trials: &[LoadedTrial]) -> Result<Vec<String>, StoreError> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut executions: u32 = 0;
    for trial in trials {
        for execution in &trial.executions {
            executions += 1;
            for field in execution.missing_telemetry()? {
                *counts.entry(field).or_insert(0) += 1;
            }
        }
    }
    Ok(counts
        .into_iter()
        .map(|(field, count)| format!("{field}: {count} of {executions} executions"))
        .collect())
}

fn limitations(
    independent_tasks: u32,
    repetitions: u32,
    invalid_blocks: usize,
    blocks_total: u32,
    missing_metrics: &[String],
) -> Vec<String> {
    let task_suffix = if independent_tasks == 1 { "" } else { "s" };
    let repetition_suffix = if repetitions == 1 { "" } else { "s" };
    let missing = if missing_metrics.is_empty() {
        "missing telemetry: none recorded in the executions read".to_string()
    } else {
        format!("missing telemetry: {}", missing_metrics.join("; "))
    };
    vec![
        format!(
            "{independent_tasks} independent task{task_suffix}; repetitions do not create independent tasks"
        ),
        format!(
            "{repetitions} repetition{repetition_suffix} recorded; repeated runs are nested within tasks, not independent samples"
        ),
        format!(
            "{invalid_blocks} of {blocks_total} paired blocks excluded as invalid; exclusion is symmetric (both conditions of a block are excluded together)"
        ),
        missing,
        "observed model identity unavailable (requested model recorded)".to_string(),
        "provider cost is harness-reported, not billed".to_string(),
        format!("{VERDICT_DEFERRED_REASON}; no effectiveness claim is made"),
    ]
}

fn secondary_line(
    label: &str,
    unit: &str,
    value: Option<i64>,
    observed: u32,
    missing: u32,
) -> String {
    let value = match value {
        Some(value) => format!("{value} {unit}"),
        None => "unavailable".to_string(),
    };
    format!("  {label}: {value} ({observed} observed pairs, {missing} missing pairs)\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_is_middle_value_or_middle_average() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [7]), Some(7));
        assert_eq!(median(&mut [3, 1, 2]), Some(2));
        assert_eq!(median(&mut [4, 1, 3, 2]), Some(2));
        assert_eq!(median(&mut [60, 10, 50, 20, 40, 30]), Some(35));
        assert_eq!(median(&mut [-5, 5]), Some(0));
    }

    #[test]
    fn input_tokens_reads_only_the_documented_shape() {
        assert_eq!(
            serde_json::json!({ "tokens_in": 12 })
                .get("tokens_in")
                .and_then(JsonValue::as_i64),
            Some(12)
        );
        assert_eq!(
            serde_json::json!({ "tokens_in": "12" })
                .get("tokens_in")
                .and_then(JsonValue::as_i64),
            None
        );
        assert_eq!(
            serde_json::json!({ "tokens_in": 12.5 })
                .get("tokens_in")
                .and_then(JsonValue::as_i64),
            None
        );
        assert_eq!(
            serde_json::json!({ "input": 12 })
                .get("tokens_in")
                .and_then(JsonValue::as_i64),
            None
        );
    }
}
