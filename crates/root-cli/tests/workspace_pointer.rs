//! Sprint 013 §6: opt-in project pointer `<repo>/.root/workspace.json`.
//!
//! The pointer carries only `{ workspace_id, root_dir_hint }`; canonical state
//! stays under `~/.root/work/`. A missing pointer binds by repo identity; a
//! stale pointer fails closed without creating a divergent workspace.
//!
//! Hermetic: isolated HOME/ROOT_DIR/TMPDIR per child process, no process env
//! mutation, no shared global state.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_workspace_pointer_{name}_{}_{}",
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
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("campfire");
        let root_dir = base.join("root");
        let home = base.join("home");
        let tmpdir = base.join("tmp");
        for dir in [&repo, &root_dir, &home, &tmpdir] {
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
        Self {
            base,
            repo,
            root_dir,
            home,
            tmpdir,
        }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
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

    fn pointer_path(&self) -> PathBuf {
        self.repo.join(".root").join("workspace.json")
    }

    fn write_pointer(&self, workspace_id: &str) {
        let dir = self.repo.join(".root");
        std::fs::create_dir_all(&dir).unwrap();
        let body = serde_json::json!({ "workspace_id": workspace_id, "root_dir_hint": null });
        std::fs::write(
            self.pointer_path(),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    fn workspace_ids_in_index(&self) -> Vec<String> {
        let path = self.root_dir.join("work").join("index.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_string())
            .collect()
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
        .expect("git must be available for workspace pointer tests");
    assert!(status.success(), "git {:?} failed", args);
}

#[test]
fn workspace_init_with_pointer_writes_and_resolves() {
    let fixture = Fixture::new("write");
    let init = fixture.json(&["workspace", "init", "--write-pointer", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();

    let raw = std::fs::read_to_string(fixture.pointer_path()).unwrap();
    let pointer: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(pointer["workspace_id"], workspace_id.as_str(), "raw={raw}");

    // The pointer is a hint only: exactly workspace_id + root_dir_hint.
    let keys: Vec<&str> = pointer
        .as_object()
        .unwrap()
        .keys()
        .map(|key| key.as_str())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec!["root_dir_hint", "workspace_id"], "raw={raw}");
    assert!(
        pointer.get("secrets").is_none() && pointer.get("work").is_none(),
        "pointer must not carry work data: {raw}"
    );

    // A later command resolves the same workspace through the pointer.
    let status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str());
}

#[test]
fn pointer_absent_legacy_bind() {
    let fixture = Fixture::new("legacy");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    assert!(!fixture.pointer_path().exists());

    // Binding by repo identity resolves the same workspace without a duplicate.
    let first = fixture.json(&["workspace", "status", "--json"]);
    let second = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(first["workspace"]["id"], workspace_id.as_str());
    assert_eq!(second["workspace"]["id"], workspace_id.as_str());
    assert_eq!(fixture.workspace_ids_in_index(), vec![workspace_id]);
}

#[test]
fn pointer_stale_fails_closed() {
    let fixture = Fixture::new("stale");
    let init = fixture.json(&["workspace", "init", "--json"]);
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    fixture.write_pointer("root_ws_UNKNOWN");

    let output = fixture.run(&["workspace", "status", "--json"]);
    assert!(
        !output.status.success(),
        "stale pointer must fail closed: stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = value["message"].as_str().unwrap();
    assert!(
        message.contains("Unknown workspace id"),
        "message must name the unknown id: {message}"
    );
    assert!(
        message.contains("root restore") && message.contains("bind"),
        "message must route to `root restore` to bind: {message}"
    );

    // No divergent workspace in the index.
    assert_eq!(
        fixture.workspace_ids_in_index(),
        vec![workspace_id],
        "a stale pointer must never create a second workspace"
    );
}

#[test]
fn pointer_malformed_is_error() {
    let fixture = Fixture::new("malformed");
    fixture.json(&["workspace", "init", "--json"]);
    let dir = fixture.repo.join(".root");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(fixture.pointer_path(), b"{ not json").unwrap();

    let output = fixture.run(&["workspace", "status", "--json"]);
    assert!(
        !output.status.success(),
        "malformed pointer must fail: stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = value["message"].as_str().unwrap();
    assert!(
        message.contains("Workspace pointer"),
        "message must name the pointer: {message}"
    );
    assert!(
        message.contains("root restore"),
        "message must tell the user how to recover: {message}"
    );
}

#[test]
fn workspace_init_without_pointer_writes_nothing() {
    let fixture = Fixture::new("no-pointer");
    fixture.json(&["workspace", "init", "--json"]);
    assert!(
        !fixture.pointer_path().exists(),
        "init without --write-pointer must not create {}",
        fixture.pointer_path().display()
    );
    assert!(!fixture.repo.join(".root").exists());
}
