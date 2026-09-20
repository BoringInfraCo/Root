//! Sprint 005 integration: durable work state via the `root` CLI.
//!
//! Each invocation is a separate process, so these tests also prove that work
//! state survives process restarts and is discoverable from the repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_workspace_{name}_{}_{}",
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
        std::fs::create_dir_all(repo.join(".git")).unwrap();
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

fn assert_ok(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "expected success: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn workspace_lifecycle_and_persistence() {
    let fixture = Fixture::new("lifecycle");

    let init = fixture.json(&["workspace", "init", "--json"]);
    assert_eq!(init["success"], true);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    assert!(workspace_id.starts_with("root_ws_"));
    assert!(Path::new(init["database"].as_str().unwrap()).exists());

    let goal = fixture.json(&["goal", "set", "Implement workspace invitations", "--json"]);
    assert_eq!(goal["goal"]["statement"], "Implement workspace invitations");
    assert_eq!(goal["goal"]["status"], "active");

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

    fixture.json(&[
        "finding",
        "add",
        "Invite consumption fails inside the transaction",
        "--evidence",
        "tests/invitations.test.ts",
        "--json",
    ]);

    std::fs::write(fixture.repo.join("create.ts"), b"export const x = 1;\n").unwrap();
    let artifact = fixture.json(&["artifact", "add", "create.ts", "--json"]);
    assert_eq!(artifact["artifact"]["kind"], "file");
    assert!(artifact["artifact"]["fingerprint"].as_str().is_some());

    // A fresh process must rediscover the same durable workspace.
    let status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id);
    assert_eq!(
        status["goal"]["statement"],
        "Implement workspace invitations"
    );
    assert_eq!(status["counts"]["decisions"], 1);
    assert_eq!(status["counts"]["findings"], 1);
    assert_eq!(status["counts"]["artifacts"], 1);
    assert_eq!(
        status["repository"]["path"],
        std::fs::canonicalize(&fixture.repo)
            .unwrap()
            .display()
            .to_string()
    );
    assert!(status["work_revision"].as_i64().unwrap() >= 5);

    let shown = fixture.json(&["decision", "show", &decision_id, "--json"]);
    assert_eq!(shown["decision"]["id"], decision_id);

    let decisions = fixture.json(&["decision", "list", "--json"]);
    assert_eq!(decisions["decisions"].as_array().unwrap().len(), 1);
    let findings = fixture.json(&["finding", "list", "--json"]);
    assert_eq!(findings["findings"].as_array().unwrap().len(), 1);
    let artifacts = fixture.json(&["artifact", "list", "--json"]);
    assert_eq!(artifacts["artifacts"].as_array().unwrap().len(), 1);
}

#[test]
fn setting_a_new_goal_supersedes_the_previous_one() {
    let fixture = Fixture::new("goals");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "First goal", "--json"]);
    let second = fixture.json(&["goal", "set", "Second goal", "--json"]);
    assert_eq!(second["goal"]["statement"], "Second goal");

    let status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(status["goal"]["statement"], "Second goal");
    assert_eq!(status["counts"]["goals"], 2);
}

#[test]
fn double_init_is_rejected() {
    let fixture = Fixture::new("double");
    assert_ok(&fixture.run(&["workspace", "init"]));
    let output = fixture.run(&["workspace", "init", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["success"], false);
    assert!(value["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
}

#[test]
fn mutations_require_an_initialized_workspace() {
    let fixture = Fixture::new("uninitialized");
    let output = fixture.run(&["goal", "set", "no workspace", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["message"]
        .as_str()
        .unwrap()
        .contains("No Root workspace"));
}

#[test]
fn missing_artifact_path_is_rejected() {
    let fixture = Fixture::new("missing_artifact");
    fixture.json(&["workspace", "init", "--json"]);
    let output = fixture.run(&["artifact", "add", "does-not-exist.ts", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["message"].as_str().unwrap().contains("missing"));
}

#[test]
fn command_outside_git_repository_is_rejected() {
    let fixture = Fixture::new("norepo");
    let output = Command::new(root_bin())
        .args(["workspace", "init", "--json"])
        .current_dir(&fixture.base)
        .env("ROOT_DIR", &fixture.root_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["message"]
        .as_str()
        .unwrap()
        .contains("Not inside a Git repository"));
}
