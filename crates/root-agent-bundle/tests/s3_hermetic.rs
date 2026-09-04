//! S3 hermetic tests: Claude inspect/export scaffolding with isolated
//! CLAUDE_CONFIG_DIR. MCP apply/enable is gated. Never touches real ~/.claude.

use std::sync::Mutex;

static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn unique_tmp(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), n))
}

struct EnvGuard {
    saved_root: Option<std::ffi::OsString>,
    saved_home: Option<std::ffi::OsString>,
    saved_claude: Option<std::ffi::OsString>,
    saved_path: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn isolate(tmp: &std::path::Path) -> Self {
        let saved_root = std::env::var_os("ROOT_DIR");
        let saved_home = std::env::var_os("HOME");
        let saved_claude = std::env::var_os("CLAUDE_CONFIG_DIR");
        let home = tmp.join("home");
        let cfg = home.join(".claude");
        std::env::set_var("ROOT_DIR", tmp.join("root"));
        std::env::set_var("HOME", &home);
        std::env::set_var("CLAUDE_CONFIG_DIR", &cfg);
        let _ = std::fs::create_dir_all(&cfg);
        let _ = std::fs::create_dir_all(home.join(".agents").join("skills"));
        Self {
            saved_root,
            saved_home,
            saved_claude,
            saved_path: None,
        }
    }

    fn prepend_path(&mut self, dir: &std::path::Path) {
        if self.saved_path.is_none() {
            self.saved_path = Some(std::env::var_os("PATH").unwrap_or_default());
        }
        let mut new_path = dir.as_os_str().to_os_string();
        new_path.push(":");
        new_path.push(std::env::var_os("PATH").unwrap_or_default());
        std::env::set_var("PATH", new_path);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.saved_root {
            Some(v) => std::env::set_var("ROOT_DIR", v),
            None => std::env::remove_var("ROOT_DIR"),
        }
        match &self.saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.saved_claude {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
        if let Some(v) = &self.saved_path {
            std::env::set_var("PATH", v);
        }
    }
}

fn write_fake_claude(bin_dir: &std::path::Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let path = bin_dir.join("claude");
    std::fs::write(&path, "#!/bin/sh\necho '2.1.260 (Claude Code)'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
}

#[test]
fn inspect_sees_isolated_claude_dir_not_real_home() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_inspect");
    let mut guard = EnvGuard::isolate(&tmp);
    let cfg = std::env::var_os("CLAUDE_CONFIG_DIR").unwrap();
    std::fs::write(std::path::Path::new(&cfg).join("CLAUDE.md"), "# isolated\n").unwrap();
    std::fs::write(
        std::path::Path::new(&cfg).join("settings.json"),
        r#"{"model":"claude-sonnet-4-6","permissions":{"allow":["Bash(ls)"]}}"#,
    )
    .unwrap();
    std::fs::write(
        std::path::Path::new(&cfg).join(".claude.json"),
        r#"{"mcpServers":{"github":{"command":"npx"}}}"#,
    )
    .unwrap();
    let bin = tmp.join("bin");
    write_fake_claude(&bin);
    guard.prepend_path(&bin);

    let report = root_agent_bundle::claude::inspect().unwrap();
    assert!(report.present);
    assert_eq!(report.version.as_deref(), Some("2.1.260"));
    assert!(
        report.version_supported,
        "2.1.260 must be the frozen S3 gate"
    );
    let isolated = std::path::Path::new(&cfg).display().to_string();
    assert_eq!(report.config_dir, isolated);
    assert_eq!(report.global_state_dir, isolated);
    assert!(
        report.config_dir.contains("s3_inspect"),
        "inspect leaked a non-isolated path: {}",
        report.config_dir
    );
    assert!(report.claude_md_present);
    assert!(report.settings_present);
    assert_eq!(report.mcp_servers, vec!["github".to_string()]);
    assert!(
        report
            .held
            .iter()
            .any(|h| h.reason == root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR),
        "held must use the stable v0.4.1 MCP error, got {:?}",
        report.held
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn export_accepts_frozen_version_and_holds_mcp() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_export");
    let mut guard = EnvGuard::isolate(&tmp);
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    std::fs::write(cfg.join("CLAUDE.md"), "# isolated\n").unwrap();
    std::fs::write(
        cfg.join("settings.json"),
        r#"{"model":"claude-sonnet-4-6","permissions":{"allow":["Bash(ls)"]}}"#,
    )
    .unwrap();
    let bin = tmp.join("bin");
    write_fake_claude(&bin);
    guard.prepend_path(&bin);
    let out = tmp.join("bundle");
    let opts = root_agent_bundle::export::ExportOptions {
        skills: vec![],
        include_mcp: vec![],
        include_executable: vec![],
        no_timestamp: true,
    };
    let manifest = root_agent_bundle::export::export_claude(&out, &opts).unwrap();
    assert_eq!(manifest.adapter, "claude");
    assert_eq!(manifest.source_agent_version, "2.1.260");
    assert!(manifest.mcp.is_empty());
    assert_eq!(
        manifest.settings.get("model").and_then(|v| v.as_str()),
        Some("claude-sonnet-4-6")
    );
    assert!(!manifest.settings.contains_key("permissions"));
    assert!(manifest.files.iter().any(|f| f.rel == "CLAUDE.md"));

    let out2 = tmp.join("bundle-mcp");
    let opts_mcp = root_agent_bundle::export::ExportOptions {
        skills: vec![],
        include_mcp: vec!["github".to_string()],
        include_executable: vec![],
        no_timestamp: true,
    };
    let err = root_agent_bundle::export::export_claude(&out2, &opts_mcp).unwrap_err();
    assert_eq!(
        err.to_string(),
        root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR
    );
    assert!(manifest
        .held
        .iter()
        .any(|h| h.reason == root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn claude_scopes_serde_and_parse_agree() {
    use root_agent_bundle::scope::Scope;
    assert_eq!(Scope::ClaudeHome.as_str(), "claude_home");
    assert_eq!(Scope::ClaudeGlobalState.as_str(), "claude_global_state");
    assert_eq!(
        serde_json::to_string(&Scope::ClaudeHome).unwrap(),
        "\"claude_home\""
    );
    assert_eq!(
        serde_json::to_string(&Scope::ClaudeGlobalState).unwrap(),
        "\"claude_global_state\""
    );
    assert!(Scope::parse("../.claude.json").is_err());
    assert!(root_agent_bundle::scope::validate_rel("../.claude.json").is_err());
}

fn file_entry(
    scope: root_agent_bundle::scope::Scope,
    rel: &str,
    bytes: &[u8],
    executable: bool,
) -> (
    root_agent_bundle::manifest::BundleFile,
    (String, Vec<u8>),
    Option<root_agent_bundle::manifest::NeedsApproval>,
) {
    let digest = root_lockfile::compute_sha256(bytes);
    let approval = executable.then(|| root_agent_bundle::manifest::NeedsApproval {
        scope,
        rel: rel.to_string(),
        sha256: digest.clone(),
        reason: format!("executable {}", rel),
    });
    let file = root_agent_bundle::manifest::BundleFile {
        scope,
        rel: rel.to_string(),
        sha256: digest.clone(),
        size: bytes.len() as u64,
        mode: if executable {
            "0755".to_string()
        } else {
            "0644".to_string()
        },
        kind: if executable {
            root_agent_bundle::manifest::FileKind::Executable
        } else {
            root_agent_bundle::manifest::FileKind::PromptContent
        },
        executable,
    };
    (file, (digest, bytes.to_vec()), approval)
}

fn held_bundle(
    tmp: &std::path::Path,
    claude_md: &[u8],
    skill_md: &[u8],
    skill_sh: &[u8],
) -> (
    std::path::PathBuf,
    root_agent_bundle::manifest::Manifest,
    Vec<String>,
) {
    let (md_file, md_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::ClaudeHome,
        "CLAUDE.md",
        claude_md,
        false,
    );
    let (native_file, native_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::ClaudeHome,
        "skills/docs-writer/SKILL.md",
        skill_md,
        false,
    );
    let (sh_file, sh_blob, sh_approval) = file_entry(
        root_agent_bundle::scope::Scope::SharedSkills,
        "repo-helper/run.sh",
        skill_sh,
        true,
    );
    let mut manifest = root_agent_bundle::manifest::Manifest::new_for(
        root_agent_bundle::CLAUDE_ADAPTER_ID,
        "2.1.260".to_string(),
        None,
    );
    manifest.files = vec![md_file, native_file, sh_file];
    manifest.settings.insert(
        "model".to_string(),
        serde_json::Value::String("claude-sonnet-4-6".to_string()),
    );
    manifest.needs_approval = vec![sh_approval.expect("executable skill requires approval")];
    manifest.validate().unwrap();
    let bundle = tmp.join("bundle");
    root_agent_bundle::blob::write_bundle_dir(&bundle, &manifest, &[md_blob, native_blob, sh_blob])
        .unwrap();
    let approvals: Vec<String> = manifest
        .needs_approval
        .iter()
        .map(|a| a.sha256.clone())
        .collect();
    (bundle, manifest, approvals)
}

fn claude_json_canary() -> &'static [u8] {
    br#"{"oauth":"do-not-copy","mcpServers":{"github":{"command":"npx"}}}"#
}

fn seed_target(_tmp: &std::path::Path, settings: &str) {
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    std::fs::write(cfg.join("CLAUDE.md"), b"# Target CLAUDE.md\nold\n").unwrap();
    std::fs::write(cfg.join("settings.json"), settings.as_bytes()).unwrap();
    std::fs::write(cfg.join(".claude.json"), claude_json_canary()).unwrap();
}

fn snapshot_rels() -> Vec<String> {
    let snaps = root_agent_bundle::snapshot::list_snapshots().unwrap();
    snaps
        .into_iter()
        .flat_map(|s| s.entries.into_iter().map(|e| e.rel))
        .collect()
}

#[test]
fn gated_errors_are_identical() {
    assert_eq!(
        root_agent_bundle::claude::mcp_export_gated_error().to_string(),
        root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR
    );
    assert_eq!(
        root_agent_bundle::claude::mcp_apply_gated_error().to_string(),
        root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR
    );
    assert_eq!(
        root_agent_bundle::claude::claude_mcp_held_error().to_string(),
        root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR
    );
}

#[test]
fn claude_mcp_manifest_is_rejected_before_plan_lock_or_snapshot() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_mcp_invalid");
    let _env = EnvGuard::isolate(&tmp);
    let mut manifest = root_agent_bundle::manifest::Manifest::new_for(
        root_agent_bundle::CLAUDE_ADAPTER_ID,
        "2.1.260".to_string(),
        None,
    );
    let command = vec!["npx".to_string()];
    let hash = root_agent_bundle::manifest::mcp_command_hash(&command, &[], &None, &[]);
    manifest.mcp.insert(
        "github".to_string(),
        root_agent_bundle::manifest::McpEntry {
            transport: "stdio".to_string(),
            enabled: false,
            needs_env: vec![],
            command_sha256: Some(hash.clone()),
            command,
            args: vec![],
            cwd: None,
            env_keys: vec![],
        },
    );
    manifest
        .needs_approval
        .push(root_agent_bundle::manifest::NeedsApproval {
            scope: root_agent_bundle::scope::Scope::ClaudeGlobalState,
            rel: ".claude.json#mcpServers.github".to_string(),
            sha256: hash,
            reason: "MCP".to_string(),
        });
    let err = manifest.validate().unwrap_err().to_string();
    assert_eq!(err, root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR);

    let bundle = tmp.join("bundle");
    root_agent_bundle::blob::write_bundle_dir(&bundle, &manifest, &[]).unwrap();
    let load_err = root_agent_bundle::manifest::load_bundle(&bundle)
        .unwrap_err()
        .to_string();
    assert_eq!(load_err, root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR);
    let plan_err = root_agent_bundle::plan::compute_plan(&bundle, &manifest)
        .unwrap_err()
        .to_string();
    assert_eq!(plan_err, root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR);
    let apply_err = root_agent_bundle::apply::apply_bundle(&bundle, "deadbeef", &[])
        .unwrap_err()
        .to_string();
    assert_eq!(apply_err, root_agent_bundle::claude::CLAUDE_MCP_HELD_ERROR);

    let root = tmp.join("root");
    assert!(
        !root.join("agent-apply.json").exists(),
        "journal must not be created for an invalid Claude MCP bundle"
    );
    let snaps = root.join("agent-snapshots");
    assert!(
        !snaps.exists() || std::fs::read_dir(&snaps).unwrap().next().is_none(),
        "no snapshots for an invalid Claude MCP bundle"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn settings_apply_preserves_unknowns_skills_and_never_touches_claude_json() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_apply");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_claude(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let pre_settings = r#"{
  "model": "old-model",
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {"Stop": []}
}"#;
    seed_target(&tmp, pre_settings);
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    let pre_claude_json = std::fs::read(cfg.join(".claude.json")).unwrap();

    let claude_md = b"# Source CLAUDE.md\nUse Root.\n";
    let skill_md = b"# docs-writer\nWrite concise docs.\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let (bundle, _manifest, approvals) = held_bundle(&tmp, claude_md, skill_md, skill_sh);
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &_manifest).unwrap();
    assert!(
        !plan
            .target_preconditions
            .keys()
            .any(|k| k.contains(".claude.json")),
        "plan must not mention .claude.json: {:?}",
        plan.target_preconditions
    );

    let no_approve =
        root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &[]).unwrap_err();
    assert!(
        no_approve.to_string().contains("hash-bound approval"),
        "got: {no_approve}"
    );

    root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();

    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cfg.join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        settings.get("model").and_then(|v| v.as_str()),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        settings
            .pointer("/permissions/allow/0")
            .and_then(|v| v.as_str()),
        Some("Bash(ls)")
    );
    assert!(
        settings.get("hooks").is_some(),
        "unknown keys must be preserved"
    );
    assert_eq!(std::fs::read(cfg.join("CLAUDE.md")).unwrap(), claude_md);
    let native = cfg.join("skills").join("docs-writer").join("SKILL.md");
    assert_eq!(std::fs::read(&native).unwrap(), skill_md);
    let sh = tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("repo-helper")
        .join("run.sh");
    assert_eq!(std::fs::read(&sh).unwrap(), skill_sh);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(&sh).unwrap().permissions().mode() & 0o111,
            0
        );
    }
    assert_eq!(
        std::fs::read(cfg.join(".claude.json")).unwrap(),
        pre_claude_json,
        "apply must not change .claude.json"
    );
    let rels = snapshot_rels();
    assert!(
        !rels
            .iter()
            .any(|r| r == ".claude.json" || r.contains("claude.json")),
        "snapshots must not include .claude.json, got {rels:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rollback_restores_settings_and_tombstones_created_skills() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_rollback");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_claude(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let pre_settings = r#"{"model":"old-model","keep":true}"#;
    seed_target(&tmp, pre_settings);
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    let pre_md = std::fs::read(cfg.join("CLAUDE.md")).unwrap();
    let pre_settings_bytes = std::fs::read(cfg.join("settings.json")).unwrap();
    let pre_claude_json = std::fs::read(cfg.join(".claude.json")).unwrap();

    let (bundle, manifest, approvals) = held_bundle(
        &tmp,
        b"# Source CLAUDE.md\n",
        b"# docs-writer\n",
        b"#!/bin/sh\necho repo-helper\n",
    );
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();
    root_agent_bundle::apply::rollback_last().unwrap();

    assert_eq!(std::fs::read(cfg.join("CLAUDE.md")).unwrap(), pre_md);
    assert_eq!(
        std::fs::read(cfg.join("settings.json")).unwrap(),
        pre_settings_bytes
    );
    assert_eq!(
        std::fs::read(cfg.join(".claude.json")).unwrap(),
        pre_claude_json
    );
    assert!(!cfg
        .join("skills")
        .join("docs-writer")
        .join("SKILL.md")
        .exists());
    assert!(!tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("repo-helper")
        .join("run.sh")
        .exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn strict_json_settings_are_rejected() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_jsonc");
    let _env = EnvGuard::isolate(&tmp);
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    std::fs::write(cfg.join("settings.json"), "{\n  \"model\": \"x\",\n}\n").unwrap();
    let err = root_agent_bundle::claude::read_allowed_settings(&cfg.join("settings.json"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("strict JSON") || err.contains("parse") || err.contains("trailing"),
        "got: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn version_mismatch_is_unsupported() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_ver");
    let mut env = EnvGuard::isolate(&tmp);
    std::fs::create_dir_all(tmp.join("bin")).unwrap();
    let path = tmp.join("bin").join("claude");
    std::fs::write(&path, "#!/bin/sh\necho '2.1.259 (Claude Code)'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    env.prepend_path(&tmp.join("bin"));
    let report = root_agent_bundle::claude::inspect().unwrap();
    assert_eq!(report.version.as_deref(), Some("2.1.259"));
    assert!(!report.version_supported);
    let err = root_agent_bundle::export::export_claude(
        &tmp.join("bundle"),
        &root_agent_bundle::export::ExportOptions {
            skills: vec![],
            include_mcp: vec![],
            include_executable: vec![],
            no_timestamp: true,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("2.1.259") && err.contains("2.1.260"),
        "got: {err}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn symlink_and_fifo_are_rejected() {
    let _lock = lock_env();
    let tmp = unique_tmp("s3_fifo");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_claude(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));
    let cfg = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp/nowhere", cfg.join("CLAUDE.md")).unwrap();
        let err = root_agent_bundle::export::export_claude(
            &tmp.join("bundle-symlink"),
            &root_agent_bundle::export::ExportOptions {
                skills: vec![],
                include_mcp: vec![],
                include_executable: vec![],
                no_timestamp: true,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.to_lowercase().contains("symlink"), "got: {err}");
    }

    let (bundle, manifest, _) = held_bundle(
        &tmp,
        b"# CLAUDE.md\n",
        b"# docs-writer\n",
        b"#!/bin/sh\necho x\n",
    );
    assert!(manifest.mcp.is_empty());
    let fifo = bundle
        .join("blobs")
        .join("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&fifo);
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let err = root_agent_bundle::manifest::load_bundle(&bundle)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("regular") || err.contains("non-regular"),
            "got: {err}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
