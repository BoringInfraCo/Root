//! Post-apply verification (read-only, secret-safe).
//!
//! Banned: merged-config dumps (`opencode debug config` equivalents), `mcp
//! get --json` (unredacted env), unbounded output capture. Allowed: bounded
//! `--version` probe, TOML/JSON parse checks, allowlist shape checks, hash
//! comparisons, redacted `mcp list` names.

use crate::codex::{find_on_path, probe_codex_version, CODEX_BINARY};
use crate::manifest::{Manifest, SUPPORTED_CODEX_VERSIONS};
use crate::scope::resolve_target;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub agent: String,
    pub success: bool,
    pub version: Option<String>,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Standalone verify (no bundle needed): binary + version + config parse.
pub fn verify_codex() -> Result<VerifyReport> {
    let mut checks = Vec::new();
    let bin = find_on_path(CODEX_BINARY);
    let mut version = None;
    match bin {
        None => checks.push(Check {
            name: "binary_present".to_string(),
            passed: false,
            detail: "codex not found on PATH".to_string(),
        }),
        Some(path) => {
            checks.push(Check {
                name: "binary_present".to_string(),
                passed: true,
                detail: path.display().to_string(),
            });
            match probe_codex_version(&path) {
                Ok(v) => {
                    let supported = SUPPORTED_CODEX_VERSIONS.contains(&v.as_str());
                    checks.push(Check {
                        name: "version_supported".to_string(),
                        passed: supported,
                        detail: format!("{} (supported: {:?})", v, SUPPORTED_CODEX_VERSIONS),
                    });
                    version = Some(v);
                }
                Err(e) => checks.push(Check {
                    name: "version_supported".to_string(),
                    passed: false,
                    detail: format!("{}", e),
                }),
            }
        }
    }
    // Config parse check (no values logged).
    let home = crate::codex::codex_home()?;
    let config = home.join("config.toml");
    if config.exists() {
        match std::fs::read(&config) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                let text = String::from_utf8_lossy(&bytes);
                match text.parse::<toml_edit::DocumentMut>() {
                    Ok(_) => checks.push(Check {
                        name: "config_parses".to_string(),
                        passed: true,
                        detail: "config.toml parses".to_string(),
                    }),
                    Err(e) => checks.push(Check {
                        name: "config_parses".to_string(),
                        passed: false,
                        detail: format!("config.toml parse error: {}", e),
                    }),
                }
            }
            _ => checks.push(Check {
                name: "config_parses".to_string(),
                passed: false,
                detail: "config.toml unreadable or oversized".to_string(),
            }),
        }
    } else {
        checks.push(Check {
            name: "config_parses".to_string(),
            passed: true,
            detail: "no config.toml (absent is valid)".to_string(),
        });
    }
    let success = checks.iter().all(|c| c.passed);
    Ok(VerifyReport {
        agent: "codex".to_string(),
        success,
        version,
        checks,
    })
}

/// Post-apply verification: standalone checks + bundle hash agreement +
/// MCP-disabled invariant for imported servers.
pub fn verify_codex_applied(bundle_dir: &Path, manifest: &Manifest) -> Result<()> {
    let report = verify_codex()?;
    if !report.success {
        anyhow::bail!(
            "verification failed: post-apply checks did not pass ({:?})",
            report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        );
    }
    // Hash agreement for file blobs.
    for f in &manifest.files {
        let target = resolve_target(f.scope, &f.rel)?;
        let live = std::fs::read(&target)
            .with_context(|| format!("verification failed: {} missing after apply", f.rel))?;
        let digest = root_lockfile::compute_sha256(&live);
        if digest != f.sha256.to_lowercase() && digest != f.sha256 {
            anyhow::bail!(
                "verification failed: hash mismatch for '{}' after apply",
                f.rel
            );
        }
    }
    // MCP-disabled invariant.
    let home = crate::codex::codex_home()?;
    let config = home.join("config.toml");
    if !manifest.mcp.is_empty() {
        let text =
            std::fs::read_to_string(&config).context("verification failed: config.toml missing")?;
        let doc: toml_edit::DocumentMut = text
            .parse()
            .context("verification failed: config.toml unparsable")?;
        for id in manifest.mcp.keys() {
            let server = doc.get("mcp_servers").and_then(|v| v.get(id));
            let Some(server) = server else {
                anyhow::bail!(
                    "verification failed: MCP server '{}' missing after apply",
                    id
                );
            };
            let enabled = server
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if enabled {
                anyhow::bail!(
                    "verification failed: MCP server '{}' is enabled; bundles must apply disabled",
                    id
                );
            }
        }
    }
    let _ = bundle_dir;
    Ok(())
}

/// Post-enable verification: server exists and is enabled, config parses.
pub fn verify_codex_enabled(id: &str) -> Result<()> {
    let home = crate::codex::codex_home()?;
    let text = std::fs::read_to_string(home.join("config.toml"))
        .context("verification failed: config.toml missing")?;
    let doc: toml_edit::DocumentMut = text
        .parse()
        .context("verification failed: config.toml unparsable")?;
    let enabled = doc
        .get("mcp_servers")
        .and_then(|v| v.get(id))
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        anyhow::bail!(
            "verification failed: server '{}' not enabled after enable",
            id
        );
    }
    Ok(())
}
