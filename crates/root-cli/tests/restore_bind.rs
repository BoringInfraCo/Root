//! Sprint 013 §7.6: `root restore --dry-run` must report the durable work bind
//! (env-first ordering) without mutating anything — including under
//! `--rebind`, where the proposed pointer repair is reported but never
//! written. A real restore persists the pointer repair only after environment
//! restoration succeeds.
//!
//! Nix limitation: the real `root restore` path validates Nix availability
//! (`root_core::restore_dry_run` -> `restore_validate`) before it reaches the
//! work bind. Real Nix is not available in CI. These tests therefore install a
//! *fake* `nix` shim on the child `PATH` that answers the availability and
//! experimental-feature probes (`nix --version`, `nix eval nixpkgs#hello`) with
//! success. The fixture uses an empty, valid `root.lock` (0 packages), so the
//! restore plan is a genuine no-op: no package is installed, no profile is
//! written, and no real Nix is ever invoked or mutated. The shim only satisfies
//! a binary probe — exactly the case the sprint brief allows.
//!
//! Hermetic: isolated HOME/ROOT_DIR/TMPDIR/PATH per child process. No process
//! env mutation and no global state, so tests in this file need no mutex.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_restore_bind_{name}_{}_{}",
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

    /// A fake `nix` that succeeds on the probe commands only. It never touches
    /// the filesystem and never installs anything.
    fn write_nix_shim(&self) {
        let path = self.bin.join("nix");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Write the canonical (empty, schema v2) root.lock. `marker` changes the
    /// bytes so a checkpoint recorded against one lock diverges from another.
    fn write_lock(&self, marker: &str) {
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

    fn init_workspace(&self) -> serde_json::Value {
        self.json(&["workspace", "init", "--json"])
    }

    fn database_path(&self, workspace_id: &str) -> PathBuf {
        self.root_dir
            .join("work")
            .join(workspace_id)
            .join("state.db")
    }

    fn profile_path(&self) -> PathBuf {
        self.home.join(".root").join("profiles").join("default")
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
        .expect("git must be available for restore bind tests");
    assert!(status.success(), "git {:?} failed", args);
}

fn db_fingerprint(path: &Path) -> String {
    root_work::fingerprint_file(path).expect("state.db must be fingerprintable")
}

#[test]
fn restore_dry_run_reports_work_bind_without_mutation() {
    let fixture = Fixture::new("bind");
    fixture.write_lock("a");
    let init = fixture.init_workspace();
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();

    fixture.json(&["goal", "set", "Ship the restore bind", "--json"]);
    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    let checkpoint_id = checkpoint["checkpoint"]["id"].as_str().unwrap().to_string();

    let db = fixture.database_path(&workspace_id);
    let before_fingerprint = db_fingerprint(&db);
    let before_mtime = std::fs::metadata(&db).unwrap().modified().unwrap();
    let before_status = fixture.json(&["workspace", "status", "--json"]);
    let before_revision = before_status["work_revision"].as_i64().unwrap();
    let before_checkpoints = fixture.json(&["checkpoint", "list", "--json"])["checkpoints"]
        .as_array()
        .unwrap()
        .len();
    assert!(!fixture.profile_path().exists());

    let report = fixture.json(&["restore", "--dry-run", "--json"]);

    let bind = &report["work_bind"];
    assert_eq!(bind["available"], true, "json={report}");
    assert_eq!(bind["workspace_id"], workspace_id.as_str(), "json={report}");
    assert_eq!(
        bind["latest_checkpoint_id"],
        checkpoint_id.as_str(),
        "json={report}"
    );
    assert!(
        bind["work_revision"].as_i64().unwrap() >= 1,
        "work_revision must be reported: {report}"
    );

    // --dry-run mutates nothing: DB bytes, mtime, work events, profile.
    assert_eq!(db_fingerprint(&db), before_fingerprint, "DB bytes changed");
    assert_eq!(
        std::fs::metadata(&db).unwrap().modified().unwrap(),
        before_mtime,
        "DB mtime changed"
    );
    let after_status = fixture.json(&["workspace", "status", "--json"]);
    assert_eq!(
        after_status["work_revision"].as_i64().unwrap(),
        before_revision
    );
    assert_eq!(
        fixture.json(&["checkpoint", "list", "--json"])["checkpoints"]
            .as_array()
            .unwrap()
            .len(),
        before_checkpoints
    );
    assert!(
        !fixture.profile_path().exists(),
        "dry-run must not create a Nix profile"
    );
}

#[test]
fn work_bind_unavailable_outside_workspace() {
    let fixture = Fixture::new("unbound");
    fixture.write_lock("a");

    // Repository exists but no workspace was initialized.
    let output = fixture.run(&["restore", "--dry-run", "--json"]);
    assert!(
        output.status.success(),
        "restore must not fail because no workspace is bound: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let bind = &report["work_bind"];
    assert_eq!(bind["available"], false, "json={report}");
    assert!(bind["workspace_id"].is_null(), "json={report}");
    assert!(bind["latest_checkpoint_id"].is_null(), "json={report}");
    let notes = bind["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|note| note.as_str().unwrap().contains("no workspace bound")),
        "notes must explain the missing bind: {report}"
    );
}

#[test]
fn work_bind_surfaces_environment_drift_before_resume() {
    let fixture = Fixture::new("drift");
    std::fs::write(fixture.root_dir.join("Rootfile"), b"[packages]\n").unwrap();
    fixture.write_lock("recorded");

    let init = fixture.init_workspace();
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    fixture.json(&["goal", "set", "Environment drift", "--json"]);
    let checkpoint = fixture.json(&["checkpoint", "create", "--json"]);
    assert_eq!(
        checkpoint["checkpoint"]["environment_status"], "observed",
        "fixture must record a full environment: {checkpoint}"
    );

    // Diverge the lock digest after the checkpoint.
    fixture.write_lock("changed");

    let report = fixture.json(&["restore", "--dry-run", "--json"]);
    let bind = &report["work_bind"];
    assert_eq!(bind["available"], true, "json={report}");

    let drift = &bind["drift"];
    assert_eq!(drift["level"], "warning", "json={report}");
    let kinds: Vec<&str> = drift["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"environment.lock"),
        "environment.lock drift must surface at bind time: {report}"
    );
    assert!(
        !kinds.contains(&"environment.rootfile"),
        "only the diverged item should appear: {report}"
    );
    // Every item is an environment item: the drift screen runs at env-bind time,
    // before any work/resume projection exists in this output.
    assert!(kinds.iter().all(|kind| kind.starts_with("environment.")));
    assert_eq!(
        report["work_bind"]["workspace_id"],
        workspace_id.as_str(),
        "json={report}"
    );
    assert!(
        report.get("decisions").is_none() && report.get("findings").is_none(),
        "restore must not project work state before the drift screen: {report}"
    );
}

fn pointer_path(repo: &Path) -> PathBuf {
    repo.join(".root").join("workspace.json")
}

fn pointer_bytes_and_mtime(repo: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let path = pointer_path(repo);
    let bytes = std::fs::read(&path).expect("pointer file must exist");
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    (bytes, mtime)
}

fn combined(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr)
}

#[test]
fn restore_dry_run_does_not_migrate() {
    let fixture = Fixture::new("dry_run_ro");
    fixture.write_lock("a");
    let init = fixture.init_workspace();
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
    fixture.json(&["goal", "set", "Read-only dry run", "--json"]);
    fixture.json(&["checkpoint", "create", "--json"]);

    let db = fixture.database_path(&workspace_id);
    let before_bytes = std::fs::read(&db).unwrap();
    let before_mtime = std::fs::metadata(&db).unwrap().modified().unwrap();

    fixture.json(&["restore", "--dry-run", "--json"]);

    assert_eq!(
        std::fs::read(&db).unwrap(),
        before_bytes,
        "dry-run migrated/changed the database bytes"
    );
    assert_eq!(
        std::fs::metadata(&db).unwrap().modified().unwrap(),
        before_mtime,
        "dry-run changed the database mtime"
    );
    let dir = db.parent().unwrap();
    assert!(
        !dir.join("state.db-wal").exists(),
        "dry-run created a -wal sidecar"
    );
    assert!(
        !dir.join("state.db-shm").exists(),
        "dry-run created a -shm sidecar"
    );
}

#[test]
fn restore_fails_on_stale_pointer() {
    let fixture = Fixture::new("stale_pointer");
    fixture.write_lock("a");
    fixture.init_workspace();

    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    std::fs::write(
        pointer_path(&fixture.repo),
        b"{\"workspace_id\":\"root_ws_UNKNOWN\",\"root_dir_hint\":null}",
    )
    .unwrap();

    let output = fixture.run(&["restore", "--dry-run", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "stale pointer must fail closed with exit 2: {}",
        combined(&output)
    );
    let text = combined(&output);
    assert!(
        text.contains("Unknown workspace id") || text.contains("rebind"),
        "stale pointer refusal must name the remediation: {text}"
    );
    assert!(
        text.contains("rebind"),
        "stale pointer refusal must name `root restore --rebind`: {text}"
    );
}

#[test]
fn restore_fails_on_malformed_pointer() {
    let fixture = Fixture::new("malformed_pointer");
    fixture.write_lock("a");
    fixture.init_workspace();

    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    std::fs::write(pointer_path(&fixture.repo), b"{ not json").unwrap();

    let output = fixture.run(&["restore", "--dry-run", "--json"]);
    assert_ne!(
        output.status.code(),
        Some(0),
        "malformed pointer must fail closed: {}",
        combined(&output)
    );
    let text = combined(&output);
    assert!(
        text.to_lowercase().contains("pointer"),
        "malformed pointer refusal must name the pointer: {text}"
    );
}

#[test]
fn restore_dry_run_rebind_reports_proposal_without_writing_pointer() {
    let fixture = Fixture::new("rebind_dry_run");
    fixture.write_lock("a");
    let init = fixture.init_workspace();
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();

    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    std::fs::write(pointer_path(&fixture.repo), b"{ not json").unwrap();
    let (before_bytes, before_mtime) = pointer_bytes_and_mtime(&fixture.repo);

    let report = fixture.json(&["restore", "--rebind", "--dry-run", "--json"]);

    // The proposed rebind is reported: current state + target + no write.
    let rebind = &report["rebind"];
    assert_eq!(
        rebind["workspace_id"],
        workspace_id.as_str(),
        "rebind must report the proposed workspace id: {report}"
    );
    assert_eq!(
        rebind["applied"], false,
        "dry-run must not claim the pointer was written: {report}"
    );
    assert_eq!(
        rebind["current_state"], "malformed",
        "rebind must report the current pointer state: {report}"
    );
    assert!(
        rebind["current_workspace_id"].is_null(),
        "a malformed pointer has no readable id: {report}"
    );
    assert!(
        rebind["pointer_path"]
            .as_str()
            .unwrap()
            .ends_with(".root/workspace.json"),
        "rebind must name the pointer file: {report}"
    );

    // Byte-identical pointer file: content and mtime.
    let (after_bytes, after_mtime) = pointer_bytes_and_mtime(&fixture.repo);
    assert_eq!(
        after_bytes, before_bytes,
        "--dry-run --rebind rewrote the pointer bytes"
    );
    assert_eq!(
        after_mtime, before_mtime,
        "--dry-run --rebind touched the pointer mtime"
    );

    // The work bind reports the pending repair instead of failing closed on
    // the broken pointer the operator already asked to fix.
    let bind = &report["work_bind"];
    assert_eq!(bind["available"], false, "json={report}");
    let notes = bind["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|note| {
            let note = note.as_str().unwrap();
            note.contains("malformed") && note.contains(workspace_id.as_str())
        }),
        "notes must describe the proposed repair: {report}"
    );
}

#[test]
fn restore_rebind_env_failure_leaves_pointer_unchanged() {
    let fixture = Fixture::new("rebind_env_fail");
    fixture.write_lock("a");
    fixture.init_workspace();

    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    std::fs::write(pointer_path(&fixture.repo), b"{ not json").unwrap();
    let (before_bytes, before_mtime) = pointer_bytes_and_mtime(&fixture.repo);

    // Inject a deterministic environment-restore failure: unsupported lock
    // schema. The pointer must never be rewritten when env restore fails.
    std::fs::write(
        fixture.root_dir.join("root.lock"),
        b"{\"version\":99,\"platform\":\"\",\"packages\":[]}\n",
    )
    .unwrap();

    let output = fixture.run(&["restore", "--rebind", "--json"]);
    let text = combined(&output);
    assert_ne!(
        output.status.code(),
        Some(0),
        "env restore failure must exit non-zero: {text}"
    );
    assert!(
        text.contains("root.lock") && text.contains("99"),
        "failure must come from environment restoration, not an earlier step: {text}"
    );

    let (after_bytes, after_mtime) = pointer_bytes_and_mtime(&fixture.repo);
    assert_eq!(
        after_bytes, before_bytes,
        "pointer was rewritten despite env restore failure"
    );
    assert_eq!(
        after_mtime, before_mtime,
        "pointer mtime changed despite env restore failure"
    );
    assert!(
        !fixture.profile_path().exists(),
        "a failed env restore must not leave a Nix profile behind"
    );
}

#[test]
fn restore_rebind_repairs_pointer_after_env_restore() {
    let fixture = Fixture::new("rebind_apply");
    fixture.write_lock("a");
    let init = fixture.init_workspace();
    let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();

    std::fs::create_dir_all(fixture.repo.join(".root")).unwrap();
    std::fs::write(pointer_path(&fixture.repo), b"{ not json").unwrap();

    let report = fixture.json(&["restore", "--rebind", "--json"]);

    let rebind = &report["rebind"];
    assert_eq!(
        rebind["workspace_id"],
        workspace_id.as_str(),
        "rebind must report the repaired workspace id: {report}"
    );
    assert_eq!(rebind["applied"], true, "json={report}");
    assert_eq!(rebind["current_state"], "malformed", "json={report}");

    let pointer = root_work::pointer::load(&fixture.repo)
        .unwrap()
        .expect("pointer must exist after rebind");
    assert_eq!(pointer.workspace_id, workspace_id);

    let bind = &report["work_bind"];
    assert_eq!(bind["available"], true, "json={report}");
    assert_eq!(
        bind["workspace_id"],
        workspace_id.as_str(),
        "work bind must resolve the repaired pointer: {report}"
    );
}
