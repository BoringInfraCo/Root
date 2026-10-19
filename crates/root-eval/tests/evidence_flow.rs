//! Milestone 3, Phase A: end-to-end evidence flow over the public API.
//!
//! This is the worked example the eval store (M3-B) builds on: one supervised
//! harness run → execution digest → redaction → content-addressed artifact →
//! verifier run → repository diff → sealed manifest → verification →
//! classification → tamper detection.
//!
//! Crate-local only: temporary trees under unique `std::env::temp_dir()` roots;
//! `ROOT_DIR` is never set. Processes spawned: the fake harness under
//! `run_supervised`, the verifier commands under `run_verifier`, and bounded
//! `git` setup/`capture_diff` calls — nothing else.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    capture_diff, classify_trial, redact_bytes, run_supervised, run_verifier, seal_manifest,
    verify_artifacts, verify_seal, ArtifactStore, DiffCaps, ExecutionDigest, SealErrorKind,
    SealInputs, SupervisorConfig, TrialOutcomeClass, VerifierDigest, VerifierOutcome, VerifierSpec,
    VerifierStep, VerifierVerdict, DEFAULT_DIFF_TIMEOUT_MS, REDACTION_SECRET_KEY_ASSIGNMENT,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m3-flow-{}-{}-{}",
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

fn git(args: &[&str], cwd: &Path) {
    let output = Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod +x");
    }
}

const SECRET_LINE: &str = "GITHUB_TOKEN=ghp_flow_test_secret";

#[test]
fn supervised_run_flows_into_a_sealed_and_verified_manifest() {
    let root = unique_dir("seal-flow");
    let repo = root.join("repo");
    let evals_root = root.join("evals");
    fs::create_dir_all(&repo).expect("create repo dir");

    // A real repository with one committed file, one tracked modification,
    // and one untracked addition.
    fs::write(repo.join("README.md"), "fixture\n").expect("write README");
    git(&["init"], &repo);
    git(&["add", "."], &repo);
    git(
        &[
            "-c",
            "user.name=flow-test",
            "-c",
            "user.email=flow-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
        &repo,
    );
    fs::write(repo.join("README.md"), "fixture changed\n").expect("modify README");
    fs::write(repo.join("untracked.txt"), "agent output\n").expect("add untracked");

    // 1. Supervised harness run: golden-shaped NDJSON plus a secret line.
    let harness = repo.join("harness.sh");
    write_executable(
        &harness,
        &format!(
            r#"#!/bin/sh
echo '{{"type":"step_start","timestamp":1,"sessionID":"ses_flow","part":{{"type":"step-start"}}}}'
echo '{SECRET_LINE}'
echo '{{"type":"step_finish","timestamp":2,"sessionID":"ses_flow","part":{{"type":"step-finish","reason":"stop","tokens":{{"total":10,"input":8,"output":1,"reasoning":1,"cache":{{"write":0,"read":0}}}},"cost":0}}}}'
"#
        ),
    );
    let supervised = run_supervised(
        &[harness.to_string_lossy().into_owned()],
        &[],
        &repo,
        &SupervisorConfig::new(10_000, 1024 * 1024),
    );
    assert_eq!(supervised.termination.as_str(), "completed");
    assert!(
        String::from_utf8_lossy(&supervised.raw_stdout).contains("ghp_flow_test_secret"),
        "the raw artifact carries the secret until redaction"
    );

    // 2. Execution digest from the supervised result.
    let execution = ExecutionDigest::from_supervised(&supervised).expect("execution digest");
    assert!(execution.raw_sha256.starts_with("sha256:"));

    // 3. Redact before persistence, then store the bounded artifact.
    let (redacted, redactions) = redact_bytes(&supervised.raw_stdout);
    assert_eq!(redactions.len(), 1);
    assert_eq!(redactions[0].pattern, REDACTION_SECRET_KEY_ASSIGNMENT);
    assert_eq!(redactions[0].count, 1);
    assert!(!String::from_utf8_lossy(&redacted).contains("ghp_flow_test_secret"));

    let store = ArtifactStore::new(&evals_root);
    let stdout_artifact = store
        .put("execution-stdout", &redacted)
        .expect("put stdout");
    let stored = store.get(&stdout_artifact.sha256).expect("get stdout");
    assert_eq!(stored, redacted);
    assert!(!String::from_utf8_lossy(&stored).contains("ghp_flow_test_secret"));

    // 4. Repository change capture.
    let diff = capture_diff(
        &repo,
        &DiffCaps {
            max_bytes: 512 * 1024,
            timeout_ms: DEFAULT_DIFF_TIMEOUT_MS,
        },
    )
    .expect("capture diff");
    assert!(diff.status_porcelain.contains("README.md"));
    assert!(
        diff.untracked.contains(&"untracked.txt".to_string()),
        "untracked additions must be listed: {:?}",
        diff.untracked
    );
    assert!(diff.diff_text.contains("+fixture changed"));

    // 5. Verifier run: one passing command; the verifier must not mutate.
    let verifier_script = repo.join("verify.sh");
    write_executable(&verifier_script, "#!/bin/sh\nexit 0\n");
    let spec = VerifierSpec::new(
        "flow-verifier",
        vec![VerifierStep {
            argv: vec![
                "sh".to_string(),
                verifier_script.to_string_lossy().into_owned(),
            ],
            expected_exit: 0,
        }],
    );
    let outcome: VerifierOutcome = run_verifier(&spec, &repo, &[]);
    assert_eq!(outcome.verdict, VerifierVerdict::Pass);
    assert!(!outcome.mutation_detected);
    assert_eq!(outcome.pre_tree_sha256, outcome.post_tree_sha256);

    // 6. Seal the manifest.
    let manifest = seal_manifest(SealInputs {
        experiment_id: "eval-flow-001",
        trial_id: "trial-flow-0001",
        plan_hash: &root_eval::sha256_digest(b"plan"),
        task_content_sha256: &root_eval::sha256_digest(b"task"),
        manifest_sha256: &root_eval::sha256_digest(b"context"),
        execution: &execution,
        repo_diff_sha256: &diff.diff_sha256,
        verifier: Some(&VerifierDigest::from(&outcome)),
        artifacts: std::slice::from_ref(&stdout_artifact),
        redactions: &redactions,
        sealed_at_ms: 1_700_000_000_000,
    })
    .expect("seal");

    // 7. Verification: seal, artifacts, and classification.
    verify_seal(&manifest).expect("seal verifies");
    verify_artifacts(&manifest, &store).expect("artifacts verify");
    assert_eq!(
        classify_trial(&supervised.termination, Some(&outcome)),
        TrialOutcomeClass::CompletedPassed
    );

    // 8. Tamper detection: any artifact change fails verification.
    let path = store.path_for(&stdout_artifact.sha256).expect("path");
    let mut tampered = stored.clone();
    tampered[0] ^= 0x01;
    fs::write(&path, &tampered).expect("tamper");
    assert_eq!(
        verify_artifacts(&manifest, &store)
            .expect_err("tamper must fail")
            .kind(),
        SealErrorKind::ArtifactHashMismatch
    );
    // A mutated manifest field fails too.
    let mut mutated = manifest.clone();
    mutated.trial_id = "trial-flow-9999".to_string();
    assert_eq!(
        verify_seal(&mutated)
            .expect_err("mutation must fail")
            .kind(),
        SealErrorKind::SealMismatch
    );

    let _ = fs::remove_dir_all(&root);
}
