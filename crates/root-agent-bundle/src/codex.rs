//! Codex adapter (S1, read-only inspect + export builders).
//!
//! Native Rust only: PATH resolution via `split_paths`, version probe via
//! `Command::new(abs).arg("--version")` with output cap. No shell, no
//! Python, no jq. Never reads `auth.json`, sqlite, sessions, history.

use crate::manifest::{
    looks_like_literal_secret_argument, valid_env_name, valid_mcp_cwd, valid_mcp_server_id,
    MAX_ENV_NAMES, SUPPORTED_CODEX_VERSIONS,
};
use crate::scope::{scope_root, Scope};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Allowlisted passive settings for S1 (exact keys only).
pub const ALLOWED_SETTINGS: &[&str] = &["model", "model_reasoning_effort", "service_tier"];

pub const CODEX_BINARY: &str = "codex";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct InspectReport {
    pub agent: String,
    pub present: bool,
    pub version: Option<String>,
    pub version_supported: bool,
    pub codex_home: String,
    pub config_present: bool,
    pub agents_md_present: bool,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub held: Vec<HeldEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeldEntry {
    pub source: String,
    pub reason: String,
}

/// Resolve a binary on PATH without shelling out.
///
/// Iterates `split_paths`, joins, checks `symlink_metadata` (never follows),
/// requires regular file + any execute bit on Unix.
pub fn find_on_path(binary: &str) -> Option<PathBuf> {
    if binary.is_empty() || binary.contains('/') || binary.contains('\0') {
        return None;
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        // Reject relative PATH entries and `..` (defense-in-depth).
        if dir.is_relative() {
            continue;
        }
        if dir.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            continue;
        }
        let candidate = dir.join(binary);
        // Existence check without following (guards against dangling links).
        if std::fs::symlink_metadata(&candidate).is_err() {
            continue;
        }
        // Resolve symlinks (the standard Codex install is a symlink into
        // packages/standalone/...) then require a regular executable file.
        // Best-effort against TOCTOU: callers exec immediately after resolve.
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return Some(candidate);
    }
    None
}

/// Probe `codex --version` with a timeout and a real output cap.
///
/// Expected: `codex-cli X.Y.Z`. Uses `spawn` + `try_wait` polling (std has no
/// `wait_timeout`) and reads stdout through `take(CAP+1)` so a rogue binary
/// cannot force unbounded capture: exceeding the cap is an error, not silent
/// truncation. The child is killed on timeout.
pub fn probe_codex_version(binary: &Path) -> Result<String> {
    probe_version_with_timeout(binary, &["--version"], 10, 4096)
}

fn probe_version_with_timeout(
    binary: &Path,
    args: &[&str],
    timeout_secs: u64,
    cap_bytes: u64,
) -> Result<String> {
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let probe_home = ProbeHome::create()?;
    let mut child = std::process::Command::new(binary)
        .args(args)
        // Codex currently creates tmp/arg0 state even for `--version`.
        // Never allow a read-only probe to write into the user's CODEX_HOME.
        .env("CODEX_HOME", probe_home.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to execute codex --version")?;
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = stdout
            .take(cap_bytes + 1)
            .read_to_end(&mut buf)
            .map(|_| buf);
        let _ = output_tx.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut output = None;
    let status;
    loop {
        match output_rx.try_recv() {
            Ok(Ok(buf)) if buf.len() as u64 > cap_bytes => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("codex --version output exceeds {} bytes", cap_bytes);
            }
            Ok(Ok(buf)) => output = Some(buf),
            Ok(Err(e)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e).context("Failed to read child output");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Failed to read codex --version output");
            }
        }
        match child.try_wait().context("Failed to poll child")? {
            Some(completed) => {
                status = completed;
                break;
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("codex --version timed out after {}s", timeout_secs);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    if !status.success() {
        anyhow::bail!("codex --version failed with status {}", status);
    }
    let buf = match output {
        Some(buf) => buf,
        None => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match output_rx.recv_timeout(remaining) {
                Ok(Ok(buf)) => buf,
                Ok(Err(e)) => return Err(e).context("Failed to read child output"),
                Err(_) => anyhow::bail!("codex --version output read timed out"),
            }
        }
    };
    if buf.len() as u64 > cap_bytes {
        anyhow::bail!("codex --version output exceeds {} bytes", cap_bytes);
    }
    let text = String::from_utf8(buf)
        .context("codex --version output is not valid UTF-8")?
        .trim()
        .to_string();
    // Exact S1 syntax: `codex-cli X.Y.Z` (also tolerate the historical
    // `codex X.Y.Z` label). Extra lines/tokens are rejected.
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() != 2 || !matches!(tokens[0], "codex-cli" | "codex") {
        anyhow::bail!("Unparseable codex --version output: '{}'", text);
    }
    let parts: Vec<&str> = tokens[1].split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        anyhow::bail!("Unparseable codex --version output: '{}'", text);
    }
    Ok(tokens[1].to_string())
}

struct ProbeHome(PathBuf);

impl ProbeHome {
    fn create() -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..32 {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "root-codex-version-{}-{}-{}",
                std::process::id(),
                nonce,
                id
            ));
            #[cfg(unix)]
            let result = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = std::fs::DirBuilder::new();
                builder.mode(0o700).create(&path)
            };
            #[cfg(not(unix))]
            let result = std::fs::create_dir(&path);
            match result {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).context("Failed to create isolated probe directory"),
            }
        }
        anyhow::bail!("Failed to allocate isolated probe directory")
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ProbeHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn codex_home() -> Result<PathBuf> {
    scope_root(Scope::CodexHome)
}

/// Read-only inspect (never mutates, never requires auth).
pub fn inspect() -> Result<InspectReport> {
    let home = codex_home()?;
    let mut held = vec![
        HeldEntry {
            source: "auth.json".to_string(),
            reason: "secret: never inspected or exported".to_string(),
        },
        HeldEntry {
            source: "sessions/, *.sqlite*, history.jsonl, cache/, logs/".to_string(),
            reason: "volatile: never inspected or exported".to_string(),
        },
        HeldEntry {
            source: "[projects.*].trust_level, [hooks.state], installation_id".to_string(),
            reason: "machine-local trust/identity: never exported".to_string(),
        },
        HeldEntry {
            source: "/etc/codex/, requirements.toml, managed_config.toml".to_string(),
            reason: "managed/admin: never exported".to_string(),
        },
    ];
    let Some(bin) = find_on_path(CODEX_BINARY) else {
        return Ok(InspectReport {
            agent: "codex".to_string(),
            present: false,
            version: None,
            version_supported: false,
            codex_home: home.display().to_string(),
            config_present: home.join("config.toml").exists(),
            agents_md_present: home.join("AGENTS.md").exists(),
            skills: vec![],
            mcp_servers: vec![],
            held,
        });
    };
    let version = probe_codex_version(&bin).ok();
    let version_supported = version
        .as_deref()
        .map(|v| SUPPORTED_CODEX_VERSIONS.contains(&v))
        .unwrap_or(false);
    // Config keys present (names only) + MCP names (names only).
    let config_path = home.join("config.toml");
    let config_present = config_path.exists();
    let mut mcp_servers = Vec::new();
    if config_present {
        if let Ok(names) = mcp_server_names(&config_path) {
            mcp_servers = names;
        }
    }
    // Skills: names only.
    let mut skills = Vec::new();
    if let Ok(root) = scope_root(Scope::SharedSkills) {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                if let Ok(ft) = e.file_type() {
                    if ft.is_dir() {
                        if let Some(name) = e.file_name().to_str() {
                            skills.push(name.to_string());
                        }
                    }
                }
            }
            skills.sort();
        }
    }
    held.push(HeldEntry {
        source: "notify, [mcp_servers.*], [model_providers.*], [shell_environment_policy], hooks, plugins".to_string(),
        reason: "executable: requires hash-bound approval".to_string(),
    });
    Ok(InspectReport {
        agent: "codex".to_string(),
        present: true,
        version,
        version_supported,
        codex_home: home.display().to_string(),
        config_present,
        agents_md_present: home.join("AGENTS.md").exists(),
        skills,
        mcp_servers,
        held,
    })
}

/// MCP server names from config.toml (names only, no values logged).
pub fn mcp_server_names(config_path: &Path) -> Result<Vec<String>> {
    let bytes =
        read_file_capped(config_path, MAX_CONFIG_BYTES).context("Failed to read config.toml")?;
    let text = String::from_utf8(bytes).context("config.toml is not valid UTF-8")?;
    let doc: toml_edit::DocumentMut = text.parse().context("Failed to parse config.toml")?;
    let mut names = Vec::new();
    if let Some(servers) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
        for (k, _) in servers.iter() {
            names.push(k.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Read allowlisted passive settings (values validated as short plain strings).
pub fn read_allowed_settings(config_path: &Path) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut out = BTreeMap::new();
    if !config_path.exists() {
        return Ok(out);
    }
    let bytes = read_file_capped(config_path, MAX_CONFIG_BYTES)?;
    let text = String::from_utf8(bytes).context("config.toml is not valid UTF-8")?;
    let value: toml::Value = toml::from_str(&text).context("Failed to parse config.toml")?;
    if let toml::Value::Table(table) = value {
        for key in ALLOWED_SETTINGS {
            if let Some(toml::Value::String(s)) = table.get(*key) {
                if s.len() > 256 || s.contains('\0') || s.contains('\n') {
                    continue; // hold silently (not allowlisted shape)
                }
                out.insert((*key).to_string(), serde_json::Value::String(s.clone()));
            }
        }
    }
    Ok(out)
}

/// Sanitized MCP descriptor (names + structure only; secret values never read
/// into exportable form — env values are replaced by key names).
#[derive(Debug, Clone)]
pub struct SanitizedMcp {
    pub transport: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env_keys: Vec<String>,
    pub needs_env: Vec<String>,
}

pub fn sanitize_mcp_server(config_path: &Path, id: &str) -> Result<SanitizedMcp> {
    if !valid_mcp_server_id(id) {
        anyhow::bail!(
            "invalid MCP server id '{}': expected portable identifier",
            id
        );
    }
    let bytes = read_file_capped(config_path, MAX_CONFIG_BYTES)?;
    let text = String::from_utf8(bytes).context("config.toml is not valid UTF-8")?;
    let value: toml::Value = toml::from_str(&text)?;
    let table = value
        .get("mcp_servers")
        .and_then(|v| v.get(id))
        .context("MCP server not found")?;
    sanitize_mcp_value(table)
}

/// Sanitize a live `mcp_servers.<id>` TOML value: structure + env key NAMES
/// only. Secret values are never extracted.
pub fn sanitize_mcp_value(table: &toml::Value) -> Result<SanitizedMcp> {
    let table = table
        .as_table()
        .context("MCP server must be a TOML table")?;
    const ALLOWED_MCP_KEYS: &[&str] = &[
        "command",
        "args",
        "cwd",
        "env_vars",
        "env",
        "enabled",
        "transport",
    ];
    for key in table.keys() {
        if !ALLOWED_MCP_KEYS.contains(&key.as_str()) {
            anyhow::bail!(
                "MCP server uses unsupported field '{}'; refusing lossy export",
                key
            );
        }
    }
    if let Some(transport) = table.get("transport") {
        if transport.as_str() != Some("stdio") {
            anyhow::bail!("MCP server transport must be stdio");
        }
    }
    if let Some(enabled) = table.get("enabled") {
        enabled
            .as_bool()
            .context("MCP enabled field must be a boolean")?;
    }
    let command_str = table
        .get("command")
        .and_then(|v| v.as_str())
        .context("MCP stdio server must declare a string command")?;
    if command_str.is_empty()
        || command_str.len() > 1024
        || command_str.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
    {
        anyhow::bail!("MCP command is malformed");
    }
    let command = vec![command_str.to_string()];
    let mut args = Vec::new();
    if let Some(raw_args) = table.get("args") {
        let a = raw_args.as_array().context("MCP args must be an array")?;
        for item in a {
            let s = item
                .as_str()
                .context("MCP args must contain strings only")?;
            if s.is_empty()
                || s.len() > 1024
                || s.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
                || looks_like_literal_secret_argument(s)
            {
                anyhow::bail!("MCP args contain malformed or secret-bearing content");
            }
            args.push(s.to_string());
        }
        if args.len() > 64 {
            anyhow::bail!("Too many MCP args");
        }
    }
    let cwd = match table.get("cwd") {
        Some(v) => Some(v.as_str().context("MCP cwd must be a string")?.to_string()),
        None => None,
    };
    if !valid_mcp_cwd(&cwd) {
        anyhow::bail!("MCP cwd is not portable");
    }
    // Literal env tables contain values, so fail closed rather than discarding
    // them and pretending the descriptor is equivalent. Portable references
    // must use `env_vars = ["NAME"]`.
    let mut env_keys = Vec::new();
    if let Some(env) = table.get("env") {
        let env = env.as_table().context("MCP env must be a table")?;
        if !env.is_empty() {
            anyhow::bail!("MCP literal env values cannot be exported; use env_vars references");
        }
    }
    if let Some(raw_vars) = table.get("env_vars") {
        let vars = raw_vars
            .as_array()
            .context("MCP env_vars must be an array")?;
        for item in vars {
            let name = item
                .as_str()
                .context("MCP env_vars must contain strings only")?;
            if !valid_env_name(name) {
                anyhow::bail!("MCP env_vars contains invalid name '{}';", name);
            }
            env_keys.push(name.to_string());
        }
    }
    if env_keys.len() > MAX_ENV_NAMES {
        anyhow::bail!("Too many MCP env_vars entries");
    }
    env_keys.sort();
    if env_keys.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("MCP env_vars contains duplicate names");
    }
    let needs_env = env_keys.clone();
    Ok(SanitizedMcp {
        transport: "stdio".to_string(),
        command,
        args,
        cwd,
        env_keys,
        needs_env,
    })
}

/// Read-only enable preflight for one MCP server (no lock, no writes).
///
/// Returns the canonical descriptor hash of the LIVE entry plus a plan hash
/// binding `{server, descriptor_hash, config_precondition}`. `enable` must
/// present both the plan hash (freshness) and the descriptor hash (approval).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnablePlan {
    pub server: String,
    pub descriptor_hash: String,
    pub config_precondition: String,
    pub needs_env: Vec<String>,
    pub plan_hash: String,
}

pub fn enable_plan(server_id: &str) -> Result<EnablePlan> {
    if !valid_mcp_server_id(server_id) {
        anyhow::bail!("invalid MCP server id");
    }
    let home = codex_home()?;
    let config_path = home.join("config.toml");
    let bytes =
        read_file_capped(&config_path, MAX_CONFIG_BYTES).context("config.toml not found")?;
    let live_hash = root_lockfile::compute_sha256(&bytes);
    let text = String::from_utf8(bytes).context("config.toml is not valid UTF-8")?;
    let value: toml::Value = toml::from_str(&text).context("Failed to parse config.toml")?;
    let table = value
        .get("mcp_servers")
        .and_then(|v| v.get(server_id))
        .context("MCP server not found in config.toml")?;
    if table
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        anyhow::bail!("MCP server '{}' is already enabled", server_id);
    }
    let san = sanitize_mcp_value(table)?;
    let descriptor_hash =
        crate::manifest::mcp_command_hash(&san.command, &san.args, &san.cwd, &san.env_keys);
    let config_precondition = format!("sha256:{}", live_hash);
    let canonical = serde_json::json!({
        "server": server_id,
        "descriptor_hash": descriptor_hash,
        "config_precondition": config_precondition,
    });
    let plan_hash = root_lockfile::compute_sha256(
        &serde_json::to_vec(&canonical).context("Failed to serialize enable plan")?,
    );
    Ok(EnablePlan {
        server: server_id.to_string(),
        descriptor_hash,
        config_precondition,
        needs_env: san.needs_env,
        plan_hash,
    })
}

fn read_file_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        anyhow::bail!("config.toml exceeds size limit");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_on_path_rejects_bad_names() {
        assert!(find_on_path("").is_none());
        assert!(find_on_path("a/b").is_none());
    }

    #[test]
    fn find_on_path_accepts_absolute_dirs_and_symlinks() {
        // Regression test: absolute PATH entries (which contain RootDir)
        // must be searched, and symlinked binaries must resolve.
        let dir =
            std::env::temp_dir().join(format!("root_agent_bundle_path_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let real = dir.join("probe-bin");
        std::fs::write(&real, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink(&real, dir.join("probe-link")).unwrap();
        }
        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        let found_bin = find_on_path("probe-bin");
        let found_link = find_on_path("probe-link");
        match old {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        assert!(found_bin.is_some(), "absolute PATH dir must be searched");
        #[cfg(unix)]
        assert!(found_link.is_some(), "symlinked binary must resolve");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_probe_rejects_oversize_output() {
        let dir =
            std::env::temp_dir().join(format!("root_agent_bundle_probe_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("bigver.sh");
        // 5KB of output exceeds the 4KB cap → error, not silent truncation.
        std::fs::write(
            &script,
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 5000 ]; do printf x; i=$((i + 1)); done\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let err = probe_version_with_timeout(&script, &[], 10, 4096).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "got: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_probe_times_out() {
        let dir =
            std::env::temp_dir().join(format!("root_agent_bundle_probe_t_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("slowver.sh");
        // Shell builtins only: another test temporarily narrows PATH while
        // exercising binary discovery, so depending on `sleep` is racy.
        std::fs::write(&script, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let err = probe_version_with_timeout(&script, &[], 1, 4096).unwrap_err();
        assert!(err.to_string().contains("timed out"), "got: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowed_settings_keys_only() {
        let dir = std::env::temp_dir().join(format!(
            "root_agent_bundle_codex_test_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "model = \"gpt-x\"\nnotify = [\"/binevil\"]\n[mcp_servers.y]\ncommand = \"npx\"\n",
        )
        .unwrap();
        let settings = read_allowed_settings(&path).unwrap();
        assert!(settings.contains_key("model"));
        assert!(!settings.contains_key("notify"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_probe_isolates_codex_home() {
        let dir = std::env::temp_dir().join(format!(
            "root_agent_bundle_probe_home_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("side-effect.sh");
        let user_home = dir.join("user-codex-home");
        std::fs::create_dir_all(&user_home).unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nmkdir -p \"$CODEX_HOME/tmp/arg0\"\ntouch \"$CODEX_HOME/tmp/arg0/residue\"\nif [ \"$CODEX_HOME\" = \"{}\" ]; then touch \"{}/leaked\"; fi\nprintf 'codex-cli 0.150.1\\n'\n",
                user_home.display(),
                user_home.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let version = probe_version_with_timeout(&script, &[], 10, 4096).unwrap();
        assert_eq!(version, "0.150.1");
        assert!(
            !user_home.join("leaked").exists(),
            "read-only probe must not pass the user's CODEX_HOME to Codex"
        );
        assert!(
            !user_home.join("tmp").exists(),
            "read-only probe must not leave vendor residue in user config"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitizer_rejects_remote_unknown_and_literal_secret_forms() {
        let remote: toml::Value = toml::from_str(
            "url = \"https://example.invalid/mcp\"\nbearer_token_env_var = \"TOKEN\"\n",
        )
        .unwrap();
        assert!(sanitize_mcp_value(&remote).is_err());

        let literal_env: toml::Value =
            toml::from_str("command = \"server\"\n[env]\nTOKEN = \"literal\"\n").unwrap();
        assert!(sanitize_mcp_value(&literal_env).is_err());

        let secret_arg: toml::Value =
            toml::from_str("command = \"server\"\nargs = [\"--api-key=literal\"]\n").unwrap();
        assert!(sanitize_mcp_value(&secret_arg).is_err());
    }

    #[test]
    fn sanitizer_accepts_strict_stdio_with_env_references() {
        let value: toml::Value = toml::from_str(
            "command = \"npx\"\nargs = [\"-y\", \"server-package\"]\nenv_vars = [\"API_TOKEN\"]\nenabled = true\ntransport = \"stdio\"\n",
        )
        .unwrap();
        let sanitized = sanitize_mcp_value(&value).unwrap();
        assert_eq!(sanitized.command, vec!["npx"]);
        assert_eq!(sanitized.needs_env, vec!["API_TOKEN"]);
    }
}
