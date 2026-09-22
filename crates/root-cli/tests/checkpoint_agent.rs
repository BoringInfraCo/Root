//! Sprint 013 group A: checkpoint captures the agent environment.
//!
//! Hermetic child-env isolation (isolated HOME/CODEX_HOME/ROOT_DIR/TMPDIR +
//! fake codex/opencode/claude shims on PATH). No process-env mutation; a
//! file-local mutex serializes tests in this binary.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn root_bin() -> &'static str {
    env!("CARGO_BIN_EXE_root")
}

struct Iso {
    base: PathBuf,
    home: PathBuf,
    codex_home: PathBuf,
    xdg: PathBuf,
    oc: PathBuf,
    cl: PathBuf,
    root_dir: PathBuf,
    tmpdir: PathBuf,
    bin: PathBuf,
}

impl Iso {
    fn new(name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "root-cli-checkpoint-agent-{name}-{}-{id}",
            std::process::id()
        ));
        let home = base.join("home");
        let codex_home = base.join("codex");
        let xdg = base.join("xdg");
        let oc = base.join("oc");
        let cl = base.join("cl");
        let root_dir = base.join("root");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for dir in [
            &base,
            &home,
            &codex_home,
            &xdg,
            &oc,
            &cl,
            &root_dir,
            &tmpdir,
            &bin,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let iso = Self {
            base,
            home,
            codex_home,
            xdg,
            oc,
            cl,
            root_dir,
            tmpdir,
            bin,
        };
        iso.write_shim("codex", "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n");
        iso.write_shim("opencode", "#!/bin/sh\nprintf '1.18.27\\n'\n");
        iso.write_shim("claude", "#!/bin/sh\nprintf '2.1.260\\n'\n");
        iso
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

    fn path_env(&self) -> std::ffi::OsString {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        std::env::join_paths(dirs).unwrap()
    }

    fn run_in(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new(root_bin())
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("XDG_DATA_HOME", &self.xdg)
            .env("OPENCODE_CONFIG_DIR", &self.oc)
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("CLAUDE_CONFIG_DIR", &self.cl)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", self.path_env())
            .output()
            .unwrap()
    }

    /// Create a git repo with a captured `.root/agent.toml` from Codex.
    fn repo_with_capture(&self, name: &str) -> Repo {
        setup_codex_portable(self);
        let repo = self.base.join(name);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        std::fs::write(repo.join("README.md"), b"# campfire\n").unwrap();
        let out = repo.join(".root/agent.toml");
        let res = self.run_in(
            &repo,
            &[
                "agent",
                "capture",
                "--from",
                "codex",
                "--apply",
                "--out",
                out.to_str().unwrap(),
            ],
        );
        assert_eq!(
            res.status.code(),
            Some(0),
            "capture --apply must succeed: stdout={} stderr={}",
            String::from_utf8_lossy(&res.stdout),
            String::from_utf8_lossy(&res.stderr)
        );
        assert!(out.exists(), "agent.toml must exist after capture --apply");
        Repo { path: repo }
    }

    /// Create a plain git repo with no agent environment.
    fn repo_without_capture(&self, name: &str) -> Repo {
        let repo = self.base.join(name);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("README.md"), b"# campfire\n").unwrap();
        Repo { path: repo }
    }
}

impl Drop for Iso {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Repo {
    path: PathBuf,
}

fn setup_codex_portable(iso: &Iso) {
    std::fs::write(
        iso.codex_home.join("config.toml"),
        "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\n",
    )
    .unwrap();
    std::fs::write(iso.codex_home.join("AGENTS.md"), "# agents\n").unwrap();
    let skill_dir = iso.home.join(".agents/skills/docs-writer");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# skill\n").unwrap();
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {}\nstdout={}\nstderr={}",
            e,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn json_ok(output: std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output)
}

fn agent_env_ref(report: &serde_json::Value) -> Option<&serde_json::Value> {
    let value = report["checkpoint"].get("agent_env_ref")?;
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

#[test]
fn checkpoint_captures_agent_environment() {
    let _guard = lock_tests();
    let iso = Iso::new("captures");
    let repo = iso.repo_with_capture("campfire");

    iso.run_in(&repo.path, &["workspace", "init", "--json"]);
    iso.run_in(
        &repo.path,
        &["goal", "set", "Ship harness-aware resume", "--json"],
    );
    let output = iso.run_in(&repo.path, &["checkpoint", "create", "--json"]);
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let report = json_ok(output);

    let raw_ref = agent_env_ref(&report)
        .expect("checkpoint should carry an agent_env_ref")
        .as_str()
        .expect("agent_env_ref is a JSON string")
        .to_string();
    let summary: serde_json::Value = serde_json::from_str(&raw_ref).unwrap();
    assert_eq!(summary["adapter"], "codex");
    assert_eq!(
        summary["credential_refs"],
        serde_json::json!(["GITHUB_TOKEN"])
    );
    assert_eq!(summary["skills"], serde_json::json!(["docs-writer"]));

    // Names only: no credential value ever appears in the checkpoint output.
    for needle in ["ghp_", "sk-", "github_pat_", "Bearer "] {
        assert!(
            !raw.contains(needle),
            "checkpoint output leaked {needle}: {raw}"
        );
    }

    // Real-pipeline checkpoints always bind agent_env to agent_env_sha256:
    // the stored digest is exactly `CanonicalEnv::env_hash` of the stored
    // value — the same computation `resume --with` uses to verify it.
    let snapshot: serde_json::Value =
        serde_json::from_str(report["checkpoint"]["snapshot"].as_str().unwrap()).unwrap();
    let stored_env = &snapshot["agent_env"];
    assert!(
        stored_env.is_object(),
        "snapshot must embed the canonical agent_env: {snapshot}"
    );
    let digest = snapshot["agent_env_sha256"]
        .as_str()
        .expect("snapshot must embed agent_env_sha256 alongside agent_env");
    assert_eq!(digest.len(), 64, "digest must be full sha256 hex: {digest}");
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "digest must be lowercase hex: {digest}"
    );
    let canonical: root_agent_bundle::canonical::CanonicalEnv =
        serde_json::from_value(stored_env.clone()).unwrap();
    assert_eq!(
        canonical.env_hash().unwrap(),
        digest,
        "write-path digest must equal the resume verification recomputation"
    );

    // And verification passes end to end for this checkpoint.
    let resume = json_ok(iso.run_in(&repo.path, &["resume", "--with", "codex", "--json"]));
    assert_eq!(
        resume["mapping_available"], true,
        "resume --with must verify and map the captured environment: {resume}"
    );
}

#[test]
fn checkpoint_immutable_after_work_mutation() {
    let _guard = lock_tests();
    let iso = Iso::new("immutable");
    let repo = iso.repo_with_capture("campfire");

    iso.run_in(&repo.path, &["workspace", "init", "--json"]);
    iso.run_in(&repo.path, &["goal", "set", "Goal", "--json"]);
    let created = json_ok(iso.run_in(
        &repo.path,
        &[
            "checkpoint",
            "create",
            "--message",
            "before more work",
            "--json",
        ],
    ));
    let id = created["checkpoint"]["id"].as_str().unwrap().to_string();

    let first = iso.run_in(&repo.path, &["checkpoint", "show", &id, "--json"]);
    let first_bytes = first.stdout.clone();
    let first_json = parse_json(&first);

    iso.run_in(
        &repo.path,
        &["decision", "add", "recorded after checkpoint", "--json"],
    );

    let second = iso.run_in(&repo.path, &["checkpoint", "show", &id, "--json"]);
    assert_eq!(
        second.stdout, first_bytes,
        "checkpoint show output must be byte-identical after work mutation"
    );
    let second_json = parse_json(&second);
    assert_eq!(second_json["checkpoint"], first_json["checkpoint"]);
    assert_eq!(
        second_json["checkpoint"]["snapshot"],
        first_json["checkpoint"]["snapshot"]
    );
    assert_eq!(
        second_json["checkpoint"]["rootfile_digest"],
        first_json["checkpoint"]["rootfile_digest"]
    );

    let last = json_ok(iso.run_in(&repo.path, &["checkpoint", "show", "--last", "--json"]));
    assert_eq!(last["checkpoint"]["id"], id.as_str());
}

#[test]
fn checkpoint_without_agent_env_has_none() {
    let _guard = lock_tests();
    let iso = Iso::new("none");
    let repo = iso.repo_without_capture("campfire");

    iso.run_in(&repo.path, &["workspace", "init", "--json"]);
    let report = json_ok(iso.run_in(&repo.path, &["checkpoint", "create", "--json"]));
    assert!(agent_env_ref(&report).is_none());
}

#[test]
fn malformed_agent_env_fails_checkpoint() {
    let _guard = lock_tests();
    let iso = Iso::new("malformed");
    let repo = iso.repo_without_capture("campfire");
    std::fs::create_dir_all(repo.path.join(".root")).unwrap();
    std::fs::write(repo.path.join(".root/agent.toml"), b"{ not valid toml").unwrap();

    iso.run_in(&repo.path, &["workspace", "init", "--json"]);
    // Sprint 013: an existing agent environment that fails capture must abort
    // the checkpoint atomically rather than being reported as absent.
    let output = iso.run_in(&repo.path, &["checkpoint", "create", "--json"]);
    assert!(
        !output.status.success(),
        "malformed agent env must fail checkpoint creation"
    );
    let list = json_ok(iso.run_in(&repo.path, &["checkpoint", "list", "--json"]));
    assert_eq!(
        list["checkpoints"].as_array().unwrap().len(),
        0,
        "no checkpoint row may be committed"
    );
}

#[test]
fn checkpoint_refuses_secret_shaped_agent_env() {
    // Optional coverage: the refusal itself lives in `agent_env::capture` /
    // canonical `validate` unit tests. Through the CLI a refused capture must
    // never block checkpoint creation nor leak the value.
    let _guard = lock_tests();
    let iso = Iso::new("secret");
    let repo = iso.repo_without_capture("campfire");
    std::fs::create_dir_all(repo.path.join(".root")).unwrap();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    let toml =
        format!("[environment]\nneeds_env = []\n\n[provenance]\ncaptured_at = \"{secret}\"\n");
    std::fs::write(repo.path.join(".root/agent.toml"), toml).unwrap();

    iso.run_in(&repo.path, &["workspace", "init", "--json"]);
    let output = iso.run_in(&repo.path, &["checkpoint", "create", "--json"]);
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let raw_err = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "secret-shaped agent env must fail checkpoint creation"
    );
    assert!(
        !raw.contains(secret) && !raw_err.contains(secret),
        "checkpoint leaked a secret value"
    );
    let list = json_ok(iso.run_in(&repo.path, &["checkpoint", "list", "--json"]));
    assert_eq!(
        list["checkpoints"].as_array().unwrap().len(),
        0,
        "no checkpoint row may be committed"
    );
}
