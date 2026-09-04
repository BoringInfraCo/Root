//! Claude Code adapter (S3): inspect + export builders.
//!
//! Native Rust only. Reuses `codex::find_on_path` and the shared version-probe
//! timeout/cap. Never reads `.credentials.json`, Keychain, sessions, transcripts,
//! or OAuth fields from `.claude.json`.
//!
//! v0.4.1 holds all Claude MCP. Live sentinel evidence on Claude Code 2.1.260
//! showed no disable mapping that prevented process launch from two unrelated
//! working directories. Bundles with nonempty `mcp` are invalid. Export,
//! enable-plan, and enable return `CLAUDE_MCP_HELD_ERROR`. Apply does not
//! read or write `.claude.json`.

use crate::codex::{find_on_path, HeldEntry};
use crate::manifest::{Manifest, CLAUDE_ADAPTER_ID, SUPPORTED_CLAUDE_VERSIONS};
use crate::scope::{claude_config_dir, claude_global_state_dir, scope_root, Scope};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Allowlisted passive settings for S3 (exact keys only).
pub const ALLOWED_SETTINGS: &[&str] = &["model"];

pub const CLAUDE_BINARY: &str = "claude";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const SETTINGS_JSON: &str = "settings.json";
const CLAUDE_JSON: &str = ".claude.json";

#[derive(Debug, Clone, Serialize)]
pub struct InspectReport {
    pub agent: String,
    pub present: bool,
    pub version: Option<String>,
    pub version_supported: bool,
    pub config_dir: String,
    pub global_state_dir: String,
    pub settings_present: bool,
    pub claude_md_present: bool,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub held: Vec<HeldEntry>,
}

pub fn claude_home() -> Result<PathBuf> {
    claude_config_dir()
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(claude_home()?.join(SETTINGS_JSON))
}

pub fn global_state_path() -> Result<PathBuf> {
    Ok(claude_global_state_dir()?.join(CLAUDE_JSON))
}

/// Probe `claude --version` with timeout/cap. Isolates `CLAUDE_CONFIG_DIR`
/// so a read-only probe cannot write the user's real Claude home.
pub fn probe_claude_version(binary: &Path) -> Result<String> {
    let probe_home = crate::codex::ProbeHome::create_named("root-claude-version")?;
    let isolated = probe_home.path();
    let text = crate::codex::probe_command_output(
        binary,
        &["--version"],
        10,
        4096,
        &[("CLAUDE_CONFIG_DIR", isolated.as_os_str())],
        "claude",
    )?;
    parse_claude_version(&text)
}

fn parse_claude_version(text: &str) -> Result<String> {
    // Documented: `2.1.211 (Claude Code)` or a bare `X.Y.Z`.
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let version = match tokens.as_slice() {
        [v] => *v,
        [v, "(Claude", "Code)"] => *v,
        ["claude", v] => *v,
        _ => anyhow::bail!("Unparseable claude --version output: '{}'", text),
    };
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        anyhow::bail!("Unparseable claude --version output: '{}'", text);
    }
    Ok(version.to_string())
}

/// Read-only inspect (never mutates, never requires auth).
pub fn inspect() -> Result<InspectReport> {
    let home = claude_home()?;
    let state_dir = claude_global_state_dir()?;
    let held = default_held();
    let settings = settings_path()?;
    let settings_present = settings.exists();
    let Some(bin) = find_on_path(CLAUDE_BINARY) else {
        return Ok(InspectReport {
            agent: CLAUDE_ADAPTER_ID.to_string(),
            present: false,
            version: None,
            version_supported: false,
            config_dir: home.display().to_string(),
            global_state_dir: state_dir.display().to_string(),
            settings_present,
            claude_md_present: home.join("CLAUDE.md").exists(),
            skills: vec![],
            mcp_servers: vec![],
            held,
        });
    };
    let version = probe_claude_version(&bin).ok();
    let version_supported = version
        .as_deref()
        .map(|v| SUPPORTED_CLAUDE_VERSIONS.contains(&v))
        .unwrap_or(false);
    let mcp_servers = list_user_mcp_names()?;
    let mut skills = Vec::new();
    collect_skill_names(home.join("skills"), &mut skills);
    if let Ok(root) = scope_root(Scope::SharedSkills) {
        collect_skill_names(root, &mut skills);
    }
    skills.sort();
    skills.dedup();
    Ok(InspectReport {
        agent: CLAUDE_ADAPTER_ID.to_string(),
        present: true,
        version,
        version_supported,
        config_dir: home.display().to_string(),
        global_state_dir: state_dir.display().to_string(),
        settings_present,
        claude_md_present: home.join("CLAUDE.md").exists(),
        skills,
        mcp_servers,
        held,
    })
}

fn default_held() -> Vec<HeldEntry> {
    vec![
        HeldEntry {
            source: ".credentials.json, macOS Keychain, ANTHROPIC_* tokens".to_string(),
            reason: "secret: never inspected or exported".to_string(),
        },
        HeldEntry {
            source: ".claude.json (sign-in, OAuth, projects, MCP)".to_string(),
            reason: CLAUDE_MCP_HELD_ERROR.to_string(),
        },
        HeldEntry {
            source: "user-scope, local-scope, and remote MCP".to_string(),
            reason: CLAUDE_MCP_HELD_ERROR.to_string(),
        },
        HeldEntry {
            source: "permissions, hooks, env, rules/, commands/, agents/, workflows/, plugins/"
                .to_string(),
            reason: "held by default in bundle v1 (never exported)".to_string(),
        },
        HeldEntry {
            source: "running Claude process".to_string(),
            reason: "stop claude during apply/rollback of settings.json".to_string(),
        },
    ]
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

fn load_json_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        anyhow::bail!(
            "refusing {}: symlink (bundle v1 rejects all symlinks)",
            path.display()
        );
    }
    if !meta.is_file() {
        anyhow::bail!("{} exists but is not a regular file", path.display());
    }
    if meta.len() > MAX_CONFIG_BYTES {
        anyhow::bail!("{} exceeds {} bytes", path.display(), MAX_CONFIG_BYTES);
    }
    let bytes = std::fs::read(path)?;
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not strict JSON", path.display()))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => anyhow::bail!("{} root must be a JSON object", path.display()),
    }
}

/// User-scope MCP ids only (`mcpServers` at the top of `.claude.json`).
/// Local-scope (`projects.*.mcpServers`) is held and not listed as portable.
pub fn list_user_mcp_names() -> Result<Vec<String>> {
    let path = global_state_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let obj = load_json_object(&path)?;
    let mut names = Vec::new();
    if let Some(mcp) = obj.get("mcpServers").and_then(|v| v.as_object()) {
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
    let obj = load_json_object(config_path)?;
    if let Some(s) = obj.get("model").and_then(|v| v.as_str()) {
        if s.len() <= 256 && !s.contains('\0') && !s.contains('\n') {
            out.insert("model".to_string(), Value::String(s.to_string()));
        }
    }
    Ok(out)
}

/// Stable v0.4.1 Claude MCP refusal. Export, enable-plan, enable, and
/// `Manifest::validate` all use this exact string.
pub const CLAUDE_MCP_HELD_ERROR: &str =
    "unsupported in v0.4.1 on Claude Code 2.1.260; MCP is held.";

pub fn claude_mcp_held_error() -> anyhow::Error {
    anyhow::anyhow!("{CLAUDE_MCP_HELD_ERROR}")
}

pub fn mcp_export_gated_error() -> anyhow::Error {
    claude_mcp_held_error()
}

pub fn mcp_apply_gated_error() -> anyhow::Error {
    claude_mcp_held_error()
}

/// Patch `settings.json` with allowlisted keys; preserve unknown target keys.
pub fn prepare_settings_patch(manifest: &Manifest) -> Result<(PathBuf, Vec<u8>)> {
    let path = settings_path()?;
    let mut obj = if path.exists() {
        load_json_object(&path)?
    } else {
        Map::new()
    };
    for key in ALLOWED_SETTINGS {
        if let Some(val) = manifest.settings.get(*key) {
            obj.insert((*key).to_string(), val.clone());
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(obj))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        anyhow::bail!("Patched settings.json exceeds size limit");
    }
    Ok((path, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_version_accepts_banner_and_bare_semver() {
        assert_eq!(parse_claude_version("2.1.211").unwrap(), "2.1.211");
        assert_eq!(
            parse_claude_version("2.1.211 (Claude Code)").unwrap(),
            "2.1.211"
        );
        assert!(parse_claude_version("claude 2.1").is_err());
        assert!(parse_claude_version("").is_err());
    }
}
