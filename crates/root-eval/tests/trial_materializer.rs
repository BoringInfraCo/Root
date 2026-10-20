//! Milestone 2, Track A: isolated trial materialization tests.
//!
//! Crate-local tests only. Temporary trees live under unique
//! `std::env::temp_dir()` subdirectories; `ROOT_DIR` is never set. The only
//! processes ever spawned are the bounded `git` invocations inside
//! `materialize_trial` (checkout/verification) and the test setup that
//! builds the source repository — no harness, agent, or network process.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    allowed_env, check_no_leak, cleanup_trial, materialize_trial, residue_scan, scrub_env,
    verify_declared_delta, verify_isolation, ConditionOverlay, ContextRoots, DiscoveryFlags,
    EffectiveContextManifest, IsolationErrorKind, MaterializerErrorKind, OverlayAction,
    OverlayEntry, RepositoryLock, Residue, TrialSpec,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m2-{}-{}-{}",
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

/// A minimal git repository with one committed file; returns `(dir, commit)`.
fn init_source_repo() -> (PathBuf, String) {
    let dir = unique_dir("source");
    fs::write(dir.join("README.md"), "# fixture\n").expect("write README");
    fs::write(dir.join("main.txt"), "hello\n").expect("write main");
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
    assert_eq!(commit.len(), 40, "expected a full commit sha");
    (dir, commit)
}

const SKILL_BODY: &str = "---\nname: root\ndescription: fixture skill\n---\n\n# Root\n\nBody.\n";

/// Build `(base_tree, treatment_tree)` fixture roots for manifest resolution.
fn fixture_trees() -> (PathBuf, PathBuf) {
    let base = unique_dir("base-tree");
    for dir in ["home/config", "project"] {
        fs::create_dir_all(base.join(dir)).expect("create base dirs");
    }
    let treatment = unique_dir("treatment-tree");
    for dir in ["home/config", "project"] {
        fs::create_dir_all(treatment.join(dir)).expect("create treatment dirs");
    }
    let skill_dir = treatment.join("project/.opencode/skill/root");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(skill_dir.join("SKILL.md"), SKILL_BODY).expect("write skill");
    (base, treatment)
}

fn resolve(tree: &Path) -> EffectiveContextManifest {
    let home = tree.join("home");
    let xdg = tree.join("home/config");
    let project = tree.join("project");
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    root_eval::resolve_manifest(&roots, &DiscoveryFlags::default()).expect("resolve fixture")
}

fn make_spec(
    trial_id: &str,
    condition_id: &str,
    manifest: EffectiveContextManifest,
    overlay: ConditionOverlay,
    source: &Path,
    commit: &str,
    tree_sha: &str,
) -> TrialSpec {
    TrialSpec {
        trial_id: trial_id.to_string(),
        experiment_id: "exp-materializer-001".to_string(),
        task_id: "install-ripgrep-001".to_string(),
        task_version: 1,
        condition_id: condition_id.to_string(),
        manifest,
        overlay,
        plan_hash: fake_digest("plan"),
        repository: RepositoryLock {
            commit: commit.to_string(),
            tree_sha256: tree_sha.to_string(),
        },
        worktree_source: source.to_path_buf(),
    }
}

/// Relative repo files excluding `.git/` (clone metadata is not compared).
fn repo_files(repo: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path).expect("stat");
            if metadata.is_dir() {
                walk(&path, root, out);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if !relative.starts_with(".git/") && relative != ".git" {
                    out.push(relative);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(repo, repo, &mut files);
    files
}

fn read_repo(repo: &Path, relative: &str) -> Vec<u8> {
    fs::read(repo.join(relative))
        .unwrap_or_else(|error| panic!("read {}/{}: {error}", repo.display(), relative))
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

// ---------------------------------------------------------------------------
// 1. Two trials are disjoint and mutually invisible
// ---------------------------------------------------------------------------

#[test]
fn two_trials_are_disjoint_and_isolated() {
    let base_dir = unique_dir("base");
    let (source, commit) = init_source_repo();
    let tree_sha = fake_digest("tree");
    let (base_tree, _) = fixture_trees();
    let manifest = resolve(&base_tree);

    let spec_a = make_spec(
        "trial-a",
        "baseline",
        manifest.clone(),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &tree_sha,
    );
    let spec_b = make_spec(
        "trial-b",
        "baseline",
        manifest,
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &tree_sha,
    );
    let layout_a = materialize_trial(&spec_a, &base_dir).expect("materialize A");
    let layout_b = materialize_trial(&spec_b, &base_dir).expect("materialize B");

    assert_ne!(layout_a.root, layout_b.root);
    assert_ne!(layout_a.home_dir, layout_b.home_dir);
    assert_ne!(layout_a.xdg_config_home, layout_b.xdg_config_home);
    assert_ne!(layout_a.fake_bin_dir, layout_b.fake_bin_dir);

    assert_eq!(verify_isolation(&layout_a, &layout_b), Ok(()));
    assert_eq!(verify_isolation(&layout_b, &layout_a), Ok(()));

    // Checkout integrity: HEAD is the locked commit and the tracked tree is
    // clean. (Materialization intentionally adds *untracked* trial files —
    // skill, opencode.json — after the clean-checkout verification, so the
    // untracked-files listing is excluded here.)
    let head = git(&["rev-parse", "HEAD"], &layout_a.repo_dir);
    assert_eq!(head, commit);
    let status = git(
        &["status", "--porcelain", "--untracked-files=no"],
        &layout_a.repo_dir,
    );
    assert!(status.is_empty(), "tracked tree must be clean");

    // A file written in trial A is invisible from trial B.
    fs::write(layout_a.repo_dir.join("a-only.txt"), "a\n").expect("write A file");
    assert!(
        !layout_b.repo_dir.join("a-only.txt").exists(),
        "trial B must not see trial A's file"
    );
    assert_eq!(verify_isolation(&layout_a, &layout_b), Ok(()));

    // Layouts stay byte-clean: the only repo difference is the file we added.
    let mut files_a = repo_files(&layout_a.repo_dir);
    let files_b = repo_files(&layout_b.repo_dir);
    files_a.retain(|file| file != "a-only.txt");
    assert_eq!(files_a, files_b);

    #[cfg(unix)]
    {
        for dir in [
            &layout_a.root,
            &layout_a.home_dir,
            &layout_a.xdg_config_home,
            &layout_a.tmpdir,
            &layout_a.fake_bin_dir,
            &layout_a.repo_dir,
        ] {
            assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
        }
        assert_eq!(mode_of(&layout_a.env_file), 0o600);
    }

    cleanup_trial(&layout_a).expect("cleanup A");
    cleanup_trial(&layout_b).expect("cleanup B");
}

// ---------------------------------------------------------------------------
// 2. Baseline vs treatment: manifests differ only by the declared skill,
//    and so do the materialized trees.
// ---------------------------------------------------------------------------

#[test]
fn baseline_vs_treatment_delta() {
    let base_dir = unique_dir("ab");
    let (source, commit) = init_source_repo();
    let tree_sha = fake_digest("tree");
    let (base_tree, treatment_tree) = fixture_trees();
    let base_manifest = resolve(&base_tree);
    let treatment_manifest = resolve(&treatment_tree);

    assert!(
        base_manifest.skills().is_empty(),
        "baseline fixture carries no skills"
    );
    assert_eq!(
        treatment_manifest.skills().len(),
        1,
        "treatment fixture carries exactly the root skill"
    );
    let skill = &treatment_manifest.skills()[0];
    assert_eq!(
        skill.token, "project:.opencode/skill/root/SKILL.md",
        "treatment skill uses the audit's project path"
    );

    let overlay = ConditionOverlay {
        entries: vec![OverlayEntry {
            token: skill.token.clone(),
            action: OverlayAction::Add,
            sha256: Some(skill.sha256.clone()),
        }],
    };
    assert_eq!(
        verify_declared_delta(&base_manifest, &treatment_manifest, &overlay),
        Ok(()),
        "fixtures must satisfy the M1 declared-delta audit"
    );

    let baseline = make_spec(
        "trial-base",
        "baseline",
        base_manifest,
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &tree_sha,
    );
    let treatment = make_spec(
        "trial-treat",
        "treatment",
        treatment_manifest,
        overlay,
        &source,
        &commit,
        &tree_sha,
    );
    let layout_base = materialize_trial(&baseline, &base_dir).expect("materialize baseline");
    let layout_treat = materialize_trial(&treatment, &base_dir).expect("materialize treatment");

    // Baseline gets no skill directory at all.
    assert!(
        !layout_base.repo_dir.join(".opencode/skill").exists(),
        "baseline must not materialize a skill dir"
    );

    let files_base = repo_files(&layout_base.repo_dir);
    let files_treat = repo_files(&layout_treat.repo_dir);
    let base_set: std::collections::BTreeSet<&str> =
        files_base.iter().map(String::as_str).collect();
    let treat_set: std::collections::BTreeSet<&str> =
        files_treat.iter().map(String::as_str).collect();
    let only_treat: Vec<&&str> = treat_set.difference(&base_set).collect();
    let only_base: Vec<&&str> = base_set.difference(&treat_set).collect();
    assert_eq!(
        only_treat,
        vec![&".opencode/skill/root/SKILL.md"],
        "materialized trees differ only by the declared skill file"
    );
    assert!(only_base.is_empty(), "nothing is missing from treatment");

    // Shared files are byte-identical (hermetic opencode.json included).
    for shared in base_set.intersection(&treat_set) {
        assert_eq!(
            read_repo(&layout_base.repo_dir, shared),
            read_repo(&layout_treat.repo_dir, shared),
            "shared file {shared} must be byte-identical"
        );
    }

    // The materialized skill is byte-identical to the resolved source.
    assert_eq!(
        read_repo(&layout_treat.repo_dir, ".opencode/skill/root/SKILL.md"),
        SKILL_BODY.as_bytes()
    );

    cleanup_trial(&layout_base).expect("cleanup baseline");
    cleanup_trial(&layout_treat).expect("cleanup treatment");
}

// ---------------------------------------------------------------------------
// 3. Parent secrets never enter the trial environment
// ---------------------------------------------------------------------------

#[test]
fn parent_secret_is_never_inherited() {
    let secret_name = format!("ROOT_EVAL_M2_SCRUB_{}", std::process::id());
    let secret_value = format!(
        "super-secret-{}-{}",
        std::process::id(),
        COUNTER.load(Ordering::SeqCst)
    );
    std::env::set_var(&secret_name, &secret_value);

    let base_dir = unique_dir("scrub");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-scrub",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("materialize");

    let parent: BTreeMap<String, String> = std::env::vars().collect();
    assert!(
        parent.contains_key(&secret_name),
        "test precondition: the fake secret is in the parent env"
    );
    let scrubbed = scrub_env(&layout, &parent);
    assert!(
        !scrubbed.contains_key(secret_name.as_str()),
        "scrubbed env must not carry the secret name"
    );
    assert!(
        !scrubbed.values().any(|value| value == &secret_value),
        "scrubbed env must not carry the secret value"
    );

    let env = allowed_env(&layout);
    assert!(
        !env.values().any(|value| value == &secret_value),
        "allowlist env must not carry the secret value"
    );
    // PATH is the fake bin only: hermetic trials cannot reach host tools.
    assert_eq!(
        env.get("PATH").map(String::as_str),
        Some(layout.fake_bin_dir.to_string_lossy().as_ref())
    );

    let env_file = fs::read_to_string(&layout.env_file).expect("read env file");
    assert!(
        !env_file.contains(&secret_value),
        "env file must not contain the secret value"
    );
    assert!(
        env_file.contains("OPENCODE_DISABLE_AUTOUPDATE='1'"),
        "hermetic autoupdate gate is seeded"
    );
    assert!(
        env_file.contains("OPENCODE_DB=':memory:'"),
        "hermetic db mode is seeded"
    );

    std::env::remove_var(&secret_name);
    cleanup_trial(&layout).expect("cleanup");
}

/// Regression for the GitHub Actions failure on 2026-10-20.
///
/// CI exports `GITHUB_RUN_ATTEMPT=1`; the allowlisted trial env also carries
/// `OPENCODE_DISABLE_AUTOUPDATE=1`. A value-equality scrub check therefore
/// reported a secret leak even though the parent variable was never
/// inherited. The guard must compare names only.
#[test]
fn common_ci_value_does_not_trigger_a_false_secret_leak() {
    let base_dir = unique_dir("scrub-ci");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-scrub-ci",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("materialize");

    let parent = BTreeMap::from([("GITHUB_RUN_ATTEMPT".to_string(), "1".to_string())]);
    let scrubbed = scrub_env(&layout, &parent);
    assert!(
        !scrubbed.contains_key("GITHUB_RUN_ATTEMPT"),
        "the CI variable itself must never be inherited"
    );
    assert_eq!(
        scrubbed
            .get("OPENCODE_DISABLE_AUTOUPDATE")
            .map(String::as_str),
        Some("1"),
        "the allowlisted value is unrelated to the parent variable"
    );

    cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 4. Cleanup + residue scan
// ---------------------------------------------------------------------------

#[test]
fn cleanup_and_residue_scan() {
    let base_dir = unique_dir("residue");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-res",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("materialize");

    // The clean `none` pids marker is not residue.
    let scan = residue_scan(&base_dir).expect("scan");
    assert!(
        scan.iter().any(|residue| matches!(
            residue,
            Residue::TrialDir { path } if path == &layout.root
        )),
        "scan lists the live trial dir"
    );
    assert!(
        !scan
            .iter()
            .any(|residue| matches!(residue, Residue::LeftoverProcessMarker { .. })),
        "clean pids marker is not residue"
    );

    // A stale lock file is found.
    let stale = layout.root.join("stale.lock");
    fs::write(&stale, "heartbeat\n").expect("write stale lock");
    let scan = residue_scan(&base_dir).expect("rescan");
    assert!(
        scan.iter().any(|residue| matches!(
            residue,
            Residue::StaleLock { path } if path == &stale
        )),
        "scan finds the stale lock file"
    );

    // check_no_leak: the trial's own new files are fine; outside files fail.
    let before = vec![layout.root.clone()];
    let after = vec![layout.root.clone(), stale.clone()];
    assert_eq!(check_no_leak(&layout, &before, &after), Ok(()));
    let outside = base_dir.join("outside.txt");
    let after_leak = vec![layout.root.clone(), outside.clone()];
    let error = check_no_leak(&layout, &before, &after_leak).expect_err("outside file must leak");
    assert_eq!(error.kind(), root_eval::IsolationErrorKind::LeakDetected);

    assert_eq!(cleanup_trial(&layout), Ok(()));
    assert!(!layout.root.exists(), "trial root is removed");
    let rescan = residue_scan(&base_dir).expect("rescan after cleanup");
    assert!(rescan.is_empty(), "no residue after cleanup: {rescan:?}");
    // Cleanup is idempotent.
    assert_eq!(cleanup_trial(&layout), Ok(()));
}

// ---------------------------------------------------------------------------
// 5. Fake tools are offline (grep-verifiable, no network primitives)
// ---------------------------------------------------------------------------

#[test]
fn fake_tools_are_offline() {
    let base_dir = unique_dir("tools");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-tools",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("materialize");

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
    assert_eq!(names, vec!["brew", "curl", "npm", "pip", "root"]);

    for name in &names {
        let path = layout.fake_bin_dir.join(name);
        let content =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {name}: {error}"));
        for needle in ["curl", "wget", "socket"] {
            assert!(
                !content.contains(needle),
                "fake tool `{name}` must not reference `{needle}`"
            );
        }
        assert!(
            content.contains("TRIAL_AUDIT_LOG"),
            "fake tool `{name}` must audit to $TRIAL_AUDIT_LOG"
        );
        #[cfg(unix)]
        assert_eq!(mode_of(&path), 0o700, "fake tool `{name}` is executable");
    }

    let root_script = fs::read_to_string(layout.fake_bin_dir.join("root")).expect("read root");
    for command in ["doctor", "plan", "install", "verify", "rollback"] {
        assert!(
            root_script.contains(command),
            "fake root must handle `{command}` deterministically"
        );
    }

    cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 6. Validation is fail-closed: bad ids, existing dirs, bad commits
// ---------------------------------------------------------------------------

#[test]
fn validation_is_fail_closed() {
    let base_dir = unique_dir("fail");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let manifest = resolve(&base_tree);

    // Invalid trial ids never reach the filesystem.
    let bad = make_spec(
        "Bad_ID!",
        "baseline",
        manifest.clone(),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let error = materialize_trial(&bad, &base_dir).expect_err("bad trial_id refused");
    assert_eq!(
        error.kind(),
        MaterializerErrorKind::InvalidSpec,
        "bad trial_id is an InvalidSpec"
    );
    assert!(
        !base_dir.join("Bad_ID!").exists(),
        "refused trials create nothing"
    );

    // Fresh directories are required: the second materialization fails.
    let spec = make_spec(
        "trial-once",
        "baseline",
        manifest.clone(),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("first materialization");
    let error = materialize_trial(&spec, &base_dir).expect_err("duplicate refused");
    assert_eq!(error.kind(), MaterializerErrorKind::TrialExists);

    // A commit that does not exist in the source fails closed.
    let wrong_commit = make_spec(
        "trial-wrong",
        "baseline",
        manifest,
        ConditionOverlay { entries: vec![] },
        &source,
        "0000000000000000000000000000000000000000",
        &fake_digest("tree"),
    );
    let error = materialize_trial(&wrong_commit, &base_dir).expect_err("bad commit refused");
    assert!(
        matches!(
            error.kind(),
            MaterializerErrorKind::Git | MaterializerErrorKind::CommitMismatch
        ),
        "unknown commit fails closed: {error}"
    );

    cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 7. Non-git sources are copied verbatim (recorded, not commit-pinned)
// ---------------------------------------------------------------------------

#[test]
fn non_git_source_is_copied() {
    let base_dir = unique_dir("copy");
    let source = unique_dir("plain-source");
    fs::write(source.join("note.txt"), "plain\n").expect("write note");
    fs::create_dir_all(source.join("sub")).expect("write sub");
    fs::write(source.join("sub/inner.txt"), "inner\n").expect("write inner");

    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-copy",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        "0000000000000000000000000000000000000000",
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("copy materialization");
    assert_eq!(read_repo(&layout.repo_dir, "note.txt"), b"plain\n");
    assert_eq!(read_repo(&layout.repo_dir, "sub/inner.txt"), b"inner\n");
    assert!(
        !layout.repo_dir.join(".git").exists(),
        "plain copies carry no git metadata"
    );
    cleanup_trial(&layout).expect("cleanup");
}

// ---------------------------------------------------------------------------
// 8. Overlapping roots are not isolated
// ---------------------------------------------------------------------------

#[test]
fn overlapping_roots_fail_isolation() {
    let base_dir = unique_dir("overlap");
    let (source, commit) = init_source_repo();
    let (base_tree, _) = fixture_trees();
    let spec = make_spec(
        "trial-same",
        "baseline",
        resolve(&base_tree),
        ConditionOverlay { entries: vec![] },
        &source,
        &commit,
        &fake_digest("tree"),
    );
    let layout = materialize_trial(&spec, &base_dir).expect("materialize");
    let error =
        verify_isolation(&layout, &layout).expect_err("a trial is not isolated from itself");
    assert_eq!(error.kind(), IsolationErrorKind::OverlappingRoots);
    cleanup_trial(&layout).expect("cleanup");
}
