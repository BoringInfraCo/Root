//! Plan: target precondition hashes + canonical plan hash.
//!
//! `plan` is read-only and MUST NOT acquire the mutation lock.
//! `apply` revalidates the plan hash under the lock and reports drift.

use crate::manifest::{manifest_hash, Manifest};
use crate::scope::resolve_target;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct PlanReport {
    pub agent: String,
    pub bundle_hash: String,
    pub plan_hash: String,
    pub target_preconditions: BTreeMap<String, String>,
    pub will_create: Vec<String>,
    pub will_update: Vec<String>,
    pub will_keep: Vec<String>,
    /// Preview of `config.toml` key changes (allowlisted settings only;
    /// values are non-secret model identifiers).
    pub settings_changes: Vec<SettingChange>,
    /// Preview of MCP declarations to add (disabled; commands shown for
    /// informed hash-bound approval — never secret values).
    pub mcp_to_add: Vec<McpPreview>,
    pub held: Vec<HeldOut>,
    pub needs_env: Vec<String>,
    pub needs_approval: Vec<ApprovalOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingChange {
    pub key: String,
    pub old: Option<String>,
    pub new: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpPreview {
    pub id: String,
    pub transport: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeldOut {
    pub source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalOut {
    pub target: String,
    pub sha256: String,
    pub reason: String,
}

/// Precondition descriptor per target: `missing` | `sha256:<hex>` (+ mode).
pub fn target_precondition(scope: crate::scope::Scope, rel: &str) -> Result<(String, String)> {
    let key = format!("{}:{}", scope.as_str(), rel);
    let target = resolve_target(scope, rel)?;
    let meta = std::fs::symlink_metadata(&target);
    match meta {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((key, "missing".to_string())),
        Err(e) => Err(e.into()),
        Ok(m) => {
            if m.file_type().is_symlink() {
                return Ok((key, "symlink:held".to_string()));
            }
            if m.is_dir() {
                return Ok((key, "dir:present".to_string()));
            }
            let digest = crate::manifest::hash_file_capped(&target, 16 * 1024 * 1024)?;
            Ok((key, format!("sha256:{}", digest)))
        }
    }
}

pub fn compute_plan(_bundle_dir: &Path, manifest: &Manifest) -> Result<PlanReport> {
    let bundle_hash = manifest_hash(manifest)?;
    let mut preconditions = BTreeMap::new();
    let mut will_create = Vec::new();
    let mut will_update = Vec::new();
    let mut will_keep = Vec::new();
    // Config.toml patch is computed, not blob-compared: precondition on the
    // live config file hash.
    for f in &manifest.files {
        let (key, state) = target_precondition(f.scope, &f.rel)?;
        preconditions.insert(key.clone(), state.clone());
        let target = resolve_target(f.scope, &f.rel)?;
        let label = format!("{}:{}", f.scope.as_str(), f.rel);
        if state == "missing" {
            will_create.push(label);
            continue;
        }
        if state == "symlink:held" {
            anyhow::bail!(
                "Refusing to plan write through symlink target '{}'; bundle v1 rejects symlinks",
                label
            );
        }
        if state == "dir:present" {
            anyhow::bail!(
                "Refusing to replace directory target '{}' with a bundle file",
                label
            );
        }
        // Compare live bytes to blob bytes when possible.
        let live_hash = state.strip_prefix("sha256:").unwrap_or("");
        if live_hash.eq_ignore_ascii_case(&f.sha256) {
            will_keep.push(label);
            continue;
        }
        let _ = target;
        will_update.push(label);
    }
    // Settings/MCP changes ride along with the adapter config file; surface as
    // update when present.
    if !manifest.settings.is_empty() || !manifest.mcp.is_empty() {
        let (cfg_scope, cfg_rel) = config_target_for(&manifest.adapter)?;
        let label = format!("{}:{}", cfg_scope.as_str(), cfg_rel);
        let key = label.clone();
        if !preconditions.contains_key(&key) {
            let (k, state) = target_precondition(cfg_scope, &cfg_rel)?;
            if state == "symlink:held" {
                anyhow::bail!(
                    "Refusing to plan config patch through symlink target '{}'; bundle v1 rejects symlinks",
                    label
                );
            }
            if state == "dir:present" {
                anyhow::bail!("Refusing to replace directory target '{}'", label);
            }
            if state == "missing" {
                will_create.push(label.clone());
            } else {
                // Config updates are structural patches. Conservatively
                // surface them even when the resulting bytes may be identical;
                // apply revalidates this exact target precondition under lock.
                will_update.push(label.clone());
            }
            preconditions.insert(k, state);
        }
    }
    // Config change preview (read-only; values are allowlisted non-secrets).
    let mut settings_changes = Vec::new();
    let mut mcp_to_add = Vec::new();
    if !manifest.settings.is_empty() || !manifest.mcp.is_empty() {
        let (live_settings, live_mcp) = live_config_preview(&manifest.adapter)?;
        for (key, new_val) in &manifest.settings {
            let new_str = new_val.as_str().unwrap_or("").to_string();
            let old = live_settings
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if old.as_deref() != Some(new_str.as_str()) {
                settings_changes.push(SettingChange {
                    key: key.clone(),
                    old,
                    new: new_str,
                });
            }
        }
        for (id, entry) in &manifest.mcp {
            mcp_to_add.push(McpPreview {
                id: id.clone(),
                transport: entry.transport.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                exists: live_mcp.iter().any(|n| n == id),
            });
        }
        mcp_to_add.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let plan_hash = plan_hash_for(&bundle_hash, &preconditions)?;
    Ok(PlanReport {
        agent: manifest.adapter.clone(),
        bundle_hash,
        plan_hash,
        target_preconditions: preconditions,
        will_create,
        will_update,
        will_keep,
        settings_changes,
        mcp_to_add,
        held: manifest
            .held
            .iter()
            .map(|h| HeldOut {
                source: h.source.clone(),
                reason: h.reason.clone(),
            })
            .collect(),
        needs_env: manifest.needs_env.clone(),
        needs_approval: manifest
            .needs_approval
            .iter()
            .map(|a| ApprovalOut {
                target: format!("{}:{}", a.scope.as_str(), a.rel),
                sha256: a.sha256.clone(),
                reason: a.reason.clone(),
            })
            .collect(),
    })
}

fn config_target_for(adapter: &str) -> Result<(crate::scope::Scope, String)> {
    match adapter {
        crate::manifest::ADAPTER_ID => {
            Ok((crate::scope::Scope::CodexHome, "config.toml".to_string()))
        }
        crate::manifest::OPENCODE_ADAPTER_ID => Ok((
            crate::scope::Scope::OpenCodeHome,
            crate::opencode::config_rel()?,
        )),
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

fn live_config_preview(
    adapter: &str,
) -> Result<(
    std::collections::BTreeMap<String, serde_json::Value>,
    Vec<String>,
)> {
    match adapter {
        crate::manifest::ADAPTER_ID => {
            let home = crate::scope::scope_root(crate::scope::Scope::CodexHome)?;
            let config_path = home.join("config.toml");
            Ok((
                crate::codex::read_allowed_settings(&config_path).unwrap_or_default(),
                crate::codex::mcp_server_names(&config_path).unwrap_or_default(),
            ))
        }
        crate::manifest::OPENCODE_ADAPTER_ID => {
            let config_path = crate::opencode::live_config_path()?;
            Ok((
                crate::opencode::read_allowed_settings(&config_path).unwrap_or_default(),
                crate::opencode::mcp_server_names(&config_path).unwrap_or_default(),
            ))
        }
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

pub fn plan_hash_for(
    bundle_hash: &str,
    preconditions: &BTreeMap<String, String>,
) -> Result<String> {
    let canonical = serde_json::json!({
        "bundle_hash": bundle_hash,
        "target_preconditions": preconditions,
    });
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(root_lockfile::compute_sha256(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_hash_stable_and_sensitive() {
        let mut a = BTreeMap::new();
        a.insert("k".to_string(), "missing".to_string());
        let mut b = a.clone();
        b.insert("k".to_string(), "sha256:abc".to_string());
        let h1 = plan_hash_for("bh", &a).unwrap();
        let h2 = plan_hash_for("bh", &a).unwrap();
        let h3 = plan_hash_for("bh", &b).unwrap();
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }
}
