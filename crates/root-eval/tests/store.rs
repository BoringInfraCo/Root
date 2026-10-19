//! Milestone 3, Phase B: the local evaluation SQLite store.
//!
//! Every test runs under a unique `std::env::temp_dir()` root; `ROOT_DIR` is
//! never set and the continuity database is never touched. Raw `rusqlite`
//! connections are opened directly only to simulate tampering and to count
//! rows the public API does not expose.
//!
//! Documented limit: with [`root_eval::EVAL_STORE_SCHEMA_VERSION`] == 1, the
//! pre-migration backup guard (`current > 0 && current < 1`) is unreachable —
//! there is no older positive version to migrate from yet. The integration
//! test below pins the reachable no-op behavior (version 0 and current
//! version), and the guard's full truth table is unit-tested inside
//! `store.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};

use root_eval::{
    sha256_digest, ArtifactRef, ArtifactStore, EvalStore, EvidenceManifest, ExecutionDigest,
    ExecutionRow, ExperimentRow, SealInputs, StoreErrorKind, StoreIntegrityCheck, TerminationKind,
    TrialOutcomeClass, TrialRow, TrialState, VerificationRow, VerifierDigest, VerifierVerdict,
    EVAL_STORE_SCHEMA_VERSION,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m3-store-{}-{}-{}",
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

fn backup_path(db: &Path, from: u32) -> PathBuf {
    PathBuf::from(format!("{}.backup-v{from}", db.display()))
}

fn raw_connection(db: &Path) -> Connection {
    Connection::open(db).expect("open raw connection")
}

fn count_rows(db: &Path, table: &str) -> i64 {
    raw_connection(db)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
}

fn experiment_row(experiment_id: &str) -> ExperimentRow {
    ExperimentRow {
        experiment_id: experiment_id.to_string(),
        plan_hash: digest("plan"),
        created_at_ms: 1_700_000_000_000,
    }
}

fn trial_row(
    experiment_id: &str,
    trial_id: &str,
    condition_id: &str,
    order_index: u64,
) -> TrialRow {
    TrialRow {
        trial_id: trial_id.to_string(),
        experiment_id: experiment_id.to_string(),
        task_id: "install-supported-cli".to_string(),
        task_version: 1,
        condition_id: condition_id.to_string(),
        repetition: 1,
        order_index,
        plan_hash: digest("plan"),
        manifest_sha256: digest(&format!("context-{trial_id}")),
        task_content_sha256: digest("task"),
        repository_commit: Some("0".repeat(40)),
        repository_tree_sha256: Some(digest("tree")),
        state: TrialState::Planned,
        outcome_class: None,
        invalid_reason: None,
    }
}

fn execution_row(trial_id: &str, execution_id: &str, missing_telemetry_json: &str) -> ExecutionRow {
    ExecutionRow {
        execution_id: execution_id.to_string(),
        trial_id: trial_id.to_string(),
        termination: TerminationKind::Completed,
        exit_code: Some(0),
        wall_ms: 1_234,
        started_at_ms: 1_700_000_000_000,
        finished_at_ms: 1_700_000_001_234,
        requested_model: "opencode/big-pickle".to_string(),
        observed_model: None,
        raw_sha256: digest(&format!("raw-{execution_id}")),
        events_sha256: digest(&format!("events-{execution_id}")),
        session_ids_json: "[\"ses_store_0001\"]".to_string(),
        usage_json: "{\"tokens_in\":8,\"tokens_out\":1,\"cost\":0.0}".to_string(),
        missing_telemetry_json: missing_telemetry_json.to_string(),
    }
}

fn verification_row(trial_id: &str, verification_id: &str) -> VerificationRow {
    VerificationRow {
        verification_id: verification_id.to_string(),
        trial_id: trial_id.to_string(),
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
    }
}

/// Seal a manifest whose identity digests match `trial`'s row, so the store's
/// cross-checks pass.
fn seal_for(trial: &TrialRow, artifacts: &[ArtifactRef], sealed_at_ms: u64) -> EvidenceManifest {
    let execution = ExecutionDigest {
        raw_sha256: digest("raw stdout"),
        normalized_events_sha256: digest("normalized events"),
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
        sealed_at_ms,
    })
    .expect("seal manifest")
}

// ---------------------------------------------------------------------------
// Schema versioning
// ---------------------------------------------------------------------------

#[test]
fn fresh_open_creates_v1_and_reopen_is_stable() {
    let root = unique_dir("fresh");
    let evals_root = root.join("evals");
    let experiment_id = "eval-fresh-001";

    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    assert_eq!(
        store.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );
    assert_eq!(store.experiment_id(), experiment_id);
    assert!(!store.is_read_only());
    let db_path = store.path().to_path_buf();
    assert_eq!(db_path, evals_root.join(experiment_id).join("state.db"));
    assert!(db_path.is_file());

    let conn = raw_connection(&db_path);
    let meta_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))
        .expect("count meta");
    let version: u32 = conn
        .query_row("SELECT schema_version FROM meta", [], |row| row.get(0))
        .expect("read version");
    assert_eq!(meta_rows, 1, "meta must hold exactly one version row");
    assert_eq!(version, EVAL_STORE_SCHEMA_VERSION);
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
             ('meta','experiments','trials','executions','verifications','evidence',
              'artifacts','evidence_artifacts')",
            [],
            |row| row.get(0),
        )
        .expect("count tables");
    assert_eq!(tables, 8, "v1 schema must create all eight tables");
    drop(conn);
    drop(store);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir_mode = fs::metadata(evals_root.join(experiment_id))
            .expect("experiment dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "experiment directory must be 0700");
        let file_mode = fs::metadata(&db_path)
            .expect("db file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "database file must be 0600");
    }

    let reopened = EvalStore::open(&evals_root, experiment_id).expect("reopen");
    assert_eq!(
        reopened.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn missing_meta_table_is_initialized_on_open() {
    let root = unique_dir("meta-missing");
    let evals_root = root.join("evals");
    let experiment_id = "eval-meta-001";
    let dir = evals_root.join(experiment_id);
    fs::create_dir_all(&dir).expect("create dir");
    let db_path = dir.join("state.db");
    // An existing database file with no schema at all.
    drop(Connection::open(&db_path).expect("create empty db"));

    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    assert_eq!(
        store.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );
    let version: u32 = raw_connection(&db_path)
        .query_row("SELECT schema_version FROM meta", [], |row| row.get(0))
        .expect("version row");
    assert_eq!(version, EVAL_STORE_SCHEMA_VERSION);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn newer_schema_version_is_refused_without_partial_read() {
    let root = unique_dir("newer");
    let evals_root = root.join("evals");
    let experiment_id = "eval-newer-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let db_path = store.path().to_path_buf();
    drop(store);
    {
        let conn = raw_connection(&db_path);
        conn.execute("UPDATE meta SET schema_version = 99", [])
            .expect("bump version");
    }

    let error = EvalStore::open(&evals_root, experiment_id).expect_err("must refuse newer");
    assert_eq!(error.kind(), StoreErrorKind::UnsupportedSchema);
    assert!(error.to_string().contains("99"), "{error}");

    let error = EvalStore::open_read_only(&evals_root, experiment_id).expect_err("must refuse");
    assert_eq!(error.kind(), StoreErrorKind::UnsupportedSchema);

    // Never partially read, migrated, or backed up.
    let conn = raw_connection(&db_path);
    let version: u32 = conn
        .query_row("SELECT schema_version FROM meta", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 99);
    assert!(!backup_path(&db_path, 99).exists());
    assert!(!backup_path(&db_path, 1).exists());

    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Roundtrip, relationships, summaries
// ---------------------------------------------------------------------------

#[test]
fn full_roundtrip_joins_executions_verifications_and_evidence() {
    let root = unique_dir("roundtrip");
    let evals_root = root.join("evals");
    let experiment_id = "eval-roundtrip-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let cas = ArtifactStore::new(&evals_root);

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let baseline = trial_row(experiment_id, "trial-0001", "baseline", 0);
    let treatment = trial_row(experiment_id, "trial-0002", "treatment", 1);
    store
        .record_trial(baseline.clone())
        .expect("baseline trial");
    store
        .record_trial(treatment.clone())
        .expect("treatment trial");

    let execution_baseline = execution_row("trial-0001", "exec-0001", "[\"cost\"]");
    let execution_treatment = execution_row("trial-0002", "exec-0002", "[]");
    store
        .record_execution(execution_baseline.clone())
        .expect("baseline execution");
    store
        .record_execution(execution_treatment.clone())
        .expect("treatment execution");

    let verification = verification_row("trial-0001", "verify-0001");
    store
        .record_verification(verification.clone())
        .expect("verification");

    let artifact_baseline = cas
        .put("execution-stdout", b"baseline stdout\n")
        .expect("put baseline artifact");
    let manifest_baseline = seal_for(
        &baseline,
        std::slice::from_ref(&artifact_baseline),
        1_700_000_000_000,
    );
    store
        .record_evidence(&manifest_baseline)
        .expect("baseline evidence");

    let artifact_treatment = cas
        .put("execution-stdout", b"treatment stdout\n")
        .expect("put treatment artifact");
    let manifest_treatment = seal_for(
        &treatment,
        std::slice::from_ref(&artifact_treatment),
        1_700_000_000_100,
    );
    store
        .record_evidence(&manifest_treatment)
        .expect("treatment evidence");

    store
        .set_trial_outcome("trial-0001", TrialOutcomeClass::CompletedPassed, None)
        .expect("baseline outcome");
    store
        .set_trial_outcome("trial-0002", TrialOutcomeClass::CompletedFailed, None)
        .expect("treatment outcome");

    // trial_full joins the latest rows and the outcome class.
    let full = store.trial_full("trial-0001").expect("trial_full");
    assert_eq!(full.trial.trial_id, "trial-0001");
    assert_eq!(full.trial.condition_id, "baseline");
    assert_eq!(full.trial.state, TrialState::Sealed);
    assert_eq!(full.outcome_class, Some(TrialOutcomeClass::CompletedPassed));
    assert_eq!(full.trial.outcome_class, full.outcome_class);
    assert_eq!(
        full.latest_execution.as_ref().expect("execution"),
        &execution_baseline
    );
    assert_eq!(
        full.latest_verification.as_ref().expect("verification"),
        &verification
    );
    let evidence = full.evidence.as_ref().expect("evidence");
    assert_eq!(evidence.seal_sha256, manifest_baseline.seal_sha256);
    assert_eq!(evidence.sealed_at_ms, 1_700_000_000_000);
    assert_eq!(
        evidence.manifest().expect("stored manifest parses"),
        manifest_baseline,
        "the stored manifest round-trips byte-for-byte"
    );

    let full_treatment = store.trial_full("trial-0002").expect("trial_full");
    assert!(full_treatment.latest_execution.is_some());
    assert!(full_treatment.latest_verification.is_none());
    assert_eq!(
        full_treatment.outcome_class,
        Some(TrialOutcomeClass::CompletedFailed)
    );

    // The summary counts the exact states and classes.
    let summary = store
        .experiment_summary(experiment_id)
        .expect("experiment summary");
    assert_eq!(summary.planned, 0);
    assert_eq!(summary.executed, 0);
    assert_eq!(summary.verified, 0);
    assert_eq!(summary.sealed, 2);
    assert_eq!(summary.invalid, 0);
    assert_eq!(
        summary.outcome_class_counts,
        vec![
            ("completed_failed".to_string(), 1),
            ("completed_passed".to_string(), 1),
        ]
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn relationships_filter_by_experiment_and_trial_and_dedupe_artifacts() {
    let root = unique_dir("relationships");
    let evals_root = root.join("evals");
    let experiment_id = "eval-rel-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let cas = ArtifactStore::new(&evals_root);
    let db_path = store.path().to_path_buf();

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let first = trial_row(experiment_id, "trial-0001", "baseline", 1);
    let second = trial_row(experiment_id, "trial-0002", "treatment", 0);
    store.record_trial(first.clone()).expect("trial 1");
    store.record_trial(second.clone()).expect("trial 2");

    store
        .record_execution(execution_row("trial-0001", "exec-0001", "[]"))
        .expect("execution 1");
    store
        .record_verification(verification_row("trial-0001", "verify-0001"))
        .expect("verification 1");

    // Two manifests sharing one content address.
    let shared = cas
        .put("execution-stdout", b"shared bytes\n")
        .expect("shared artifact");
    let only_first = cas
        .put("verifier-stdout", b"first only\n")
        .expect("first artifact");
    let only_second = cas
        .put("verifier-stdout", b"second only\n")
        .expect("second artifact");
    let manifest_first = seal_for(&first, &[shared.clone(), only_first], 1_700_000_000_000);
    let manifest_second = seal_for(&second, &[shared.clone(), only_second], 1_700_000_000_100);
    store.record_evidence(&manifest_first).expect("evidence 1");
    store.record_evidence(&manifest_second).expect("evidence 2");

    // Trials are filtered by experiment and ordered by order_index.
    let trials = store.list_trials(experiment_id).expect("list trials");
    assert_eq!(trials.len(), 2);
    assert_eq!(trials[0].trial_id, "trial-0002", "order_index sorts first");
    assert_eq!(trials[1].trial_id, "trial-0001");
    assert!(store
        .list_trials("eval-somewhere-else")
        .expect("unknown experiment lists empty")
        .is_empty());

    // Executions/verifications/evidence filter by trial.
    assert_eq!(
        store
            .executions_for_trial("trial-0001")
            .expect("execs")
            .len(),
        1
    );
    assert!(store
        .executions_for_trial("trial-0002")
        .expect("execs")
        .is_empty());
    assert_eq!(
        store
            .verifications_for_trial("trial-0001")
            .expect("verifications")
            .len(),
        1
    );
    assert!(store
        .verifications_for_trial("trial-0002")
        .expect("verifications")
        .is_empty());
    assert_eq!(
        store
            .evidence_for_trial("trial-0001")
            .expect("evidence")
            .len(),
        1
    );
    assert_eq!(
        store
            .evidence_for_trial("trial-0002")
            .expect("evidence")
            .len(),
        1
    );

    // The artifact registry dedupes by content address; the join table keeps
    // one row per (seal, logical name).
    assert_eq!(count_rows(&db_path, "artifacts"), 3);
    assert_eq!(count_rows(&db_path, "evidence_artifacts"), 4);
    let row = store.artifact(&shared.sha256).expect("shared artifact");
    assert_eq!(row.bytes, shared.bytes);
    assert_eq!(row.logical_name, "execution-stdout");

    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Failed publication and duplicates
// ---------------------------------------------------------------------------

#[test]
fn failed_publication_rolls_back_every_row() {
    let root = unique_dir("rollback");
    let evals_root = root.join("evals");
    let experiment_id = "eval-rollback-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let cas = ArtifactStore::new(&evals_root);
    let db_path = store.path().to_path_buf();

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
    store.record_trial(trial.clone()).expect("trial");

    let first = cas
        .put("execution-stdout", b"first artifact\n")
        .expect("first artifact");
    let first_manifest = seal_for(&trial, std::slice::from_ref(&first), 1_700_000_000_000);
    store
        .record_evidence(&first_manifest)
        .expect("first evidence");

    // A manifest whose second artifact contradicts the registered byte count
    // of an already-registered content address: the insert fails after the
    // evidence row and the first artifact were written, so the whole
    // transaction must roll back.
    let fresh = cas
        .put("verifier-stdout", b"fresh artifact\n")
        .expect("fresh artifact");
    let conflict = ArtifactRef {
        logical_name: "conflict".to_string(),
        sha256: first.sha256.clone(),
        bytes: first.bytes + 5,
    };
    let failing = seal_for(&trial, &[fresh.clone(), conflict], 1_700_000_000_100);

    let error = store.record_evidence(&failing).expect_err("must fail");
    assert_eq!(error.kind(), StoreErrorKind::InvalidInput);

    // No partial rows survive: not the evidence row, not the fresh artifact
    // registry row, not the join row.
    assert_eq!(
        store
            .evidence_for_trial("trial-0001")
            .expect("evidence")
            .len(),
        1
    );
    assert_eq!(
        store
            .evidence(&failing.seal_sha256)
            .expect_err("failed seal must not exist")
            .kind(),
        StoreErrorKind::NotFound
    );
    assert_eq!(
        store
            .artifact(&fresh.sha256)
            .expect_err("fresh artifact must not be registered")
            .kind(),
        StoreErrorKind::NotFound
    );
    assert_eq!(count_rows(&db_path, "evidence"), 1);
    assert_eq!(count_rows(&db_path, "artifacts"), 1);
    assert_eq!(count_rows(&db_path, "evidence_artifacts"), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn duplicate_primary_keys_are_typed_errors() {
    let root = unique_dir("duplicates");
    let evals_root = root.join("evals");
    let experiment_id = "eval-dup-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let cas = ArtifactStore::new(&evals_root);

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    assert_eq!(
        store
            .record_experiment(experiment_row(experiment_id))
            .expect_err("duplicate experiment")
            .kind(),
        StoreErrorKind::Duplicate
    );

    let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
    store.record_trial(trial.clone()).expect("trial");
    assert_eq!(
        store
            .record_trial(trial.clone())
            .expect_err("duplicate trial")
            .kind(),
        StoreErrorKind::Duplicate
    );

    let execution = execution_row("trial-0001", "exec-0001", "[]");
    store
        .record_execution(execution.clone())
        .expect("execution");
    assert_eq!(
        store
            .record_execution(execution)
            .expect_err("duplicate execution")
            .kind(),
        StoreErrorKind::Duplicate
    );

    let verification = verification_row("trial-0001", "verify-0001");
    store
        .record_verification(verification.clone())
        .expect("verification");
    assert_eq!(
        store
            .record_verification(verification)
            .expect_err("duplicate verification")
            .kind(),
        StoreErrorKind::Duplicate
    );

    let artifact = cas
        .put("execution-stdout", b"duplicate evidence\n")
        .expect("artifact");
    let manifest = seal_for(&trial, &[artifact], 1_700_000_000_000);
    store.record_evidence(&manifest).expect("first evidence");
    assert_eq!(
        store
            .record_evidence(&manifest)
            .expect_err("duplicate evidence")
            .kind(),
        StoreErrorKind::Duplicate
    );

    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Integrity
// ---------------------------------------------------------------------------

#[test]
fn tampered_artifacts_and_manifest_rows_fail_integrity_naming_the_trial() {
    let root = unique_dir("tamper");
    let evals_root = root.join("evals");
    let experiment_id = "eval-tamper-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let cas = ArtifactStore::new(&evals_root);
    let db_path = store.path().to_path_buf();

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let trial = trial_row(experiment_id, "trial-0001", "treatment", 0);
    store.record_trial(trial.clone()).expect("trial");
    let artifact = cas
        .put("execution-stdout", b"baseline stdout\n")
        .expect("artifact");
    let manifest = seal_for(&trial, std::slice::from_ref(&artifact), 1_700_000_000_000);
    store.record_evidence(&manifest).expect("evidence");

    let report = store
        .verify_store_evidence(&cas)
        .expect("integrity check runs");
    assert!(report.is_intact(), "{report:?}");
    assert_eq!(report.checked, 1);

    // Tamper the artifact bytes (same length: only the hash can catch it).
    let artifact_path = cas.path_for(&artifact.sha256).expect("artifact path");
    fs::write(&artifact_path, b"tampered stdout\n").expect("tamper artifact");
    let report = store
        .verify_store_evidence(&cas)
        .expect("integrity check runs");
    assert!(!report.is_intact());
    let failure = report
        .failures_for_trial("trial-0001")
        .into_iter()
        .next()
        .expect("failure names the trial");
    assert_eq!(failure.check, StoreIntegrityCheck::Artifact);
    assert_eq!(failure.seal_sha256, manifest.seal_sha256);
    assert!(
        failure.message.contains("hash mismatch"),
        "unexpected message: {}",
        failure.message
    );

    // Restore the bytes; the manifest verifies again.
    fs::write(&artifact_path, b"baseline stdout\n").expect("restore artifact");
    assert!(store
        .verify_store_evidence(&cas)
        .expect("integrity check runs")
        .is_intact());

    // Tamper the stored manifest_json row: the re-seal check must catch it.
    let conn = raw_connection(&db_path);
    let stored_json: String = conn
        .query_row(
            "SELECT manifest_json FROM evidence WHERE seal_sha256 = ?1",
            params![manifest.seal_sha256],
            |row| row.get(0),
        )
        .expect("stored manifest json");
    let mut value: serde_json::Value =
        serde_json::from_str(&stored_json).expect("manifest json parses");
    value["sealed_at_ms"] = serde_json::json!(1_700_000_000_999u64);
    conn.execute(
        "UPDATE evidence SET manifest_json = ?1 WHERE seal_sha256 = ?2",
        params![value.to_string(), manifest.seal_sha256],
    )
    .expect("tamper manifest row");
    drop(conn);

    let report = store
        .verify_store_evidence(&cas)
        .expect("integrity check runs");
    assert!(!report.is_intact());
    let failure = report
        .failures_for_trial("trial-0001")
        .into_iter()
        .next()
        .expect("failure names the trial");
    assert_eq!(failure.check, StoreIntegrityCheck::Seal);
    assert!(
        failure.message.contains("seal mismatch"),
        "unexpected message: {}",
        failure.message
    );

    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Read-only mode
// ---------------------------------------------------------------------------

#[test]
fn read_only_store_refuses_every_write_and_still_reads() {
    let root = unique_dir("read-only");
    let evals_root = root.join("evals");
    let experiment_id = "eval-readonly-001";
    let cas = ArtifactStore::new(&evals_root);
    let manifest;
    let db_path;
    {
        let store = EvalStore::open(&evals_root, experiment_id).expect("open");
        db_path = store.path().to_path_buf();
        store
            .record_experiment(experiment_row(experiment_id))
            .expect("experiment");
        let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
        store.record_trial(trial.clone()).expect("trial");
        store
            .record_execution(execution_row("trial-0001", "exec-0001", "[]"))
            .expect("execution");
        let artifact = cas
            .put("execution-stdout", b"read-only bytes\n")
            .expect("artifact");
        manifest = seal_for(&trial, &[artifact], 1_700_000_000_000);
        store.record_evidence(&manifest).expect("evidence");
        store
            .set_trial_outcome("trial-0001", TrialOutcomeClass::CompletedPassed, None)
            .expect("outcome");
    }

    let store = EvalStore::open_read_only(&evals_root, experiment_id).expect("read-only open");
    assert!(store.is_read_only());
    assert_eq!(
        store.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );

    // Reads still work.
    assert_eq!(
        store
            .experiment(experiment_id)
            .expect("experiment")
            .plan_hash,
        digest("plan")
    );
    assert_eq!(
        store.trial("trial-0001").expect("trial").trial_id,
        "trial-0001"
    );
    assert_eq!(
        store.trial_full("trial-0001").expect("full").outcome_class,
        Some(TrialOutcomeClass::CompletedPassed)
    );
    assert_eq!(store.list_trials(experiment_id).expect("trials").len(), 1);
    assert_eq!(
        store
            .executions_for_trial("trial-0001")
            .expect("execs")
            .len(),
        1
    );
    assert_eq!(
        store
            .evidence_for_trial("trial-0001")
            .expect("evidence")
            .len(),
        1
    );
    assert_eq!(
        store
            .experiment_summary(experiment_id)
            .expect("summary")
            .sealed,
        1
    );

    // Every write path is refused with the typed read-only error.
    let trial = trial_row(experiment_id, "trial-0002", "treatment", 1);
    let execution = execution_row("trial-0001", "exec-0002", "[]");
    let verification = verification_row("trial-0001", "verify-0001");
    for (operation, error) in [
        (
            "record_experiment",
            store
                .record_experiment(experiment_row(experiment_id))
                .expect_err("experiment"),
        ),
        (
            "record_trial",
            store.record_trial(trial).expect_err("trial"),
        ),
        (
            "record_execution",
            store.record_execution(execution).expect_err("execution"),
        ),
        (
            "record_verification",
            store
                .record_verification(verification)
                .expect_err("verification"),
        ),
        (
            "record_evidence",
            store.record_evidence(&manifest).expect_err("evidence"),
        ),
        (
            "set_trial_outcome",
            store
                .set_trial_outcome("trial-0001", TrialOutcomeClass::CompletedPassed, None)
                .expect_err("outcome"),
        ),
    ] {
        assert_eq!(error.kind(), StoreErrorKind::ReadOnly, "{operation}");
    }

    // Nothing was mutated, and the read-only open created no sidecars.
    assert_eq!(count_rows(&db_path, "trials"), 1);
    assert!(!PathBuf::from(format!("{}-wal", db_path.display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", db_path.display())).exists());

    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Missing telemetry, invalid outcomes, backups, validation
// ---------------------------------------------------------------------------

#[test]
fn missing_telemetry_is_stored_exactly_and_never_coerced() {
    let root = unique_dir("missing-telemetry");
    let evals_root = root.join("evals");
    let experiment_id = "eval-telemetry-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
    store.record_trial(trial).expect("trial");

    store
        .record_execution(execution_row("trial-0001", "exec-0001", "[\"cost\"]"))
        .expect("execution with missing cost");
    store
        .record_execution(execution_row("trial-0001", "exec-0002", "[]"))
        .expect("execution with no missing telemetry");

    let executions = store.executions_for_trial("trial-0001").expect("execs");
    assert_eq!(executions.len(), 2);
    assert_eq!(
        executions[0].missing_telemetry_json, "[\"cost\"]",
        "missing telemetry must round-trip exactly, never coerced to empty"
    );
    assert_eq!(
        executions[0].missing_telemetry().expect("parse"),
        vec!["cost".to_string()]
    );
    assert_eq!(executions[1].missing_telemetry_json, "[]");
    assert!(executions[1].missing_telemetry().expect("parse").is_empty());
    assert_eq!(
        executions[0].session_ids().expect("sessions"),
        vec!["ses_store_0001".to_string()]
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn invalid_outcome_marks_state_and_a_retry_recovers() {
    let root = unique_dir("invalid-state");
    let evals_root = root.join("evals");
    let experiment_id = "eval-invalid-001";
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");

    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");
    let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
    store.record_trial(trial).expect("trial");
    store
        .record_execution(execution_row("trial-0001", "exec-0001", "[]"))
        .expect("execution");
    assert_eq!(
        store.trial("trial-0001").expect("trial").state,
        TrialState::Executed
    );

    store
        .set_trial_outcome(
            "trial-0001",
            TrialOutcomeClass::SpawnFailed,
            Some("harness binary missing".to_string()),
        )
        .expect("invalid outcome");
    let row = store.trial("trial-0001").expect("trial");
    assert_eq!(row.state, TrialState::Invalid);
    assert_eq!(row.outcome_class, Some(TrialOutcomeClass::SpawnFailed));
    assert_eq!(
        row.invalid_reason.as_deref(),
        Some("harness binary missing")
    );
    assert_eq!(
        store
            .experiment_summary(experiment_id)
            .expect("summary")
            .invalid,
        1
    );

    // A retry (new execution attempt) does not clear the invalid class.
    store
        .record_execution(execution_row("trial-0001", "exec-0002", "[]"))
        .expect("retry execution");
    assert_eq!(
        store.trial("trial-0001").expect("trial").state,
        TrialState::Invalid,
        "an invalid outcome class wins over a new attempt"
    );

    // Recording a non-invalid outcome recomputes the milestone state.
    store
        .set_trial_outcome("trial-0001", TrialOutcomeClass::CompletedPassed, None)
        .expect("recovered outcome");
    let row = store.trial("trial-0001").expect("trial");
    assert_eq!(
        row.state,
        TrialState::Executed,
        "no verification or evidence yet"
    );
    assert_eq!(row.outcome_class, Some(TrialOutcomeClass::CompletedPassed));
    assert_eq!(row.invalid_reason, None);
    let summary = store.experiment_summary(experiment_id).expect("summary");
    assert_eq!(summary.invalid, 0);
    assert_eq!(summary.executed, 1);
    assert_eq!(
        summary.outcome_class_counts,
        vec![("completed_passed".to_string(), 1)]
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn pre_migration_backup_is_a_noop_for_the_current_schema() {
    let root = unique_dir("backup");
    let evals_root = root.join("evals");
    let experiment_id = "eval-backup-001";

    // Fresh open and reopen: no backup for the current schema.
    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    let db_path = store.path().to_path_buf();
    drop(store);
    let store = EvalStore::open(&evals_root, experiment_id).expect("reopen");
    assert_eq!(
        store.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );
    drop(store);
    assert!(!backup_path(&db_path, 1).exists());

    // A database whose meta table exists with no version row reads as version
    // 0; version 0 is "no schema yet" and must not be backed up either.
    let second_id = "eval-backup-002";
    let second_dir = evals_root.join(second_id);
    fs::create_dir_all(&second_dir).expect("second dir");
    let second_db = second_dir.join("state.db");
    raw_connection(&second_db)
        .execute_batch("CREATE TABLE meta (schema_version INTEGER NOT NULL);")
        .expect("seed empty meta");
    let store = EvalStore::open(&evals_root, second_id).expect("open version 0");
    assert_eq!(
        store.schema_version().expect("version"),
        EVAL_STORE_SCHEMA_VERSION
    );
    drop(store);
    assert!(!backup_path(&second_db, 0).exists());
    assert!(!backup_path(&second_db, 1).exists());

    // With the current schema version at 1, `current > 0 && current < 1` is
    // unsatisfiable; the guard's full truth table is unit-tested in store.rs.
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn validation_rejects_malformed_ids_digests_and_json() {
    let root = unique_dir("validation");
    let evals_root = root.join("evals");
    let experiment_id = "eval-validation-001";

    assert_eq!(
        EvalStore::open(&evals_root, "")
            .expect_err("empty id")
            .kind(),
        StoreErrorKind::InvalidInput
    );
    assert_eq!(
        EvalStore::open(&evals_root, "../escape")
            .expect_err("path escape")
            .kind(),
        StoreErrorKind::InvalidInput
    );

    let store = EvalStore::open(&evals_root, experiment_id).expect("open");
    store
        .record_experiment(experiment_row(experiment_id))
        .expect("experiment");

    // Cross-experiment rows are refused.
    let mut foreign = trial_row("eval-somewhere-else", "trial-foreign", "baseline", 0);
    assert_eq!(
        store
            .record_trial(foreign.clone())
            .expect_err("foreign experiment")
            .kind(),
        StoreErrorKind::InvalidInput
    );

    // Malformed digests are refused.
    foreign.experiment_id = experiment_id.to_string();
    foreign.plan_hash = "sha256:PENDING".to_string();
    assert_eq!(
        store
            .record_trial(foreign.clone())
            .expect_err("pending digest")
            .kind(),
        StoreErrorKind::InvalidInput
    );

    let trial = trial_row(experiment_id, "trial-0001", "baseline", 0);
    store.record_trial(trial).expect("trial");

    // Malformed JSON columns are refused.
    let mut execution = execution_row("trial-0001", "exec-0001", "not json");
    assert_eq!(
        store
            .record_execution(execution.clone())
            .expect_err("bad json")
            .kind(),
        StoreErrorKind::InvalidInput
    );
    execution.missing_telemetry_json = "[\"cost\"]".to_string();
    execution.session_ids_json = "{\"session\":\"x\"}".to_string();
    assert_eq!(
        store
            .record_execution(execution)
            .expect_err("wrong json shape")
            .kind(),
        StoreErrorKind::InvalidInput
    );

    // Unknown trials are typed not-found errors.
    assert_eq!(
        store
            .record_execution(execution_row("trial-ghost", "exec-ghost", "[]"))
            .expect_err("missing trial")
            .kind(),
        StoreErrorKind::NotFound
    );

    // Outcome coherence: invalid classes need a reason; valid classes must not
    // carry one.
    assert_eq!(
        store
            .set_trial_outcome("trial-0001", TrialOutcomeClass::VerifierMutation, None)
            .expect_err("invalid class without reason")
            .kind(),
        StoreErrorKind::InvalidInput
    );
    assert_eq!(
        store
            .set_trial_outcome(
                "trial-0001",
                TrialOutcomeClass::CompletedPassed,
                Some("no".to_string()),
            )
            .expect_err("valid class with reason")
            .kind(),
        StoreErrorKind::InvalidInput
    );

    // A manifest whose seal does not verify is refused as an integrity error.
    let cas = ArtifactStore::new(&evals_root);
    let artifact = cas
        .put("execution-stdout", b"validation bytes\n")
        .expect("artifact");
    let trial = store.trial("trial-0001").expect("trial");
    let mut manifest = seal_for(&trial, &[artifact], 1_700_000_000_000);
    manifest.sealed_at_ms = 1_700_000_000_001;
    assert_eq!(
        store
            .record_evidence(&manifest)
            .expect_err("tampered seal")
            .kind(),
        StoreErrorKind::Integrity
    );

    let _ = fs::remove_dir_all(&root);
}
