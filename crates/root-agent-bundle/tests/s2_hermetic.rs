//! S2 hermetic tests: OpenCode portable agent-bundle with isolated ROOT_DIR/HOME/XDG.
//!
//! All env-mutating tests hold TEST_MUTEX (see root-core TEST_MUTEX pattern).
//! Never touches the real ~/.config/opencode.

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
    saved_xdg: Option<std::ffi::OsString>,
    saved_opencode: Option<std::ffi::OsString>,
    saved_codex: Option<std::ffi::OsString>,
    saved_path: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn isolate(tmp: &std::path::Path) -> Self {
        let saved_root = std::env::var_os("ROOT_DIR");
        let saved_home = std::env::var_os("HOME");
        let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let saved_opencode = std::env::var_os("OPENCODE_CONFIG_DIR");
        let saved_codex = std::env::var_os("CODEX_HOME");
        std::env::set_var("ROOT_DIR", tmp.join("root"));
        std::env::set_var("HOME", tmp.join("home"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.join("xdg-config"));
        // Prefer XDG_CONFIG_HOME + HOME; do not honor a leftover OPENCODE_CONFIG_DIR.
        std::env::remove_var("OPENCODE_CONFIG_DIR");
        std::env::set_var("CODEX_HOME", tmp.join("home").join(".codex"));
        let _ = std::fs::create_dir_all(tmp.join("home").join(".codex"));
        let _ = std::fs::create_dir_all(tmp.join("home").join(".agents").join("skills"));
        let _ = std::fs::create_dir_all(tmp.join("xdg-config").join("opencode"));
        Self {
            saved_root,
            saved_home,
            saved_xdg,
            saved_opencode,
            saved_codex,
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
        match &self.saved_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &self.saved_opencode {
            Some(v) => std::env::set_var("OPENCODE_CONFIG_DIR", v),
            None => std::env::remove_var("OPENCODE_CONFIG_DIR"),
        }
        match &self.saved_codex {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
        if let Some(v) = &self.saved_path {
            std::env::set_var("PATH", v);
        }
    }
}

fn opencode_dir(tmp: &std::path::Path) -> std::path::PathBuf {
    tmp.join("xdg-config").join("opencode")
}

fn write_fake_opencode(bin_dir: &std::path::Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let path = bin_dir.join("opencode");
    std::fs::write(&path, "#!/bin/sh\necho '1.18.27'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
}

fn write_fake_codex(bin_dir: &std::path::Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let path = bin_dir.join("codex");
    std::fs::write(&path, "#!/bin/sh\necho 'codex-cli 0.150.1'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
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

fn mcp_github() -> (
    root_agent_bundle::manifest::McpEntry,
    root_agent_bundle::manifest::NeedsApproval,
) {
    let command = vec!["npx".to_string()];
    let args = vec![
        "-y".to_string(),
        "@modelcontextprotocol/server-github".to_string(),
    ];
    let env_keys = vec!["ROOT_S2_ACCEPT_TOKEN".to_string()];
    let mcp_hash = root_agent_bundle::manifest::mcp_command_hash(&command, &args, &None, &env_keys);
    let (scope, rel) = root_agent_bundle::manifest::mcp_approval_target(
        root_agent_bundle::OPENCODE_ADAPTER_ID,
        "github",
    )
    .unwrap();
    let approval = root_agent_bundle::manifest::NeedsApproval {
        scope,
        rel,
        sha256: mcp_hash.clone(),
        reason: "MCP stdio command for server 'github'".to_string(),
    };
    let entry = root_agent_bundle::manifest::McpEntry {
        transport: "stdio".to_string(),
        enabled: false,
        needs_env: env_keys.clone(),
        command_sha256: Some(mcp_hash),
        command,
        args,
        cwd: None,
        env_keys,
    };
    (entry, approval)
}

fn source_opencode_json() -> String {
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "model": "anthropic/claude-sonnet-4-5",
        "unknown_source_experimental": "held-do-not-export",
        "mcp": {
            "github": {
                "type": "local",
                "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                "enabled": true,
                "environment": {
                    "ROOT_S2_ACCEPT_TOKEN": "{env:ROOT_S2_ACCEPT_TOKEN}"
                }
            },
            "remote-docs": {
                "type": "remote",
                "url": "https://example.invalid/mcp",
                "enabled": true
            }
        }
    })
    .to_string()
}

fn target_opencode_json() -> String {
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "model": "gpt-4.1",
        "target_only_experimental": "preserve-this-value",
        "mcp": {
            "other": {
                "type": "local",
                "command": ["echo"],
                "enabled": true,
                "timeout": 30
            }
        }
    })
    .to_string()
}

fn parse_json(path: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}\n{text}", path.display()))
}

fn representative_bundle(
    tmp: &std::path::Path,
    agents_src: &[u8],
    skill_md: &[u8],
    skill_sh: &[u8],
) -> (
    std::path::PathBuf,
    root_agent_bundle::manifest::Manifest,
    Vec<String>,
) {
    let (agents_file, agents_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::OpenCodeHome,
        "AGENTS.md",
        agents_src,
        false,
    );
    let (md_file, md_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::SharedSkills,
        "docs-writer/SKILL.md",
        skill_md,
        false,
    );
    let (sh_file, sh_blob, sh_approval) = file_entry(
        root_agent_bundle::scope::Scope::SharedSkills,
        "repo-helper/run.sh",
        skill_sh,
        true,
    );
    let (mcp_entry, mcp_approval) = mcp_github();
    let mut manifest = root_agent_bundle::manifest::Manifest::new_for(
        root_agent_bundle::OPENCODE_ADAPTER_ID,
        "1.18.27".to_string(),
        None,
    );
    manifest.files = vec![agents_file, md_file, sh_file];
    manifest.settings.insert(
        "model".to_string(),
        serde_json::Value::String("anthropic/claude-sonnet-4-5".to_string()),
    );
    manifest.mcp.insert("github".to_string(), mcp_entry);
    manifest.needs_env = vec!["ROOT_S2_ACCEPT_TOKEN".to_string()];
    manifest.needs_approval = vec![
        sh_approval.expect("executable skill requires approval"),
        mcp_approval,
    ];
    manifest.validate().unwrap();
    let bundle = tmp.join("bundle");
    root_agent_bundle::blob::write_bundle_dir(&bundle, &manifest, &[agents_blob, md_blob, sh_blob])
        .unwrap();
    let approvals: Vec<String> = manifest
        .needs_approval
        .iter()
        .map(|a| a.sha256.clone())
        .collect();
    (bundle, manifest, approvals)
}

#[test]
fn inspect_sees_isolated_config_dir_not_real_home() {
    let real_home = std::env::var("HOME").unwrap_or_default();
    let _lock = lock_env();
    let tmp = unique_tmp("s2_inspect");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let oc = opencode_dir(&tmp);
    std::fs::write(oc.join("AGENTS.md"), b"# isolated OpenCode AGENTS.md\n").unwrap();
    std::fs::write(oc.join("opencode.json"), br#"{"model":"isolated-model"}"#).unwrap();

    let report = root_agent_bundle::opencode::inspect().unwrap();
    assert_eq!(report.agent, "opencode");
    assert!(report.present);
    assert_eq!(report.version.as_deref(), Some("1.18.27"));
    assert!(report.version_supported);
    assert!(root_agent_bundle::SUPPORTED_OPENCODE_VERSIONS.contains(&"1.18.27"));
    assert!(report.agents_md_present);
    assert!(report.config_present);
    let isolated = oc.display().to_string();
    assert_eq!(
        report.config_dir, isolated,
        "inspect must resolve $XDG_CONFIG_HOME/opencode"
    );
    assert!(
        report.config_dir.contains("xdg-config"),
        "inspect must use isolated XDG config, got {}",
        report.config_dir
    );
    if !real_home.is_empty() {
        assert_ne!(
            report.config_dir,
            format!("{}/.config/opencode", real_home),
            "inspect must not see the real OpenCode home"
        );
        assert!(
            !report.config_dir.starts_with(&real_home),
            "inspect config_dir leaked real HOME {}: {}",
            real_home,
            report.config_dir
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn export_allowlisted_content_holds_unknowns_and_skips_remote_mcp() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_export");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let agents_src = b"# Source AGENTS.md\nUse Root for installs.\n";
    let skill_md = b"# docs-writer\nWrite concise docs.\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let oc = opencode_dir(&tmp);
    std::fs::write(oc.join("AGENTS.md"), agents_src).unwrap();
    std::fs::write(oc.join("opencode.json"), source_opencode_json()).unwrap();
    std::fs::create_dir_all(oc.join("skills").join("docs-writer")).unwrap();
    std::fs::write(
        oc.join("skills").join("docs-writer").join("SKILL.md"),
        skill_md,
    )
    .unwrap();
    let shared = tmp.join("home").join(".agents").join("skills");
    std::fs::create_dir_all(shared.join("repo-helper")).unwrap();
    std::fs::write(shared.join("repo-helper").join("run.sh"), skill_sh).unwrap();

    let bundle = tmp.join("bundle");
    let opts = root_agent_bundle::export::ExportOptions {
        skills: vec!["docs-writer".to_string(), "repo-helper".to_string()],
        include_mcp: vec!["github".to_string()],
        include_executable: vec!["repo-helper".to_string()],
        no_timestamp: true,
    };
    let manifest = root_agent_bundle::export::export_opencode(&bundle, &opts).unwrap();
    assert_eq!(manifest.bundle_version, 1);
    assert_eq!(manifest.adapter, "opencode");
    assert_eq!(manifest.source_agent_version, "1.18.27");
    assert_eq!(
        manifest.disclosure,
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    );
    assert_eq!(
        manifest.settings.get("model").and_then(|v| v.as_str()),
        Some("anthropic/claude-sonnet-4-5")
    );
    assert!(
        !manifest
            .settings
            .contains_key("unknown_source_experimental"),
        "unknown source JSON keys must be held, not exported"
    );
    assert!(
        !manifest.settings.contains_key("$schema"),
        "$schema must be held"
    );
    assert!(
        manifest.held.iter().any(|h| h.source.contains("unknown")),
        "held must record unknown source fields: {:?}",
        manifest.held
    );
    let github = manifest.mcp.get("github").expect("github MCP exported");
    assert!(!github.enabled, "exported MCP must be enabled=false");
    assert_eq!(github.needs_env, vec!["ROOT_S2_ACCEPT_TOKEN".to_string()]);
    assert!(!manifest.mcp.contains_key("remote-docs"));

    assert!(manifest
        .files
        .iter()
        .any(|f| f.rel == "AGENTS.md" && f.scope == root_agent_bundle::scope::Scope::OpenCodeHome));
    assert!(manifest
        .files
        .iter()
        .any(|f| f.rel == "skills/docs-writer/SKILL.md"
            && f.scope == root_agent_bundle::scope::Scope::OpenCodeHome));
    assert!(manifest.files.iter().any(|f| f.rel == "repo-helper/run.sh"
        && f.scope == root_agent_bundle::scope::Scope::SharedSkills
        && f.executable));

    let remote_bundle = tmp.join("bundle-remote");
    let remote_opts = root_agent_bundle::export::ExportOptions {
        skills: vec![],
        include_mcp: vec!["remote-docs".to_string()],
        include_executable: vec![],
        no_timestamp: true,
    };
    let err = root_agent_bundle::export::export_opencode(&remote_bundle, &remote_opts).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("remote")
            || msg.contains("not local")
            || msg.contains("unsupported field")
            || msg.contains("url"),
        "remote MCP must not export, got: {msg}"
    );
    assert!(!remote_bundle.exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn apply_without_approvals_fails() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_noapprove");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    std::fs::write(
        opencode_dir(&tmp).join("opencode.json"),
        target_opencode_json(),
    )
    .unwrap();
    let (bundle, manifest, _) = representative_bundle(
        &tmp,
        b"# Source AGENTS.md\n",
        b"# docs-writer\n",
        b"#!/bin/sh\necho repo-helper\n",
    );
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    let err = root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &[]).unwrap_err();
    assert!(
        err.to_string().contains("hash-bound approval"),
        "got: {}",
        err
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn apply_writes_disabled_local_mcp_preserves_unknowns_and_executable_mode() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_apply");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let agents_src = b"# Source AGENTS.md\nUse Root for installs.\n";
    let skill_md = b"# docs-writer\nWrite concise docs.\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let pre_agents = b"# Target AGENTS.md\nold prompt\n";
    let oc = opencode_dir(&tmp);
    std::fs::write(oc.join("AGENTS.md"), pre_agents).unwrap();
    std::fs::write(oc.join("opencode.json"), target_opencode_json()).unwrap();

    let (bundle, manifest, approvals) = representative_bundle(&tmp, agents_src, skill_md, skill_sh);
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    let applied =
        root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();
    assert!(applied.mcp_imported.contains(&"github".to_string()));

    let config_path = oc.join("opencode.json");
    let parsed = parse_json(&config_path);
    let github = parsed
        .pointer("/mcp/github")
        .unwrap_or_else(|| panic!("mcp.github missing:\n{parsed}"));
    assert_eq!(github.get("type").and_then(|v| v.as_str()), Some("local"));
    assert_eq!(github.get("enabled").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        github
            .pointer("/environment/ROOT_S2_ACCEPT_TOKEN")
            .and_then(|v| v.as_str()),
        Some("{env:ROOT_S2_ACCEPT_TOKEN}")
    );
    let rendered = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !rendered.contains("dummy-not-a-real-secret"),
        "secret values leaked into config"
    );
    assert_eq!(
        parsed
            .get("target_only_experimental")
            .and_then(|v| v.as_str()),
        Some("preserve-this-value")
    );
    assert_eq!(
        parsed.get("$schema").and_then(|v| v.as_str()),
        Some("https://opencode.ai/config.json")
    );
    assert_eq!(
        parsed
            .pointer("/mcp/other/enabled")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        parsed
            .pointer("/mcp/other/timeout")
            .and_then(|v| v.as_u64()),
        Some(30)
    );
    assert_eq!(std::fs::read(oc.join("AGENTS.md")).unwrap(), agents_src);
    let sh_path = tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("repo-helper")
        .join("run.sh");
    assert_eq!(std::fs::read(&sh_path).unwrap(), skill_sh);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(&sh_path).unwrap().permissions().mode() & 0o111,
            0
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn enable_requires_plan_approval_env_and_provenance() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_enable");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));
    std::env::remove_var("ROOT_S2_DEFINITELY_ABSENT_VAR");

    let config = serde_json::json!({
        "model": "gpt-x",
        "mcp": {
            "testy": {
                "type": "local",
                "command": ["npx"],
                "enabled": false,
                "environment": {
                    "ROOT_S2_DEFINITELY_ABSENT_VAR": "{env:ROOT_S2_DEFINITELY_ABSENT_VAR}"
                }
            }
        }
    });
    std::fs::write(
        opencode_dir(&tmp).join("opencode.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();

    let plan = root_agent_bundle::opencode::enable_plan("testy").unwrap();
    let err = root_agent_bundle::apply::enable_opencode_server("testy", "bogus", &[]).unwrap_err();
    assert!(err.to_string().contains("plan hash"), "got: {}", err);
    let err = root_agent_bundle::apply::enable_opencode_server("testy", &plan.plan_hash, &[])
        .unwrap_err();
    assert!(err.to_string().contains("approval"), "got: {}", err);
    let err = root_agent_bundle::apply::enable_opencode_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("no completed agent-bundle provenance"),
        "got: {}",
        err
    );

    let mut provenance = std::collections::BTreeMap::new();
    provenance.insert(
        root_agent_bundle::journal::mcp_provenance_key("opencode", "testy"),
        plan.descriptor_hash.clone(),
    );
    root_agent_bundle::journal::write_journal(&root_agent_bundle::journal::ApplyJournal {
        op_id: "op_reviewed_import".to_string(),
        agent: "opencode".to_string(),
        plan_hash: "reviewed-bundle-plan".to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: root_agent_bundle::journal::Phase::Done,
        completed_paths: vec!["opencode_home:opencode.json".to_string()],
        target_preconditions: Default::default(),
        mcp_provenance: provenance,
        prior_mcp_provenance: Default::default(),
    })
    .unwrap();

    let err = root_agent_bundle::apply::enable_opencode_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("secret references missing"),
        "got: {}",
        err
    );

    std::env::set_var(
        "ROOT_S2_DEFINITELY_ABSENT_VAR",
        "present-not-a-secret-to-write",
    );
    let report = root_agent_bundle::apply::enable_opencode_server(
        "testy",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap();
    assert!(report.mcp_imported.contains(&"testy".to_string()));
    let enabled = parse_json(&opencode_dir(&tmp).join("opencode.json"));
    assert_eq!(
        enabled
            .pointer("/mcp/testy/enabled")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    let rendered = std::fs::read_to_string(opencode_dir(&tmp).join("opencode.json")).unwrap();
    assert!(
        !rendered.contains("present-not-a-secret-to-write"),
        "secret value leaked into config"
    );
    assert_eq!(
        enabled
            .pointer("/mcp/testy/environment/ROOT_S2_DEFINITELY_ABSENT_VAR")
            .and_then(|v| v.as_str()),
        Some("{env:ROOT_S2_DEFINITELY_ABSENT_VAR}")
    );
    std::env::remove_var("ROOT_S2_DEFINITELY_ABSENT_VAR");
    let err = root_agent_bundle::opencode::enable_plan("testy").unwrap_err();
    assert!(err.to_string().contains("already enabled"), "got: {}", err);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rollback_last_restores_pre_apply_bytes_and_tombstones_created_skills() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_rollback");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let agents_src = b"# Source AGENTS.md\nUse Root for installs.\n";
    let skill_md = b"# docs-writer\nWrite concise docs.\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let pre_agents = b"# Target AGENTS.md\nold prompt\n";
    let pre_config = target_opencode_json();
    let oc = opencode_dir(&tmp);
    std::fs::write(oc.join("AGENTS.md"), pre_agents).unwrap();
    std::fs::write(oc.join("opencode.json"), &pre_config).unwrap();

    let (bundle, manifest, approvals) = representative_bundle(&tmp, agents_src, skill_md, skill_sh);
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();

    let sh_path = tmp
        .join("home")
        .join(".agents")
        .join("skills")
        .join("repo-helper")
        .join("run.sh");
    assert!(sh_path.exists());

    root_agent_bundle::apply::rollback_last().unwrap();
    assert_eq!(std::fs::read(oc.join("AGENTS.md")).unwrap(), pre_agents);
    assert_eq!(
        std::fs::read(oc.join("opencode.json")).unwrap(),
        pre_config.as_bytes()
    );
    assert!(
        !sh_path.exists(),
        "created executable skill must be tombstoned"
    );
    assert!(
        !tmp.join("home")
            .join(".agents")
            .join("skills")
            .join("docs-writer")
            .join("SKILL.md")
            .exists(),
        "created markdown skill must be tombstoned"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn verify_treats_missing_imported_mcp_as_failure() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_verify");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let agents_src = b"# Source AGENTS.md\n";
    let skill_md = b"# docs-writer\n";
    let skill_sh = b"#!/bin/sh\necho repo-helper\n";
    let oc = opencode_dir(&tmp);
    std::fs::write(oc.join("AGENTS.md"), agents_src).unwrap();
    std::fs::write(
        oc.join("opencode.json"),
        serde_json::json!({ "model": "anthropic/claude-sonnet-4-5" }).to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(
        tmp.join("home")
            .join(".agents")
            .join("skills")
            .join("docs-writer"),
    )
    .unwrap();
    std::fs::create_dir_all(
        tmp.join("home")
            .join(".agents")
            .join("skills")
            .join("repo-helper"),
    )
    .unwrap();
    std::fs::write(
        tmp.join("home")
            .join(".agents")
            .join("skills")
            .join("docs-writer")
            .join("SKILL.md"),
        skill_md,
    )
    .unwrap();
    std::fs::write(
        tmp.join("home")
            .join(".agents")
            .join("skills")
            .join("repo-helper")
            .join("run.sh"),
        skill_sh,
    )
    .unwrap();

    let (bundle, manifest, _) = representative_bundle(&tmp, agents_src, skill_md, skill_sh);
    let err = root_agent_bundle::verify::verify_opencode_applied(&bundle, &manifest).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("github") || msg.to_lowercase().contains("missing"),
        "missing imported MCP must fail verify, got: {msg}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn adapter_mismatch_rejects_cross_agent_apply() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_mismatch");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    write_fake_codex(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let bytes = b"# prompt\n";
    let (oc_file, oc_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::OpenCodeHome,
        "AGENTS.md",
        bytes,
        false,
    );
    let mut opencode = root_agent_bundle::manifest::Manifest::new_for(
        root_agent_bundle::OPENCODE_ADAPTER_ID,
        "1.18.27".to_string(),
        None,
    );
    opencode
        .files
        .push(root_agent_bundle::manifest::BundleFile {
            scope: root_agent_bundle::scope::Scope::CodexHome,
            rel: "AGENTS.md".to_string(),
            sha256: root_lockfile::compute_sha256(bytes),
            size: bytes.len() as u64,
            mode: "0644".to_string(),
            kind: root_agent_bundle::manifest::FileKind::PromptContent,
            executable: false,
        });
    assert!(
        opencode.validate().is_err(),
        "OpenCode bundle must not contain CodexHome files"
    );

    let mut mixed = root_agent_bundle::manifest::Manifest::new("0.150.1".to_string(), None);
    mixed.files.push(oc_file.clone());
    assert!(
        mixed.validate().is_err(),
        "Codex bundle must not contain OpenCodeHome files"
    );

    let mut valid_opencode = root_agent_bundle::manifest::Manifest::new_for(
        root_agent_bundle::OPENCODE_ADAPTER_ID,
        "1.18.27".to_string(),
        None,
    );
    valid_opencode.files.push(oc_file);
    valid_opencode.validate().unwrap();
    let oc_bundle = tmp.join("oc-bundle");
    root_agent_bundle::blob::write_bundle_dir(&oc_bundle, &valid_opencode, &[oc_blob]).unwrap();

    let (cx_file, cx_blob, _) = file_entry(
        root_agent_bundle::scope::Scope::CodexHome,
        "AGENTS.md",
        bytes,
        false,
    );
    let mut valid_codex = root_agent_bundle::manifest::Manifest::new("0.150.1".to_string(), None);
    valid_codex.files.push(cx_file);
    valid_codex.validate().unwrap();
    let cx_bundle = tmp.join("cx-bundle");
    root_agent_bundle::blob::write_bundle_dir(&cx_bundle, &valid_codex, &[cx_blob]).unwrap();

    let oc_agents = opencode_dir(&tmp).join("AGENTS.md");
    let cx_agents = tmp.join("home").join(".codex").join("AGENTS.md");
    let plan = root_agent_bundle::plan::compute_plan(&cx_bundle, &valid_codex).unwrap();
    root_agent_bundle::apply::apply_bundle(&cx_bundle, &plan.plan_hash, &[]).unwrap();
    assert!(cx_agents.exists(), "Codex apply writes CodexHome");
    assert!(
        !oc_agents.exists(),
        "Codex bundle must not apply into OpenCode home"
    );

    let plan = root_agent_bundle::plan::compute_plan(&oc_bundle, &valid_opencode).unwrap();
    root_agent_bundle::apply::apply_bundle(&oc_bundle, &plan.plan_hash, &[]).unwrap();
    assert!(oc_agents.exists(), "OpenCode apply writes OpenCodeHome");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn jsonc_trailing_commas_apply_and_enable_path() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_jsonc");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    let oc = opencode_dir(&tmp);
    std::fs::write(
        oc.join("opencode.jsonc"),
        r#"{
  "model": "gpt-4.1",
  "target_only_experimental": "preserve-this-value",
  "server": { "port": 4096, },
}
"#,
    )
    .unwrap();
    let (bundle, manifest, approvals) = representative_bundle(
        &tmp,
        b"# Source AGENTS.md\n",
        b"# docs-writer\n",
        b"#!/bin/sh\necho repo-helper\n",
    );
    let plan = root_agent_bundle::plan::compute_plan(&bundle, &manifest).unwrap();
    root_agent_bundle::apply::apply_bundle(&bundle, &plan.plan_hash, &approvals).unwrap();
    let live = root_agent_bundle::opencode::live_config_path().unwrap();
    assert!(
        live.ends_with("opencode.jsonc"),
        "must patch existing jsonc, got {}",
        live.display()
    );
    let text = std::fs::read_to_string(&live).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["target_only_experimental"], "preserve-this-value");
    assert_eq!(parsed["mcp"]["github"]["enabled"], false);
    assert_eq!(
        parsed["mcp"]["github"]["environment"]["ROOT_S2_ACCEPT_TOKEN"],
        "{env:ROOT_S2_ACCEPT_TOKEN}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn opencode_enable_does_not_use_codex_provenance() {
    let _lock = lock_env();
    let tmp = unique_tmp("s2_provns");
    let mut env = EnvGuard::isolate(&tmp);
    write_fake_opencode(&tmp.join("bin"));
    env.prepend_path(&tmp.join("bin"));

    std::fs::write(
        opencode_dir(&tmp).join("opencode.json"),
        r#"{
  "mcp": {
    "github": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
      "enabled": false,
      "environment": { "ROOT_S2_ACCEPT_TOKEN": "{env:ROOT_S2_ACCEPT_TOKEN}" }
    }
  }
}
"#,
    )
    .unwrap();
    let plan = root_agent_bundle::opencode::enable_plan("github").unwrap();
    let mut provenance = std::collections::BTreeMap::new();
    provenance.insert(
        root_agent_bundle::journal::mcp_provenance_key("codex", "github"),
        plan.descriptor_hash.clone(),
    );
    root_agent_bundle::journal::write_journal(&root_agent_bundle::journal::ApplyJournal {
        op_id: "op_codex_github".to_string(),
        agent: "codex".to_string(),
        plan_hash: "codex-plan".to_string(),
        snapshot_id: None,
        snapshot_manifest_hash: None,
        phase: root_agent_bundle::journal::Phase::Done,
        completed_paths: vec![],
        target_preconditions: Default::default(),
        mcp_provenance: provenance,
        prior_mcp_provenance: Default::default(),
    })
    .unwrap();
    std::env::set_var("ROOT_S2_ACCEPT_TOKEN", "dummy");
    let err = root_agent_bundle::apply::enable_opencode_server(
        "github",
        &plan.plan_hash,
        std::slice::from_ref(&plan.descriptor_hash),
    )
    .unwrap_err();
    std::env::remove_var("ROOT_S2_ACCEPT_TOKEN");
    assert!(
        err.to_string().contains("no completed") && err.to_string().contains("opencode"),
        "Codex provenance must not authorize OpenCode enable, got: {}",
        err
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
