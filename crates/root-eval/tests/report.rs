//! Milestone 4: the paired feasibility report over a real [`EvalStore`].
//!
//! Every test runs under a unique `std::env::temp_dir()` root with a real
//! SQLite store, real content-addressed artifact bytes, and real evidence
//! seals. `ROOT_DIR` is never set and the continuity database is never
//! touched. The report must never emit an effectiveness verdict: the
//! `beneficial`/`harmful` word scan below is test-enforced.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    build_feasibility_report, sha256_digest, ArtifactRef, ArtifactStore, DiscordantPairs,
    EvalStore, EvidenceManifest, ExecutionDigest, ExecutionRow, ExperimentRow, FeasibilityReport,
    InvalidBlock, ReportError, ReportErrorKind, ReportVerdict, SealInputs, SecondaryStats,
    StoreError, StoreErrorKind, TaskDirection, TerminationKind, TrialOutcomeClass, TrialRow,
    TrialState, VerificationRow, VerifierDigest, VerifierVerdict, FEASIBILITY_LABEL,
    INSUFFICIENT_EVIDENCE_LABEL, REPORT_SCHEMA_VERSION, VERDICT_DEFERRED_LINE,
    VERDICT_DEFERRED_REASON,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m4-report-{}-{}-{}",
        prefix,
        std::process::id(),
        id
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn digest(seed: &str) -> String {
    sha256_digest(seed.as_bytes())
}

/// One experiment under a unique root: real store, real CAS, real seals.
struct Fixture {
    root: PathBuf,
    store: EvalStore,
    cas: ArtifactStore,
    experiment_id: String,
}

impl Fixture {
    fn new(prefix: &str, experiment_id: &str) -> Self {
        let root = unique_dir(prefix);
        let evals_root = root.join("evals");
        let store = EvalStore::open(&evals_root, experiment_id).expect("open eval store");
        store
            .record_experiment(ExperimentRow {
                experiment_id: experiment_id.to_string(),
                plan_hash: digest("plan"),
                created_at_ms: 1_700_000_000_000,
            })
            .expect("record experiment");
        Self {
            root,
            cas: ArtifactStore::new(&evals_root),
            store,
            experiment_id: experiment_id.to_string(),
        }
    }

    fn report(&self) -> FeasibilityReport {
        build_feasibility_report(&self.store, &self.experiment_id).expect("build report")
    }

    fn cleanup(self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Declarative trial plan with the report-relevant fields.
struct TrialPlan<'a> {
    trial_id: &'a str,
    task_id: &'a str,
    task_version: u64,
    condition: &'a str,
    repetition: u64,
    order_index: u64,
    plan_hash: String,
    manifest_sha256: String,
    outcome: Option<TrialOutcomeClass>,
    invalid_reason: Option<String>,
    wall_ms: u64,
    tokens_in: Option<u64>,
    missing_telemetry: Vec<String>,
}

impl<'a> TrialPlan<'a> {
    fn new(
        trial_id: &'a str,
        task_id: &'a str,
        condition: &'a str,
        repetition: u64,
        order_index: u64,
    ) -> Self {
        Self {
            trial_id,
            task_id,
            task_version: 1,
            condition,
            repetition,
            order_index,
            plan_hash: digest("plan"),
            manifest_sha256: digest(&format!("manifest-{trial_id}")),
            outcome: None,
            invalid_reason: None,
            wall_ms: 1_000,
            tokens_in: Some(100),
            missing_telemetry: vec!["cost".to_string()],
        }
    }

    fn outcome(mut self, outcome: TrialOutcomeClass) -> Self {
        self.outcome = Some(outcome);
        self
    }

    fn invalid(mut self, class: TrialOutcomeClass, reason: &str) -> Self {
        self.outcome = Some(class);
        self.invalid_reason = Some(reason.to_string());
        self
    }

    fn plan_hash(mut self, plan_hash: &str) -> Self {
        self.plan_hash = plan_hash.to_string();
        self
    }

    fn manifest(mut self, manifest_sha256: &str) -> Self {
        self.manifest_sha256 = manifest_sha256.to_string();
        self
    }

    fn wall_ms(mut self, wall_ms: u64) -> Self {
        self.wall_ms = wall_ms;
        self
    }

    fn tokens_in(mut self, tokens_in: Option<u64>) -> Self {
        self.tokens_in = tokens_in;
        self
    }

    fn missing(mut self, fields: &[&str]) -> Self {
        self.missing_telemetry = fields.iter().map(|field| field.to_string()).collect();
        self
    }
}

/// Record one planned trial with execution, verification, and a real sealed
/// evidence manifest over real artifact bytes.
fn record(fixture: &Fixture, plan: TrialPlan<'_>) -> TrialRow {
    let state = if plan.outcome.is_some_and(|class| class.is_invalid()) {
        TrialState::Invalid
    } else {
        TrialState::Planned
    };
    let trial = TrialRow {
        trial_id: plan.trial_id.to_string(),
        experiment_id: fixture.experiment_id.clone(),
        task_id: plan.task_id.to_string(),
        task_version: plan.task_version,
        condition_id: plan.condition.to_string(),
        repetition: plan.repetition,
        order_index: plan.order_index,
        plan_hash: plan.plan_hash.clone(),
        manifest_sha256: plan.manifest_sha256.clone(),
        task_content_sha256: digest("task-content"),
        repository_commit: Some("0".repeat(40)),
        repository_tree_sha256: Some(digest("tree")),
        state,
        outcome_class: plan.outcome,
        invalid_reason: plan.invalid_reason.clone(),
    };
    fixture
        .store
        .record_trial(trial.clone())
        .expect("record trial");

    let usage_json = match plan.tokens_in {
        Some(tokens) => format!("{{\"tokens_in\":{tokens},\"tokens_out\":7,\"cost\":0.0}}"),
        None => "{}".to_string(),
    };
    fixture
        .store
        .record_execution(ExecutionRow {
            execution_id: format!("exec-{}", plan.trial_id),
            trial_id: plan.trial_id.to_string(),
            termination: TerminationKind::Completed,
            exit_code: Some(0),
            wall_ms: plan.wall_ms,
            started_at_ms: 1_700_000_000_000,
            finished_at_ms: 1_700_000_000_000 + plan.wall_ms,
            requested_model: "opencode/big-pickle".to_string(),
            observed_model: None,
            raw_sha256: digest(&format!("raw-{}", plan.trial_id)),
            events_sha256: digest(&format!("events-{}", plan.trial_id)),
            session_ids_json: format!("[\"ses_{}\"]", plan.trial_id),
            usage_json,
            missing_telemetry_json: serde_json::to_string(&plan.missing_telemetry)
                .expect("missing telemetry json"),
        })
        .expect("record execution");

    fixture
        .store
        .record_verification(VerificationRow {
            verification_id: format!("verify-{}", plan.trial_id),
            trial_id: plan.trial_id.to_string(),
            verifier_id: "run-suite".to_string(),
            verdict: VerifierVerdict::Pass,
            mutation_detected: false,
            exit_codes_json: "[0]".to_string(),
            stdout_sha256: digest("verifier stdout"),
            stderr_sha256: digest("verifier stderr"),
            pre_tree_sha256: digest("tree"),
            post_tree_sha256: digest("tree"),
            wall_ms: 42,
            timed_out: false,
        })
        .expect("record verification");

    let artifact = fixture
        .cas
        .put(
            "execution-stdout",
            format!("stdout-{}\n", plan.trial_id).as_bytes(),
        )
        .expect("put artifact");
    let manifest = seal_for(&trial, &[artifact]);
    fixture
        .store
        .record_evidence(&manifest)
        .expect("record evidence");

    trial
}

/// Seal a manifest whose identity digests match `trial`'s row.
fn seal_for(trial: &TrialRow, artifacts: &[ArtifactRef]) -> EvidenceManifest {
    let execution = ExecutionDigest {
        raw_sha256: digest(&format!("raw-{}", trial.trial_id)),
        normalized_events_sha256: digest(&format!("events-{}", trial.trial_id)),
        termination: TerminationKind::Completed,
        exit_code: Some(0),
    };
    let verifier = VerifierDigest {
        verifier_id: "run-suite".to_string(),
        outcome: VerifierVerdict::Pass,
        stdout_sha256: digest("verifier stdout"),
        stderr_sha256: digest("verifier stderr"),
        mutation_detected: false,
    };
    let repo_diff_sha256 = digest("repo diff");
    root_eval::seal_manifest(SealInputs {
        experiment_id: &trial.experiment_id,
        trial_id: &trial.trial_id,
        plan_hash: &trial.plan_hash,
        task_content_sha256: &trial.task_content_sha256,
        manifest_sha256: &trial.manifest_sha256,
        execution: &execution,
        repo_diff_sha256: &repo_diff_sha256,
        verifier: Some(&verifier),
        artifacts,
        redactions: &[],
        sealed_at_ms: 1_700_000_000_000,
    })
    .expect("seal evidence manifest")
}

// ---------------------------------------------------------------------------
// Normal dataset
// ---------------------------------------------------------------------------

#[test]
fn normal_dataset_counts_pairs_directions_and_medians() {
    let fixture = Fixture::new("normal", "eval-m4-normal");

    // 3 tasks × 2 repetitions × 2 conditions, mixed pass/fail. The last two
    // columns are the treatment-side deltas (duration ms, input tokens).
    let blocks = [
        (
            "task-01",
            0u64,
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedPassed,
            10u64,
            100u64,
        ),
        (
            "task-01",
            1,
            TrialOutcomeClass::CompletedFailed,
            TrialOutcomeClass::CompletedPassed,
            20,
            200,
        ),
        (
            "task-02",
            0,
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedPassed,
            30,
            300,
        ),
        (
            "task-02",
            1,
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedFailed,
            40,
            400,
        ),
        (
            "task-03",
            0,
            TrialOutcomeClass::CompletedFailed,
            TrialOutcomeClass::CompletedFailed,
            50,
            500,
        ),
        (
            "task-03",
            1,
            TrialOutcomeClass::CompletedPassed,
            TrialOutcomeClass::CompletedPassed,
            60,
            600,
        ),
    ];
    let mut order = 0u64;
    for (task, repetition, baseline_outcome, treatment_outcome, delta_ms, delta_tokens) in blocks {
        let baseline_id = format!("{task}-r{repetition}-baseline");
        let treatment_id = format!("{task}-r{repetition}-treatment");
        record(
            &fixture,
            TrialPlan::new(&baseline_id, task, "baseline", repetition, order)
                .outcome(baseline_outcome),
        );
        order += 1;
        record(
            &fixture,
            TrialPlan::new(&treatment_id, task, "treatment", repetition, order)
                .outcome(treatment_outcome)
                .wall_ms(1_000 + delta_ms)
                .tokens_in(Some(100 + delta_tokens)),
        );
        order += 1;
    }

    let report = fixture.report();
    assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
    assert_eq!(report.experiment_id, "eval-m4-normal");
    assert_eq!(report.blocks_total, 6);
    assert_eq!(report.valid_blocks, 6);
    assert!(report.invalid_blocks.is_empty());
    assert_eq!(report.independent_tasks, 3);
    assert_eq!(report.repetitions, 2);
    assert_eq!(report.feasibility_label, FEASIBILITY_LABEL);
    assert_eq!(
        report.verdict,
        ReportVerdict::Deferred {
            reason: VERDICT_DEFERRED_REASON.to_string()
        }
    );

    assert_eq!(report.condition_counts.len(), 2);
    let baseline = &report.condition_counts[0];
    assert_eq!(baseline.condition_id, "baseline");
    assert_eq!(
        (
            baseline.completed_passed,
            baseline.completed_failed,
            baseline.invalid,
            baseline.missing
        ),
        (4, 2, 0, 0)
    );
    assert!(baseline.task_failures_by_class.is_empty());
    let treatment = &report.condition_counts[1];
    assert_eq!(treatment.condition_id, "treatment");
    assert_eq!(
        (
            treatment.completed_passed,
            treatment.completed_failed,
            treatment.invalid,
            treatment.missing
        ),
        (4, 2, 0, 0)
    );
    assert!(treatment.task_failures_by_class.is_empty());

    assert_eq!(report.discordant.baseline_fail_treatment_pass, 1);
    assert_eq!(report.discordant.baseline_pass_treatment_fail, 1);

    assert_eq!(
        report.task_direction,
        vec![
            TaskDirection {
                task_id: "task-01".to_string(),
                task_version: 1,
                better: 1,
                same: 1,
                worse: 0,
            },
            TaskDirection {
                task_id: "task-02".to_string(),
                task_version: 1,
                better: 0,
                same: 1,
                worse: 1,
            },
            TaskDirection {
                task_id: "task-03".to_string(),
                task_version: 1,
                better: 0,
                same: 2,
                worse: 0,
            },
        ]
    );

    assert_eq!(report.secondary.median_duration_delta_ms, Some(35));
    assert_eq!(report.secondary.median_input_token_delta, Some(350));
    assert_eq!(report.secondary.observed_pairs, 6);
    assert_eq!(report.secondary.missing_pairs, 0);

    assert_eq!(report.missing_metrics, vec!["cost: 12 of 12 executions"]);

    assert_eq!(report.limitations.len(), 8);
    for expected in [
        "3 independent tasks; repetitions do not create independent tasks",
        "2 repetitions recorded; repeated runs are nested within tasks, not independent samples",
        "0 of 6 paired blocks excluded as invalid; exclusion is symmetric (both conditions of a block are excluded together)",
        "host contamination: none detected in the trials read (no symlink resolved outside a trial root; no host-sized file in a trial stub directory)",
        "missing telemetry: cost: 12 of 12 executions",
        "observed model identity unavailable (requested model recorded)",
        "provider cost is harness-reported, not billed",
        "verdict engine deferred per feasibility-spike scope decision; no effectiveness claim is made",
    ] {
        assert!(
            report.limitations.contains(&expected.to_string()),
            "missing limitation `{expected}`: {:?}",
            report.limitations
        );
    }

    // Canonical JSON: sorted keys at every level, deferred verdict inside.
    let json = report.to_json().expect("canonical json");
    assert!(json.starts_with("{\"blocks_total\":6,"), "{json}");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert_eq!(
        value["feasibility_label"],
        serde_json::json!(FEASIBILITY_LABEL)
    );
    assert_eq!(
        value["secondary"]["median_duration_delta_ms"],
        serde_json::json!(35)
    );
    assert_eq!(value["verdict"]["kind"], serde_json::json!("deferred"));
    assert_eq!(
        value["verdict"]["reason"],
        serde_json::json!(VERDICT_DEFERRED_REASON)
    );

    let text = report.render_text();
    assert!(text.contains("  baseline: completed_passed=4 completed_failed=2 invalid=0 missing=0"));
    assert!(text.contains("  treatment: completed_passed=4 completed_failed=2 invalid=0 missing=0"));
    assert!(text.contains("baseline fail → treatment pass: 1"));
    assert!(text.contains("baseline pass → treatment fail: 1"));
    assert!(text.contains("task-01 v1: 1 better / 1 same / 0 worse"));
    assert!(text.contains("task-03 v1: 0 better / 2 same / 0 worse"));
    assert!(text.contains("median duration delta: 35 ms (6 observed pairs, 0 missing pairs)"));
    assert!(
        text.contains("median input token delta: 350 tokens (6 observed pairs, 0 missing pairs)")
    );
    assert!(text.contains("cost: 12 of 12 executions"));
    assert!(text.contains(VERDICT_DEFERRED_LINE));

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Symmetric exclusion
// ---------------------------------------------------------------------------

#[test]
fn invalid_class_excludes_both_sides_symmetrically() {
    let fixture = Fixture::new("exclusion", "eval-m4-exclusion");
    record(
        &fixture,
        TrialPlan::new("t1-b", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("t1-t", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("t2-b", "task-02", "baseline", 0, 2)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("t2-t", "task-02", "treatment", 0, 3).invalid(
            TrialOutcomeClass::VerifierMutation,
            "verifier mutated the scored tree",
        ),
    );

    let report = fixture.report();
    assert_eq!(report.blocks_total, 2);
    assert_eq!(report.valid_blocks, 1);
    assert_eq!(
        report.invalid_blocks,
        vec![InvalidBlock {
            task_id: "task-02".to_string(),
            task_version: 1,
            repetition: 0,
            reason: "treatment outcome class invalid: verifier_mutation (verifier mutated the scored tree)"
                .to_string(),
        }]
    );

    // Counts summarize recorded trials: the invalid treatment trial and the
    // (otherwise valid) baseline side of the excluded block are both shown...
    assert_eq!(report.condition_counts.len(), 2);
    let baseline = &report.condition_counts[0];
    assert_eq!(
        (
            baseline.completed_passed,
            baseline.completed_failed,
            baseline.invalid,
            baseline.missing
        ),
        (2, 0, 0, 0)
    );
    let treatment = &report.condition_counts[1];
    assert_eq!(
        (
            treatment.completed_passed,
            treatment.completed_failed,
            treatment.invalid,
            treatment.missing
        ),
        (1, 0, 1, 0)
    );
    assert_eq!(
        treatment.task_failures_by_class,
        BTreeMap::from([("verifier_mutation".to_string(), 1)])
    );

    // ... but the whole block is excluded from pairing, direction, and
    // independent-task counting.
    assert_eq!(
        report.task_direction,
        vec![TaskDirection {
            task_id: "task-01".to_string(),
            task_version: 1,
            better: 0,
            same: 1,
            worse: 0,
        }]
    );
    assert_eq!(report.independent_tasks, 1);
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    assert!(report.limitations.contains(
        &"1 of 2 paired blocks excluded as invalid; exclusion is symmetric (both conditions of a block are excluded together)".to_string()
    ));
    assert!(report
        .limitations
        .contains(&"1 independent task; repetitions do not create independent tasks".to_string()));

    let text = report.render_text();
    assert!(text.contains("task failures by class: verifier_mutation=1"));

    fixture.cleanup();
}

#[test]
fn missing_side_excludes_the_block() {
    let fixture = Fixture::new("missing-side", "eval-m4-missing-side");
    record(
        &fixture,
        TrialPlan::new("b0", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("t0", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    // Repetition 1 has no baseline trial at all.
    record(
        &fixture,
        TrialPlan::new("t1", "task-01", "treatment", 1, 3)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );

    let report = fixture.report();
    assert_eq!(report.blocks_total, 2);
    assert_eq!(report.valid_blocks, 1);
    assert_eq!(
        report.invalid_blocks,
        vec![InvalidBlock {
            task_id: "task-01".to_string(),
            task_version: 1,
            repetition: 1,
            reason: "baseline trial missing".to_string(),
        }]
    );
    assert_eq!(report.condition_counts[0].completed_passed, 1);
    assert_eq!(report.condition_counts[1].completed_passed, 2);

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Condition distinctness and plan identity
// ---------------------------------------------------------------------------

#[test]
fn identical_condition_manifests_are_not_distinct() {
    let fixture = Fixture::new("not-distinct", "eval-m4-not-distinct");
    let shared = digest("same-manifest");
    record(
        &fixture,
        TrialPlan::new("b", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .manifest(&shared),
    );
    record(
        &fixture,
        TrialPlan::new("t", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .manifest(&shared),
    );

    let report = fixture.report();
    assert_eq!(report.valid_blocks, 0);
    assert_eq!(
        report.invalid_blocks,
        vec![InvalidBlock {
            task_id: "task-01".to_string(),
            task_version: 1,
            repetition: 0,
            reason: "conditions not distinct".to_string(),
        }]
    );
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);

    fixture.cleanup();
}

#[test]
fn differing_plan_hashes_exclude_the_block() {
    let fixture = Fixture::new("plan-mismatch", "eval-m4-plan-mismatch");
    record(
        &fixture,
        TrialPlan::new("b", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .plan_hash(&digest("plan-a")),
    );
    record(
        &fixture,
        TrialPlan::new("t", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .plan_hash(&digest("plan-b")),
    );

    let report = fixture.report();
    assert_eq!(report.valid_blocks, 0);
    assert_eq!(report.invalid_blocks.len(), 1);
    assert_eq!(report.invalid_blocks[0].reason, "plan hash mismatch");
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Insufficient evidence and the toy difference
// ---------------------------------------------------------------------------

#[test]
fn fewer_than_two_valid_blocks_or_tasks_is_insufficient_evidence() {
    // One valid block: below the valid-block threshold.
    let fixture = Fixture::new("insufficient-1", "eval-m4-insufficient-1");
    record(
        &fixture,
        TrialPlan::new("b", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("t", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    let report = fixture.report();
    assert_eq!(report.valid_blocks, 1);
    assert_eq!(report.independent_tasks, 1);
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    assert!(matches!(report.verdict, ReportVerdict::Deferred { .. }));
    assert!(report
        .to_json()
        .expect("json")
        .contains(INSUFFICIENT_EVIDENCE_LABEL));
    fixture.cleanup();

    // Two valid blocks but only one independent task: below the task
    // threshold. Repetitions do not create independent tasks.
    let fixture = Fixture::new("insufficient-2", "eval-m4-insufficient-2");
    for repetition in 0..2u64 {
        let baseline_id = format!("b-{repetition}");
        let treatment_id = format!("t-{repetition}");
        record(
            &fixture,
            TrialPlan::new(
                &baseline_id,
                "task-01",
                "baseline",
                repetition,
                repetition * 2,
            )
            .outcome(TrialOutcomeClass::CompletedPassed),
        );
        record(
            &fixture,
            TrialPlan::new(
                &treatment_id,
                "task-01",
                "treatment",
                repetition,
                repetition * 2 + 1,
            )
            .outcome(TrialOutcomeClass::CompletedPassed),
        );
    }
    let report = fixture.report();
    assert_eq!(report.valid_blocks, 2);
    assert_eq!(report.independent_tasks, 1);
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    fixture.cleanup();
}

#[test]
fn ten_run_toy_difference_stays_deferred_and_verdict_free() {
    let fixture = Fixture::new("toy", "eval-m4-toy");
    for repetition in 0..10u64 {
        let baseline_id = format!("b-{repetition}");
        let treatment_id = format!("t-{repetition}");
        let baseline_outcome = if repetition == 0 {
            TrialOutcomeClass::CompletedFailed
        } else {
            TrialOutcomeClass::CompletedPassed
        };
        record(
            &fixture,
            TrialPlan::new(
                &baseline_id,
                "task-01",
                "baseline",
                repetition,
                repetition * 2,
            )
            .outcome(baseline_outcome),
        );
        record(
            &fixture,
            TrialPlan::new(
                &treatment_id,
                "task-01",
                "treatment",
                repetition,
                repetition * 2 + 1,
            )
            .outcome(TrialOutcomeClass::CompletedPassed),
        );
    }

    let report = fixture.report();
    assert_eq!(report.valid_blocks, 10);
    assert_eq!(report.independent_tasks, 1);
    assert_eq!(report.discordant.baseline_fail_treatment_pass, 1);
    assert_eq!(report.discordant.baseline_pass_treatment_fail, 0);
    assert_eq!(
        report.task_direction,
        vec![TaskDirection {
            task_id: "task-01".to_string(),
            task_version: 1,
            better: 1,
            same: 9,
            worse: 0,
        }]
    );
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    assert!(matches!(report.verdict, ReportVerdict::Deferred { .. }));
    assert!(report
        .limitations
        .contains(&"1 independent task; repetitions do not create independent tasks".to_string()));

    let json = report.to_json().expect("json").to_lowercase();
    let text = report.render_text().to_lowercase();
    for needle in ["beneficial", "harmful", "benefit", "harm"] {
        assert!(!json.contains(needle), "JSON must not contain `{needle}`");
        assert!(!text.contains(needle), "text must not contain `{needle}`");
    }

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Empty experiment
// ---------------------------------------------------------------------------

#[test]
fn empty_experiment_is_a_valid_zero_report() {
    let fixture = Fixture::new("empty", "eval-m4-empty");
    let report = fixture.report();

    assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
    assert_eq!(report.experiment_id, "eval-m4-empty");
    assert_eq!(report.blocks_total, 0);
    assert_eq!(report.valid_blocks, 0);
    assert!(report.invalid_blocks.is_empty());
    assert!(report.condition_counts.is_empty());
    assert_eq!(report.independent_tasks, 0);
    assert_eq!(report.repetitions, 0);
    assert_eq!(report.discordant, DiscordantPairs::default());
    assert!(report.task_direction.is_empty());
    assert_eq!(
        report.secondary,
        SecondaryStats {
            median_duration_delta_ms: None,
            median_input_token_delta: None,
            observed_pairs: 0,
            missing_pairs: 0,
        }
    );
    assert!(report.missing_metrics.is_empty());
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    assert_eq!(report.limitations.len(), 8);
    assert!(report
        .limitations
        .contains(&"0 independent tasks; repetitions do not create independent tasks".to_string()));
    assert!(report
        .limitations
        .contains(&"missing telemetry: none recorded in the executions read".to_string()));

    let text = report.render_text();
    assert!(text.contains("(no conditions recorded)"));
    assert!(text.contains(VERDICT_DEFERRED_LINE));
    assert!(report.to_json().is_ok());

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Missing telemetry
// ---------------------------------------------------------------------------

#[test]
fn missing_telemetry_is_aggregated_with_occurrence_counts() {
    let fixture = Fixture::new("telemetry", "eval-m4-telemetry");
    record(
        &fixture,
        TrialPlan::new("b0", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .missing(&["cost", "observed_model"]),
    );
    record(
        &fixture,
        TrialPlan::new("t0", "task-01", "treatment", 0, 1)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .missing(&["cost", "observed_model", "tokens.input"]),
    );
    record(
        &fixture,
        TrialPlan::new("b1", "task-01", "baseline", 1, 2)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .missing(&["cost"])
            .tokens_in(None),
    );
    record(
        &fixture,
        TrialPlan::new("t1", "task-01", "treatment", 1, 3)
            .outcome(TrialOutcomeClass::CompletedPassed)
            .missing(&[]),
    );

    let report = fixture.report();
    assert_eq!(
        report.missing_metrics,
        vec![
            "cost: 3 of 4 executions".to_string(),
            "observed_model: 2 of 4 executions".to_string(),
            "tokens.input: 1 of 4 executions".to_string(),
        ]
    );

    // Repetition 1's baseline execution carries no `tokens_in`: its token
    // delta is missing while its duration delta is observed.
    assert_eq!(report.secondary.observed_pairs, 1);
    assert_eq!(report.secondary.missing_pairs, 1);
    assert_eq!(report.secondary.median_duration_delta_ms, Some(0));
    assert_eq!(report.secondary.median_input_token_delta, Some(0));

    assert!(report
        .limitations
        .contains(&"missing telemetry: cost: 3 of 4 executions; observed_model: 2 of 4 executions; tokens.input: 1 of 4 executions".to_string()));

    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Determinism, errors, and pairing limits
// ---------------------------------------------------------------------------

#[test]
fn report_is_byte_identical_across_builds_and_read_only_reopens() {
    let fixture = Fixture::new("determinism", "eval-m4-determinism");
    for repetition in 0..2u64 {
        let baseline_id = format!("b-{repetition}");
        let treatment_id = format!("t-{repetition}");
        record(
            &fixture,
            TrialPlan::new(
                &baseline_id,
                "task-01",
                "baseline",
                repetition,
                repetition * 2,
            )
            .outcome(TrialOutcomeClass::CompletedPassed),
        );
        record(
            &fixture,
            TrialPlan::new(
                &treatment_id,
                "task-01",
                "treatment",
                repetition,
                repetition * 2 + 1,
            )
            .outcome(TrialOutcomeClass::CompletedPassed),
        );
    }

    let first = fixture.report();
    let second = fixture.report();
    assert_eq!(
        first.to_json().expect("json"),
        second.to_json().expect("json")
    );
    assert_eq!(first.render_text(), second.render_text());

    // A strictly read-only reopen reproduces the same bytes.
    let Fixture {
        root,
        store,
        cas: _,
        experiment_id,
    } = fixture;
    drop(store);
    let evals_root = root.join("evals");
    let read_only = EvalStore::open_read_only(&evals_root, &experiment_id).expect("read-only open");
    let third = build_feasibility_report(&read_only, &experiment_id).expect("build report");
    assert_eq!(
        first.to_json().expect("json"),
        third.to_json().expect("json")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn unknown_experiment_is_a_typed_store_error() {
    let fixture = Fixture::new("unknown", "eval-m4-unknown");
    let error =
        build_feasibility_report(&fixture.store, "eval-m4-missing").expect_err("must be refused");
    assert_eq!(error.kind(), ReportErrorKind::Store);
    assert!(matches!(
        error,
        ReportError::Store(StoreError::NotFound { .. })
    ));
    assert!(matches!(
        StoreError::NotFound {
            entity: "experiment".to_string(),
            id: "eval-m4-missing".to_string(),
        }
        .kind(),
        StoreErrorKind::NotFound
    ));
    fixture.cleanup();
}

#[test]
fn pairing_requires_two_conditions() {
    // Three conditions: the paired report refuses the experiment.
    let fixture = Fixture::new("three", "eval-m4-three");
    for (index, condition) in ["a", "b", "c"].iter().enumerate() {
        let trial_id = format!("t-{condition}");
        record(
            &fixture,
            TrialPlan::new(&trial_id, "task-01", condition, 0, index as u64)
                .outcome(TrialOutcomeClass::CompletedPassed),
        );
    }
    let error = build_feasibility_report(&fixture.store, &fixture.experiment_id)
        .expect_err("three conditions cannot be paired");
    assert_eq!(error.kind(), ReportErrorKind::InvalidExperiment);
    assert!(error.to_string().contains("found 3"), "{error}");
    fixture.cleanup();

    // One condition: a valid report whose every block is invalid.
    let fixture = Fixture::new("one", "eval-m4-one");
    record(
        &fixture,
        TrialPlan::new("only-passed", "task-01", "baseline", 0, 0)
            .outcome(TrialOutcomeClass::CompletedPassed),
    );
    record(
        &fixture,
        TrialPlan::new("only-unclassified", "task-01", "baseline", 1, 1),
    );
    let report = fixture.report();
    assert_eq!(report.valid_blocks, 0);
    assert_eq!(report.blocks_total, 2);
    assert_eq!(report.invalid_blocks.len(), 2);
    assert!(report
        .invalid_blocks
        .iter()
        .all(|block| block.reason == "only one condition recorded"));
    assert_eq!(report.condition_counts.len(), 1);
    assert_eq!(
        (
            report.condition_counts[0].completed_passed,
            report.condition_counts[0].missing,
        ),
        (1, 1)
    );
    assert_eq!(report.feasibility_label, INSUFFICIENT_EVIDENCE_LABEL);
    fixture.cleanup();
}
