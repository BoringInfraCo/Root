//! Milestone 4: the scheduler's public API over the audit's experiment shape.
//!
//! `schedule.rs` is a pure module (no environment, clock, filesystem, or
//! process input); this test proves the public re-exports compile and that
//! the audit's "ten tasks × two conditions × `R` repetitions, recorded seed"
//! shape produces a dense, balanced, deterministic order.

use root_eval::{
    generate_schedule, PlannedTrial, ScheduleError, ScheduleErrorKind, ScheduleSpec,
    ScheduleTaskRef, MAX_CONDITIONS, MAX_TRIALS,
};

/// The audit's plan example: 10 tasks, baseline/treatment, seed 881204.
fn audit_spec(seed: u64) -> ScheduleSpec {
    ScheduleSpec {
        experiment_id: "eval-audit-shape".to_string(),
        tasks: (0..10)
            .map(|index| ScheduleTaskRef {
                task_id: format!("task-{index:02}"),
                version: 1,
            })
            .collect(),
        conditions: vec!["baseline".to_string(), "treatment".to_string()],
        repetitions: 3,
        seed,
    }
}

#[test]
fn audit_shape_schedule_is_dense_balanced_and_deterministic() {
    let spec = audit_spec(881_204);
    let trials: Vec<PlannedTrial> = generate_schedule(&spec).expect("valid spec");

    // 10 tasks × 3 repetitions × 2 conditions = 60 trials.
    assert_eq!(trials.len(), 60);

    // `order_index` is dense and 0-based in the returned order.
    assert!(trials
        .iter()
        .enumerate()
        .all(|(position, trial)| trial.order_index == position as u32));

    // 30 blocks, each contributing one first position; position 0 is exactly
    // balanced across the two conditions (15/15).
    let first_positions: Vec<&PlannedTrial> = trials
        .iter()
        .filter(|trial| trial.block_position == 0)
        .collect();
    assert_eq!(first_positions.len(), 30);
    assert_eq!(
        first_positions
            .iter()
            .filter(|trial| trial.condition_id == "baseline")
            .count(),
        15
    );
    assert_eq!(
        first_positions
            .iter()
            .filter(|trial| trial.condition_id == "treatment")
            .count(),
        15
    );

    // Every task carries three blocks, each with a first position.
    for task in &spec.tasks {
        let firsts = trials
            .iter()
            .filter(|trial| trial.task_id == task.task_id && trial.block_position == 0)
            .count();
        assert_eq!(firsts, 3, "task {} must own three blocks", task.task_id);
    }

    // Deterministic across calls and across a value-equal spec copy.
    assert_eq!(generate_schedule(&spec).expect("valid spec"), trials);
    assert_eq!(
        generate_schedule(&spec.clone()).expect("valid spec"),
        trials
    );

    // The re-exported limits are the documented ones.
    assert_eq!(MAX_CONDITIONS, 256);
    assert_eq!(MAX_TRIALS, u64::from(u32::MAX) + 1);
}

#[test]
fn scheduler_errors_are_publicly_matchable() {
    let mut spec = audit_spec(1);
    spec.conditions = vec!["baseline".to_string()];
    let error: ScheduleError = generate_schedule(&spec).expect_err("one condition is refused");
    assert_eq!(error.kind(), ScheduleErrorKind::TooFewConditions);
    assert_eq!(
        ScheduleErrorKind::from_token(error.kind().as_str()),
        Some(ScheduleErrorKind::TooFewConditions)
    );
}
