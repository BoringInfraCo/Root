//! Plan-hash determinism and pinning tests (Milestone 1, Phase B).
//!
//! The hash is computed from an [`ExperimentSpec`] that pins the A/B fixture's
//! real manifest hashes, so the tests exercise the same values an experiment
//! plan would seal.

mod context_support;

use context_support::resolve_named;
use root_eval::{
    compute_plan_hash, sha256_digest, verify_plan, BenchmarkTaskRef, ConditionSpec, EvalErrorKind,
    ExperimentSpec, HarnessLock, RepositoryLock, EXPERIMENT_SPEC_SCHEMA_VERSION,
};

fn digest(seed: &str) -> String {
    sha256_digest(seed.as_bytes())
}

fn fixture_manifests() -> (String, String) {
    let baseline = resolve_named("12-ab-treatment", "base");
    let treatment = resolve_named("12-ab-treatment", "treatment-added");
    (
        baseline.sha256().to_string(),
        treatment.sha256().to_string(),
    )
}

fn spec() -> ExperimentSpec {
    let (baseline, treatment) = fixture_manifests();
    ExperimentSpec {
        schema_version: EXPERIMENT_SPEC_SCHEMA_VERSION,
        experiment_id: "root-skill-install-ab".to_string(),
        tasks: vec![
            BenchmarkTaskRef {
                task_id: "install-ripgrep-001".to_string(),
                version: 1,
                content_sha256: digest("task-ripgrep"),
            },
            BenchmarkTaskRef {
                task_id: "install-poppler-002".to_string(),
                version: 1,
                content_sha256: digest("task-poppler"),
            },
            BenchmarkTaskRef {
                task_id: "install-fd-003".to_string(),
                version: 2,
                content_sha256: digest("task-fd"),
            },
        ],
        conditions: vec![
            ConditionSpec {
                id: "baseline".to_string(),
                expected_manifest_sha256: baseline,
                overlay_present: false,
            },
            ConditionSpec {
                id: "treatment".to_string(),
                expected_manifest_sha256: treatment,
                overlay_present: true,
            },
        ],
        harness: HarnessLock {
            name: "opencode".to_string(),
            version: "1.18.27".to_string(),
            binary_sha256: digest("opencode-1.18.27-binary"),
        },
        repository: RepositoryLock {
            commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            tree_sha256: digest("fixture-repository-tree"),
        },
        created_at: "2026-10-18T00:00:00Z".to_string(),
    }
}

#[test]
fn plan_hash_is_deterministic_across_repeated_construction() {
    let first = compute_plan_hash(&spec()).expect("hashes");
    let second = compute_plan_hash(&spec()).expect("hashes");
    assert_eq!(first, second);
    assert!(root_eval::is_canonical_sha256_digest(&first));
}

#[test]
fn task_and_condition_declaration_order_do_not_matter() {
    let canonical = compute_plan_hash(&spec()).expect("hashes");

    let mut permuted = spec();
    permuted.tasks.reverse();
    permuted.conditions.reverse();
    assert_eq!(compute_plan_hash(&permuted).expect("hashes"), canonical);

    let mut rotated = spec();
    rotated.tasks.rotate_left(1);
    rotated.conditions.rotate_left(1);
    assert_eq!(compute_plan_hash(&rotated).expect("hashes"), canonical);
}

#[test]
fn changing_a_condition_manifest_changes_the_hash() {
    let canonical = compute_plan_hash(&spec()).expect("hashes");
    let mut changed = spec();
    changed.conditions[0].expected_manifest_sha256 =
        changed.conditions[1].expected_manifest_sha256.clone();
    assert_ne!(compute_plan_hash(&changed).expect("hashes"), canonical);
}

#[test]
fn task_version_and_content_bumps_change_the_hash() {
    let canonical = compute_plan_hash(&spec()).expect("hashes");

    let mut version_bump = spec();
    version_bump.tasks[0].version = 2;
    assert_ne!(compute_plan_hash(&version_bump).expect("hashes"), canonical);

    let mut content_bump = spec();
    content_bump.tasks[0].content_sha256 = digest("task-ripgrep-edited");
    assert_ne!(compute_plan_hash(&content_bump).expect("hashes"), canonical);
}

#[test]
fn harness_repository_and_overlay_fields_are_pinned() {
    let canonical = compute_plan_hash(&spec()).expect("hashes");

    let mut harness_version = spec();
    harness_version.harness.version = "1.18.28".to_string();
    assert_ne!(
        compute_plan_hash(&harness_version).expect("hashes"),
        canonical
    );

    let mut harness_binary = spec();
    harness_binary.harness.binary_sha256 = digest("other-binary");
    assert_ne!(
        compute_plan_hash(&harness_binary).expect("hashes"),
        canonical
    );

    let mut repository = spec();
    repository.repository.commit = "89abcdef0123456789abcdef0123456789abcdef".to_string();
    assert_ne!(compute_plan_hash(&repository).expect("hashes"), canonical);

    let mut overlay = spec();
    overlay.conditions[1].overlay_present = false;
    assert_ne!(compute_plan_hash(&overlay).expect("hashes"), canonical);
}

#[test]
fn created_at_is_the_only_time_like_input_and_it_is_hashed() {
    let canonical = compute_plan_hash(&spec()).expect("hashes");
    let mut later = spec();
    later.created_at = "2026-10-19T12:34:56Z".to_string();
    assert_ne!(compute_plan_hash(&later).expect("hashes"), canonical);

    // Re-using the same explicit value reproduces the hash exactly.
    let mut same = later;
    same.created_at = "2026-10-18T00:00:00Z".to_string();
    assert_eq!(compute_plan_hash(&same).expect("hashes"), canonical);
}

#[test]
fn verify_plan_pins_the_exact_hash() {
    let spec = spec();
    let hash = compute_plan_hash(&spec).expect("hashes");
    assert_eq!(verify_plan(&spec, &hash), Ok(()));

    let mut tampered = spec.clone();
    tampered.experiment_id = "root-skill-install-ab-copy".to_string();
    let error = verify_plan(&tampered, &hash).expect_err("tampered plan refused");
    assert_eq!(error.kind(), EvalErrorKind::PlanHashMismatch);

    let error = verify_plan(&spec, "sha256:PENDING").expect_err("malformed expected hash");
    assert_eq!(error.kind(), EvalErrorKind::DigestInvalid);
}

#[test]
fn validation_refuses_unpinnable_specs() {
    let mut no_tasks = spec();
    no_tasks.tasks.clear();
    assert_eq!(
        compute_plan_hash(&no_tasks).expect_err("refused").kind(),
        EvalErrorKind::EmptyField
    );

    let mut wrong_harness = spec();
    wrong_harness.harness.name = "codex".to_string();
    assert_eq!(
        compute_plan_hash(&wrong_harness)
            .expect_err("refused")
            .kind(),
        EvalErrorKind::UnsupportedHarness
    );

    let mut pending = spec();
    pending.conditions[0].expected_manifest_sha256 = "sha256:PENDING".to_string();
    assert_eq!(
        compute_plan_hash(&pending).expect_err("refused").kind(),
        EvalErrorKind::DigestInvalid
    );
}
