//! OpenCode adapter (S2, read-only inspect + export builders + JSONC patch).
//!
//! Native Rust only. Reuses `codex::find_on_path` and the shared version-probe
//! timeout/cap. Never reads auth tokens, `mcp-auth.json`, sessions, sqlite, or
//! logs. Never calls `opencode debug config`.

use crate::codex::{find_on_path, EnablePlan, HeldEntry, SanitizedMcp};
use crate::manifest::{
    looks_like_literal_secret_argument, valid_env_name, valid_mcp_server_id, Manifest,
    MAX_ENV_NAMES, OPENCODE_ADAPTER_ID, SUPPORTED_OPENCODE_VERSIONS,
};
use crate::scope::{opencode_config_dir, scope_root, Scope};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Allowlisted passive settings for S2 (exact keys only).
pub const ALLOWED_SETTINGS: &[&str] = &["model"];

pub const OPENCODE_BINARY: &str = "opencode";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CONFIG_JSON: &str = "opencode.json";
const CONFIG_JSONC: &str = "opencode.jsonc";

#[derive(Debug, Clone, Serialize)]
pub struct InspectReport {
    pub agent: String,
    pub present: bool,
    pub version: Option<String>,
    pub version_supported: bool,
    pub config_dir: String,
    pub config_present: bool,
    pub agents_md_present: bool,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub held: Vec<HeldEntry>,
}

pub fn opencode_home() -> Result<PathBuf> {
    opencode_config_dir()
}

/// Live config path: `opencode.json` if present, else `opencode.jsonc`.
/// Missing files default to `opencode.json` (create-on-apply).
pub fn live_config_path() -> Result<PathBuf> {
    let dir = opencode_home()?;
    let json = dir.join(CONFIG_JSON);
    let jsonc = dir.join(CONFIG_JSONC);
    match std::fs::symlink_metadata(&json) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("refusing opencode.json: symlink (bundle v1 rejects all symlinks)");
        }
        Ok(meta) if meta.is_file() => return Ok(json),
        Ok(_) => anyhow::bail!("opencode.json exists but is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to stat opencode.json"),
    }
    match std::fs::symlink_metadata(&jsonc) {
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("refusing opencode.jsonc: symlink (bundle v1 rejects all symlinks)");
        }
        Ok(meta) if meta.is_file() => Ok(jsonc),
        Ok(_) => anyhow::bail!("opencode.jsonc exists but is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json),
        Err(e) => Err(e).context("Failed to stat opencode.jsonc"),
    }
}

pub fn config_rel() -> Result<String> {
    let path = live_config_path()?;
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .context("opencode config file name is not UTF-8")
}

/// Probe `opencode --version` with timeout/cap. Isolates config dirs so a
/// read-only probe cannot write the user's real OpenCode config.
pub fn probe_opencode_version(binary: &Path) -> Result<String> {
    let probe_home = crate::codex::ProbeHome::create_named("root-opencode-version")?;
    let isolated = probe_home.path();
    let text = crate::codex::probe_command_output(
        binary,
        &["--version"],
        10,
        4096,
        &[
            ("OPENCODE_CONFIG_DIR", isolated.as_os_str()),
            ("XDG_CONFIG_HOME", isolated.as_os_str()),
            ("XDG_DATA_HOME", isolated.as_os_str()),
            ("OPENCODE_DISABLE_AUTOUPDATE", std::ffi::OsStr::new("1")),
        ],
        "opencode",
    )?;
    parse_opencode_version(&text)
}

fn parse_opencode_version(text: &str) -> Result<String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let version = match tokens.as_slice() {
        [v] => *v,
        ["opencode", v] => *v,
        _ => anyhow::bail!("Unparseable opencode --version output: '{}'", text),
    };
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        anyhow::bail!("Unparseable opencode --version output: '{}'", text);
    }
    Ok(version.to_string())
}

/// Read-only inspect (never mutates, never requires auth).
pub fn inspect() -> Result<InspectReport> {
    let home = opencode_home()?;
    let mut held = vec![
        HeldEntry {
            source: "auth tokens, ~/.local/share/opencode/mcp-auth.json".to_string(),
            reason: "secret: never inspected or exported".to_string(),
        },
        HeldEntry {
            source: "sessions/, *.sqlite*, logs/, share state".to_string(),
            reason: "volatile: never inspected or exported".to_string(),
        },
        HeldEntry {
            source: "$schema, provider, remote MCP, unknown config fields".to_string(),
            reason: "held by default in bundle v1 (never exported)".to_string(),
        },
    ];
    let config_path = live_config_path()?;
    let config_present = config_path.exists();
    let Some(bin) = find_on_path(OPENCODE_BINARY) else {
        return Ok(InspectReport {
            agent: OPENCODE_ADAPTER_ID.to_string(),
            present: false,
            version: None,
            version_supported: false,
            config_dir: home.display().to_string(),
            config_present,
            agents_md_present: home.join("AGENTS.md").exists(),
            skills: vec![],
            mcp_servers: vec![],
            held,
        });
    };
    let version = probe_opencode_version(&bin).ok();
    let version_supported = version
        .as_deref()
        .map(|v| SUPPORTED_OPENCODE_VERSIONS.contains(&v))
        .unwrap_or(false);
    let mut mcp_servers = Vec::new();
    if config_present {
        mcp_servers = mcp_server_names(&config_path).with_context(|| {
            format!(
                "opencode config present but unreadable: {}",
                config_path.display()
            )
        })?;
    }
    let mut skills = Vec::new();
    collect_skill_names(home.join("skills"), &mut skills);
    if let Ok(root) = scope_root(Scope::SharedSkills) {
        collect_skill_names(root, &mut skills);
    }
    skills.sort();
    skills.dedup();
    held.push(HeldEntry {
        source: "mcp.*.command, executable skill files".to_string(),
        reason: "executable: requires hash-bound approval".to_string(),
    });
    Ok(InspectReport {
        agent: OPENCODE_ADAPTER_ID.to_string(),
        present: true,
        version,
        version_supported,
        config_dir: home.display().to_string(),
        config_present,
        agents_md_present: home.join("AGENTS.md").exists(),
        skills,
        mcp_servers,
        held,
    })
}

fn collect_skill_names(root: PathBuf, skills: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for e in entries.flatten() {
        if let Ok(ft) = e.file_type() {
            if ft.is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    skills.push(name.to_string());
                }
            }
        }
    }
}

pub fn mcp_server_names(config_path: &Path) -> Result<Vec<String>> {
    let value = load_config_value(config_path)?;
    let mut names = Vec::new();
    if let Some(mcp) = value.get("mcp").and_then(|v| v.as_object()) {
        names.extend(mcp.keys().cloned());
    }
    names.sort();
    Ok(names)
}

pub fn read_allowed_settings(config_path: &Path) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    if !config_path.exists() {
        return Ok(out);
    }
    let value = load_config_value(config_path)?;
    if let Some(s) = value.get("model").and_then(|v| v.as_str()) {
        if s.len() <= 256 && !s.contains('\0') && !s.contains('\n') {
            out.insert("model".to_string(), Value::String(s.to_string()));
        }
    }
    Ok(out)
}

pub fn sanitize_mcp_server(config_path: &Path, id: &str) -> Result<SanitizedMcp> {
    if !valid_mcp_server_id(id) {
        anyhow::bail!(
            "invalid MCP server id '{}': expected portable identifier",
            id
        );
    }
    let value = load_config_value(config_path)?;
    let table = value
        .get("mcp")
        .and_then(|v| v.get(id))
        .context("MCP server not found")?;
    sanitize_mcp_value(table)
}

/// Sanitize a live `mcp.<id>` JSON value: local stdio only. `{env:NAME}`
/// values become env key names; literal environment values are refused.
pub fn sanitize_mcp_value(value: &Value) -> Result<SanitizedMcp> {
    let table = value
        .as_object()
        .context("MCP server must be a JSON object")?;
    const ALLOWED_MCP_KEYS: &[&str] = &["type", "command", "enabled", "environment"];
    for key in table.keys() {
        if !ALLOWED_MCP_KEYS.contains(&key.as_str()) {
            anyhow::bail!(
                "MCP server uses unsupported field '{}'; refusing lossy export",
                key
            );
        }
    }
    match table.get("type").and_then(|v| v.as_str()) {
        Some("local") => {}
        Some(other) => anyhow::bail!(
            "MCP server type '{}' is not local; refusing export of remote/oauth/url MCP",
            other
        ),
        None => anyhow::bail!("MCP server type must be \"local\" (maps to stdio)"),
    }
    if let Some(enabled) = table.get("enabled") {
        enabled
            .as_bool()
            .context("MCP enabled field must be a boolean")?;
    }
    let command_arr = table
        .get("command")
        .and_then(|v| v.as_array())
        .context("MCP local server must declare a command array")?;
    if command_arr.is_empty() {
        anyhow::bail!("MCP command array must not be empty");
    }
    let mut parts = Vec::new();
    for item in command_arr {
        let s = item
            .as_str()
            .context("MCP command must contain strings only")?;
        if s.is_empty() || s.len() > 1024 || s.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
            anyhow::bail!("MCP command is malformed");
        }
        parts.push(s.to_string());
    }
    let command = vec![parts[0].clone()];
    let args = parts[1..].to_vec();
    if args
        .iter()
        .any(|arg| looks_like_literal_secret_argument(arg))
    {
        anyhow::bail!("MCP args contain malformed or secret-bearing content");
    }
    if args.len() > 64 {
        anyhow::bail!("Too many MCP args");
    }
    let mut env_keys = Vec::new();
    if let Some(env) = table.get("environment") {
        let env = env
            .as_object()
            .context("MCP environment must be a JSON object")?;
        for (key, val) in env {
            if !valid_env_name(key) {
                anyhow::bail!("MCP environment contains invalid name '{}'", key);
            }
            let raw = val
                .as_str()
                .context("MCP environment values must be strings")?;
            match parse_env_ref(raw) {
                Some(name) if name == key => env_keys.push(key.clone()),
                Some(_) => anyhow::bail!(
                    "MCP environment key '{}' does not match its {{env:NAME}} reference",
                    key
                ),
                None => anyhow::bail!(
                    "MCP literal env values cannot be exported; use {{env:NAME}} references"
                ),
            }
        }
    }
    if env_keys.len() > MAX_ENV_NAMES {
        anyhow::bail!("Too many MCP environment entries");
    }
    env_keys.sort();
    if env_keys.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("MCP environment contains duplicate names");
    }
    let needs_env = env_keys.clone();
    Ok(SanitizedMcp {
        transport: "stdio".to_string(),
        command,
        args,
        cwd: None,
        env_keys,
        needs_env,
    })
}

fn parse_env_ref(value: &str) -> Option<&str> {
    let rest = value.strip_prefix("{env:")?;
    let name = rest.strip_suffix('}')?;
    if valid_env_name(name) {
        Some(name)
    } else {
        None
    }
}

pub fn enable_plan(server_id: &str) -> Result<EnablePlan> {
    if !valid_mcp_server_id(server_id) {
        anyhow::bail!("invalid MCP server id");
    }
    let config_path = live_config_path()?;
    let bytes =
        read_file_capped(&config_path, MAX_CONFIG_BYTES).context("opencode config not found")?;
    let live_hash = root_lockfile::compute_sha256(&bytes);
    let text = String::from_utf8(bytes).context("opencode config is not valid UTF-8")?;
    let value = parse_jsonc(&text)?;
    let table = value
        .get("mcp")
        .and_then(|v| v.get(server_id))
        .context("MCP server not found in opencode config")?;
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

/// Strip comments and trailing commas outside JSON strings, then parse.
pub fn parse_jsonc(text: &str) -> Result<Value> {
    let stripped = strip_jsonc_comments(text)?;
    let stripped = strip_trailing_commas(&stripped);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(trimmed).context("Failed to parse OpenCode JSONC")
}

fn strip_jsonc_comments(input: &str) -> Result<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() {
            if chars[i + 1] == '/' {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if chars[i + 1] == '*' {
                i += 2;
                let mut found = false;
                while i + 1 < chars.len() {
                    if chars[i] == '*' && chars[i + 1] == '/' {
                        i += 2;
                        found = true;
                        break;
                    }
                    i += 1;
                }
                if !found {
                    anyhow::bail!("unterminated block comment in JSONC");
                }
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    if in_string {
        anyhow::bail!("unterminated string in JSONC");
    }
    Ok(out)
}

fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn load_config_value(path: &Path) -> Result<Value> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("refusing config: symlink (bundle v1 rejects all symlinks)");
    }
    if !meta.is_file() {
        anyhow::bail!("opencode config must be a regular file");
    }
    let bytes = read_file_capped(path, MAX_CONFIG_BYTES)?;
    let text = String::from_utf8(bytes).context("opencode config is not valid UTF-8")?;
    parse_jsonc(&text)
}

pub fn render_pretty_json(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).context("Failed to serialize JSON config")?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    if bytes.len() > MAX_CONFIG_BYTES as usize {
        anyhow::bail!("Patched opencode config exceeds size limit");
    }
    Ok(bytes)
}

/// Patch allowlisted settings + disabled local MCP entries onto a live JSON
/// object. Unknown target keys (including `$schema` and unrelated MCP servers)
/// are preserved.
pub fn patch_config_value(value: &mut Value, manifest: &Manifest) -> Result<()> {
    let obj = value
        .as_object_mut()
        .context("opencode config must be a JSON object")?;
    if let Some(model) = manifest.settings.get("model").and_then(|v| v.as_str()) {
        obj.insert("model".to_string(), Value::String(model.to_string()));
    }
    if manifest.mcp.is_empty() {
        return Ok(());
    }
    if !obj.contains_key("mcp") {
        obj.insert("mcp".to_string(), json!({}));
    }
    let mcp = obj
        .get_mut("mcp")
        .and_then(|v| v.as_object_mut())
        .context("target mcp is not an object; refusing to patch")?;
    for (id, entry) in &manifest.mcp {
        if entry.enabled {
            anyhow::bail!("Refusing to apply enabled MCP entry '{}'", id);
        }
        if !valid_mcp_server_id(id) {
            anyhow::bail!("invalid MCP server id '{}'", id);
        }
        let mut server = match mcp.get(id) {
            Some(Value::Object(existing)) => existing.clone(),
            Some(_) => anyhow::bail!("target mcp '{}' is not an object; refusing to patch", id),
            None => Map::new(),
        };
        server.insert("type".to_string(), json!("local"));
        let mut command = entry.command.clone();
        command.extend(entry.args.iter().cloned());
        server.insert("command".to_string(), json!(command));
        server.insert("enabled".to_string(), json!(false));
        if entry.env_keys.is_empty() {
            server.remove("environment");
        } else {
            let mut env = Map::new();
            for k in &entry.env_keys {
                env.insert(k.clone(), Value::String(format!("{{env:{}}}", k)));
            }
            server.insert("environment".to_string(), Value::Object(env));
        }
        mcp.insert(id.clone(), Value::Object(server));
    }
    Ok(())
}

pub fn set_mcp_enabled(value: &mut Value, id: &str, enabled: bool) -> Result<()> {
    let server = value
        .get_mut("mcp")
        .and_then(|v| v.as_object_mut())
        .and_then(|mcp| mcp.get_mut(id))
        .context("MCP server not found")?;
    let obj = server
        .as_object_mut()
        .context("MCP server must be a JSON object")?;
    obj.insert("enabled".to_string(), json!(enabled));
    Ok(())
}

fn read_file_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        anyhow::bail!("opencode config exceeds size limit");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{mcp_command_hash, McpEntry, NeedsApproval, OPENCODE_ADAPTER_ID};
    use crate::scope::Scope;

    #[test]
    fn parse_opencode_version_accepts_exact_and_labeled() {
        assert_eq!(parse_opencode_version("1.18.27").unwrap(), "1.18.27");
        assert_eq!(
            parse_opencode_version("opencode 1.18.27").unwrap(),
            "1.18.27"
        );
        assert!(parse_opencode_version("opencode 1.18.27 extra").is_err());
        assert!(parse_opencode_version("v1.18.27").is_err());
        assert!(parse_opencode_version("1.18").is_err());
    }

    #[test]
    fn jsonc_parse_strips_comments_and_preserves_unknown_keys() {
        let text = r#"
        {
          // line comment
          "$schema": "https://example.invalid/schema.json",
          "model": "gpt-x",
          "experimental": { "keep": true },
          /* block comment */
          "mcp": {
            "other": { "type": "remote", "url": "https://example.invalid", "enabled": true },
            "github": {
              "type": "local",
              "command": ["npx", "-y", "pkg"],
              "enabled": true,
              "timeout": 30
            }
          }
        }
        "#;
        let mut value = parse_jsonc(text).unwrap();
        assert_eq!(value["$schema"], "https://example.invalid/schema.json");
        assert_eq!(value["model"], "gpt-x");
        assert_eq!(value["experimental"]["keep"], true);

        let command = vec!["npx".to_string()];
        let args = vec!["-y".to_string(), "pkg".to_string()];
        let env = vec!["API_TOKEN".to_string()];
        let hash = mcp_command_hash(&command, &args, &None, &env);
        let mut manifest = Manifest::new_for(OPENCODE_ADAPTER_ID, "1.18.27".to_string(), None);
        manifest
            .settings
            .insert("model".to_string(), json!("gpt-y"));
        manifest.mcp.insert(
            "github".to_string(),
            McpEntry {
                transport: "stdio".to_string(),
                enabled: false,
                needs_env: env.clone(),
                command_sha256: Some(hash.clone()),
                command,
                args,
                cwd: None,
                env_keys: env.clone(),
            },
        );
        manifest.needs_env = env;
        manifest.needs_approval.push(NeedsApproval {
            scope: Scope::OpenCodeHome,
            rel: "opencode.json#mcp.github".to_string(),
            sha256: hash,
            reason: "MCP command".to_string(),
        });
        patch_config_value(&mut value, &manifest).unwrap();
        assert_eq!(value["$schema"], "https://example.invalid/schema.json");
        assert_eq!(value["experimental"]["keep"], true);
        assert_eq!(value["model"], "gpt-y");
        assert_eq!(value["mcp"]["other"]["url"], "https://example.invalid");
        assert_eq!(value["mcp"]["other"]["enabled"], true);
        assert_eq!(value["mcp"]["github"]["timeout"], 30);
        assert_eq!(value["mcp"]["github"]["enabled"], false);
        assert_eq!(value["mcp"]["github"]["type"], "local");
        assert_eq!(
            value["mcp"]["github"]["environment"]["API_TOKEN"],
            "{env:API_TOKEN}"
        );
        assert_eq!(
            value["mcp"]["github"]["command"],
            json!(["npx", "-y", "pkg"])
        );
    }

    #[test]
    fn jsonc_does_not_strip_comment_markers_inside_strings() {
        let text = r#"{ "model": "foo // bar", "note": "/* keep */" }"#;
        let v = parse_jsonc(text).unwrap();
        assert_eq!(v["model"], "foo // bar");
        assert_eq!(v["note"], "/* keep */");
    }

    #[test]
    fn jsonc_accepts_trailing_commas() {
        let text = r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-sonnet-4-5",
  "autoupdate": true,
  "server": { "port": 4096, },
  "mcp": {
    "github": {
      "type": "local",
      "command": ["npx", "-y", "pkg"],
      "enabled": true,
    },
  },
}"#;
        let v = parse_jsonc(text).unwrap();
        assert_eq!(v["model"], "anthropic/claude-sonnet-4-5");
        assert_eq!(v["server"]["port"], 4096);
        assert_eq!(v["mcp"]["github"]["enabled"], true);
        assert_eq!(v["mcp"]["github"]["command"][0], "npx");
    }

    #[test]
    fn sanitizer_accepts_local_env_refs_and_command_only() {
        let with_env = json!({
            "type": "local",
            "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
            "enabled": true,
            "environment": {
                "GITHUB_PERSONAL_ACCESS_TOKEN": "{env:GITHUB_PERSONAL_ACCESS_TOKEN}"
            }
        });
        let sanitized = sanitize_mcp_value(&with_env).unwrap();
        assert_eq!(sanitized.command, vec!["npx"]);
        assert_eq!(
            sanitized.args,
            vec!["-y", "@modelcontextprotocol/server-github"]
        );
        assert_eq!(sanitized.needs_env, vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]);
        assert_eq!(sanitized.transport, "stdio");

        let command_only = json!({
            "type": "local",
            "command": ["npx"]
        });
        let sanitized = sanitize_mcp_value(&command_only).unwrap();
        assert!(sanitized.needs_env.is_empty());
        assert!(sanitized.args.is_empty());
    }

    #[test]
    fn sanitizer_refuses_literal_env_and_remote() {
        let literal = json!({
            "type": "local",
            "command": ["npx"],
            "environment": { "TOKEN": "secret-value" }
        });
        assert!(sanitize_mcp_value(&literal).is_err());

        let remote = json!({
            "type": "remote",
            "url": "https://example.invalid/mcp"
        });
        assert!(sanitize_mcp_value(&remote).is_err());

        let headers = json!({
            "type": "local",
            "command": ["npx"],
            "headers": { "Authorization": "Bearer x" }
        });
        assert!(sanitize_mcp_value(&headers).is_err());

        let command_string = json!({
            "type": "local",
            "command": "npx"
        });
        assert!(sanitize_mcp_value(&command_string).is_err());
    }

    #[test]
    fn version_probe_isolates_opencode_config_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "root_agent_bundle_opencode_probe_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("side-effect.sh");
        let user_home = dir.join("user-opencode-home");
        std::fs::create_dir_all(&user_home).unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nmkdir -p \"$OPENCODE_CONFIG_DIR/residue\"\nmkdir -p \"$XDG_CONFIG_HOME/opencode-xdg\"\nif [ \"$OPENCODE_CONFIG_DIR\" = \"{}\" ] || [ \"$XDG_CONFIG_HOME\" = \"{}\" ]; then touch \"{}/leaked\"; fi\nprintf '1.18.27\\n'\n",
                user_home.display(),
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
        let version = probe_opencode_version(&script).unwrap();
        assert_eq!(version, "1.18.27");
        assert!(
            !user_home.join("leaked").exists(),
            "read-only probe must not pass the user's OpenCode config dir"
        );
        assert!(
            !user_home.join("residue").exists(),
            "read-only probe must not leave vendor residue in user config"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
