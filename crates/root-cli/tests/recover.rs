//! Sprint 010 integration: `root recover` after interruption.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_recover_{name}_{}_{}",
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
        .expect("git must be available for recover tests");
    assert!(status.success(), "git {:?} failed", args);
}

fn not_recoverable_items(report: &serde_json::Value) -> Vec<String> {
    report["not_recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn recover_without_checkpoint_reports_work_state() {
    let fixture = Fixture::new("nocheckpoint");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement invitations", "--json"]);
    fixture.json(&["decision", "add", "Tokens expire after 24h", "--json"]);

    let report = fixture.json(&["recover", "--json"]);
    assert_eq!(report["workspace"]["name"], "campfire");
    assert!(report["last_checkpoint"].is_null());
    assert_eq!(report["drift"]["level"], "none");
    assert_eq!(report["work_state"]["available"], true);
    assert_eq!(report["work_state"]["active_decisions"], 1);
    let recoverable: Vec<String> = report["recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect();
    assert!(recoverable.contains(&"goal".to_string()));
    assert!(recoverable.contains(&"decisions".to_string()));
    assert!(!recoverable.contains(&"checkpoint reference".to_string()));
    assert!(report["recommended_action"]
        .as_str()
        .unwrap()
        .contains("Create a checkpoint"));

    let items = not_recoverable_items(&report);
    assert!(items.contains(&"unrecorded agent conversation".to_string()));
    assert!(items.contains(&"unsaved editor state".to_string()));
    assert!(items.contains(&"commands not observed by Root".to_string()));
}

#[test]
fn recover_with_checkpoint_recommends_resuming() {
    let fixture = Fixture::new("checkpoint");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement invitations", "--json"]);
    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    let checkpoint_id = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();

    let report = fixture.json(&["recover", "--json"]);
    assert_eq!(report["last_checkpoint"]["id"], checkpoint_id.as_str());
    let recoverable: Vec<String> = report["recoverable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect();
    assert!(recoverable.contains(&"checkpoint reference".to_string()));
    assert_eq!(
        report["recommended_action"],
        format!("Resume from {checkpoint_id}.")
    );
}

#[test]
fn recover_surfaces_drift_after_head_change() {
    let fixture = Fixture::new("drift");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    std::fs::write(fixture.repo.join("work.ts"), b"export {}\n").unwrap();
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
            "more work",
        ],
    );

    let report = fixture.json(&["recover", "--json"]);
    assert_eq!(report["drift"]["level"], "warning");
    assert!(report["drift"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == "repository.head"));
    assert_eq!(
        report["recommended_action"],
        "Inspect drift before resuming."
    );
}

#[test]
fn recover_human_output_is_inspectable() {
    let fixture = Fixture::new("human");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let output = fixture.run(&["recover"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for section in [
        "Root Recovery",
        "Last durable checkpoint",
        "Repository",
        "Environment",
        "Work state",
        "Recoverable",
        "Not recoverable",
        "Recommended action",
    ] {
        assert!(stdout.contains(section), "missing {section}:\n{stdout}");
    }
    assert!(stdout.contains("commands not observed by Root"));
}
