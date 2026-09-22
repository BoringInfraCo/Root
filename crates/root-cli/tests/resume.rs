//! Sprint 007 integration: deterministic resume packages and drift detection.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_resume_{name}_{}_{}",
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
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&root_dir).unwrap();
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
        Self {
            base,
            repo,
            root_dir,
        }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("ROOT_DIR", &self.root_dir)
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

fn git(repo: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for resume tests");
    assert!(status.success(), "git {:?} failed", args);
}

fn drift_kinds(resume: &serde_json::Value) -> Vec<String> {
    resume["drift"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn resume_from_latest_checkpoint_projects_active_work() {
    let fixture = Fixture::new("latest");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement workspace invitations", "--json"]);
    fixture.json(&["decision", "add", "Invitations expire after 24h", "--json"]);
    fixture.json(&[
        "finding",
        "add",
        "Consumption fails inside the transaction",
        "--json",
    ]);
    fixture.write_artifact("src/create.ts");
    fixture.json(&["artifact", "add", "src/create.ts", "--json"]);

    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    let checkpoint_id = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();

    let resume = fixture.json(&["resume", "--json"]);
    assert_eq!(resume["checkpoint"]["id"], checkpoint_id.as_str());
    assert_eq!(resume["workspace"]["name"], "campfire");
    assert_eq!(
        resume["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert_eq!(resume["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(resume["findings"].as_array().unwrap().len(), 1);
    assert_eq!(resume["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(resume["drift"]["level"], "none");
    assert_eq!(resume["unresolved_work"].as_array().unwrap().len(), 1);
    assert!(resume["unresolved_work"][0]
        .as_str()
        .unwrap()
        .starts_with("Recorded observation:"));
    assert!(resume["suggested_continuation"][0]
        .as_str()
        .unwrap()
        .contains("Investigate:"));
}

#[test]
fn resume_uses_explicit_checkpoint() {
    let fixture = Fixture::new("explicit");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    let first = fixture.json(&["checkpoint", "create", "--json"]);
    let first_id = first["checkpoint"]["id"].as_str().unwrap().to_string();

    fixture.json(&[
        "decision",
        "add",
        "Recorded after first checkpoint",
        "--json",
    ]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let resume = fixture.json(&["resume", "--checkpoint", &first_id, "--json"]);
    assert_eq!(resume["checkpoint"]["id"], first_id.as_str());
    assert_eq!(resume["decisions"].as_array().unwrap().len(), 0);
}

#[test]
fn resume_detects_dirty_state_drift() {
    let fixture = Fixture::new("dirty");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    std::fs::write(fixture.repo.join("README.md"), b"# campfire changed\n").unwrap();

    let resume = fixture.json(&["resume", "--json"]);
    assert_eq!(resume["drift"]["level"], "informational");
    assert!(drift_kinds(&resume)
        .iter()
        .any(|kind| kind == "repository.dirty"));
}

#[test]
fn resume_detects_missing_artifact_drift() {
    let fixture = Fixture::new("missing");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.write_artifact("src/tracked.ts");
    fixture.json(&["artifact", "add", "src/tracked.ts", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    std::fs::remove_file(fixture.repo.join("src/tracked.ts")).unwrap();

    let resume = fixture.json(&["resume", "--json"]);
    assert_eq!(resume["drift"]["level"], "warning");
    assert!(drift_kinds(&resume)
        .iter()
        .any(|kind| kind == "artifact.missing"));
}

#[test]
fn resume_detects_changed_artifact_drift() {
    let fixture = Fixture::new("changed");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.write_artifact("src/tracked.ts");
    fixture.json(&["artifact", "add", "src/tracked.ts", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    std::fs::write(fixture.repo.join("src/tracked.ts"), b"export { changed }\n").unwrap();

    let resume = fixture.json(&["resume", "--json"]);
    assert_eq!(resume["drift"]["level"], "warning");
    assert!(drift_kinds(&resume)
        .iter()
        .any(|kind| kind == "artifact.changed"));
}

#[test]
fn resume_human_output_labels_suggestions() {
    let fixture = Fixture::new("human");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.json(&["finding", "add", "Consumption fails", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let output = fixture.run(&["resume"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Root Resume"), "{stdout}");
    assert!(stdout.contains("Suggested continuation"), "{stdout}");
    assert!(stdout.contains("Suggestion"), "{stdout}");
    assert!(stdout.contains("Drift"), "{stdout}");
}

#[test]
fn resume_without_checkpoint_errors_clearly() {
    let fixture = Fixture::new("nocheckpoint");
    fixture.json(&["workspace", "init", "--json"]);
    let output = fixture.run(&["resume", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["message"]
        .as_str()
        .unwrap()
        .contains("root checkpoint create"));
}

fn drift_level(resume: &serde_json::Value, kind: &str) -> Option<String> {
    resume["drift"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == kind)
        .map(|item| item["level"].as_str().unwrap().to_string())
}

fn assert_sorted_newest_first(value: &serde_json::Value) {
    let items = value.as_array().unwrap();
    for pair in items.windows(2) {
        let a_time = pair[0]["created_at"].as_str().unwrap();
        let a_id = pair[0]["id"].as_str().unwrap();
        let b_time = pair[1]["created_at"].as_str().unwrap();
        let b_id = pair[1]["id"].as_str().unwrap();
        assert!(
            a_time > b_time || (a_time == b_time && a_id < b_id),
            "items not newest-first (created_at desc, id asc): ({a_time},{a_id}) then ({b_time},{b_id})"
        );
    }
}

/// §7.2: a long-running workspace is projected within the 10/10/20 caps, with
/// omitted counts, full `current_state` counts, newest-first order, superseded
/// entries excluded, and byte-identical output across two runs.
///
/// `SessionRecord` is provenance-only and has no CLI surface, so the two
/// sessions in the fixture are seeded through the durable `WorkStore` API. The
/// same API seeds the bulk records to keep the test fast; `root resume` (the
/// unit under test) is always a separate process.
#[test]
fn long_running_resume_is_bounded_and_deterministic() {
    let fixture = Fixture::new("long_running");
    let repository = root_work::Repository::discover(&fixture.repo).unwrap();
    let init = root_work::WorkStore::init_at(&fixture.root_dir, repository.clone()).unwrap();
    let workspace_id = init.workspace.id.clone();

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
        store
            .start_session(Some("codex"), Some("agent-one"))
            .unwrap();
        store
            .start_session(Some("claude"), Some("agent-two"))
            .unwrap();
        // `list_*` is newest-first, so the last entry is the earliest record.
        let decision = store.list_decisions().unwrap().last().unwrap().id.clone();
        let finding = store.list_findings().unwrap().last().unwrap().id.clone();
        (decision, finding)
    };

    // No public API supersedes decisions/findings; flip one of each directly so
    // the projection proves it excludes superseded rows.
    {
        let database = root_work::paths::database_path(&fixture.root_dir, &workspace_id);
        let connection = root_work::db::open(&database).unwrap();
        connection
            .execute(
                "UPDATE decisions SET status = 'superseded' WHERE id = ?1",
                (superseded_decision.as_str(),),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE findings SET status = 'superseded' WHERE id = ?1",
                (superseded_finding.as_str(),),
            )
            .unwrap();
    }

    {
        let mut store =
            root_work::WorkStore::open_at(&fixture.root_dir, repository.clone()).unwrap();
        for index in 0..20 {
            root_continuity::create_on_store(
                &mut store,
                &repository,
                &fixture.root_dir,
                Some(&format!("checkpoint {index:02}")),
                root_work::ProvenanceContext::default(),
            )
            .unwrap();
        }
    }

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

    let decisions = report["decisions"].as_array().unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert!(decisions.iter().all(|entry| entry["status"] == "active"));
    assert!(findings.iter().all(|entry| entry["status"] == "active"));
    assert!(!decisions
        .iter()
        .any(|entry| entry["id"] == superseded_decision.as_str()));
    assert!(!findings
        .iter()
        .any(|entry| entry["id"] == superseded_finding.as_str()));

    assert_sorted_newest_first(&report["decisions"]);
    assert_sorted_newest_first(&report["findings"]);
    assert_sorted_newest_first(&report["artifacts"]);

    let first = fixture.run(&["resume", "--json"]);
    let second = fixture.run(&["resume", "--json"]);
    assert!(first.status.success() && second.status.success());
    assert_eq!(
        first.stdout, second.stdout,
        "two resume runs must be byte-identical"
    );
}

/// §7.3: resume names every drift branch with its level; the report level is the
/// max of its items; a clean checkpoint reports none.
#[test]
fn drift_branches_surface_in_resume() {
    let fixture = Fixture::new("drift_branches");
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

    // HEAD change -> warning.
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

    // Dirty working tree -> informational (HEAD change still a warning).
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

    // Missing artifact -> warning; overall level is the max.
    std::fs::remove_file(fixture.repo.join("src/tracked.ts")).unwrap();
    let missing = fixture.json(&["resume", "--json"]);
    assert_eq!(
        drift_level(&missing, "artifact.missing").as_deref(),
        Some("warning")
    );
    assert_eq!(
        drift_level(&missing, "repository.dirty").as_deref(),
        Some("informational")
    );
    assert_eq!(
        drift_level(&missing, "repository.head").as_deref(),
        Some("warning")
    );
    assert_eq!(missing["drift"]["level"], "warning");

    // Human output names each branch.
    let human = fixture.run(&["resume"]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("HEAD changed"), "{stdout}");
}
