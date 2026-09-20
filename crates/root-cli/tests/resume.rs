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
