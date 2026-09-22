//! Sprint 013 CLI: `root workspace export` / `import` acceptance tests.
//!
//! Hermetic child-env isolation: every invocation gets its own `HOME`,
//! `CODEX_HOME`, `ROOT_DIR`, `TMPDIR`, and a private `PATH` with fake agent
//! shims. No process-env mutation, so tests in this binary need no mutex.
//!
//! Export moves recorded work state between Root directories. Import always
//! targets a *fresh* workspace and is integrity-checked and all-or-nothing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn tmp(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "root_cli_workspace_transfer_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Env {
    base: PathBuf,
    root_a: PathBuf,
    root_b: PathBuf,
    repo_a: PathBuf,
    repo_b: PathBuf,
    home: PathBuf,
    codex_home: PathBuf,
    tmpdir: PathBuf,
    bin: PathBuf,
}

impl Env {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let root_a = base.join("root_a");
        let root_b = base.join("root_b");
        let repo_a = base.join("repo_a");
        let repo_b = base.join("clone").join("repo_b");
        let home = base.join("home");
        let codex_home = base.join("codex");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for dir in [
            &base,
            &root_a,
            &root_b,
            &repo_a,
            &repo_b,
            &home,
            &codex_home,
            &tmpdir,
            &bin,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        init_repo(&repo_a);
        init_repo(&repo_b);

        let env = Self {
            base,
            root_a,
            root_b,
            repo_a,
            repo_b,
            home,
            codex_home,
            tmpdir,
            bin,
        };
        env.write_shim("codex", "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n");
        env.write_shim("opencode", "#!/bin/sh\nprintf '1.18.27\\n'\n");
        env.write_shim("claude", "#!/bin/sh\nprintf '2.1.260\\n'\n");
        env.write_codex_portable();
        env
    }

    fn write_shim(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn write_codex_portable(&self) {
        std::fs::write(
            self.codex_home.join("config.toml"),
            "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\n",
        )
        .unwrap();
        std::fs::write(self.codex_home.join("AGENTS.md"), "# agents\n").unwrap();
        let skill = self.home.join(".agents/skills/docs-writer");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "# skill\n").unwrap();
    }

    fn run_in(&self, dir: &Path, root_dir: &Path, args: &[&str]) -> std::process::Output {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(dirs).unwrap();
        Command::new(root_bin())
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env("ROOT_DIR", root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path)
            .output()
            .unwrap()
    }

    fn json_ok(&self, dir: &Path, root_dir: &Path, args: &[&str]) -> serde_json::Value {
        let output = self.run_in(dir, root_dir, args);
        assert!(
            output.status.success(),
            "command {:?} failed: status={:?} stderr={} stdout={}",
            args,
            output.status.code(),
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

    fn db_path(&self, root_dir: &Path, workspace_id: &str) -> PathBuf {
        root_dir.join("work").join(workspace_id).join("state.db")
    }

    fn transfer_path(&self) -> PathBuf {
        self.base.join("transfer.rootws.json")
    }

    /// Populate ROOT_DIR A with a goal, two decisions, a finding, an artifact,
    /// a captured agent environment, and a checkpoint. Returns the workspace id.
    fn prepare_source(&self) -> String {
        let init = self.json_ok(&self.repo_a, &self.root_a, &["workspace", "init", "--json"]);
        let workspace_id = init["workspace"]["id"].as_str().unwrap().to_string();
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["goal", "set", "Ship transfer acceptance", "--json"],
        );
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["decision", "add", "Document is versioned", "--json"],
        );
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["decision", "add", "Import is all-or-nothing", "--json"],
        );
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["finding", "add", "Digests detect tampering", "--json"],
        );
        std::fs::write(self.repo_a.join("notes.md"), b"# notes\n").unwrap();
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["artifact", "add", "notes.md", "--json"],
        );

        std::fs::create_dir_all(self.repo_a.join(".root")).unwrap();
        let capture = self.run_in(
            &self.repo_a,
            &self.root_a,
            &[
                "agent",
                "capture",
                "--from",
                "codex",
                "--apply",
                "--out",
                ".root/agent.toml",
            ],
        );
        assert!(
            capture.status.success(),
            "capture --apply must succeed: stdout={} stderr={}",
            String::from_utf8_lossy(&capture.stdout),
            String::from_utf8_lossy(&capture.stderr)
        );
        self.json_ok(
            &self.repo_a,
            &self.root_a,
            &["checkpoint", "create", "--json"],
        );
        workspace_id
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn init_repo(repo: &Path) {
    git(repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), b"# campfire\n").unwrap();
    git(repo, &["add", "-A"]);
    git(
        repo,
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
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must be available for workspace transfer tests");
    assert!(status.success(), "git {:?} failed", args);
}

#[test]
fn two_root_dir_export_import_acceptance() {
    let env = Env::new("acceptance");
    let workspace_id = env.prepare_source();

    let out = env.transfer_path();
    let export = env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["workspace", "export", "-o", out.to_str().unwrap(), "--json"],
    );
    assert_eq!(export["workspace_id"], workspace_id.as_str(), "{export}");
    assert_eq!(export["goals"], 1, "{export}");
    assert_eq!(export["decisions"], 2, "{export}");
    assert_eq!(export["findings"], 1, "{export}");
    assert_eq!(export["artifacts"], 1, "{export}");
    assert_eq!(export["checkpoints"], 1, "{export}");
    assert!(export["bytes"].as_u64().unwrap() > 0, "{export}");
    assert_eq!(
        export["payload_sha256"].as_str().unwrap().len(),
        64,
        "{export}"
    );
    assert!(out.exists(), "export document must exist");

    // Fresh ROOT_DIR B and a different project path (a simulated clone).
    let import = env.json_ok(
        &env.base,
        &env.root_b,
        &[
            "workspace",
            "import",
            out.to_str().unwrap(),
            "--project",
            env.repo_b.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(import["workspace_id"], workspace_id.as_str(), "{import}");
    assert_eq!(import["goals"], 1, "{import}");
    assert_eq!(import["decisions"], 2, "{import}");
    assert_eq!(import["checkpoints"], 1, "{import}");
    assert!(
        import["latest_checkpoint_id"].is_string(),
        "latest checkpoint must be reported: {import}"
    );

    // The workspace resolves in B and its state is intact.
    let status = env.json_ok(&env.repo_b, &env.root_b, &["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str(), "{status}");

    let checkpoints = env.json_ok(&env.repo_b, &env.root_b, &["checkpoint", "list", "--json"]);
    assert_eq!(
        checkpoints["checkpoints"].as_array().unwrap().len(),
        1,
        "{checkpoints}"
    );

    let resume = env.json_ok(
        &env.repo_b,
        &env.root_b,
        &["resume", "--with", "codex", "--json"],
    );
    assert_eq!(
        resume["package"]["goal"]["statement"], "Ship transfer acceptance",
        "{resume}"
    );

    // State physically lives in each Root directory's own database.
    assert_ne!(
        env.db_path(&env.root_a, &workspace_id),
        env.db_path(&env.root_b, &workspace_id),
        "A and B must not share a database path"
    );
}

#[test]
fn export_import_tamper_refused() {
    let env = Env::new("tamper");
    env.prepare_source();
    let out = env.transfer_path();
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["workspace", "export", "-o", out.to_str().unwrap(), "--json"],
    );

    // Mutate a payload byte: the digest must no longer match.
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let name = value["workspace"]["name"].as_str().unwrap().to_string();
    value["workspace"]["name"] = serde_json::Value::String(format!("{name}-tampered"));
    std::fs::write(&out, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let output = env.run_in(
        &env.base,
        &env.root_b,
        &[
            "workspace",
            "import",
            out.to_str().unwrap(),
            "--project",
            env.repo_b.to_str().unwrap(),
            "--json",
        ],
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "tampered transfer must be refused: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        combined.to_lowercase().contains("integrity")
            || combined.to_lowercase().contains("digest")
            || combined.to_lowercase().contains("tamper"),
        "refusal must name integrity: {combined}"
    );
}

#[test]
fn import_into_existing_workspace_refused() {
    let env = Env::new("existing");
    env.prepare_source();
    let out = env.transfer_path();
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["workspace", "export", "-o", out.to_str().unwrap(), "--json"],
    );

    let import_args = [
        "workspace",
        "import",
        out.to_str().unwrap(),
        "--project",
        env.repo_b.to_str().unwrap(),
        "--json",
    ];
    env.json_ok(&env.base, &env.root_b, &import_args);

    let second = env.run_in(&env.base, &env.root_b, &import_args);
    assert_ne!(
        second.status.code(),
        Some(0),
        "second import into the same Root directory must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let combined = String::from_utf8_lossy(&second.stdout).to_string()
        + &String::from_utf8_lossy(&second.stderr);
    assert!(
        combined.to_lowercase().contains("already exists")
            || combined.to_lowercase().contains("fresh"),
        "refusal must explain the fresh-workspace rule: {combined}"
    );
}

#[test]
fn export_checkpoint_is_as_of_and_import_matches() {
    let env = Env::new("as_of");
    let workspace_id = env.prepare_source();

    // Checkpoint 1 is created by prepare_source; capture its id.
    let checkpoints = env.json_ok(&env.repo_a, &env.root_a, &["checkpoint", "list", "--json"]);
    let checkpoint_id = checkpoints["checkpoints"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Work after checkpoint 1.
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["goal", "set", "Ship the next release", "--json"],
    );
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["decision", "add", "Decision after checkpoint", "--json"],
    );
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["finding", "add", "Finding after checkpoint", "--json"],
    );
    std::fs::write(env.repo_a.join("after.md"), b"# after\n").unwrap();
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["artifact", "add", "after.md", "--json"],
    );
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["checkpoint", "create", "--json"],
    );

    // As-of export at checkpoint 1.
    let as_of_path = env.base.join("as_of.rootws.json");
    let export = env.json_ok(
        &env.repo_a,
        &env.root_a,
        &[
            "workspace",
            "export",
            "-o",
            as_of_path.to_str().unwrap(),
            "--checkpoint",
            &checkpoint_id,
            "--json",
        ],
    );
    assert_eq!(export["goals"], 1, "{export}");
    // prepare_source creates two decisions; only the post-checkpoint one is
    // excluded.
    assert_eq!(export["decisions"], 2, "{export}");
    assert_eq!(export["findings"], 1, "{export}");
    assert_eq!(export["artifacts"], 1, "{export}");
    assert_eq!(export["checkpoints"], 1, "{export}");

    let doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&as_of_path).unwrap()).unwrap();
    let doc = &doc;
    assert_eq!(doc["goals"][0]["status"], "active", "{doc}");
    assert_eq!(
        doc["goals"][0]["statement"], "Ship transfer acceptance",
        "{doc}"
    );
    assert_eq!(doc["decisions"].as_array().unwrap().len(), 2, "{doc}");
    assert!(
        doc.to_string().contains("Document is versioned")
            && !doc.to_string().contains("Decision after checkpoint"),
        "as-of document must not contain post-checkpoint decisions: {doc}"
    );
    assert!(
        doc.to_string().contains("Digests detect tampering")
            && !doc.to_string().contains("Finding after checkpoint"),
        "as-of document must not contain post-checkpoint findings: {doc}"
    );
    assert!(
        doc.to_string().contains("notes.md") && !doc.to_string().contains("after.md"),
        "as-of document must not contain post-checkpoint artifacts: {doc}"
    );
    assert!(
        doc.to_string().contains("Ship transfer acceptance")
            && !doc.to_string().contains("Ship the next release"),
        "as-of document must not contain the later goal: {doc}"
    );
    let second_checkpoint_id = doc["checkpoints"][0]["id"].as_str().unwrap();
    assert_eq!(second_checkpoint_id, checkpoint_id, "{doc}");

    // Full export without --checkpoint still has everything.
    let full_path = env.base.join("full.rootws.json");
    let full_export = env.json_ok(
        &env.repo_a,
        &env.root_a,
        &[
            "workspace",
            "export",
            "-o",
            full_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(full_export["goals"], 2, "{full_export}");
    assert_eq!(full_export["decisions"], 3, "{full_export}");
    assert_eq!(full_export["findings"], 2, "{full_export}");
    assert_eq!(full_export["artifacts"], 2, "{full_export}");
    assert_eq!(full_export["checkpoints"], 2, "{full_export}");

    // Import the as-of document into a fresh Root directory.
    let import = env.json_ok(
        &env.base,
        &env.root_b,
        &[
            "workspace",
            "import",
            as_of_path.to_str().unwrap(),
            "--project",
            env.repo_b.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(import["workspace_id"], workspace_id.as_str(), "{import}");
    assert_eq!(import["goals"], 1, "{import}");
    assert_eq!(import["decisions"], 2, "{import}");
    assert_eq!(import["findings"], 1, "{import}");
    assert_eq!(import["artifacts"], 1, "{import}");
    assert_eq!(import["checkpoints"], 1, "{import}");

    let status = env.json_ok(&env.repo_b, &env.root_b, &["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str(), "{status}");

    let goals = env.json_ok(&env.repo_b, &env.root_b, &["goal", "show", "--json"]);
    assert_eq!(
        goals["goal"]["statement"], "Ship transfer acceptance",
        "{goals}"
    );
    assert_eq!(goals["goal"]["status"], "active", "{goals}");

    let decisions = env.json_ok(&env.repo_b, &env.root_b, &["decision", "list", "--json"]);
    let statements: Vec<&str> = decisions["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["statement"].as_str())
        .collect();
    assert_eq!(statements.len(), 2, "{decisions}");
    assert!(statements.contains(&"Document is versioned"), "{decisions}");
    assert!(
        statements.contains(&"Import is all-or-nothing"),
        "{decisions}"
    );
    assert!(
        !decisions.to_string().contains("Decision after checkpoint"),
        "post-checkpoint decision must not be imported: {decisions}"
    );

    let imported_checkpoints =
        env.json_ok(&env.repo_b, &env.root_b, &["checkpoint", "list", "--json"]);
    assert_eq!(
        imported_checkpoints["checkpoints"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{imported_checkpoints}"
    );
    assert_eq!(
        imported_checkpoints["checkpoints"][0]["id"], checkpoint_id,
        "{imported_checkpoints}"
    );
}

#[test]
fn import_into_leftover_partial_database_retries_cleanly() {
    let env = Env::new("leftover_cli");
    env.prepare_source();
    let out = env.transfer_path();
    let export = env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["workspace", "export", "-o", out.to_str().unwrap(), "--json"],
    );
    let workspace_id = export["workspace_id"].as_str().unwrap().to_string();

    // Simulate a pre-fix failed import: an empty, fully-migrated database
    // sits at the final path but is absent from the workspace index.
    let db = env.db_path(&env.root_b, &workspace_id);
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    {
        // Fully migrated schema, no workspace rows: exactly what a rolled-back
        // pre-fix import left behind.
        drop(root_work::db::open(&db).unwrap());
    }

    // The import must replace the leftover and succeed without manual cleanup.
    let import = env.json_ok(
        &env.base,
        &env.root_b,
        &[
            "workspace",
            "import",
            out.to_str().unwrap(),
            "--project",
            env.repo_b.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(import["workspace_id"], workspace_id.as_str(), "{import}");
    assert_eq!(import["decisions"], 2, "{import}");
    let status = env.json_ok(&env.repo_b, &env.root_b, &["workspace", "status", "--json"]);
    assert_eq!(status["workspace"]["id"], workspace_id.as_str(), "{status}");
}

#[test]
fn export_is_read_only() {
    let env = Env::new("read_only");
    let workspace_id = env.prepare_source();
    let db = env.db_path(&env.root_a, &workspace_id);
    assert!(db.exists(), "work database must exist before export");

    let before_bytes = std::fs::read(&db).unwrap();
    let before_mtime = std::fs::metadata(&db).unwrap().modified().unwrap();

    let out = env.transfer_path();
    env.json_ok(
        &env.repo_a,
        &env.root_a,
        &["workspace", "export", "-o", out.to_str().unwrap(), "--json"],
    );

    assert_eq!(
        std::fs::read(&db).unwrap(),
        before_bytes,
        "export must not change the work database bytes"
    );
    assert_eq!(
        std::fs::metadata(&db).unwrap().modified().unwrap(),
        before_mtime,
        "export must not change the work database mtime"
    );
    let dir = db.parent().unwrap();
    assert!(
        !dir.join("state.db-wal").exists(),
        "export created a -wal file"
    );
    assert!(
        !dir.join("state.db-shm").exists(),
        "export created a -shm file"
    );
}
