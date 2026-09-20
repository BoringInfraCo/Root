//! Sprint 006 integration: durable, immutable checkpoints.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_checkpoint_{name}_{}_{}",
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
        .expect("git must be available for checkpoint tests");
    assert!(status.success(), "git {:?} failed", args);
}

#[test]
fn clean_and_dirty_repository_checkpoints() {
    let fixture = Fixture::new("dirty");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Implement invitations", "--json"]);

    let clean = fixture.json(&["checkpoint", "create", "--json"]);
    assert_eq!(clean["checkpoint"]["git_dirty"], false);
    assert!(clean["checkpoint"]["git_head"].as_str().is_some());
    assert!(clean["checkpoint"]["git_dirty_fingerprint"]
        .as_str()
        .is_some());
    assert_eq!(clean["checkpoint"]["environment_status"], "missing");

    std::fs::write(fixture.repo.join("untracked.ts"), b"export {}\n").unwrap();
    let dirty = fixture.json(&["checkpoint", "create", "--json"]);
    assert_eq!(dirty["checkpoint"]["git_dirty"], true);
    assert_ne!(
        clean["checkpoint"]["git_dirty_fingerprint"],
        dirty["checkpoint"]["git_dirty_fingerprint"]
    );
}

#[test]
fn checkpoints_are_immutable_and_survive_restart() {
    let fixture = Fixture::new("immutable");
    fixture.json(&["workspace", "init", "--json"]);
    fixture.json(&["goal", "set", "Goal", "--json"]);
    let first = fixture.json(&[
        "checkpoint",
        "create",
        "--message",
        "before more work",
        "--json",
    ]);
    let first_id = first["checkpoint"]["id"].as_str().unwrap().to_string();
    let first_revision = first["checkpoint"]["work_revision"].as_i64().unwrap();

    fixture.json(&["decision", "add", "recorded after checkpoint", "--json"]);

    // New process: the old checkpoint must be byte-for-byte unchanged.
    let shown = fixture.json(&["checkpoint", "show", &first_id, "--json"]);
    assert_eq!(shown["checkpoint"]["work_revision"], first_revision);
    assert_eq!(shown["checkpoint"]["message"], "before more work");
    assert_eq!(
        shown["checkpoint"]["snapshot"],
        first["checkpoint"]["snapshot"]
    );

    let last = fixture.json(&["checkpoint", "show", "--last", "--json"]);
    assert_eq!(last["checkpoint"]["id"], first_id);

    let list = fixture.json(&["checkpoint", "list", "--json"]);
    assert_eq!(list["checkpoints"].as_array().unwrap().len(), 1);
}

#[test]
fn checkpoint_captures_environment_digests_when_present() {
    let fixture = Fixture::new("environment");
    std::fs::write(fixture.root_dir.join("Rootfile"), b"[packages]\n").unwrap();
    std::fs::write(fixture.root_dir.join("root.lock"), b"{}\n").unwrap();
    fixture.json(&["workspace", "init", "--json"]);

    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    assert_eq!(checkpoint["checkpoint"]["environment_status"], "observed");
    assert!(checkpoint["checkpoint"]["rootfile_digest"]
        .as_str()
        .is_some());
    assert!(checkpoint["checkpoint"]["root_lock_digest"]
        .as_str()
        .is_some());
    assert!(checkpoint["checkpoint"]["continuation_summary"]
        .as_str()
        .unwrap()
        .contains("Environment observed"));
}

#[test]
fn show_without_id_or_last_is_rejected() {
    let fixture = Fixture::new("showargs");
    fixture.json(&["workspace", "init", "--json"]);
    let output = fixture.run(&["checkpoint", "show", "--json"]);
    assert!(!output.status.success());
}

#[test]
fn checkpoint_requires_workspace() {
    let fixture = Fixture::new("noworkspace");
    let output = fixture.run(&["checkpoint", "create", "--json"]);
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["message"]
        .as_str()
        .unwrap()
        .contains("No Root workspace"));
}
