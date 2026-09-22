//! Sprint 014 §4: backward-compatibility + migration verification.
//!
//! Proves that v0.4 environment commands and v0.5 continuity commands still
//! behave in v0.6, that the bounded-resume / drift / recovery / secret-refusal
//! invariants are unchanged, that package-only lockfiles still emit schema 2
//! (max supported 3), and that v0.6 work-state additions migrate v0.5 databases
//! without data loss while refusing newer schemas and retrying failed
//! migrations.
//!
//! Hermetic: isolated `HOME`/`ROOT_DIR`/`TMPDIR`/`PATH` per child process, a
//! fake `nix` shim only where a Nix probe is unavoidable (`root plan install`),
//! unique temp dirs with cleanup, and no process-env mutation — so no global
//! test mutex is required.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_backward_compat_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    root_dir: PathBuf,
    home: PathBuf,
    tmpdir: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        let home = base.join("home");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for dir in [&repo, &root_dir, &home, &tmpdir, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("README.md"), b"# campfire\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &[
                "-c",
                "user.email=root@example.com",
                "-c",
                "user.name=Root",
                "commit",
                "-q",
                "-m",
                "initial",
            ],
        );

        let fixture = Self {
            base,
            repo,
            root_dir,
            home,
            tmpdir,
            bin,
        };
        fixture.write_nix_shim();
        fixture
    }

    /// A fake `nix` that succeeds on probes only; it never touches the
    /// filesystem and never installs anything. Real Nix is never invoked.
    fn write_nix_shim(&self) {
        let path = self.bin.join("nix");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Canonical package-only (schema v2) root.lock. `marker` changes the bytes.
    fn write_package_lock(&self, marker: &str) {
        let lock = format!(
            "{{\"version\":2,\"platform\":\"\",\"packages\":[],\"marker\":\"{marker}\"}}\n"
        );
        std::fs::write(self.root_dir.join("root.lock"), lock).unwrap();
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(&dirs).unwrap();
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "command {:?} failed: stderr={} stdout={}",
            args,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON for {:?}: {}\nstdout={}",
                args,
                e,
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn database_path(&self, workspace_id: &str) -> PathBuf {
        self.root_dir
            .join("work")
            .join(workspace_id)
            .join("state.db")
    }

    fn write_artifact(&self, relative: &str) {
        let path = self.repo.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"export {}\n").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for backward-compat tests");
    assert!(status.success(), "git {:?} failed", args);
}

/// Extract the user-facing error message from a failed `--json` invocation.
fn error_message(output: &std::process::Output) -> String {
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            String::from_utf8_lossy(&output.stdout).to_string()
                + &String::from_utf8_lossy(&output.stderr)
        })
}

fn not_recoverable_values(report: &serde_json::Value) -> Vec<String> {
    report["not_recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

fn drift_kinds(report: &serde_json::Value) -> Vec<String> {
    report["drift"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_string())
        .collect()
}

fn drift_level(report: &serde_json::Value, kind: &str) -> Option<String> {
    report["drift"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == kind)
        .map(|item| item["level"].as_str().unwrap().to_string())
}

/// §4.1 v0.5 row: every continuity command still runs and its JSON shape holds.
#[test]
fn v05_continuity_commands_and_json_shapes() {
    let fixture = Fixture::new("v05_commands");

    let init = fixture.json(&["workspace", "init", "--json"]);
    assert_eq!(init["success"], true);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    assert!(workspace_id.starts_with("root_ws_"));
    assert!(Path::new(init["database"].as_str().unwrap()).exists());

    let status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str());
    assert_eq!(status["counts"]["goals"], 0);
    assert_eq!(status["counts"]["decisions"], 0);
    assert_eq!(status["counts"]["findings"], 0);
    assert_eq!(status["counts"]["artifacts"], 0);

    let goal = fixture.json(&["goal", "set", "Implement workspace invitations", "--json"]);
    assert_eq!(goal["goal"]["statement"], "Implement workspace invitations");
    assert_eq!(goal["goal"]["status"], "active");
    let shown_goal = fixture.json(&["goal", "show", "--json"]);
    assert_eq!(
        shown_goal["goal"]["statement"],
        "Implement workspace invitations"
    );

    let decision = fixture.json(&[
        "decision",
        "add",
        "Tokens expire after 24 hours",
        "--rationale",
        "Limits exposure of leaked invitations",
        "--json",
    ]);
    let decision_id = decision["decision"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        decision["decision"]["rationale"],
        "Limits exposure of leaked invitations"
    );
    assert_eq!(
        fixture.json(&["decision", "show", &decision_id, "--json"])["decision"]["id"],
        decision_id.as_str()
    );
    assert_eq!(
        fixture.json(&["decision", "list", "--json"])["decisions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let finding = fixture.json(&[
        "finding",
        "add",
        "Invite consumption fails inside the transaction",
        "--evidence",
        "tests/invitations.test.ts",
        "--json",
    ]);
    let finding_id = finding["finding"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        finding["finding"]["evidence_ref"],
        "tests/invitations.test.ts"
    );
    assert_eq!(
        fixture.json(&["finding", "show", &finding_id, "--json"])["finding"]["id"],
        finding_id.as_str()
    );
    assert_eq!(
        fixture.json(&["finding", "list", "--json"])["findings"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    std::fs::write(fixture.repo.join("create.ts"), b"export const x = 1;\n").unwrap();
    let artifact = fixture.json(&["artifact", "add", "create.ts", "--json"]);
    assert_eq!(artifact["artifact"]["kind"], "file");
    assert!(artifact["artifact"]["fingerprint"].as_str().is_some());
    assert_eq!(
        fixture.json(&["artifact", "list", "--json"])["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let checkpoint = fixture.json(&["checkpoint", "create", "--message", "slice done", "--json"]);
    let checkpoint_id = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();
    assert!(checkpoint_id.starts_with("root_cp_"));
    assert_eq!(checkpoint["checkpoint"]["message"], "slice done");
    assert_eq!(
        fixture.json(&["checkpoint", "show", &checkpoint_id, "--json"])["checkpoint"]["id"],
        checkpoint_id.as_str()
    );
    assert_eq!(
        fixture.json(&["checkpoint", "list", "--json"])["checkpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let resume = fixture.json(&["resume", "--json"]);
    assert_eq!(resume["checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(
        resume["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert_eq!(
        fixture.json(&["resume", "--checkpoint", &checkpoint_id, "--json"])["checkpoint"]["id"],
        checkpoint_id.as_str()
    );

    let handoff = fixture.json(&["handoff", "--to", "codex", "--json"]);
    assert_eq!(handoff["to"], "codex");
    assert_eq!(handoff["checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(
        handoff["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert!(handoff["instructions"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("checkpoint"));

    let recover = fixture.json(&["recover", "--json"]);
    assert_eq!(recover["workspace"]["id"], workspace_id.as_str());
    assert_eq!(recover["work_state"]["available"], true);
    assert_eq!(recover["work_state"]["active_decisions"], 1);
    assert_eq!(recover["work_state"]["active_findings"], 1);

    let mut actual = not_recoverable_values(&recover);
    let mut expected = vec![
        "commands not observed by Root".to_string(),
        "unrecorded agent conversation".to_string(),
        "unsaved editor state".to_string(),
    ];
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "not_recoverable triple changed: {recover}"
    );
}

/// §4.1: `resume` without `--with` keeps the byte-compatible v0.5 shape;
/// `--with` is the additive v0.6 extension.
#[test]
fn resume_v05_shape_preserved_and_with_is_extended() {
    let fixture = Fixture::new("resume_shape");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement workspace invitations", "--json"]);
    fixture.json(&["decision", "add", "Invitations expire after 24h", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let plain = fixture.json(&["resume", "--json"]);
    for absent in [
        "steps",
        "package",
        "mapping_available",
        "target",
        "translation",
        "next",
    ] {
        assert!(
            plain.get(absent).is_none(),
            "v0.5 `resume` must not carry {absent}: {plain}"
        );
    }
    for present in [
        "workspace",
        "goal",
        "checkpoint",
        "current_state",
        "unresolved_work",
        "decisions",
        "findings",
        "artifacts",
        "decisions_omitted",
        "findings_omitted",
        "artifacts_omitted",
        "repository_state",
        "environment_state",
        "drift",
        "suggested_continuation",
        "provenance",
    ] {
        assert!(
            plain.get(present).is_some(),
            "v0.5 `resume` lost field {present}: {plain}"
        );
    }

    let with = fixture.json(&["resume", "--with", "codex", "--json"]);
    assert_eq!(with["target"], "codex");
    assert!(with.get("package").is_some(), "`--with` must carry package");
    assert!(
        with.get("mapping_available").is_some(),
        "`--with` must report mapping_available"
    );
    assert!(with.get("steps").is_some(), "`--with` must carry steps");
    let steps = with["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7, "seven observable steps: {with}");
    assert_eq!(steps[0]["name"], "prepare config");
    assert_eq!(steps[6]["name"], "launch continuation");
    assert_eq!(with["package"]["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(
        with["package"]["checkpoint"]["id"],
        plain["checkpoint"]["id"]
    );
}

/// §4.2: a long-running v0.5 fixture still projects within the 10/10/20 caps
/// with exact omitted counts, and `--with` trims within the same caps.
#[test]
fn long_running_resume_caps_and_omitted_are_unchanged() {
    let fixture = Fixture::new("long_running");
    let repository = root_work::Repository::discover(&fixture.repo).unwrap();
    let init = root_work::WorkStore::init_at(&fixture.root_dir, repository.clone()).unwrap();
    assert!(init.created);

    let (superseded_decision, superseded_finding) = {
        let mut store =
            root_work::WorkStore::open_at(&fixture.root_dir, repository.clone()).unwrap();
        store.set_goal("Ship bounded resume").unwrap();
        for index in 0..26 {
            store
                .add_decision(&format!("Decision {index:02}"), None)
                .unwrap();
        }
        for index in 0..51 {
            store
                .add_finding(&format!("Finding {index:02}"), None)
                .unwrap();
        }
        for index in 0..100 {
            let relative = format!("src/artifact_{index:03}.ts");
            fixture.write_artifact(&relative);
            let fingerprint = root_work::fingerprint_file(&fixture.repo.join(&relative)).unwrap();
            store
                .add_artifact(root_work::NewArtifact {
                    kind: "file",
                    uri: &relative,
                    fingerprint: Some(&fingerprint),
                    evidence_ref: None,
                })
                .unwrap();
        }
        let decision = store.list_decisions().unwrap()[0].id.clone();
        let finding = store.list_findings().unwrap()[0].id.clone();
        (decision, finding)
    };

    // No public API supersedes decisions/findings; flip one of each directly so
    // the projection proves it still excludes superseded rows (v0.5 contract).
    {
        let database = fixture.database_path(&init.workspace.id);
        let conn = root_work::db::open(&database).unwrap();
        conn.execute(
            "UPDATE decisions SET status = 'superseded' WHERE id = ?1",
            (superseded_decision.as_str(),),
        )
        .unwrap();
        conn.execute(
            "UPDATE findings SET status = 'superseded' WHERE id = ?1",
            (superseded_finding.as_str(),),
        )
        .unwrap();
    }

    fixture.json(&["checkpoint", "create", "--json"]);

    let report = fixture.json(&["resume", "--json"]);
    assert_eq!(report["decisions"].as_array().unwrap().len(), 10);
    assert_eq!(report["findings"].as_array().unwrap().len(), 10);
    assert_eq!(report["artifacts"].as_array().unwrap().len(), 20);
    assert_eq!(report["decisions_omitted"], 15);
    assert_eq!(report["findings_omitted"], 40);
    assert_eq!(report["artifacts_omitted"], 80);
    assert_eq!(report["current_state"]["active_decisions"], 25);
    assert_eq!(report["current_state"]["active_findings"], 50);
    assert_eq!(report["current_state"]["artifacts"], 100);

    let with = fixture.json(&["resume", "--with", "codex", "--json"]);
    let package = &with["package"];
    assert_eq!(package["decisions"].as_array().unwrap().len(), 10);
    assert_eq!(package["findings"].as_array().unwrap().len(), 10);
    assert_eq!(package["artifacts"].as_array().unwrap().len(), 20);
    assert_eq!(package["decisions_omitted"], 15);
    assert_eq!(package["findings_omitted"], 40);
    assert_eq!(package["artifacts_omitted"], 80);

    // Two runs are byte-identical (determinism is part of the v0.5 contract).
    let first = fixture.run(&["resume", "--json"]);
    let second = fixture.run(&["resume", "--json"]);
    assert!(first.status.success() && second.status.success());
    assert_eq!(first.stdout, second.stdout);
}

/// §4.1: drift kinds and levels are unchanged.
#[test]
fn drift_levels_are_unchanged() {
    let fixture = Fixture::new("drift_levels");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Keep the tree honest", "--json"]);
    fixture.write_artifact("src/tracked.ts");
    fixture.json(&["artifact", "add", "src/tracked.ts", "--json"]);
    git(&fixture.repo, &["add", "-A"]);
    git(
        &fixture.repo,
        &[
            "-c",
            "user.email=root@example.com",
            "-c",
            "user.name=Root",
            "commit",
            "-q",
            "-m",
            "track artifact",
        ],
    );
    fixture.json(&["checkpoint", "create", "--json"]);

    let clean = fixture.json(&["resume", "--json"]);
    assert_eq!(clean["drift"]["level"], "none", "json={clean}");
    assert!(clean["drift"]["items"].as_array().unwrap().is_empty());

    git(
        &fixture.repo,
        &[
            "-c",
            "user.email=root@example.com",
            "-c",
            "user.name=Root",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "head change",
        ],
    );
    let head = fixture.json(&["resume", "--json"]);
    assert_eq!(
        drift_level(&head, "repository.head").as_deref(),
        Some("warning")
    );
    assert_eq!(head["drift"]["level"], "warning");

    std::fs::write(fixture.repo.join("README.md"), b"# campfire changed\n").unwrap();
    let dirty = fixture.json(&["resume", "--json"]);
    assert_eq!(
        drift_level(&dirty, "repository.dirty").as_deref(),
        Some("informational")
    );
    assert_eq!(
        drift_level(&dirty, "repository.head").as_deref(),
        Some("warning")
    );

    std::fs::remove_file(fixture.repo.join("src/tracked.ts")).unwrap();
    let missing = fixture.json(&["resume", "--json"]);
    assert!(drift_kinds(&missing)
        .iter()
        .any(|kind| kind == "artifact.missing"));
    assert_eq!(
        drift_level(&missing, "artifact.missing").as_deref(),
        Some("warning")
    );
    assert_eq!(missing["drift"]["level"], "warning");
}

/// §4.1: the fixed `not_recoverable` triple is intact, with recoverable items
/// and recommended actions unchanged.
#[test]
fn recover_not_recoverable_triple_is_unchanged() {
    let fixture = Fixture::new("recover_triple");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement invitations", "--json"]);
    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    let checkpoint_id = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();

    let report = fixture.json(&["recover", "--json"]);
    assert_eq!(report["last_checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(
        report["recommended_action"],
        format!("Resume from {checkpoint_id}.")
    );
    let recoverable: Vec<String> = report["recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect();
    assert!(recoverable.contains(&"checkpoint reference".to_string()));

    let mut actual = not_recoverable_values(&report);
    let mut expected = vec![
        "commands not observed by Root".to_string(),
        "unrecorded agent conversation".to_string(),
        "unsaved editor state".to_string(),
    ];
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "the triple is fixed: {report}");

    // Drift flips the recommendation without changing the triple.
    git(
        &fixture.repo,
        &[
            "-c",
            "user.email=root@example.com",
            "-c",
            "user.name=Root",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "drift",
        ],
    );
    let drifted = fixture.json(&["recover", "--json"]);
    assert_eq!(drifted["drift"]["level"], "warning");
    assert_eq!(
        drifted["recommended_action"],
        "Inspect drift before resuming."
    );
    let mut actual = not_recoverable_values(&drifted);
    actual.sort();
    assert_eq!(actual, expected);
}

/// §4.1: secret-shaped decisions/findings are still refused at the work-state
/// layer, with no row and no event created.
#[test]
fn secret_refusal_at_work_state_layer_is_unchanged() {
    let fixture = Fixture::new("secret_refusal");
    fixture.json(&["workspace", "init", "--json"]);

    let repository = root_work::Repository::discover(&fixture.repo).unwrap();
    let events_before = {
        let store =
            root_work::WorkStore::open_at_read_only(&fixture.root_dir, repository.clone()).unwrap();
        store.events().unwrap().len()
    };

    let secret_decision = fixture.run(&["decision", "add", "password = hunter2-secret", "--json"]);
    assert!(
        !secret_decision.status.success(),
        "secret-shaped decision must be refused"
    );
    assert!(
        error_message(&secret_decision).contains("secret"),
        "refusal must name the secret: {}",
        error_message(&secret_decision)
    );

    let secret_finding = fixture.run(&["finding", "add", "token: abcdef123456", "--json"]);
    assert!(
        !secret_finding.status.success(),
        "secret-shaped finding must be refused"
    );
    assert!(
        error_message(&secret_finding).contains("secret"),
        "refusal must name the secret: {}",
        error_message(&secret_finding)
    );

    assert_eq!(
        fixture.json(&["decision", "list", "--json"])["decisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        fixture.json(&["finding", "list", "--json"])["findings"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let events_after = {
        let store = root_work::WorkStore::open_at_read_only(&fixture.root_dir, repository).unwrap();
        store.events().unwrap().len()
    };
    assert_eq!(
        events_after, events_before,
        "a refused mutation must append no work event"
    );
}

/// §4.1 + AGENTS.md: package-only locks still emit schema 2 with max supported 3.
#[test]
fn package_only_lock_schema_is_unchanged() {
    use std::collections::BTreeMap;

    assert_eq!(root_lockfile::ROOT_LOCK_SCHEMA_VERSION, 2);
    assert_eq!(root_lockfile::ROOT_LOCK_MAX_SUPPORTED_VERSION, 3);

    let empty: BTreeMap<String, BTreeMap<String, root_lockfile::LockedModel>> = BTreeMap::new();
    assert_eq!(root_lockfile::emit_lock_version(&empty), 2);

    let package_only = r#"{
        "version": 2,
        "platform": "aarch64-darwin",
        "packages": [
            {
                "name": "ripgrep",
                "requested": "ripgrep",
                "version": "14.1.0",
                "attribute": "ripgrep",
                "storePath": "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-ripgrep-14.1.0",
                "binaries": ["rg"]
            }
        ]
    }"#;

    assert_eq!(
        root_lockfile::peek_lock_schema_version(package_only).unwrap(),
        2
    );
    let parsed = root_lockfile::read_compatible_lock_v2_from_str(package_only).unwrap();
    assert_eq!(parsed.version, 2);
    assert_eq!(parsed.packages.len(), 1);
    assert_eq!(parsed.packages[0].name, "ripgrep");
    assert!(parsed.models.is_empty());

    root_lockfile::validate_supported_lock_version(2).unwrap();
    root_lockfile::validate_supported_lock_version(3).unwrap();
    let error = root_lockfile::validate_supported_lock_version(4)
        .unwrap_err()
        .to_string();
    assert!(error.contains("newer than this Root supports"), "{error}");
}

/// §4.1/§4.2: a package-only Rootfile + root.lock situation is untouched by the
/// v0.6 work-state additions (workspace init, goal, checkpoint).
#[test]
fn package_only_rootfile_and_lock_unaffected_by_work_state() {
    let fixture = Fixture::new("package_only_lock");
    std::fs::write(fixture.root_dir.join("Rootfile"), b"[packages]\n").unwrap();
    fixture.write_package_lock("package-only");
    let lock_path = fixture.root_dir.join("root.lock");
    let before = std::fs::read(&lock_path).unwrap();

    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&[
        "goal",
        "set",
        "Environment untouched by work state",
        "--json",
    ]);
    fixture.json(&["checkpoint", "create", "--json"]);

    assert_eq!(
        std::fs::read(&lock_path).unwrap(),
        before,
        "v0.6 work-state commands must not rewrite a package-only root.lock"
    );
    assert_eq!(
        root_lockfile::peek_lock_schema_version(&std::fs::read_to_string(&lock_path).unwrap())
            .unwrap(),
        2
    );
}

/// §4.2: a fresh ROOT_DIR creates a schema-v3 database with `agent_env_ref`
/// and a registry index entry.
#[test]
fn fresh_workspace_creates_v3_db_with_agent_env_ref() {
    let fixture = Fixture::new("fresh_v3");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    let database = fixture.database_path(&workspace_id);
    assert!(database.exists());
    assert_eq!(
        Path::new(init["database"].as_str().unwrap()),
        database.as_path()
    );

    assert_eq!(root_work::db::WORK_SCHEMA_VERSION, 3);
    let conn = root_work::db::open(&database).unwrap();
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM work_schema_migrations",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 3);
    let has_agent_env_ref: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'agent_env_ref'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_agent_env_ref, 1);
    drop(conn);

    assert!(
        fixture.root_dir.join("work").join("index.json").exists(),
        "workspace init must record an index entry"
    );
}

/// §4.2: an existing v0.5 database opens read-write through the CLI without
/// data loss; the additive `agent_env_ref` column migrates with NULL values and
/// a pre-migration backup is written.
#[test]
#[allow(clippy::type_complexity)]
fn v2_database_migrates_to_v3_without_data_loss_at_cli_level() {
    let fixture = Fixture::new("migrate_v2");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    let database = fixture.database_path(&workspace_id);

    // Downgrade the freshly created v3 database to an authentic v2 shape, then
    // seed rows across every v0.5 table (rusqlite is not a root-cli dependency;
    // root-work's public db::open + published migration SQL are enough).
    {
        let conn = root_work::db::open(&database).unwrap();
        conn.execute_batch(
            "ALTER TABLE checkpoints DROP COLUMN agent_env_ref;
             DELETE FROM work_schema_migrations WHERE version = 3;",
        )
        .unwrap();
        conn.execute_batch(&format!(
            r#"
            INSERT INTO goals (id, workspace_id, statement, status, created_at, completed_at, provenance_id)
            VALUES ('root_goal_SEED', '{ws}', 'Migrate without loss', 'active', '2026-01-01T00:00:00Z', NULL, NULL);
            INSERT INTO decisions (id, workspace_id, goal_id, statement, rationale, status, created_at, provenance_id)
            VALUES ('root_dec_SEED', '{ws}', NULL, 'Keep every field', 'compat', 'active', '2026-01-01T00:00:00Z', NULL);
            INSERT INTO findings (id, workspace_id, goal_id, statement, evidence_ref, status, created_at, provenance_id)
            VALUES ('root_find_SEED', '{ws}', NULL, 'Preserved finding', 'evidence.txt', 'active', '2026-01-01T00:00:00Z', NULL);
            INSERT INTO artifacts (id, workspace_id, kind, uri, fingerprint, created_at, provenance_id)
            VALUES ('root_art_SEED', '{ws}', 'file', 'src/seed.ts', 'deadbeef', '2026-01-01T00:00:00Z', NULL);
            INSERT INTO sessions (id, workspace_id, harness, agent_identity, started_at, ended_at, resumed_from_checkpoint_id)
            VALUES ('root_sess_SEED', '{ws}', 'codex', 'agent-seed', '2026-01-01T00:00:00Z', NULL, NULL);
            INSERT INTO provenance (id, source_type, agent, harness, session_id, evidence_ref, created_at)
            VALUES ('root_prov_SEED', 'agent', 'codex', 'codex-cli', '{ws}', 'sessions/seed', '2026-01-01T00:00:00Z');
            INSERT INTO checkpoints (
                id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
                git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
                profile_reference, environment_status, continuation_summary, snapshot,
                created_at, provenance_id
            ) VALUES (
                'root_cp_SEED', '{ws}', 'root_goal_SEED', 'seeded checkpoint', 11, 'abc123', 'main',
                1, 'dirtyfp', 'rfdigest', 'lockdigest',
                '/profiles/default', 'observed', 'seeded summary', '{{"k":1}}',
                '2026-01-01T00:00:00Z', 'root_prov_SEED'
            );
            INSERT INTO work_events (workspace_id, event_type, entity_type, entity_id, payload, timestamp)
            VALUES ('{ws}', 'seed.recorded', 'seed', 'root_seed_1', '{{"n":1}}', '2026-01-01T00:00:00Z');
            "#,
            ws = workspace_id
        ))
        .unwrap();
    }

    // Open the v2 database read-write through the CLI; this migrates it.
    let status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str());

    let backup = PathBuf::from(format!("{}.backup-v2", database.display()));
    assert!(
        backup.exists(),
        "migration must write a pre-migration backup"
    );
    assert!(
        std::fs::read(&backup)
            .unwrap()
            .starts_with(b"SQLite format 3\0"),
        "the backup must be a valid SQLite database"
    );

    let conn = root_work::db::open(&database).unwrap();
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM work_schema_migrations",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 3);

    let goal: (String, String) = conn
        .query_row(
            "SELECT statement, status FROM goals WHERE id = 'root_goal_SEED'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(goal.0, "Migrate without loss");
    assert_eq!(goal.1, "active");

    let decision: (String, Option<String>, String) = conn
        .query_row(
            "SELECT statement, rationale, status FROM decisions WHERE id = 'root_dec_SEED'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(decision.0, "Keep every field");
    assert_eq!(decision.1.as_deref(), Some("compat"));
    assert_eq!(decision.2, "active");

    let finding: (String, Option<String>) = conn
        .query_row(
            "SELECT statement, evidence_ref FROM findings WHERE id = 'root_find_SEED'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(finding.0, "Preserved finding");
    assert_eq!(finding.1.as_deref(), Some("evidence.txt"));

    let artifact: (String, String, Option<String>) = conn
        .query_row(
            "SELECT kind, uri, fingerprint FROM artifacts WHERE id = 'root_art_SEED'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(artifact.0, "file");
    assert_eq!(artifact.1, "src/seed.ts");
    assert_eq!(artifact.2.as_deref(), Some("deadbeef"));

    let session: (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT harness, agent_identity FROM sessions WHERE id = 'root_sess_SEED'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(session.0.as_deref(), Some("codex"));
    assert_eq!(session.1.as_deref(), Some("agent-seed"));

    let provenance: String = conn
        .query_row(
            "SELECT source_type FROM provenance WHERE id = 'root_prov_SEED'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provenance, "agent");

    let checkpoint: (
        Option<String>,
        i64,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT message, work_revision, git_head, root_lock_digest, snapshot,
                    environment_status, continuation_summary, rootfile_digest, agent_env_ref
             FROM checkpoints WHERE id = 'root_cp_SEED'",
            (),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(checkpoint.0.as_deref(), Some("seeded checkpoint"));
    assert_eq!(checkpoint.1, 11);
    assert_eq!(checkpoint.2.as_deref(), Some("abc123"));
    assert_eq!(checkpoint.3, "lockdigest");
    assert_eq!(checkpoint.4, "{\"k\":1}");
    assert_eq!(checkpoint.5, "observed");
    assert_eq!(checkpoint.6, "seeded summary");
    assert_eq!(checkpoint.7, "rfdigest");
    assert_eq!(
        checkpoint.8, None,
        "v2 rows migrate with a NULL agent_env_ref"
    );

    let event: (String, Option<String>) = conn
        .query_row(
            "SELECT event_type, payload FROM work_events WHERE entity_id = 'root_seed_1'",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(event.0, "seed.recorded");
    assert_eq!(event.1.as_deref(), Some("{\"n\":1}"));

    let counts = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(counts["counts"]["goals"], 1);
    assert_eq!(counts["counts"]["decisions"], 1);
    assert_eq!(counts["counts"]["findings"], 1);
    assert_eq!(counts["counts"]["artifacts"], 1);
}

/// §4.2: a newer-schema database is refused with a clear message.
#[test]
fn newer_schema_is_refused_at_cli_level() {
    let fixture = Fixture::new("newer_schema");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    let database = fixture.database_path(&workspace_id);

    {
        let conn = root_work::db::open(&database).unwrap();
        conn.execute(
            "INSERT INTO work_schema_migrations (version, applied_at) VALUES (999, 'now')",
            (),
        )
        .unwrap();
    }

    let output = fixture.run(&["workspace", "status", "--json"]);
    assert!(
        !output.status.success(),
        "a newer schema must be refused: {}",
        error_message(&output)
    );
    let message = error_message(&output);
    assert!(
        message.contains("newer than this build supports"),
        "refusal must explain the newer schema: {message}"
    );
    assert!(
        message.contains("Upgrade Root"),
        "refusal must give a remediation: {message}"
    );
}

/// §4.2: a failed migration rolls back (the version does not advance) and can
/// be retried cleanly once the obstruction is removed.
#[test]
fn failed_migration_rolls_back_and_retries_at_cli_level() {
    let fixture = Fixture::new("retry_migration");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    let database = fixture.database_path(&workspace_id);

    // Downgrade to v2, seed a checkpoint, and pre-create the column MIGRATION_V3
    // tries to add — so the v3 migration fails on the first attempt.
    let conn = root_work::db::open(&database).unwrap();
    conn.execute_batch(
        "ALTER TABLE checkpoints DROP COLUMN agent_env_ref;
         DELETE FROM work_schema_migrations WHERE version = 3;",
    )
    .unwrap();
    conn.execute_batch(&format!(
        r#"
        INSERT INTO checkpoints (
            id, workspace_id, goal_id, message, work_revision, git_head, git_branch,
            git_dirty, git_dirty_fingerprint, rootfile_digest, root_lock_digest,
            profile_reference, environment_status, continuation_summary, snapshot,
            created_at, provenance_id
        ) VALUES (
            'root_cp_RETRY', '{ws}', NULL, 'retry checkpoint', 3, 'headsha', 'main',
            0, 'cleanfp', NULL, NULL, NULL, 'missing', 'retry summary', '{{}}',
            '2026-01-01T00:00:00Z', NULL
        );
        ALTER TABLE checkpoints ADD COLUMN agent_env_ref TEXT;
        "#,
        ws = workspace_id
    ))
    .unwrap();

    let failed = fixture.run(&["workspace", "status", "--json"]);
    assert!(
        !failed.status.success(),
        "the obstructed migration must fail: {}",
        error_message(&failed)
    );
    assert!(
        error_message(&failed).contains("version 3"),
        "failure must name the migration: {}",
        error_message(&failed)
    );

    // Rollback proof: the version stayed at 2, no partial schema landed, and
    // the seeded row survived (the held connection is not a migration actor).
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM work_schema_migrations",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        version, 2,
        "a failed migration must not advance the version"
    );
    let preserved: Option<String> = conn
        .query_row(
            "SELECT message FROM checkpoints WHERE id = 'root_cp_RETRY'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved.as_deref(), Some("retry checkpoint"));

    // Remove the obstruction and retry.
    conn.execute_batch("ALTER TABLE checkpoints DROP COLUMN agent_env_ref;")
        .unwrap();
    drop(conn);

    fixture.json(&["workspace", "status", "--json"]);

    let conn = root_work::db::open(&database).unwrap();
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM work_schema_migrations",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 3, "the retry must complete the migration");
    let has_agent_env_ref: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('checkpoints') WHERE name = 'agent_env_ref'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_agent_env_ref, 1);
    let preserved: Option<String> = conn
        .query_row(
            "SELECT message FROM checkpoints WHERE id = 'root_cp_RETRY'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved.as_deref(), Some("retry checkpoint"));
}

/// §4.1 v0.4 row: `catalog` and `plan install` still parse and run without Nix.
#[test]
fn v04_env_commands_still_parse_without_nix() {
    let fixture = Fixture::new("v04_env");

    let catalog = fixture.json(&["catalog", "--json"]);
    let packages = catalog["packages"].as_array().unwrap();
    assert!(!packages.is_empty(), "catalog must not be empty");
    let ripgrep = packages
        .iter()
        .find(|package| package["name"] == "ripgrep")
        .expect("ripgrep must remain in the catalog");
    assert!(ripgrep["nix_attr"].as_str().is_some());
    assert!(!ripgrep["binaries"].as_array().unwrap().is_empty());

    // The fixture's fake `nix` satisfies the search probe; no real Nix is run.
    let plan = fixture.json(&["plan", "install", "ripgrep", "--json"]);
    assert_eq!(plan["package"], "ripgrep");
    assert_eq!(plan["found"], true, "json={plan}");
    assert_eq!(plan["nix_attr"], ripgrep["nix_attr"]);
    assert_eq!(plan["rollback_available"], true);
    assert!(!plan["expected_binaries"].as_array().unwrap().is_empty());
}
