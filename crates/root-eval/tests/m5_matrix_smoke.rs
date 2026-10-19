//! Milestone 5: reduced feasibility-matrix smoke test.
//!
//! Runs a 2-task × 2-condition × 1-repetition matrix through
//! [`run_feasibility_matrix`] in a unique temporary evals root and asserts the
//! end-to-end contract:
//!
//! - 4 trials recorded in the eval store with the calibrated outcome classes
//!   (treatment `completed_passed`, baseline `completed_failed`);
//! - the paired report builds with 2 valid blocks and 2 discordant
//!   baseline-fail/treatment-pass pairs under the feasibility-only label;
//! - [`reproduce_feasibility`] rebuilds the stored report byte-identically
//!   from sealed evidence;
//! - the canary secret-hygiene check, the disjoint-root assertions, and the
//!   residue scan all pass, and the store integrity check is intact.
//!
//! Processes spawned: the same bounded set as the full matrix (fixture `git`,
//! the fake agent under the supervisor, its fake-tool shims, and the declared
//! verifier commands). `ROOT_DIR` is never set; all temporary trees live under
//! a unique `std::env::temp_dir()` subdirectory.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    build_feasibility_report, reproduce_feasibility, run_feasibility_matrix, EvalStore,
    FeasibilityConfig, TrialOutcomeClass, FEASIBILITY_LABEL,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SMOKE_TASKS: [&str; 2] = ["01-install-cli", "02-verify-fails-rollback"];

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5-smoke-{prefix}-{}-{id}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn m5_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m5")
}

fn repo_skill() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/claude/SKILL.md")
}

#[test]
fn reduced_matrix_runs_reproduces_and_stays_clean() {
    let root = unique_dir("reduced");
    let evals_root = root.join("evals");
    fs::create_dir_all(&evals_root).expect("create evals root");

    // Reduced fixture set: two sealed records with their prompts and agents.
    let fixtures = root.join("fixtures");
    let tasks_dir = fixtures.join("tasks");
    let prompts_dir = fixtures.join("prompts");
    let agents_dir = fixtures.join("agents");
    for dir in [&tasks_dir, &prompts_dir, &agents_dir] {
        fs::create_dir_all(dir).expect("create fixture dir");
    }
    for stem in SMOKE_TASKS {
        fs::copy(
            m5_dir().join("tasks").join(format!("{stem}.task.toml")),
            tasks_dir.join(format!("{stem}.task.toml")),
        )
        .expect("copy task record");
        fs::copy(
            m5_dir().join("prompts").join(format!("{stem}.txt")),
            prompts_dir.join(format!("{stem}.txt")),
        )
        .expect("copy prompt");
        fs::copy(
            m5_dir().join("agents").join(format!("{stem}.sh")),
            agents_dir.join(format!("{stem}.sh")),
        )
        .expect("copy agent");
    }

    let mut config = FeasibilityConfig::new(
        &tasks_dir,
        &prompts_dir,
        &agents_dir,
        &evals_root,
        "m5-smoke",
    );
    config.repetitions = 1;
    config.treatment_skill = Some(repo_skill());

    let summary = run_feasibility_matrix(&config).expect("matrix run");
    assert_eq!(summary.planned_trials, 4, "{summary:#?}");
    assert_eq!(summary.completed_trials, 4);
    assert_eq!(summary.trials.len(), 4);
    assert_eq!(summary.trial_ids.len(), 4);

    // Contamination checks.
    assert!(
        summary.contamination.canary_clean,
        "canary hits: {:#?}",
        summary.contamination.canary_hits
    );
    assert!(summary.contamination.roots_distinct);
    assert!(summary.contamination.roots_pairwise_disjoint);
    assert!(summary.contamination.roots_contained);
    assert!(summary.contamination.roots_outside_store);
    assert!(
        summary.contamination.residue_clean,
        "residue: {:#?}",
        summary.contamination.residue_entries
    );
    assert!(
        summary.store_integrity_intact,
        "integrity failures: {:#?}",
        summary.store_integrity_failures
    );

    // Store: 4 trials with the calibrated outcome classes.
    let store = EvalStore::open_read_only(&evals_root, "m5-smoke").expect("open store read-only");
    let trials = store.list_trials("m5-smoke").expect("list trials");
    assert_eq!(trials.len(), 4);
    for trial in &trials {
        let expected = if trial.condition_id == "treatment" {
            TrialOutcomeClass::CompletedPassed
        } else {
            TrialOutcomeClass::CompletedFailed
        };
        assert_eq!(
            trial.outcome_class,
            Some(expected),
            "trial {} ({})",
            trial.trial_id,
            trial.condition_id
        );
        assert!(
            trial.manifest_sha256.starts_with("sha256:"),
            "trial {}",
            trial.trial_id
        );
        assert_eq!(trial.plan_hash, summary.plan_hash);
    }

    // Fixture commits are deterministic per task: both conditions of a task
    // materialize the same source commit and tree, and the two conditions
    // carry distinct effective-context manifests.
    let mut per_task_commit: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    let mut per_task_manifests: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for trial in &trials {
        let commit = trial.repository_commit.as_deref().unwrap_or_default();
        let tree = trial.repository_tree_sha256.as_deref().unwrap_or_default();
        let entry = per_task_commit
            .entry(trial.task_id.as_str())
            .or_insert((commit, tree));
        assert_eq!(entry.0, commit, "{}", trial.task_id);
        assert_eq!(entry.1, tree, "{}", trial.task_id);
        per_task_manifests.insert(
            (trial.task_id.as_str(), trial.condition_id.as_str()),
            trial.manifest_sha256.as_str(),
        );
    }
    assert_eq!(per_task_commit.len(), 2);
    for task in per_task_commit.keys() {
        let baseline = per_task_manifests
            .get(&(*task, "baseline"))
            .expect("baseline manifest");
        let treatment = per_task_manifests
            .get(&(*task, "treatment"))
            .expect("treatment manifest");
        assert_ne!(baseline, treatment, "{task}: conditions must be distinct");
    }

    // Report: 2 valid blocks, 2 discordant pairs, feasibility-only label.
    let report = build_feasibility_report(&store, "m5-smoke").expect("report builds");
    assert_eq!(report.valid_blocks, 2, "{report:#?}");
    assert!(report.invalid_blocks.is_empty());
    assert_eq!(report.discordant.baseline_fail_treatment_pass, 2);
    assert_eq!(report.discordant.baseline_pass_treatment_fail, 0);
    assert_eq!(report.independent_tasks, 2);
    assert_eq!(report.feasibility_label, FEASIBILITY_LABEL);
    drop(store);

    // Reproduction: byte-identical report from sealed evidence only.
    let outcome = reproduce_feasibility(&evals_root, "m5-smoke").expect("reproduce");
    assert!(outcome.store_integrity_intact, "{outcome:#?}");
    assert_eq!(outcome.store_integrity_checked, 4);
    assert!(
        outcome.report_byte_identical,
        "stored {} rebuilt {}",
        outcome.stored_report_sha256, outcome.rebuilt_report_sha256
    );

    fs::remove_dir_all(&root).expect("remove smoke root");
}
