//! Deterministic, balanced trial ordering for an experiment (Milestone 4).
//!
//! [`generate_schedule`] implements the scheduler primitive the audit
//! requires before any execution:
//!
//! - "Experiment Design" → "Scale and order": *"Generate the order before
//!   execution using a recorded seed and balance condition order within each
//!   task/repetition block."*
//! - Milestone 4 exit criterion: *"deterministic schedule from seed."*
//! - `Docs/Eval/benchmarks/TASK-SPEC.md` §4.2: `task × repetition` blocks
//!   whose condition order is owned by the experiment (the recorded order
//!   seed), never by the task record.
//!
//! The function is pure: it reads no environment, no clock, and no
//! filesystem, and it spawns no processes. The same [`ScheduleSpec`]
//! (including `seed`) always yields the same `Vec<PlannedTrial>`, stable
//! under `serde` serialization.
//!
//! # Blocks, slots, and indices
//!
//! A *block* is one `(task, repetition)` pair — the paired unit the audit's
//! aggregation compares across conditions. Blocks are first enumerated in
//! *slot* order: for each task in declaration order, for each repetition
//! `0..repetitions`, so slot `k = task_index * repetitions + repetition`.
//! The slot index is reported as [`PlannedTrial::block_index`]; it identifies
//! the block and is stable across seeds.
//!
//! Execution order is a Fisher–Yates shuffle of the slots. Slots are visited
//! in shuffled order and each emits one trial per condition;
//! [`PlannedTrial::order_index`] is the flat 0-based global sequence
//! (`order_index = execution_position * condition_count + block_position`),
//! and [`PlannedTrial::block_position`] is the 0-based position of the trial
//! inside its block.
//!
//! # Deterministic PRNG
//!
//! The module embeds SplitMix64 (no external dependency): a 64-bit state
//! seeded with `seed`, advanced by the fixed golden-ratio increment
//! `0x9E37_79B9_7F4A_7C15`, and mixed with the published constants
//! `0xBF58_476D_1CE4_E5B9` and `0x94D0_49BB_1331_11EB`. Draws are consumed in
//! a fixed order: `blocks - 1` draws for the shuffle, then one draw for the
//! rotation offset. Bounded draws use modulo reduction (`draw % bound`); the
//! residual bias is bounded by `bound / 2^64` and is irrelevant for schedule
//! construction (no security boundary is claimed). Every `u64` seed is
//! accepted; there is no seed-space restriction.
//!
//! # Balance rule and residues
//!
//! Within a block the condition order is a rotation of the spec's condition
//! order: for slot `k`, position `p` runs condition
//! `(k + offset + p) mod condition_count`, where `offset` is the PRNG draw
//! taken after the shuffle. Position 0 therefore carries condition
//! `(k + offset) mod condition_count`.
//!
//! The rotation is keyed to the slot index — the block's identity — not to
//! the block's position in the shuffled execution order. That is what makes
//! position-0 balance hold *per task* and not merely in aggregate: a task's
//! slots form the contiguous window `t*R .. t*R + R`.
//!
//! Over all blocks every slot `0..blocks` appears exactly once, so condition
//! `c` is first in `floor(blocks / C)` blocks, plus one when
//! `(c - offset) mod C < blocks mod C`. When `C` divides `blocks`,
//! position-0 assignment is exactly even.
//!
//! Per task each condition is first `floor(R / C)` or `ceil(R / C)` times,
//! and the extra position-0 slots fall on the first `R mod C` repetitions.
//! With two conditions: `R` even → each condition first exactly `R / 2` times
//! per task; `R` odd → `(R + 1) / 2` versus `(R - 1) / 2`, with the majority
//! condition `(t*R + offset) mod 2` alternating between adjacent tasks.
//!
//! # Validation and limits
//!
//! Validation is fail-closed, in this order: non-empty `experiment_id`; at
//! least one task; no duplicate `(task_id, version)`; at least two
//! conditions; at most [`MAX_CONDITIONS`]; non-empty and unique condition
//! ids; at least one repetition; at most [`MAX_TRIALS`] planned trials.
//!
//! `block_position` is a `u8`, so a schedule carries at most
//! [`MAX_CONDITIONS`] conditions. `order_index` is a `u32`, so a schedule
//! carries at most [`MAX_TRIALS`] trials; larger inputs are refused with
//! [`ScheduleErrorKind::TooManyTrials`] rather than truncated.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest accepted number of conditions (`u8::MAX + 1`).
///
/// Positions inside a block run `0..condition_count` and are reported as
/// `u8` ([`PlannedTrial::block_position`]); the largest representable
/// position is `u8::MAX`, so the count is capped at `u8::MAX + 1`.
pub const MAX_CONDITIONS: usize = 256;

/// Largest accepted number of planned trials (`2^32`).
///
/// `order_index` is a `u32`; the largest 0-based index is `MAX_TRIALS - 1`.
pub const MAX_TRIALS: u64 = u32::MAX as u64 + 1;

/// SplitMix64 state increment (the 64-bit golden-ratio gamma).
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// First SplitMix64 mix constant.
const SPLITMIX_MIX_A: u64 = 0xBF58_476D_1CE4_E5B9;

/// Second SplitMix64 mix constant.
const SPLITMIX_MIX_B: u64 = 0x94D0_49BB_1331_11EB;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// One benchmark task pinned into the schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTaskRef {
    /// Task identifier as carried by the task record.
    pub task_id: String,
    /// Task record version.
    pub version: u32,
}

/// Everything the scheduler needs: the task set, the conditions, the
/// repetition count, and the recorded order seed.
///
/// All inputs are caller-supplied values; the type carries no hidden state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleSpec {
    /// Owning experiment identifier (non-empty).
    pub experiment_id: String,
    /// Benchmark tasks in scope, in declaration order.
    pub tasks: Vec<ScheduleTaskRef>,
    /// Condition ids in scope, in declaration order (at least two, unique).
    pub conditions: Vec<String>,
    /// Repetitions per task (at least one); blocks = tasks × repetitions.
    pub repetitions: u32,
    /// Recorded order seed; every `u64` is valid.
    pub seed: u64,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// One planned trial: a `(task, repetition, condition)` cell with its
/// position in the global execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTrial {
    /// 0-based global execution sequence, block by block in shuffled order.
    pub order_index: u32,
    /// Block identity: the slot `task_index * repetitions + repetition`.
    pub block_index: u32,
    /// 0-based position of this trial inside its block.
    pub block_position: u8,
    /// Task identifier.
    pub task_id: String,
    /// Task record version.
    pub task_version: u32,
    /// Condition identifier.
    pub condition_id: String,
    /// Repetition index (`0..repetitions`).
    pub repetition: u32,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`ScheduleError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScheduleErrorKind {
    /// `experiment_id` is empty.
    EmptyExperimentId,
    /// Fewer than two conditions were supplied.
    TooFewConditions,
    /// A condition id is empty.
    EmptyConditionId,
    /// Two conditions share an id.
    DuplicateCondition,
    /// `repetitions` is zero.
    ZeroRepetitions,
    /// No tasks were supplied.
    NoTasks,
    /// Two tasks share `(task_id, version)`.
    DuplicateTask,
    /// More than [`MAX_CONDITIONS`] conditions were supplied.
    TooManyConditions,
    /// More than [`MAX_TRIALS`] trials would be planned.
    TooManyTrials,
}

impl ScheduleErrorKind {
    /// Stable snake-case token used by eval storage and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyExperimentId => "empty_experiment_id",
            Self::TooFewConditions => "too_few_conditions",
            Self::EmptyConditionId => "empty_condition_id",
            Self::DuplicateCondition => "duplicate_condition",
            Self::ZeroRepetitions => "zero_repetitions",
            Self::NoTasks => "no_tasks",
            Self::DuplicateTask => "duplicate_task",
            Self::TooManyConditions => "too_many_conditions",
            Self::TooManyTrials => "too_many_trials",
        }
    }

    /// Parse the stable token produced by [`ScheduleErrorKind::as_str`].
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "empty_experiment_id" => Some(Self::EmptyExperimentId),
            "too_few_conditions" => Some(Self::TooFewConditions),
            "empty_condition_id" => Some(Self::EmptyConditionId),
            "duplicate_condition" => Some(Self::DuplicateCondition),
            "zero_repetitions" => Some(Self::ZeroRepetitions),
            "no_tasks" => Some(Self::NoTasks),
            "duplicate_task" => Some(Self::DuplicateTask),
            "too_many_conditions" => Some(Self::TooManyConditions),
            "too_many_trials" => Some(Self::TooManyTrials),
            _ => None,
        }
    }
}

/// Errors produced while validating a [`ScheduleSpec`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScheduleError {
    #[error("`experiment_id` must not be empty")]
    EmptyExperimentId,

    #[error("at least 2 conditions are required, found {found}")]
    TooFewConditions { found: usize },

    #[error("condition ids must not be empty (condition index {index})")]
    EmptyConditionId { index: usize },

    #[error("duplicate condition id `{id}`")]
    DuplicateCondition { id: String },

    #[error("`repetitions` must be at least 1")]
    ZeroRepetitions,

    #[error("at least 1 task is required")]
    NoTasks,

    #[error("duplicate task `{task_id}` version {version}")]
    DuplicateTask { task_id: String, version: u32 },

    #[error("too many conditions: {found} exceeds the maximum {max}")]
    TooManyConditions { found: usize, max: usize },

    #[error("too many planned trials: {found} exceeds the maximum {max}")]
    TooManyTrials { found: u64, max: u64 },
}

impl ScheduleError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> ScheduleErrorKind {
        match self {
            Self::EmptyExperimentId => ScheduleErrorKind::EmptyExperimentId,
            Self::TooFewConditions { .. } => ScheduleErrorKind::TooFewConditions,
            Self::EmptyConditionId { .. } => ScheduleErrorKind::EmptyConditionId,
            Self::DuplicateCondition { .. } => ScheduleErrorKind::DuplicateCondition,
            Self::ZeroRepetitions => ScheduleErrorKind::ZeroRepetitions,
            Self::NoTasks => ScheduleErrorKind::NoTasks,
            Self::DuplicateTask { .. } => ScheduleErrorKind::DuplicateTask,
            Self::TooManyConditions { .. } => ScheduleErrorKind::TooManyConditions,
            Self::TooManyTrials { .. } => ScheduleErrorKind::TooManyTrials,
        }
    }
}

// ---------------------------------------------------------------------------
// PRNG
// ---------------------------------------------------------------------------

/// SplitMix64: a fixed, dependency-free 64-bit generator.
///
/// The state is seeded directly with [`ScheduleSpec::seed`] and advanced by
/// the golden-ratio increment; each draw is the published 64-bit mixing
/// function of the advanced state. The constants are part of the frozen
/// schedule contract: changing them changes every schedule.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Seed the state (the seed *is* the initial state, not a derived one).
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance the state and return the next mixed 64-bit draw.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        z = (z ^ (z >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        z ^ (z >> 31)
    }

    /// Draw in `0..bound` via modulo reduction (`bound > 0`).
    fn below(&mut self, bound: u64) -> u64 {
        debug_assert!(bound > 0);
        self.next_u64() % bound
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Generate the balanced, seeded trial order for `spec`.
///
/// Deterministic and pure: see the module docs for the block, slot, shuffle,
/// and balance rules. Returns the trials in `order_index` order.
pub fn generate_schedule(spec: &ScheduleSpec) -> Result<Vec<PlannedTrial>, ScheduleError> {
    let block_count = validate_spec(spec)?;
    let condition_count = spec.conditions.len();
    let repetitions = spec.repetitions;

    // Fisher–Yates shuffle of the block slots, `blocks - 1` draws.
    let mut rng = SplitMix64::new(spec.seed);
    let mut slots: Vec<u32> = (0..block_count as u32).collect();
    for i in (1..block_count).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        slots.swap(i, j);
    }

    // One more draw selects the rotation offset.
    let offset = rng.below(condition_count as u64) as usize;

    let mut trials = Vec::with_capacity(block_count * condition_count);
    for (execution_position, &slot) in slots.iter().enumerate() {
        let task = &spec.tasks[(slot / repetitions) as usize];
        let repetition = slot % repetitions;
        let first = ((u64::from(slot) + offset as u64) % condition_count as u64) as usize;
        let block_base = (execution_position as u64 * condition_count as u64) as u32;
        for position in 0..condition_count {
            trials.push(PlannedTrial {
                order_index: block_base + position as u32,
                block_index: slot,
                block_position: position as u8,
                task_id: task.task_id.clone(),
                task_version: task.version,
                condition_id: spec.conditions[(first + position) % condition_count].clone(),
                repetition,
            });
        }
    }
    Ok(trials)
}

/// Fail-closed validation; returns the block count on success.
///
/// The check order is documented in the module docs and frozen by tests.
fn validate_spec(spec: &ScheduleSpec) -> Result<usize, ScheduleError> {
    if spec.experiment_id.is_empty() {
        return Err(ScheduleError::EmptyExperimentId);
    }
    if spec.tasks.is_empty() {
        return Err(ScheduleError::NoTasks);
    }
    let mut task_keys: Vec<(&str, u32)> = Vec::with_capacity(spec.tasks.len());
    for task in &spec.tasks {
        let key = (task.task_id.as_str(), task.version);
        if task_keys.contains(&key) {
            return Err(ScheduleError::DuplicateTask {
                task_id: task.task_id.clone(),
                version: task.version,
            });
        }
        task_keys.push(key);
    }
    if spec.conditions.len() < 2 {
        return Err(ScheduleError::TooFewConditions {
            found: spec.conditions.len(),
        });
    }
    if spec.conditions.len() > MAX_CONDITIONS {
        return Err(ScheduleError::TooManyConditions {
            found: spec.conditions.len(),
            max: MAX_CONDITIONS,
        });
    }
    let mut condition_ids: Vec<&str> = Vec::with_capacity(spec.conditions.len());
    for (index, condition) in spec.conditions.iter().enumerate() {
        if condition.is_empty() {
            return Err(ScheduleError::EmptyConditionId { index });
        }
        if condition_ids.contains(&condition.as_str()) {
            return Err(ScheduleError::DuplicateCondition {
                id: condition.clone(),
            });
        }
        condition_ids.push(condition.as_str());
    }
    if spec.repetitions == 0 {
        return Err(ScheduleError::ZeroRepetitions);
    }
    let block_count = (spec.tasks.len() as u64).saturating_mul(u64::from(spec.repetitions));
    let total_trials = block_count.saturating_mul(spec.conditions.len() as u64);
    if total_trials > MAX_TRIALS {
        return Err(ScheduleError::TooManyTrials {
            found: total_trials,
            max: MAX_TRIALS,
        });
    }
    Ok(block_count as usize)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn task(task_id: &str, version: u32) -> ScheduleTaskRef {
        ScheduleTaskRef {
            task_id: task_id.to_string(),
            version,
        }
    }

    fn spec(
        tasks: Vec<ScheduleTaskRef>,
        conditions: &[&str],
        repetitions: u32,
        seed: u64,
    ) -> ScheduleSpec {
        ScheduleSpec {
            experiment_id: "exp-schedule-test".to_string(),
            tasks,
            conditions: conditions.iter().map(|id| id.to_string()).collect(),
            repetitions,
            seed,
        }
    }

    fn four_tasks() -> Vec<ScheduleTaskRef> {
        vec![
            task("task-a", 1),
            task("task-b", 1),
            task("task-c", 2),
            task("task-d", 1),
        ]
    }

    fn five_tasks() -> Vec<ScheduleTaskRef> {
        vec![
            task("task-a", 1),
            task("task-b", 1),
            task("task-c", 2),
            task("task-d", 1),
            task("task-e", 3),
        ]
    }

    fn condition_index(spec: &ScheduleSpec, condition_id: &str) -> usize {
        spec.conditions
            .iter()
            .position(|condition| condition == condition_id)
            .expect("condition exists")
    }

    /// Recover the PRNG rotation offset from the first executed block.
    fn rotation_offset(spec: &ScheduleSpec, trials: &[PlannedTrial]) -> usize {
        let first = trials
            .iter()
            .min_by_key(|trial| trial.order_index)
            .expect("schedule is non-empty");
        let count = spec.conditions.len();
        let index = condition_index(spec, &first.condition_id);
        (index + count - first.block_index as usize % count) % count
    }

    /// Position-0 condition counts over all blocks, indexed by condition.
    fn first_condition_counts(spec: &ScheduleSpec, trials: &[PlannedTrial]) -> Vec<usize> {
        let mut counts = vec![0usize; spec.conditions.len()];
        for trial in trials {
            if trial.block_position == 0 {
                counts[condition_index(spec, &trial.condition_id)] += 1;
            }
        }
        counts
    }

    /// Assert the documented rotation rule for every block.
    fn assert_rotation_rule(spec: &ScheduleSpec, trials: &[PlannedTrial], offset: usize) {
        let count = spec.conditions.len();
        let mut blocks: BTreeMap<u32, Vec<&PlannedTrial>> = BTreeMap::new();
        for trial in trials {
            blocks.entry(trial.block_index).or_default().push(trial);
        }
        assert_eq!(blocks.len(), spec.tasks.len() * spec.repetitions as usize);
        for (slot, group) in blocks {
            let mut group = group;
            group.sort_by_key(|trial| trial.block_position);
            for (position, trial) in group.iter().enumerate() {
                let expected = &spec.conditions[(slot as usize + offset + position) % count];
                assert_eq!(&trial.condition_id, expected);
                assert_eq!(usize::from(trial.block_position), position);
            }
        }
    }

    fn assert_kind(spec: &ScheduleSpec, kind: ScheduleErrorKind) {
        let error = generate_schedule(spec).expect_err("spec must be refused");
        assert_eq!(error.kind(), kind);
        assert_eq!(ScheduleErrorKind::from_token(kind.as_str()), Some(kind));
    }

    #[test]
    fn same_spec_and_seed_is_deterministic_and_serialization_stable() {
        let spec = spec(four_tasks(), &["baseline", "treatment"], 3, 1234);
        let first = generate_schedule(&spec).expect("valid spec");
        let second = generate_schedule(&spec).expect("valid spec");
        assert_eq!(first, second);

        // A value-equal copy and a serialize/parse round-trip reproduce it.
        let copied = spec.clone();
        assert_eq!(generate_schedule(&copied).expect("valid spec"), first);
        let json = serde_json::to_string(&spec).expect("serializes");
        let reparsed: ScheduleSpec = serde_json::from_str(&json).expect("parses");
        assert_eq!(generate_schedule(&reparsed).expect("valid spec"), first);

        // Re-serialization is byte-stable.
        let encoded = serde_json::to_string(&first).expect("serializes");
        assert_eq!(serde_json::to_string(&second).expect("serializes"), encoded);
        let round: Vec<PlannedTrial> = serde_json::from_str(&encoded).expect("parses");
        assert_eq!(round, first);
        assert_eq!(serde_json::to_string(&round).expect("serializes"), encoded);
    }

    #[test]
    fn different_seeds_produce_different_orders() {
        let spec_a = spec(four_tasks(), &["baseline", "treatment"], 3, 1);
        let spec_b = spec(four_tasks(), &["baseline", "treatment"], 3, 2);
        let a = generate_schedule(&spec_a).expect("valid spec");
        let b = generate_schedule(&spec_b).expect("valid spec");
        assert_ne!(a, b);

        // The block execution order differs (not only the rotation offset).
        let execution_a: Vec<u32> = a.chunks(2).map(|chunk| chunk[0].block_index).collect();
        let execution_b: Vec<u32> = b.chunks(2).map(|chunk| chunk[0].block_index).collect();
        assert_ne!(execution_a, execution_b);
    }

    #[test]
    fn coverage_is_exact_and_order_indices_are_dense() {
        let spec = spec(five_tasks(), &["a", "b", "c"], 3, 11);
        let trials = generate_schedule(&spec).expect("valid spec");
        assert_eq!(trials.len(), 5 * 3 * 3);

        // `order_index` is 0..N-1 in the returned order.
        for (position, trial) in trials.iter().enumerate() {
            assert_eq!(trial.order_index, position as u32);
        }

        // Every (task, version, repetition, condition) cell exactly once.
        let mut cells = BTreeSet::new();
        for trial in &trials {
            let cell = (
                trial.task_id.clone(),
                trial.task_version,
                trial.repetition,
                trial.condition_id.clone(),
            );
            assert!(cells.insert(cell), "duplicate trial cell");
        }
        assert_eq!(cells.len(), trials.len());

        // Each block carries all conditions at positions 0..C-1.
        let mut blocks: BTreeMap<u32, Vec<&PlannedTrial>> = BTreeMap::new();
        for trial in &trials {
            blocks.entry(trial.block_index).or_default().push(trial);
        }
        assert_eq!(blocks.len(), 15);
        for (slot, mut group) in blocks {
            group.sort_by_key(|trial| trial.block_position);
            let positions: Vec<u8> = group.iter().map(|trial| trial.block_position).collect();
            assert_eq!(positions, vec![0, 1, 2]);
            let conditions: BTreeSet<&str> = group
                .iter()
                .map(|trial| trial.condition_id.as_str())
                .collect();
            assert_eq!(conditions, BTreeSet::from(["a", "b", "c"]));
            let task_index = slot as usize / 3;
            for trial in group {
                assert_eq!(trial.task_id, spec.tasks[task_index].task_id);
                assert_eq!(trial.task_version, spec.tasks[task_index].version);
                assert_eq!(trial.repetition, slot % 3);
            }
        }

        // Blocks are emitted contiguously in execution order, C trials each.
        let mut execution: Vec<u32> = Vec::new();
        for chunk in trials.chunks(3) {
            assert!(chunk
                .iter()
                .all(|trial| trial.block_index == chunk[0].block_index));
            execution.push(chunk[0].block_index);
        }
        let unique: BTreeSet<u32> = execution.iter().copied().collect();
        assert_eq!(unique.len(), execution.len());
    }

    #[test]
    fn position_zero_balance_is_exact_for_even_block_counts() {
        // 5 tasks × 4 repetitions = 20 blocks (even) → 10/10.
        let spec = spec(five_tasks(), &["baseline", "treatment"], 4, 42);
        let trials = generate_schedule(&spec).expect("valid spec");
        let offset = rotation_offset(&spec, &trials);
        assert_rotation_rule(&spec, &trials, offset);
        assert_eq!(first_condition_counts(&spec, &trials), vec![10, 10]);
    }

    #[test]
    fn position_zero_residue_rule_for_uneven_block_counts() {
        // C = 3, 20 blocks: 20 = 6×3 + 2 → the first two residues get 7.
        let three_way = spec(five_tasks(), &["a", "b", "c"], 4, 7);
        let trials = generate_schedule(&three_way).expect("valid spec");
        let offset = rotation_offset(&three_way, &trials);
        assert_rotation_rule(&three_way, &trials, offset);
        let counts = first_condition_counts(&three_way, &trials);
        let (base, extra) = (20 / 3, 20 % 3);
        for (condition, count) in counts.iter().enumerate() {
            let expected = base + usize::from((condition + 3 - offset) % 3 < extra);
            assert_eq!(*count, expected);
        }
        assert_eq!(
            counts.iter().copied().max().unwrap() - counts.iter().copied().min().unwrap(),
            1
        );

        // C = 2, 15 blocks (odd) → 8/7, difference exactly 1.
        let two_way = spec(five_tasks(), &["baseline", "treatment"], 3, 7);
        let trials = generate_schedule(&two_way).expect("valid spec");
        let counts = first_condition_counts(&two_way, &trials);
        assert_eq!(counts.iter().sum::<usize>(), 15);
        assert_eq!(
            counts.iter().copied().max().unwrap() - counts.iter().copied().min().unwrap(),
            1
        );
    }

    #[test]
    fn per_task_position_zero_balance_with_odd_repetitions() {
        // R = 3 (odd): every task gets a 2/1 split and the majority condition
        // `(t*R + offset) mod 2` alternates between adjacent tasks.
        let spec = spec(four_tasks(), &["baseline", "treatment"], 3, 99);
        let trials = generate_schedule(&spec).expect("valid spec");
        let offset = rotation_offset(&spec, &trials);
        assert_rotation_rule(&spec, &trials, offset);

        let mut per_task = vec![[0usize; 2]; spec.tasks.len()];
        for trial in &trials {
            if trial.block_position != 0 {
                continue;
            }
            let index = spec
                .tasks
                .iter()
                .position(|task| {
                    task.task_id == trial.task_id && task.version == trial.task_version
                })
                .expect("task exists");
            per_task[index][condition_index(&spec, &trial.condition_id)] += 1;
        }
        for (task_index, counts) in per_task.iter().enumerate() {
            assert_eq!(counts.iter().sum::<usize>(), 3);
            let majority = (task_index * 3 + offset) % 2;
            assert_eq!(counts[majority], 2);
            assert_eq!(counts[1 - majority], 1);
        }
        // Global counts stay even because the majority alternates.
        assert_eq!(first_condition_counts(&spec, &trials), vec![6, 6]);
    }

    #[test]
    fn per_task_position_zero_balance_with_even_repetitions() {
        // R = 4 (even): every task gets exactly R/2 first positions per
        // condition, and the global counts are exact too.
        let spec = spec(four_tasks(), &["baseline", "treatment"], 4, 5);
        let trials = generate_schedule(&spec).expect("valid spec");
        let mut per_task = vec![[0usize; 2]; spec.tasks.len()];
        for trial in &trials {
            if trial.block_position != 0 {
                continue;
            }
            let index = spec
                .tasks
                .iter()
                .position(|task| {
                    task.task_id == trial.task_id && task.version == trial.task_version
                })
                .expect("task exists");
            per_task[index][condition_index(&spec, &trial.condition_id)] += 1;
        }
        for counts in &per_task {
            assert_eq!(*counts, [2, 2]);
        }
        assert_eq!(first_condition_counts(&spec, &trials), vec![8, 8]);
    }

    #[test]
    fn repetition_blocks_cover_every_task_once_per_condition() {
        let spec = spec(four_tasks(), &["baseline", "treatment"], 3, 21);
        let trials = generate_schedule(&spec).expect("valid spec");
        for repetition in 0..3u32 {
            let cells: BTreeSet<(String, u32, String)> = trials
                .iter()
                .filter(|trial| trial.repetition == repetition)
                .map(|trial| {
                    (
                        trial.task_id.clone(),
                        trial.task_version,
                        trial.condition_id.clone(),
                    )
                })
                .collect();
            assert_eq!(cells.len(), 4 * 2);
            for task in &spec.tasks {
                for condition in &spec.conditions {
                    assert!(cells.contains(&(
                        task.task_id.clone(),
                        task.version,
                        condition.clone()
                    )));
                }
            }
        }
        let repetitions: BTreeSet<u32> = trials.iter().map(|trial| trial.repetition).collect();
        assert_eq!(repetitions, BTreeSet::from([0, 1, 2]));
    }

    #[test]
    fn minimal_schedule_is_one_block_of_two_trials() {
        let spec = spec(vec![task("task-a", 1)], &["baseline", "treatment"], 1, 0);
        let trials = generate_schedule(&spec).expect("valid spec");
        assert_eq!(trials.len(), 2);
        assert_eq!(trials[0].order_index, 0);
        assert_eq!(trials[1].order_index, 1);
        assert!(trials.iter().all(|trial| trial.block_index == 0));
        assert_eq!(trials[0].block_position, 0);
        assert_eq!(trials[1].block_position, 1);
        assert!(trials.iter().all(|trial| trial.task_id == "task-a"));
        assert!(trials.iter().all(|trial| trial.task_version == 1));
        assert!(trials.iter().all(|trial| trial.repetition == 0));
        let conditions: BTreeSet<&str> = trials
            .iter()
            .map(|trial| trial.condition_id.as_str())
            .collect();
        assert_eq!(conditions, BTreeSet::from(["baseline", "treatment"]));
    }

    #[test]
    fn audit_experiment_shape_is_fully_balanced() {
        // 10 tasks × 3 repetitions × 2 conditions = 60 trials (seed from the
        // audit's plan example).
        let tasks: Vec<ScheduleTaskRef> = (0..10)
            .map(|index| task(&format!("task-{index:02}"), 1))
            .collect();
        let spec = spec(tasks, &["baseline", "treatment"], 3, 881_204);
        let trials = generate_schedule(&spec).expect("valid spec");
        assert_eq!(trials.len(), 60);
        assert!(trials
            .iter()
            .enumerate()
            .all(|(position, trial)| trial.order_index == position as u32));
        assert_eq!(first_condition_counts(&spec, &trials), vec![15, 15]);
        let offset = rotation_offset(&spec, &trials);
        for (task_index, task_ref) in spec.tasks.iter().enumerate() {
            let firsts = trials
                .iter()
                .filter(|trial| {
                    trial.task_id == task_ref.task_id
                        && trial.task_version == task_ref.version
                        && trial.block_position == 0
                })
                .count();
            assert_eq!(firsts, 3);
            let baseline = trials
                .iter()
                .filter(|trial| {
                    trial.task_id == task_ref.task_id
                        && trial.task_version == task_ref.version
                        && trial.block_position == 0
                        && trial.condition_id == "baseline"
                })
                .count();
            // R = 3 splits 2/1 per task; the majority condition is
            // `(t*R + offset) mod 2`, so it alternates between tasks.
            let expected = if (task_index * 3 + offset).is_multiple_of(2) {
                2
            } else {
                1
            };
            assert_eq!(baseline, expected, "R = 3 splits 2/1 per task");
        }
    }

    #[test]
    fn splitmix64_golden_vectors_are_frozen() {
        // Published SplitMix64 outputs for seed 0; the schedule contract
        // depends on these constants never changing.
        let mut rng = SplitMix64::new(0);
        let expected = [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
            0xF88B_B8A8_724C_81EC,
        ];
        for value in expected {
            assert_eq!(rng.next_u64(), value);
        }
        // The seed is the initial state: seed 1's first draw is not seed 0's.
        let mut seeded = SplitMix64::new(1);
        assert_ne!(seeded.next_u64(), expected[0]);
    }

    #[test]
    fn validation_refuses_each_invalid_spec() {
        let base = || spec(four_tasks(), &["baseline", "treatment"], 3, 1);

        let mut candidate = base();
        candidate.experiment_id = String::new();
        assert_kind(&candidate, ScheduleErrorKind::EmptyExperimentId);

        let mut candidate = base();
        candidate.tasks.clear();
        assert_kind(&candidate, ScheduleErrorKind::NoTasks);

        let mut candidate = base();
        candidate.tasks.push(task("task-a", 1));
        assert_kind(&candidate, ScheduleErrorKind::DuplicateTask);

        // Same id with a different version is a distinct task.
        let mut candidate = base();
        candidate.tasks.push(task("task-a", 2));
        assert!(generate_schedule(&candidate).is_ok());

        let mut candidate = base();
        candidate.conditions = vec!["baseline".to_string()];
        assert_kind(&candidate, ScheduleErrorKind::TooFewConditions);

        let mut candidate = base();
        candidate.conditions = vec![String::new(), "treatment".to_string()];
        assert_kind(&candidate, ScheduleErrorKind::EmptyConditionId);

        let mut candidate = base();
        candidate.conditions = vec!["same".to_string(), "same".to_string()];
        assert_kind(&candidate, ScheduleErrorKind::DuplicateCondition);

        let mut candidate = base();
        candidate.repetitions = 0;
        assert_kind(&candidate, ScheduleErrorKind::ZeroRepetitions);

        let mut candidate = base();
        candidate.conditions = (0..=MAX_CONDITIONS).map(|i| format!("c{i}")).collect();
        assert_kind(&candidate, ScheduleErrorKind::TooManyConditions);

        let mut candidate = base();
        candidate.tasks = vec![task("task-a", 1), task("task-b", 1)];
        candidate.repetitions = u32::MAX;
        assert_kind(&candidate, ScheduleErrorKind::TooManyTrials);
    }

    #[test]
    fn error_kind_tokens_round_trip() {
        let kinds = [
            ScheduleErrorKind::EmptyExperimentId,
            ScheduleErrorKind::TooFewConditions,
            ScheduleErrorKind::EmptyConditionId,
            ScheduleErrorKind::DuplicateCondition,
            ScheduleErrorKind::ZeroRepetitions,
            ScheduleErrorKind::NoTasks,
            ScheduleErrorKind::DuplicateTask,
            ScheduleErrorKind::TooManyConditions,
            ScheduleErrorKind::TooManyTrials,
        ];
        let mut tokens = BTreeSet::new();
        for kind in kinds {
            assert!(tokens.insert(kind.as_str()), "tokens are unique");
            assert_eq!(ScheduleErrorKind::from_token(kind.as_str()), Some(kind));
        }
        assert_eq!(ScheduleErrorKind::from_token("nope"), None);
    }
}
