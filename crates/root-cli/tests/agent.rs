//! Sprint 011 part 3: `root agent` CLI group integration tests (read-only).
//!
//! Hermetic: isolated HOME/CODEX_HOME/XDG_CONFIG_HOME/OPENCODE_CONFIG_DIR/
//! CLAUDE_CONFIG_DIR/ROOT_DIR/TMPDIR + fake codex/opencode/claude shims on
//! PATH returning canned `--version`. Child-env isolation only (no process
//! env mutation); a file-local mutex serializes tests in this file.

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
            "root-agent-cli-011-{name}-{}-{}",
            std::process::id(),
            id
        ));
        let home = base.join("home");
        let codex_home = base.join("codex");
        let xdg = base.join("xdg");
        let oc = base.join("oc");
        let cl = base.join("cl");
        let root_dir = base.join("root");
        let tmpdir = base.join("tmp");
        let bin = base.join("bin");
        for d in [
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
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(d)
                    .unwrap();
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(d).unwrap();
            }
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
            .current_dir(&self.base)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("XDG_DATA_HOME", &self.xdg)
            .env("OPENCODE_CONFIG_DIR", &self.oc)
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("CLAUDE_CONFIG_DIR", &self.cl)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path)
            .output()
            .unwrap()
    }

    fn run_json(&self, args: &[&str]) -> (std::process::Output, serde_json::Value) {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "command {:?} failed: status={:?} stderr={} stdout={}",
            args,
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON for {:?}: {}\nstdout={}",
                args,
                e,
                String::from_utf8_lossy(&out.stdout)
            )
        });
        (out, v)
    }
}

impl Drop for Iso {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
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

fn setup_codex_minimal(iso: &Iso) {
    std::fs::write(iso.codex_home.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
    std::fs::write(iso.codex_home.join("AGENTS.md"), "# agents\n").unwrap();
}

fn root_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn inspect_json_exact_gates() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-gates");
    for (agent, version) in [
        ("codex", "0.150.1"),
        ("opencode", "1.18.27"),
        ("claude", "2.1.260"),
    ] {
        let (_, v) = iso.run_json(&["agent", "inspect", agent, "--json"]);
        assert_eq!(v["agent"], agent, "agent={agent} json={v}");
        assert_eq!(v["present"], true, "agent={agent} json={v}");
        assert_eq!(v["version"], version, "agent={agent} json={v}");
        assert_eq!(v["version_supported"], true, "agent={agent} json={v}");
        assert_eq!(v["mutated"], false, "agent={agent} json={v}");
        assert!(v.get("canonical").is_some(), "agent={agent} json={v}");
        assert_eq!(v["canonical"]["source_agent"], agent);
    }
}

#[test]
fn inspect_json_preserves_codex_report() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-report-codex");
    setup_codex_portable(&iso);
    let (_, v) = iso.run_json(&["agent", "inspect", "codex", "--json"]);
    assert_eq!(v["version_supported"], true, "json={v}");
    assert_eq!(v["mutated"], false, "json={v}");
    assert_eq!(v["canonical"]["source_agent"], "codex", "json={v}");
    let r = &v["report"];
    assert!(r.is_object(), "report must be an object: {v}");
    assert_eq!(r["agent"], "codex", "json={v}");
    assert_eq!(r["present"], true, "json={v}");
    assert_eq!(r["version"], "0.150.1", "json={v}");
    assert_eq!(r["version_supported"], true, "json={v}");
    let home = r["codex_home"].as_str().unwrap_or("");
    assert!(!home.is_empty(), "codex_home must be non-empty: {v}");
    assert!(r["config_present"].is_boolean(), "json={v}");
    assert!(r["agents_md_present"].is_boolean(), "json={v}");
    assert_eq!(r["config_present"], true, "json={v}");
    assert_eq!(r["agents_md_present"], true, "json={v}");
    let skills = r["skills"].as_array().expect("skills must be an array");
    assert!(
        skills.iter().all(|s| s.is_string()),
        "skills must be name strings: {v}"
    );
    let mcp = r["mcp_servers"]
        .as_array()
        .expect("mcp_servers must be an array");
    assert!(
        mcp.iter().any(|s| s == "github"),
        "mcp_servers must list github: {v}"
    );
    let held = r["held"].as_array().expect("held must be an array");
    assert!(!held.is_empty(), "held must be non-empty: {v}");

    let out = iso.run(&["agent", "inspect", "codex"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Config path:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter MCP servers:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter held"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn inspect_json_preserves_opencode_report() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-report-opencode");
    let (_, v) = iso.run_json(&["agent", "inspect", "opencode", "--json"]);
    assert_eq!(v["version_supported"], true, "json={v}");
    assert_eq!(v["mutated"], false, "json={v}");
    assert_eq!(v["canonical"]["source_agent"], "opencode", "json={v}");
    let r = &v["report"];
    assert!(r.is_object(), "report must be an object: {v}");
    assert_eq!(r["agent"], "opencode", "json={v}");
    assert_eq!(r["present"], true, "json={v}");
    assert_eq!(r["version"], "1.18.27", "json={v}");
    assert_eq!(r["version_supported"], true, "json={v}");
    let dir = r["config_dir"].as_str().unwrap_or("");
    assert!(!dir.is_empty(), "config_dir must be non-empty: {v}");
    assert!(r["config_present"].is_boolean(), "json={v}");
    assert!(r["agents_md_present"].is_boolean(), "json={v}");
    assert!(r["skills"].is_array(), "skills must be an array: {v}");
    assert!(
        r["mcp_servers"].is_array(),
        "mcp_servers must be an array: {v}"
    );
    let held = r["held"].as_array().expect("held must be an array");
    assert!(!held.is_empty(), "held must be non-empty: {v}");

    let out = iso.run(&["agent", "inspect", "opencode"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Config path:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter MCP servers:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter held"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn inspect_json_preserves_claude_report() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-report-claude");
    let (_, v) = iso.run_json(&["agent", "inspect", "claude", "--json"]);
    assert_eq!(v["version_supported"], true, "json={v}");
    assert_eq!(v["mutated"], false, "json={v}");
    assert_eq!(v["canonical"]["source_agent"], "claude", "json={v}");
    let r = &v["report"];
    assert!(r.is_object(), "report must be an object: {v}");
    assert_eq!(r["agent"], "claude", "json={v}");
    assert_eq!(r["present"], true, "json={v}");
    assert_eq!(r["version"], "2.1.260", "json={v}");
    assert_eq!(r["version_supported"], true, "json={v}");
    let dir = r["config_dir"].as_str().unwrap_or("");
    assert!(!dir.is_empty(), "config_dir must be non-empty: {v}");
    let state = r["global_state_dir"].as_str().unwrap_or("");
    assert!(!state.is_empty(), "global_state_dir must be non-empty: {v}");
    assert!(r["settings_present"].is_boolean(), "json={v}");
    assert!(r["claude_md_present"].is_boolean(), "json={v}");
    assert!(r["skills"].is_array(), "skills must be an array: {v}");
    assert!(
        r["mcp_servers"].is_array(),
        "mcp_servers must be an array: {v}"
    );
    let held = r["held"].as_array().expect("held must be an array");
    assert!(!held.is_empty(), "held must be non-empty: {v}");

    let out = iso.run(&["agent", "inspect", "claude"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Config path:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter MCP servers:"), "stdout={stdout}");
    assert!(stdout.contains("Adapter held"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn inspect_human_ends_no_changes() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-human");
    let out = iso.run(&["agent", "inspect", "codex"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("present"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn inspect_unknown_agent_exits_2() {
    let _guard = lock_tests();
    let iso = Iso::new("inspect-unknown");
    let out = iso.run(&["agent", "inspect", "gemini"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported bundle adapter"),
        "stderr={stderr}"
    );
    let out_json = iso.run(&["agent", "inspect", "gemini", "--json"]);
    assert_eq!(out_json.status.code(), Some(2));
}

#[test]
fn plan_codex_to_opencode_portable() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-portable");
    setup_codex_portable(&iso);

    let (_, v) = iso.run_json(&[
        "agent", "plan", "--from", "codex", "--to", "opencode", "--json",
    ]);
    assert_eq!(v["from"], "codex");
    assert_eq!(v["to"], "opencode");
    assert_eq!(v["source_supported"], true, "json={v}");
    assert_eq!(v["target_supported"], true, "json={v}");
    assert_eq!(v["mutated"], false);
    let portable = v["portable"].as_array().unwrap();
    assert!(
        portable
            .iter()
            .any(|s| s.as_str().unwrap().contains("AGENTS.md")),
        "portable={portable:?}"
    );
    assert!(
        portable.iter().any(|s| s == "tools:model"),
        "portable={portable:?}"
    );
    assert!(
        portable.iter().any(|s| s == "skill:docs-writer"),
        "portable={portable:?}"
    );
    let review = v["requires_review"].as_array().unwrap();
    assert!(
        review.iter().any(|r| r["item"] == "mcp:github"),
        "review={review:?}"
    );
    assert!(v["unsupported"].as_array().unwrap().is_empty(), "json={v}");
    let secrets = v["secrets_required"].as_array().unwrap();
    assert!(
        secrets.iter().any(|s| s == "GITHUB_TOKEN"),
        "secrets={secrets:?}"
    );
    let approvals = v["needs_approval"].as_array().unwrap();
    assert_eq!(approvals.len(), 1);
    assert!(
        approvals[0]["target"]
            .as_str()
            .unwrap()
            .contains("opencode_home"),
        "approvals={approvals:?}"
    );
    assert!(
        approvals[0]["target"]
            .as_str()
            .unwrap()
            .contains("mcp.github"),
        "approvals={approvals:?}"
    );

    let out = iso.run(&["agent", "plan", "--from", "codex", "--to", "opencode"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for heading in [
        "Portable",
        "Requires review",
        "Unsupported",
        "Secrets required",
        "Needs approval",
        "Held",
    ] {
        assert!(stdout.contains(heading), "missing {heading}: {stdout}");
    }
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn plan_same_agent_round_trips() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-roundtrip");
    setup_codex_minimal(&iso);
    let (_, v) = iso.run_json(&[
        "agent", "plan", "--from", "codex", "--to", "codex", "--json",
    ]);
    assert!(
        v["requires_review"].as_array().unwrap().is_empty(),
        "json={v}"
    );
    assert!(v["unsupported"].as_array().unwrap().is_empty(), "json={v}");
    assert_eq!(v["mutated"], false);
}

#[test]
fn plan_to_claude_surfaces_held() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-claude");
    setup_codex_portable(&iso);
    let (out, v) = (
        iso.run(&[
            "agent", "plan", "--from", "codex", "--to", "claude", "--json",
        ]),
        None::<serde_json::Value>,
    );
    let _ = v;
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let unsupported = v["unsupported"].as_array().unwrap();
    assert!(
        unsupported.iter().any(|r| r["item"] == "mcp:github"),
        "unsupported={unsupported:?}"
    );
    let item = unsupported
        .iter()
        .find(|r| r["item"] == "mcp:github")
        .unwrap();
    assert_eq!(
        item["reason"],
        "unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held."
    );
    assert!(
        v["needs_approval"].as_array().unwrap().is_empty(),
        "json={v}"
    );
    assert_eq!(v["mutated"], false);
    let raw = String::from_utf8_lossy(&out.stdout);
    assert!(
        !raw.contains(".claude.json"),
        "output must not contain .claude.json target: {raw}"
    );
}

#[test]
fn diff_symmetric_categories() {
    let _guard = lock_tests();
    let iso = Iso::new("diff-sym");
    setup_codex_portable(&iso);

    let out_ab = iso.run(&["agent", "diff", "codex", "opencode", "--json"]);
    assert_eq!(
        out_ab.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out_ab.stderr)
    );
    let ab: serde_json::Value = serde_json::from_slice(&out_ab.stdout).unwrap();
    let out_ba = iso.run(&["agent", "diff", "opencode", "codex", "--json"]);
    assert_eq!(
        out_ba.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out_ba.stderr)
    );
    let ba: serde_json::Value = serde_json::from_slice(&out_ba.stdout).unwrap();

    assert_eq!(ab["mutated"], false);
    assert_eq!(ba["mutated"], false);
    assert_eq!(ab["secrets_required"], ba["secrets_required"]);
    assert_eq!(
        ab["portable"].as_array().unwrap().len(),
        ba["portable"].as_array().unwrap().len()
    );
    assert_eq!(
        ab["requires_review"].as_array().unwrap().len(),
        ba["requires_review"].as_array().unwrap().len()
    );
    assert_eq!(
        ab["unsupported"].as_array().unwrap().len(),
        ba["unsupported"].as_array().unwrap().len()
    );
    let raw_ab = String::from_utf8_lossy(&out_ab.stdout);
    assert!(
        !raw_ab.contains("plan_hash"),
        "diff must not contain plan_hash"
    );

    let human = iso.run(&["agent", "diff", "codex", "opencode"]);
    assert_eq!(
        human.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        !stdout.contains("Plan hash"),
        "diff human must not contain plan hash"
    );
    assert!(stdout.contains("Portable"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn plan_env_malformed_exits_2() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-badenv");
    setup_codex_minimal(&iso);
    let bad = iso.tmpdir.join("bad.json");
    std::fs::write(&bad, "{bad json").unwrap();
    let out = iso.run(&[
        "agent",
        "plan",
        "--from",
        "codex",
        "--to",
        "opencode",
        "--env",
        bad.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn plan_and_diff_write_nothing() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-nowrite");
    setup_codex_portable(&iso);
    assert!(root_files(&iso.root_dir).is_empty());

    let plan = iso.run(&["agent", "plan", "--from", "codex", "--to", "opencode"]);
    assert_eq!(plan.status.code(), Some(0));
    let plan_json = iso.run(&[
        "agent", "plan", "--from", "codex", "--to", "opencode", "--json",
    ]);
    assert_eq!(plan_json.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&plan_json.stdout).unwrap();
    assert_eq!(v["mutated"], false);

    let diff = iso.run(&["agent", "diff", "codex", "opencode"]);
    assert_eq!(diff.status.code(), Some(0));
    let diff_json = iso.run(&["agent", "diff", "codex", "opencode", "--json"]);
    assert_eq!(diff_json.status.code(), Some(0));
    let dv: serde_json::Value = serde_json::from_slice(&diff_json.stdout).unwrap();
    assert_eq!(dv["mutated"], false);

    let after = root_files(&iso.root_dir);
    assert!(
        after.is_empty(),
        "plan/diff must create no files under ROOT_DIR, found: {after:?}"
    );
    let human = String::from_utf8_lossy(&plan.stdout);
    assert!(
        human.trim_end().ends_with("No changes made."),
        "human={human}"
    );
}

#[test]
fn plan_warns_on_unsupported_target_version() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-target-warn");
    setup_codex_minimal(&iso);
    iso.write_shim("opencode", "#!/bin/sh\nprintf '0.0.0\\n'\n");

    let (_, v) = iso.run_json(&[
        "agent", "plan", "--from", "codex", "--to", "opencode", "--json",
    ]);
    assert_eq!(v["target_version"], "0.0.0", "json={v}");
    assert_eq!(v["target_supported"], false, "json={v}");
    let warnings = v["warnings"].as_array().expect("warnings must be an array");
    assert!(!warnings.is_empty(), "unsupported target must warn: {v}");
    let warn = warnings
        .iter()
        .find(|w| w.as_str().unwrap_or("").contains("target"))
        .expect("target warning must be present");
    let warn = warn.as_str().unwrap();
    assert!(warn.contains("0.0.0"), "live version must be named: {warn}");
    assert!(
        warn.contains("1.18.27"),
        "supported version must be named: {warn}"
    );
    assert!(
        warn.contains("supported"),
        "warning must name supported versions: {warn}"
    );

    let out = iso.run(&["agent", "plan", "--from", "codex", "--to", "opencode"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Warnings"), "stdout={stdout}");
    assert!(stdout.contains("0.0.0"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );
}

#[test]
fn diff_reports_versions_and_warnings() {
    let _guard = lock_tests();
    let iso = Iso::new("diff-versions");
    setup_codex_minimal(&iso);

    let (_, v) = iso.run_json(&["agent", "diff", "codex", "opencode", "--json"]);
    assert_eq!(v["a_version"], "0.150.1", "json={v}");
    assert_eq!(v["b_version"], "1.18.27", "json={v}");
    assert_eq!(v["a_supported"], true, "json={v}");
    assert_eq!(v["b_supported"], true, "json={v}");
    assert!(v["warnings"].is_array(), "warnings must be an array: {v}");

    let human = iso.run(&["agent", "diff", "codex", "opencode"]);
    assert_eq!(
        human.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        !stdout.contains("Warnings"),
        "supported diff must omit Warnings section: {stdout}"
    );
    assert!(
        stdout.trim_end().ends_with("No changes made."),
        "stdout={stdout}"
    );

    iso.write_shim("opencode", "#!/bin/sh\nprintf '0.0.0\\n'\n");
    let (_, w) = iso.run_json(&["agent", "diff", "codex", "opencode", "--json"]);
    assert_eq!(w["a_version"], "0.150.1", "json={w}");
    assert_eq!(w["b_version"], "0.0.0", "json={w}");
    assert_eq!(w["a_supported"], true, "json={w}");
    assert_eq!(w["b_supported"], false, "json={w}");
    let warnings = w["warnings"].as_array().expect("warnings must be an array");
    assert!(!warnings.is_empty(), "unsupported b must warn: {w}");
    let joined = warnings
        .iter()
        .filter_map(|x| x.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("0.0.0"),
        "live version must be named: {joined}"
    );
    assert!(
        joined.contains("1.18.27"),
        "supported version must be named: {joined}"
    );

    let human_warn = iso.run(&["agent", "diff", "codex", "opencode"]);
    assert_eq!(human_warn.status.code(), Some(0));
    let stdout_warn = String::from_utf8_lossy(&human_warn.stdout);
    assert!(
        stdout_warn.contains("Warnings"),
        "unsupported diff must show Warnings: {stdout_warn}"
    );
    assert!(
        stdout_warn.trim_end().ends_with("No changes made."),
        "stdout={stdout_warn}"
    );
}

#[cfg(unix)]
#[test]
fn plan_aborts_on_symlink_target() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-symlink-abort");
    setup_codex_minimal(&iso);
    let outside = iso.base.join("outside.txt");
    std::fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, iso.oc.join("AGENTS.md")).unwrap();
    let out = iso.run(&["agent", "plan", "--from", "codex", "--to", "opencode"]);
    assert!(
        out.status.code() != Some(0),
        "symlink target must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("symlink"),
        "stderr must mention symlink: {stderr}"
    );
}

#[test]
fn plan_needs_approval_sha256_lowercase_hex() {
    let _guard = lock_tests();
    let iso = Iso::new("plan-approval-hex");
    setup_codex_portable(&iso);
    let (_, v) = iso.run_json(&[
        "agent", "plan", "--from", "codex", "--to", "opencode", "--json",
    ]);
    let approvals = v["needs_approval"]
        .as_array()
        .expect("needs_approval must be an array");
    assert!(
        !approvals.is_empty(),
        "portable MCP setup must require approval: {v}"
    );
    for a in approvals {
        let sha = a["sha256"].as_str().expect("sha256 must be a string");
        assert_eq!(sha.len(), 64, "sha256 must be 64 hex chars: {sha}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "sha256 must be hex: {sha}"
        );
        assert_eq!(
            sha,
            sha.to_lowercase(),
            "sha256 must be lowercase-hex canonical form: {sha}"
        );
    }
}

// ---------------------------------------------------------------------------
// Sprint 012: `root agent apply / verify / capture / rollback / purge`.
// Hermetic child-env only (reuses Iso above); no process-env mutation.
// ---------------------------------------------------------------------------

impl Iso {
    fn run_in(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(&dirs).unwrap();
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
            .env("PATH", &path)
            .output()
            .unwrap()
    }

    fn preflight_json(&self, to: &str, env_path: &Path) -> serde_json::Value {
        let out = self.run(&[
            "agent",
            "apply",
            "--to",
            to,
            "--env",
            env_path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "preflight must exit 2: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid preflight JSON: {}\nstdout={}",
                e,
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    fn opencode_agents_md(&self) -> PathBuf {
        self.oc.join("AGENTS.md")
    }

    /// Same isolated child env as [`Iso::run`] but with CODEX_HOME overridden
    /// (used to simulate an empty/absent source machine). No process-env
    /// mutation.
    fn run_with_codex_home(&self, codex_home: &Path, args: &[&str]) -> std::process::Output {
        let mut dirs = vec![self.bin.clone()];
        if let Some(old) = std::env::var_os("PATH") {
            if !old.is_empty() {
                dirs.extend(std::env::split_paths(&old));
            }
        }
        let path = std::env::join_paths(&dirs).unwrap();
        Command::new(root_bin())
            .args(args)
            .current_dir(&self.base)
            .env("HOME", &self.home)
            .env("CODEX_HOME", codex_home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("XDG_DATA_HOME", &self.xdg)
            .env("OPENCODE_CONFIG_DIR", &self.oc)
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
            .env("CLAUDE_CONFIG_DIR", &self.cl)
            .env("ROOT_DIR", &self.root_dir)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", &path)
            .output()
            .unwrap()
    }
}

fn write_rootfile_with_agents(repo: &Path) {
    let content = "[agents]\nenv = \".root/agent.toml\"\ndefault_target = \"opencode\"\n";
    std::fs::write(repo.join("Rootfile"), content).unwrap();
}

fn setup_repo_with_capture(iso: &Iso, name: &str, portable: bool) -> (PathBuf, PathBuf) {
    if portable {
        setup_codex_portable(iso);
    } else {
        setup_codex_minimal(iso);
    }
    let repo = iso.base.join(name);
    let dotroot = repo.join(".root");
    std::fs::create_dir_all(&dotroot).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let out = dotroot.join("agent.toml");
    let res = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        res.status.code(),
        Some(0),
        "capture --apply must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&res.stdout),
        String::from_utf8_lossy(&res.stderr)
    );
    assert!(out.exists(), "agent.toml must exist after capture --apply");
    write_rootfile_with_agents(&repo);
    assert!(repo.join("Rootfile").exists());
    (repo, out)
}

#[test]
fn documented_plan_hash_is_applyable() {
    let _guard = lock_tests();
    let iso = Iso::new("documented-plan-apply");
    let (repo, env_path) = setup_repo_with_capture(&iso, "repo", true);

    // plan with only --env (source derived from env.source_agent) and explicit
    // --to. Extract the documented plan hash + approvals.
    let (_, pv) = iso.run_json(&[
        "agent",
        "plan",
        "--env",
        env_path.to_str().unwrap(),
        "--to",
        "opencode",
        "--json",
    ]);
    assert_eq!(pv["from"], "codex", "json={pv}");
    assert_eq!(pv["to"], "opencode", "json={pv}");
    let plan_hash = pv["plan_hash"]
        .as_str()
        .expect("plan_hash must be a string")
        .to_string();
    assert!(!plan_hash.is_empty());
    let approvals: Vec<String> = pv["needs_approval"]
        .as_array()
        .expect("needs_approval must be an array")
        .iter()
        .map(|a| a["sha256"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !approvals.is_empty(),
        "portable MCP fixture must need approval: {pv}"
    );

    // Apply the DOCUMENTED plan hash directly (no re-preflight), with the
    // documented approvals, and no --to (repo Rootfile default_target).
    let mut args: Vec<String> = vec![
        "agent".into(),
        "apply".into(),
        "--env".into(),
        ".root/agent.toml".into(),
        "--apply".into(),
        "--plan-hash".into(),
        plan_hash,
    ];
    for a in &approvals {
        args.push("--approve".into());
        args.push(a.clone());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let applied = iso.run_in(&repo, &refs);
    assert_eq!(
        applied.status.code(),
        Some(0),
        "documented plan hash must apply: stdout={} stderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let stdout = String::from_utf8_lossy(&applied.stdout);
    assert!(stdout.contains("Applied"), "stdout={stdout}");
    assert!(
        !stdout.trim_end().ends_with("No changes made."),
        "mutation must not end with No changes made: {stdout}"
    );
    assert!(iso.opencode_agents_md().exists(), "target must be created");

    let verify = iso.run(&["agent", "verify", "--agent", "opencode"]);
    assert_eq!(
        verify.status.code(),
        Some(0),
        "verify must pass: stdout={} stderr={}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
}

#[test]
fn apply_refuses_empty_destination_machine() {
    let _guard = lock_tests();
    let iso = Iso::new("empty-source-refusal");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", true);

    // Point CODEX_HOME at a genuinely empty dir: declared source content is
    // absent on this machine.
    let empty_codex = iso.base.join("empty-codex");
    std::fs::create_dir_all(&empty_codex).unwrap();

    // Preflight from the empty-source state: plan exits 2, reports held
    // "absent on source" entries and a warning.
    let pre = iso.run_with_codex_home(
        &empty_codex,
        &[
            "agent",
            "apply",
            "--to",
            "opencode",
            "--env",
            env_path.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(
        pre.status.code(),
        Some(2),
        "empty-source preflight must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&pre.stdout),
        String::from_utf8_lossy(&pre.stderr)
    );
    let pv: serde_json::Value = serde_json::from_slice(&pre.stdout).unwrap();
    let plan_hash = pv["plan_hash"]
        .as_str()
        .expect("plan_hash must be a string")
        .to_string();
    let approvals: Vec<String> = pv["needs_approval"]
        .as_array()
        .expect("needs_approval must be an array")
        .iter()
        .map(|a| a["sha256"].as_str().unwrap().to_string())
        .collect();
    let held_absent = pv["held"]
        .as_array()
        .expect("held must be an array")
        .iter()
        .any(|h| {
            h["reason"]
                .as_str()
                .unwrap_or("")
                .contains("absent on source")
        });
    assert!(held_absent, "empty source must report held absent: {pv}");
    let warned = pv["warnings"]
        .as_array()
        .expect("warnings must be an array")
        .iter()
        .any(|w| w.as_str().unwrap_or("").contains("absent on this machine"));
    assert!(warned, "empty source must warn: {pv}");

    // Human preflight surfaces an explicit same-machine note.
    let human = iso.run_with_codex_home(
        &empty_codex,
        &[
            "agent",
            "apply",
            "--to",
            "opencode",
            "--env",
            env_path.to_str().unwrap(),
        ],
    );
    assert_eq!(human.status.code(), Some(2));
    let hout = String::from_utf8_lossy(&human.stdout);
    assert!(
        hout.contains("Note: source content is not available on this machine"),
        "human preflight must carry the same-machine note: {hout}"
    );
    assert!(
        hout.contains("Sprint 013"),
        "human note must name Sprint 013: {hout}"
    );

    // Applying the preflight hash must refuse: same-machine only.
    let mut args: Vec<String> = vec![
        "agent".into(),
        "apply".into(),
        "--to".into(),
        "opencode".into(),
        "--env".into(),
        env_path.to_str().unwrap().into(),
        "--apply".into(),
        "--plan-hash".into(),
        plan_hash,
    ];
    for a in &approvals {
        args.push("--approve".into());
        args.push(a.clone());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let applied = iso.run_with_codex_home(&empty_codex, &refs);
    assert_ne!(
        applied.status.code(),
        Some(0),
        "empty-source apply must refuse: stdout={} stderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(
        combined.contains("not available on this machine"),
        "refusal must name absent source content: {combined}"
    );
    assert!(
        combined.contains("Sprint 013"),
        "refusal must name Sprint 013: {combined}"
    );

    // No target artifacts written under the destination opencode dirs.
    assert!(!iso.opencode_agents_md().exists());
    assert!(!iso.oc.join("opencode.json").exists());
    assert!(!iso.oc.join("opencode.jsonc").exists());
}

#[test]
fn rootfile_stanza_env_path_and_default_target() {
    let _guard = lock_tests();
    let iso = Iso::new("rootfile-stanza");
    setup_codex_portable(&iso);
    let repo = iso.base.join("repo");
    let config = repo.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let env_out = config.join("agents.toml");
    let cap = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--allow-outside-repo",
        "--out",
        env_out.to_str().unwrap(),
    ]);
    assert_eq!(
        cap.status.code(),
        Some(0),
        "capture into config/ must succeed: stderr={}",
        String::from_utf8_lossy(&cap.stderr)
    );
    std::fs::write(
        repo.join("Rootfile"),
        "[agents]\nenv = \"config/agents.toml\"\ndefault_target = \"opencode\"\n",
    )
    .unwrap();

    // --env and --to omitted: stanza env + default target resolve; reaches the
    // plan-only preflight naming opencode (not an error).
    let out = iso.run_in(&repo, &["agent", "apply", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stanza preflight must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["to"], "opencode", "json={v}");
    assert!(v["plan_hash"].is_string(), "json={v}");

    // Escape variant: stanza env outside the repo is refused.
    std::fs::write(
        repo.join("Rootfile"),
        "[agents]\nenv = \"../x.toml\"\ndefault_target = \"opencode\"\n",
    )
    .unwrap();
    let out2 = iso.run_in(&repo, &["agent", "apply", "--json"]);
    assert_ne!(
        out2.status.code(),
        Some(0),
        "escaping stanza env must be refused: stdout={} stderr={}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        combined.contains("[agents].env") || combined.contains("escapes repo root"),
        "refusal must name the stanza env: {combined}"
    );
}

#[test]
fn explicit_env_beats_repo_stanza() {
    let _guard = lock_tests();
    let iso = Iso::new("explicit-env-wins");
    setup_codex_portable(&iso);
    let repo = iso.base.join("repo");
    let config = repo.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let good = config.join("good.toml");
    let cap = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--allow-outside-repo",
        "--out",
        good.to_str().unwrap(),
    ]);
    assert_eq!(
        cap.status.code(),
        Some(0),
        "capture: stderr={}",
        String::from_utf8_lossy(&cap.stderr)
    );
    // Repo stanza points at a malformed env file.
    std::fs::write(config.join("broken.toml"), "{ not valid toml").unwrap();
    std::fs::write(
        repo.join("Rootfile"),
        "[agents]\nenv = \"config/broken.toml\"\ndefault_target = \"opencode\"\n",
    )
    .unwrap();

    // Without --env: repo stanza (broken) is refused.
    let bad = iso.run_in(&repo, &["agent", "apply", "--json"]);
    assert_ne!(bad.status.code(), Some(0), "broken stanza must be refused");
    let bv: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert!(bv["plan_hash"].is_null(), "json={bv}");

    // Explicit --env wins over the broken repo stanza.
    let out = iso.run_in(
        &repo,
        &["agent", "apply", "--env", good.to_str().unwrap(), "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "explicit env must preflight exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["plan_hash"].is_string(), "explicit env must win: {v}");
}

#[test]
fn explicit_to_beats_default_target() {
    let _guard = lock_tests();
    let iso = Iso::new("explicit-to-wins");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", false);
    // Rootfile default_target is opencode; --to codex must win.
    let out = iso.run(&[
        "agent",
        "apply",
        "--to",
        "codex",
        "--env",
        env_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "explicit --to preflight must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["to"], "codex", "json={v}");
}

#[test]
fn missing_target_and_default_exits_2() {
    let _guard = lock_tests();
    let iso = Iso::new("missing-target");
    let (repo, _env_path) = setup_repo_with_capture(&iso, "repo", false);
    // Remove default_target from the stanza.
    std::fs::write(
        repo.join("Rootfile"),
        "[agents]\nenv = \".root/agent.toml\"\n",
    )
    .unwrap();

    let out = iso.run_in(
        &repo,
        &["agent", "apply", "--env", ".root/agent.toml", "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "missing --to/default_target must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["success"], false, "json={v}");
    let msg = v["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("[agents].default_target") && msg.contains("--to"),
        "message must name supported targets + how to set default: {msg}"
    );

    // Same for `plan`.
    let p = iso.run_in(&repo, &["agent", "plan", "--from", "codex", "--json"]);
    assert_eq!(
        p.status.code(),
        Some(2),
        "plan missing --to/default_target must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&p.stdout),
        String::from_utf8_lossy(&p.stderr)
    );
}

#[test]
fn codex_mcp_unknown_field_preserved_end_to_end() {
    let _guard = lock_tests();
    let iso = Iso::new("mcp-unknown-field");
    let config_path = iso.codex_home.join("config.toml");
    std::fs::write(iso.codex_home.join("AGENTS.md"), "# agents\n").unwrap();
    // Exportable descriptor (no unknown fields) so capture can emit it.
    std::fs::write(
        &config_path,
        "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\n",
    )
    .unwrap();
    let repo = iso.base.join("repo");
    let dotroot = repo.join(".root");
    std::fs::create_dir_all(&dotroot).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let out = dotroot.join("agent.toml");
    let cap = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        cap.status.code(),
        Some(0),
        "capture must succeed: stderr={}",
        String::from_utf8_lossy(&cap.stderr)
    );

    // Add an unknown field to the live target config after capture. plan and
    // apply both observe this state, so no drift; apply must patch the known
    // MCP keys in place and keep `timeout`.
    std::fs::write(
        &config_path,
        "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\ntimeout = 30\n",
    )
    .unwrap();

    let (_, pv) = iso.run_json(&[
        "agent",
        "plan",
        "--env",
        out.to_str().unwrap(),
        "--to",
        "codex",
        "--json",
    ]);
    let plan_hash = pv["plan_hash"]
        .as_str()
        .expect("plan_hash must be a string")
        .to_string();
    let approvals: Vec<String> = pv["needs_approval"]
        .as_array()
        .expect("needs_approval must be an array")
        .iter()
        .map(|a| a["sha256"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !approvals.is_empty(),
        "MCP fixture must need approval: {pv}"
    );

    let mut args: Vec<String> = vec![
        "agent".into(),
        "apply".into(),
        "--env".into(),
        out.to_str().unwrap().into(),
        "--to".into(),
        "codex".into(),
        "--apply".into(),
        "--plan-hash".into(),
        plan_hash,
    ];
    for a in &approvals {
        args.push("--approve".into());
        args.push(a.clone());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let applied = iso.run_in(&repo, &refs);
    assert_eq!(
        applied.status.code(),
        Some(0),
        "apply must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        text.contains("timeout = 30"),
        "unknown MCP field must survive: {text}"
    );
    assert!(
        text.contains("enabled = false"),
        "canonical apply must leave MCP disabled: {text}"
    );
}

#[test]
fn apply_requires_flag_leaves_target_untouched() {
    let _guard = lock_tests();
    let iso = Iso::new("apply-noflag");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", false);
    assert!(root_files(&iso.root_dir).is_empty());
    assert!(!iso.opencode_agents_md().exists());

    let out = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "plan-only must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Portable"), "stdout={stdout}");
    assert!(stdout.contains("Will create"), "stdout={stdout}");
    assert!(stdout.contains("Needs approval"), "stdout={stdout}");
    assert!(stdout.contains("Held"), "stdout={stdout}");
    assert!(stdout.contains("Plan hash"), "stdout={stdout}");
    assert!(
        stdout.trim_end().ends_with("to mutate."),
        "must end with plan-only footer: {stdout}"
    );
    assert!(
        stdout.contains("Plan only: no writes performed. Re-run with --apply --plan-hash"),
        "stdout={stdout}"
    );
    // Target untouched + no snapshots.
    assert!(
        !iso.opencode_agents_md().exists(),
        "target must not be created on plan-only"
    );
    assert!(
        root_files(&iso.root_dir).is_empty(),
        "no snapshots/journals on plan-only: {:?}",
        root_files(&iso.root_dir)
    );

    // WITH --apply but missing --plan-hash → same plan-only path exit 2.
    let out2 = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
    ]);
    assert_eq!(out2.status.code(), Some(2));
    let s2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        s2.contains("Plan only: no writes performed."),
        "missing --plan-hash must preflight: {s2}"
    );
    assert!(!iso.opencode_agents_md().exists());
}

#[test]
fn apply_missing_approval_names_sha() {
    let _guard = lock_tests();
    let iso = Iso::new("apply-noapprove");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", true);
    let pre = iso.preflight_json("opencode", &env_path);
    let approvals = pre["needs_approval"].as_array().unwrap();
    assert!(
        !approvals.is_empty(),
        "MCP fixture must need approval: {pre}"
    );
    let sha = approvals[0]["sha256"].as_str().unwrap().to_string();
    let plan_hash = pre["plan_hash"].as_str().unwrap().to_string();

    let out = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &plan_hash,
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "missing approval must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&sha),
        "missing approval must name sha {sha}: {stderr}"
    );

    // Extra unknown approval → exit 2.
    let out2 = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &plan_hash,
        "--approve",
        &sha,
        "--approve",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    ]);
    // If MCP needs exactly one approval, extra unknown must fail 2.
    // (If fixture needs exactly 1, this is unknown-extra; else still 2.)
    assert_eq!(
        out2.status.code(),
        Some(2),
        "extra approval must exit 2: stderr={}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let e2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        e2.contains("Unknown approval hash") || e2.contains("unknown") || e2.contains("approval"),
        "extra approval must mention approval: {e2}"
    );
}

#[test]
fn apply_drift_detects_stale_plan() {
    let _guard = lock_tests();
    let iso = Iso::new("apply-drift");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", false);
    let pre = iso.preflight_json("opencode", &env_path);
    let plan_hash = pre["plan_hash"].as_str().unwrap().to_string();

    // Touch target after plan.
    std::fs::write(iso.opencode_agents_md(), "touched-after-plan\n").unwrap();
    let before = std::fs::read(iso.opencode_agents_md()).unwrap();

    let out = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &plan_hash,
    ]);
    assert_eq!(
        out.status.code(),
        Some(5),
        "drift must exit 5: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Drift") || stderr.contains("drift"),
        "drift must be named: {stderr}"
    );
    let after = std::fs::read(iso.opencode_agents_md()).unwrap();
    assert_eq!(before, after, "drifted bytes must be unchanged");
}

#[test]
fn rollback_restores_target() {
    let _guard = lock_tests();
    let iso = Iso::new("rollback-restore");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", false);
    let pre = iso.preflight_json("opencode", &env_path);
    let plan_hash = pre["plan_hash"].as_str().unwrap().to_string();

    assert!(!iso.opencode_agents_md().exists());
    let applied = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &plan_hash,
    ]);
    assert_eq!(applied.status.code(), Some(0));
    assert!(iso.opencode_agents_md().exists());
    let applied_bytes = std::fs::read(iso.opencode_agents_md()).unwrap();

    // Mutate an unrelated source file (must not break rollback of created target).
    std::fs::write(iso.codex_home.join("AGENTS.md"), "# mutated-source\n").unwrap();

    let rb = iso.run(&["agent", "rollback", "--last"]);
    assert_eq!(
        rb.status.code(),
        Some(0),
        "rollback must exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&rb.stdout),
        String::from_utf8_lossy(&rb.stderr)
    );
    let rout = String::from_utf8_lossy(&rb.stdout);
    assert!(rout.contains("Rolled back"), "rout={rout}");
    // Created target removed → bytes back to absent.
    assert!(
        !iso.opencode_agents_md().exists(),
        "created target must be removed after rollback"
    );
    assert!(!applied_bytes.is_empty());

    // Rollback without --last → exit 2.
    let rb2 = iso.run(&["agent", "rollback"]);
    assert_eq!(rb2.status.code(), Some(2));
}

#[test]
fn capture_proposal_counts_and_writes_nothing() {
    let _guard = lock_tests();
    let iso = Iso::new("capture-proposal");
    setup_codex_portable(&iso);
    assert!(root_files(&iso.root_dir).is_empty());

    let out = iso.run(&["agent", "capture", "--from", "codex"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "proposal must exit 0: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Skills"), "stdout={stdout}");
    assert!(stdout.contains("MCP servers"), "stdout={stdout}");
    assert!(stdout.contains("Instructions"), "stdout={stdout}");
    assert!(stdout.contains("Env vars"), "stdout={stdout}");
    assert!(stdout.contains("Policies"), "stdout={stdout}");
    assert!(stdout.contains("Held"), "stdout={stdout}");
    assert!(
        stdout.contains("No changes made. Re-run with --apply --out"),
        "footer missing: {stdout}"
    );
    assert!(
        root_files(&iso.root_dir).is_empty(),
        "proposal must write nothing under ROOT_DIR"
    );
    assert!(!iso.base.join("agent.toml").exists());
    assert!(!iso.base.join("repo").exists());

    let json_out = iso.run(&["agent", "capture", "--from", "codex", "--json"]);
    assert_eq!(json_out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(v["from"], "codex");
    assert_eq!(v["mutated"], false);
    assert!(v["skills"].is_array());
    assert!(v["mcp_servers"].is_array());
    assert!(v["held"].is_array());
    assert!(!v["held"].as_array().unwrap().is_empty());
}

#[test]
fn capture_apply_overwrite_and_outside_repo() {
    let _guard = lock_tests();
    let iso = Iso::new("capture-apply");
    setup_codex_minimal(&iso);
    let repo = iso.base.join("repo");
    let dotroot = repo.join(".root");
    std::fs::create_dir_all(&dotroot).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let out = dotroot.join("agent.toml");

    let first = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "capture --apply must exit 0: stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let s1 = String::from_utf8_lossy(&first.stdout);
    assert!(
        s1.contains(&out.display().to_string()) || s1.contains("Wrote"),
        "s1={s1}"
    );
    assert!(s1.contains("Names only"), "names-only note missing: {s1}");
    assert!(
        !s1.trim_end().ends_with("No changes made."),
        "mutation must not end with No changes made: {s1}"
    );
    assert!(out.exists());

    // Overwrite refusal without --force.
    let second = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_ne!(second.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(2));
    let e2 = String::from_utf8_lossy(&second.stderr);
    assert!(
        e2.contains("without --force"),
        "overwrite refusal must name --force: {e2}"
    );

    // Force overwrite ok.
    let forced = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
        "--force",
    ]);
    assert_eq!(forced.status.code(), Some(0));

    // Outside-repo refusal.
    let outside = iso.base.join("outside.toml");
    let out3 = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        outside.to_str().unwrap(),
    ]);
    assert_eq!(out3.status.code(), Some(2));
    let e3 = String::from_utf8_lossy(&out3.stderr);
    assert!(
        e3.contains("outside repo") || e3.contains(".root"),
        "outside-repo refusal: {e3}"
    );
    assert!(!outside.exists());

    // Allow outside repo ok.
    let out4 = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        outside.to_str().unwrap(),
        "--allow-outside-repo",
    ]);
    assert_eq!(
        out4.status.code(),
        Some(0),
        "allow-outside-repo must succeed: stderr={}",
        String::from_utf8_lossy(&out4.stderr)
    );
    assert!(outside.exists());

    // Missing --out with --apply → exit 2.
    let missing = iso.run(&["agent", "capture", "--from", "codex", "--apply"]);
    assert_eq!(missing.status.code(), Some(2));
}

#[test]
fn claude_mcp_held_exits_2_and_touches_nothing() {
    let _guard = lock_tests();
    let iso = Iso::new("claude-held");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", true);
    let before = root_files(&iso.root_dir);
    assert!(before.is_empty(), "no snapshots before held apply");

    let pre = iso.run(&[
        "agent",
        "apply",
        "--to",
        "claude",
        "--env",
        env_path.to_str().unwrap(),
    ]);
    assert_eq!(
        pre.status.code(),
        Some(2),
        "claude MCP preflight must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&pre.stdout),
        String::from_utf8_lossy(&pre.stderr)
    );
    let combined =
        String::from_utf8_lossy(&pre.stdout).to_string() + &String::from_utf8_lossy(&pre.stderr);
    assert!(
        combined.contains("MCP is held") || combined.contains("unsupported in v0.4.1"),
        "held error missing: {combined}"
    );

    let applied = iso.run(&[
        "agent",
        "apply",
        "--to",
        "claude",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        "deadbeef",
    ]);
    assert_eq!(applied.status.code(), Some(2));
    let e = String::from_utf8_lossy(&applied.stderr);
    assert!(
        e.contains("MCP is held") || e.contains("unsupported in v0.4.1"),
        "held apply must name hold: {e}"
    );
    let after = root_files(&iso.root_dir);
    assert_eq!(before, after, "held apply must not create lock/snapshots");
}

#[test]
fn unsupported_target_version_apply_names_exact_list() {
    let _guard = lock_tests();
    let iso = Iso::new("unsupported-target");
    setup_codex_minimal(&iso);
    iso.write_shim("opencode", "#!/bin/sh\nprintf '9.9.9\\n'\n");
    let repo = iso.base.join("repo");
    let dotroot = repo.join(".root");
    std::fs::create_dir_all(&dotroot).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let out = dotroot.join("agent.toml");
    let cap = iso.run(&[
        "agent",
        "capture",
        "--from",
        "codex",
        "--apply",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(cap.status.code(), Some(0));

    // Preflight warns (still exit 2 with plan).
    let pre = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        out.to_str().unwrap(),
    ]);
    assert_eq!(pre.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&pre.stdout);
    assert!(
        stdout.contains("Warnings"),
        "unsupported target must warn: {stdout}"
    );
    assert!(
        stdout.contains("9.9.9"),
        "live version must be named: {stdout}"
    );

    // Apply fails closed naming exact supported list.
    let pre_json = iso.preflight_json("opencode", &out);
    let hash = pre_json["plan_hash"].as_str().unwrap().to_string();
    let apply = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        out.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &hash,
    ]);
    assert_ne!(apply.status.code(), Some(0));
    assert_eq!(apply.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(stderr.contains("9.9.9"), "must name live version: {stderr}");
    assert!(
        stderr.contains("1.18.27"),
        "must name exact supported list: {stderr}"
    );
    assert!(
        stderr.contains("Supported exact versions"),
        "must name supported versions: {stderr}"
    );
}

#[test]
fn unknown_agent_exits_2() {
    let _guard = lock_tests();
    let iso = Iso::new("unknown-agent-012");
    setup_codex_minimal(&iso);

    let v = iso.run(&["agent", "verify", "--agent", "gemini"]);
    assert_eq!(
        v.status.code(),
        Some(2),
        "unknown verify must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&v.stdout),
        String::from_utf8_lossy(&v.stderr)
    );

    let a = iso.run(&["agent", "apply", "--to", "gemini"]);
    assert_eq!(
        a.status.code(),
        Some(2),
        "unknown apply must exit 2: stdout={} stderr={}",
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&a.stderr)
    );
    let ae = String::from_utf8_lossy(&a.stderr);
    assert!(
        ae.contains("unsupported bundle adapter"),
        "unknown agent must name adapter: {ae}"
    );

    let c = iso.run(&["agent", "capture", "--from", "gemini"]);
    assert_eq!(c.status.code(), Some(2));
}

#[test]
fn purge_requires_yes_and_xor() {
    let _guard = lock_tests();
    let iso = Iso::new("purge-xor");
    let (_repo, env_path) = setup_repo_with_capture(&iso, "repo", false);
    let pre = iso.preflight_json("opencode", &env_path);
    let plan_hash = pre["plan_hash"].as_str().unwrap().to_string();
    let applied = iso.run(&[
        "agent",
        "apply",
        "--to",
        "opencode",
        "--env",
        env_path.to_str().unwrap(),
        "--apply",
        "--plan-hash",
        &plan_hash,
    ]);
    assert_eq!(applied.status.code(), Some(0));
    assert!(!root_files(&iso.root_dir).is_empty());

    // Missing --yes → Err, no deletion.
    let before = root_files(&iso.root_dir);
    let noyes = iso.run(&["agent", "purge", "--all"]);
    assert_ne!(noyes.status.code(), Some(0));
    let after = root_files(&iso.root_dir);
    assert_eq!(before, after, "purge without --yes must delete nothing");

    // XOR: both --id and --all → exit 2.
    let both = iso.run(&["agent", "purge", "--id", "x", "--all", "--yes"]);
    assert_eq!(both.status.code(), Some(2));

    // XOR: neither → exit 2.
    let neither = iso.run(&["agent", "purge", "--yes"]);
    assert_eq!(neither.status.code(), Some(2));

    // Purge all with --yes → deletes.
    let purge = iso.run(&["agent", "purge", "--all", "--yes"]);
    assert_eq!(
        purge.status.code(),
        Some(0),
        "purge --all --yes must succeed: stderr={}",
        String::from_utf8_lossy(&purge.stderr)
    );
    let pout = String::from_utf8_lossy(&purge.stdout);
    assert!(pout.contains("Deleted"), "pout={pout}");
}
