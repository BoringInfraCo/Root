//! Milestone 5: spec-conformant fake-tool materialization tests.
//!
//! Exercises [`root_eval::materialize_trial_with_fake_tools`]: normative
//! profile shims, the canonical audit-log location, the stub-bin directory,
//! the plan-based environment, and fail-closed digest verification. Legacy
//! [`root_eval::materialize_trial`] behavior is covered by
//! `trial_materializer.rs` and stays untouched.
//!
//! Temporary trees live under unique `std::env::temp_dir()` subdirectories;
//! `ROOT_DIR` is never set. The only processes spawned are the bounded `git`
//! invocations inside the materializer, the test's git fixture setup, and the
//! POSIX shell shims under test.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    allowed_env, materialize_fake_tools, materialize_trial, materialize_trial_with_fake_tools,
    script_sha256, trial_stub_bin_dir, ConditionOverlay, ContextRoots, DiscoveryFlags,
    EffectiveContextManifest, FakeToolBinding, MaterializerErrorKind, PermissionSeeding, ProfileId,
    RepositoryLock, TrialSpec, ABSENT_PROFILE_TOKEN, FAKE_TOOL_AUDIT_RELATIVE_PATH,
    STUB_BIN_DIR_NAME,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5-mat-{}-{}-{}",
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

fn git(args: &[&str], cwd: &Path) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn fake_digest(seed: &str) -> String {
    format!("sha256:{}", root_lockfile::compute_sha256(seed.as_bytes()))
}

fn init_source_repo() -> (PathBuf, String) {
    let dir = unique_dir("source");
    fs::write(dir.join("README.md"), "# fixture\n").expect("write README");
    git(&["init"], &dir);
    git(&["add", "."], &dir);
    git(
        &[
            "-c",
            "user.name=trial-test",
            "-c",
            "user.email=trial-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
        &dir,
    );
    let commit = git(&["rev-parse", "HEAD"], &dir);
    (dir, commit)
}

fn resolve_base_manifest() -> EffectiveContextManifest {
    let base = unique_dir("base-tree");
    for dir in ["home/config", "project"] {
        fs::create_dir_all(base.join(dir)).expect("create base dirs");
    }
    let home = base.join("home");
    let xdg = base.join("home/config");
    let project = base.join("project");
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    root_eval::resolve_manifest(&roots, &DiscoveryFlags::default()).expect("resolve fixture")
}

fn make_spec(trial_id: &str, source: &Path, commit: &str) -> TrialSpec {
    TrialSpec {
        trial_id: trial_id.to_string(),
        experiment_id: "exp-m5-001".to_string(),
        task_id: "install-ripgrep-001".to_string(),
        task_version: 1,
        condition_id: "treatment".to_string(),
        manifest: resolve_base_manifest(),
        overlay: ConditionOverlay { entries: vec![] },
        plan_hash: fake_digest("plan"),
        repository: RepositoryLock {
            commit: commit.to_string(),
            tree_sha256: fake_digest("tree"),
        },
        worktree_source: source.to_path_buf(),
    }
}

fn binding(provides: &str, profile: ProfileId) -> FakeToolBinding {
    FakeToolBinding::new(provides, profile.token(), Some(script_sha256(profile)))
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn fake_bin_names(layout: &root_eval::TrialLayout) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(&layout.fake_bin_dir)
        .expect("read fake-bin")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// 1. Plan-based materialization: normative shims, audit log, stub-bin, env
// ---------------------------------------------------------------------------

#[test]
fn plan_based_materialization_writes_normative_shims() {
    let base_dir = unique_dir("plan-base");
    let (source, commit) = init_source_repo();
    let spec = make_spec("trial-plan", &source, &commit);
    let bindings = vec![
        binding("root", ProfileId::RootHealthyV1),
        binding("brew", ProfileId::InstallerRecordDenyV1),
        FakeToolBinding::new("curl", ABSENT_PROFILE_TOKEN, None),
    ];

    let layout =
        materialize_trial_with_fake_tools(&spec, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect("plan-based materialization");

    // Shims present/absent exactly per manifest.
    assert_eq!(fake_bin_names(&layout), vec!["brew", "root"]);
    assert!(!layout.fake_bin_dir.join("curl").exists());
    let root_shim = fs::read(layout.fake_bin_dir.join("root")).expect("read root shim");
    assert_eq!(
        root_eval::sha256_digest(&root_shim),
        script_sha256(ProfileId::RootHealthyV1)
    );
    let brew_shim = fs::read(layout.fake_bin_dir.join("brew")).expect("read brew shim");
    assert_eq!(
        root_eval::sha256_digest(&brew_shim),
        script_sha256(ProfileId::InstallerRecordDenyV1)
    );
    #[cfg(unix)]
    {
        assert_eq!(mode_of(&layout.fake_bin_dir.join("root")), 0o700);
        assert_eq!(mode_of(&layout.fake_bin_dir.join("brew")), 0o700);
    }

    // Canonical audit log location, empty, and outside the fixture tree.
    let audit_log = layout.root.join(FAKE_TOOL_AUDIT_RELATIVE_PATH);
    assert_eq!(layout.audit_log, audit_log);
    assert!(audit_log.is_file());
    assert_eq!(fs::read(&audit_log).expect("read audit"), b"");
    assert!(!layout.repo_dir.join("audit").exists());

    // Stub-bin directory.
    let stub_bin = layout.root.join(STUB_BIN_DIR_NAME);
    assert_eq!(trial_stub_bin_dir(&layout), Some(stub_bin.clone()));
    assert!(stub_bin.is_dir());
    assert_eq!(fs::read_dir(&stub_bin).expect("stub dir").count(), 0);
    #[cfg(unix)]
    assert_eq!(mode_of(&stub_bin), 0o700);

    // Environment: TRIAL_STUB_BIN and PATH include the stub dir.
    let env = allowed_env(&layout);
    assert_eq!(
        env.get("TRIAL_STUB_BIN").map(String::as_str),
        Some(stub_bin.to_string_lossy().as_ref())
    );
    let expected_path = format!(
        "{}:{}",
        layout.fake_bin_dir.to_string_lossy(),
        stub_bin.to_string_lossy()
    );
    assert_eq!(
        env.get("PATH").map(String::as_str),
        Some(expected_path.as_str())
    );
    assert_eq!(
        env.get("TRIAL_AUDIT_LOG").map(String::as_str),
        Some(audit_log.to_string_lossy().as_ref())
    );
    let env_file = fs::read_to_string(&layout.env_file).expect("read env file");
    assert!(env_file.contains("TRIAL_STUB_BIN="));
    assert!(env_file.contains("TRIAL_AUDIT_LOG="));

    // Residue bookkeeping includes every new path.
    for path in [&audit_log, &stub_bin, &layout.fake_bin_dir.join("root")] {
        assert!(
            layout.created_paths.contains(path),
            "created_paths must list {}",
            path.display()
        );
    }
    let manifest = fs::read_to_string(&layout.residue_manifest_file).expect("read manifest");
    for path in [&audit_log, &stub_bin] {
        assert!(
            manifest.contains(path.to_string_lossy().as_ref()),
            "residue manifest must list {}",
            path.display()
        );
    }

    // End-to-end: the shim audits into the canonical log, and `install`
    // populates the stub-bin directory with an executable stub.
    let run = |argv: &[&str]| {
        Command::new(layout.fake_bin_dir.join(argv[0]))
            .args(&argv[1..])
            .env_clear()
            .envs(env.iter())
            .output()
            .expect("run shim")
    };
    let doctor = run(&["root", "doctor", "--json"]);
    assert!(doctor.status.success());
    let install = run(&["root", "install", "ripgrep", "--json"]);
    assert!(install.status.success());
    let stub = stub_bin.join("rg");
    assert!(stub.is_file(), "install creates the rg stub in stub-bin");
    let stub_run = Command::new(&stub)
        .arg("--version")
        .env_clear()
        .output()
        .expect("run stub");
    assert!(stub_run.status.success());
    assert_eq!(
        String::from_utf8_lossy(&stub_run.stdout),
        "rg 1.0.0 (trial stub)\n"
    );

    let audit_lines: Vec<String> = fs::read_to_string(&audit_log)
        .expect("read audit")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(audit_lines.len(), 2, "one line per shim invocation");
    assert!(audit_lines[0].contains("\"tool\":\"root\""));
    assert!(audit_lines[0].contains("\"profile\":\"root.healthy.v1\""));
    assert!(audit_lines[1].contains("\"argv\":[\"install\",\"ripgrep\",\"--json\"]"));

    // The strict side-channel parser (M5 acceptance engine) accepts the log
    // byte-for-byte: exactly five keys, compact JSON, seq 1..n, args-only
    // argv that its tool-prefix recovery turns back into the full command.
    let parsed = root_eval::parse_audit_log(&fs::read(&audit_log).expect("read audit"))
        .expect("audit log is contract-conformant");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].seq, 1);
    assert_eq!(parsed[0].tool, "root");
    assert_eq!(parsed[0].argv, vec!["doctor", "--json"]);
    assert_eq!(parsed[0].exit, 0);
    assert_eq!(parsed[0].profile, "root.healthy.v1");
    assert_eq!(parsed[1].argv, vec!["install", "ripgrep", "--json"]);

    root_eval::cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 2. Digest mismatch fails closed before any shim is written
// ---------------------------------------------------------------------------

#[test]
fn hash_mismatch_fails_closed() {
    let base_dir = unique_dir("mismatch-base");
    let (source, commit) = init_source_repo();
    let spec = make_spec("trial-mismatch", &source, &commit);
    let bindings = vec![FakeToolBinding::new(
        "root",
        "root.healthy.v1",
        Some("sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string()),
    )];
    let error =
        materialize_trial_with_fake_tools(&spec, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect_err("digest mismatch must fail closed");
    assert_eq!(error.kind(), MaterializerErrorKind::DigestMismatch);
    assert!(
        !base_dir.join("trial-mismatch/fake-bin/root").exists(),
        "no shim may be written on a digest mismatch"
    );
}

// ---------------------------------------------------------------------------
// 3. Two trials' audit logs are disjoint
// ---------------------------------------------------------------------------

#[test]
fn two_trials_have_disjoint_audit_logs() {
    let base_dir = unique_dir("disjoint-base");
    let (source, commit) = init_source_repo();
    let bindings = vec![binding("root", ProfileId::RootHealthyV1)];

    let spec_a = make_spec("trial-audit-a", &source, &commit);
    let spec_b = make_spec("trial-audit-b", &source, &commit);
    let layout_a = materialize_trial_with_fake_tools(
        &spec_a,
        &base_dir,
        &bindings,
        PermissionSeeding::AskMode,
    )
    .expect("materialize A");
    let layout_b = materialize_trial_with_fake_tools(
        &spec_b,
        &base_dir,
        &bindings,
        PermissionSeeding::AskMode,
    )
    .expect("materialize B");

    assert_ne!(layout_a.audit_log, layout_b.audit_log);
    assert_ne!(trial_stub_bin_dir(&layout_a), trial_stub_bin_dir(&layout_b));

    fs::write(&layout_a.audit_log, "{\"seq\":1}\n").expect("write A audit");
    assert_eq!(fs::read(&layout_b.audit_log).expect("read B audit"), b"");

    root_eval::cleanup_trial(&layout_a).expect("cleanup A");
    root_eval::cleanup_trial(&layout_b).expect("cleanup B");
}

// ---------------------------------------------------------------------------
// 4. Empty bindings write no shims (still spec-conformant env)
// ---------------------------------------------------------------------------

#[test]
fn empty_bindings_write_no_shims() {
    let base_dir = unique_dir("empty-base");
    let (source, commit) = init_source_repo();
    let spec = make_spec("trial-empty", &source, &commit);
    let layout =
        materialize_trial_with_fake_tools(&spec, &base_dir, &[], PermissionSeeding::AskMode)
            .expect("materialize");

    assert!(fake_bin_names(&layout).is_empty());
    assert!(layout.audit_log.is_file());
    assert_eq!(fs::read(&layout.audit_log).expect("read audit"), b"");
    assert_eq!(
        trial_stub_bin_dir(&layout),
        Some(layout.root.join(STUB_BIN_DIR_NAME))
    );
    assert!(allowed_env(&layout).contains_key("TRIAL_STUB_BIN"));

    // The legacy entry point is untouched: generic shims, `audit.log`, and no
    // stub-bin overlay.
    let legacy_spec = make_spec("trial-legacy", &source, &commit);
    let legacy = materialize_trial(&legacy_spec, &base_dir).expect("legacy materialize");
    assert_eq!(
        fake_bin_names(&legacy),
        vec!["brew", "curl", "npm", "pip", "root"]
    );
    assert_eq!(legacy.audit_log, legacy.root.join("audit.log"));
    assert_eq!(trial_stub_bin_dir(&legacy), None);
    assert!(!allowed_env(&legacy).contains_key("TRIAL_STUB_BIN"));
    assert_eq!(
        allowed_env(&legacy).get("PATH").map(String::as_str),
        Some(legacy.fake_bin_dir.to_string_lossy().as_ref())
    );

    root_eval::cleanup_trial(&layout).expect("cleanup");
    root_eval::cleanup_trial(&legacy).expect("cleanup legacy");
}

// ---------------------------------------------------------------------------
// 5. Materializing twice under the same id still fails closed
// ---------------------------------------------------------------------------

#[test]
fn plan_based_requires_fresh_directory() {
    let base_dir = unique_dir("fresh-base");
    let (source, commit) = init_source_repo();
    let spec = make_spec("trial-once", &source, &commit);
    let bindings = vec![binding("root", ProfileId::RootHealthyV1)];
    let layout =
        materialize_trial_with_fake_tools(&spec, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect("first run");
    let error =
        materialize_trial_with_fake_tools(&spec, &base_dir, &bindings, PermissionSeeding::AskMode)
            .expect_err("second run refused");
    assert_eq!(error.kind(), MaterializerErrorKind::TrialExists);
    root_eval::cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 6. `materialize_fake_tools` is reusable standalone (scratch dirs)
// ---------------------------------------------------------------------------

#[test]
fn standalone_writer_creates_audit_dir_and_stub_dir() {
    let scratch = unique_dir("standalone");
    let bin = scratch.join("bin");
    let stub = scratch.join("stub");
    let log = scratch.join("audit/fake-tool-audit.jsonl");
    let bindings = vec![binding("root", ProfileId::RootHealthyV1)];
    let layout = materialize_fake_tools(&bin, &log, &stub, &bindings).expect("materialize");
    assert_eq!(layout.fake_bin_dir, bin);
    assert_eq!(layout.stub_bin_dir, stub);
    assert_eq!(layout.audit_log, log);
    assert!(log.is_file());
    assert!(stub.is_dir());
    let map: BTreeMap<String, String> = allowed_env_for_scratch(&bin, &stub, &log);
    assert!(map.contains_key("TRIAL_STUB_BIN"));
}

/// Build the plan-based env shape from a scratch layout without a `TrialSpec`.
fn allowed_env_for_scratch(
    fake_bin: &Path,
    stub_bin: &Path,
    audit_log: &Path,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(
        "PATH".to_string(),
        format!(
            "{}:{}",
            fake_bin.to_string_lossy(),
            stub_bin.to_string_lossy()
        ),
    );
    env.insert(
        "TRIAL_STUB_BIN".to_string(),
        stub_bin.to_string_lossy().into_owned(),
    );
    env.insert(
        "TRIAL_AUDIT_LOG".to_string(),
        audit_log.to_string_lossy().into_owned(),
    );
    env
}
