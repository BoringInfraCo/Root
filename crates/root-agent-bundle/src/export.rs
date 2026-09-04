//! Codex export: builds manifest + blobs from live configuration.
//!
//! Read-only against live config except for writing the output bundle dir.
//! Unknown source fields are HELD (never exported). Prompt/skill files are
//! copied verbatim with the secret disclosure (no claim of secret-free).

use crate::blob::write_bundle_dir;
use crate::codex::{
    codex_home, find_on_path, probe_codex_version, read_allowed_settings, sanitize_mcp_server,
    CODEX_BINARY,
};
use crate::manifest::{
    mcp_approval_target, BundleFile, FileKind, HeldItem, Manifest, McpEntry, NeedsApproval,
    ADAPTER_SCHEMA_VERSION, BUNDLE_VERSION, MAX_FILES, MAX_FILE_BYTES, OPENCODE_ADAPTER_ID,
};
use crate::scope::{scope_root, validate_rel, Scope};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct ExportOptions {
    pub skills: Vec<String>,
    pub include_mcp: Vec<String>,
    /// Skill/MCP ids whose executable content may be included. Hash-bound
    /// approval still happens at apply time via --approve <sha256>.
    pub include_executable: Vec<String>,
    /// Omit timestamp for deterministic bundles.
    pub no_timestamp: bool,
}

fn utc_now_rfc3339() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s-since-epoch", now.as_secs())
}

fn valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut prev_dash = false;
    for b in bytes {
        let ok = b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-';
        if !ok {
            return false;
        }
        if *b == b'-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
        } else {
            prev_dash = false;
        }
    }
    true
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("md")
}

/// Collect skill files (bounded walk, no symlinks, no dirs escape).
fn collect_skill_files(skill_root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![skill_root.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let entries =
            std::fs::read_dir(&cur).with_context(|| format!("Cannot list {}", cur.display()))?;
        for entry in entries {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                anyhow::bail!(
                    "Refusing to export symlink {} (bundle v1 rejects all symlinks)",
                    entry.path().display()
                );
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                out.push(entry.path());
            }
            if out.len() + stack.len() > MAX_FILES {
                anyhow::bail!("Skill exceeds file-count limit");
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn export_codex(bundle_dir: &Path, opts: &ExportOptions) -> Result<Manifest> {
    // 1. Live version gate (exact versions only).
    let bin = find_on_path(CODEX_BINARY)
        .context("codex executable not found on PATH; refusing export")?;
    let version = probe_codex_version(&bin)?;
    if !crate::manifest::SUPPORTED_CODEX_VERSIONS.contains(&version.as_str()) {
        anyhow::bail!(
            "unsupported source agent version '{}'. S1 accepts exact versions {:?} only",
            version,
            crate::manifest::SUPPORTED_CODEX_VERSIONS
        );
    }
    let home = codex_home()?;
    let created = if opts.no_timestamp {
        None
    } else {
        Some(utc_now_rfc3339())
    };
    let mut manifest = Manifest::new(version, created);
    manifest.bundle_version = BUNDLE_VERSION;
    manifest.adapter_schema_version = ADAPTER_SCHEMA_VERSION;
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen_blob: BTreeSet<String> = BTreeSet::new();

    // 2. AGENTS.md (prompt-content, verbatim + disclosure).
    let agents = home.join("AGENTS.md");
    match std::fs::symlink_metadata(&agents) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            manifest.held.push(HeldItem {
                source: "$CODEX_HOME/AGENTS.md (absent)".to_string(),
                reason: "absent on source".to_string(),
            });
        }
        Err(e) => return Err(e).context("Failed to stat AGENTS.md"),
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!(
                    "Refusing to export symlink AGENTS.md (bundle v1 rejects all symlinks)"
                );
            }
            let bytes = std::fs::read(&agents)?;
            if (bytes.len() as u64) > MAX_FILE_BYTES {
                anyhow::bail!("AGENTS.md exceeds per-file limit");
            }
            let digest = root_lockfile::compute_sha256(&bytes);
            if !seen_blob.contains(&digest) {
                seen_blob.insert(digest.clone());
                blobs.push((digest.clone(), bytes.clone()));
            }
            manifest.files.push(BundleFile {
                scope: Scope::CodexHome,
                rel: "AGENTS.md".to_string(),
                sha256: digest,
                size: bytes.len() as u64,
                mode: "0644".to_string(),
                kind: FileKind::PromptContent,
                executable: false,
            });
        }
    }

    // 3. Allowlisted settings.
    let config_path = home.join("config.toml");
    if config_path.exists() {
        let settings = read_allowed_settings(&config_path)?;
        manifest.settings = settings;
    }
    manifest.held.push(HeldItem {
        source: "unknown source fields".to_string(),
        reason: "held by default in bundle v1 (never exported)".to_string(),
    });
    manifest.held.push(HeldItem {
        source: "auth.json, keychain, sessions/, *.sqlite*, history.jsonl, cache/, logs/"
            .to_string(),
        reason: "secret/volatile: excluded".to_string(),
    });
    manifest.held.push(HeldItem {
        source:
            "[projects.*] trust, [hooks.state], installation_id, /etc/codex/, managed_config.toml"
                .to_string(),
        reason: "machine-local/managed: excluded".to_string(),
    });
    manifest.held.push(HeldItem {
        source: "notify, hooks.json, [model_providers.*], [shell_environment_policy], plugins/, marketplaces".to_string(),
        reason: "executable or trust-gated: excluded unless explicitly included".to_string(),
    });

    // 4. Skills.
    let skills_root = scope_root(Scope::SharedSkills)?;
    for skill in &opts.skills {
        if !valid_skill_name(skill) {
            anyhow::bail!("invalid skill name '{}'", skill);
        }
        validate_rel(skill)?;
        let skill_dir = skills_root.join(skill);
        let meta = std::fs::symlink_metadata(&skill_dir)
            .with_context(|| format!("Skill '{}' not found", skill))?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            anyhow::bail!("Skill '{}' is not a directory (symlinks rejected)", skill);
        }
        let files = collect_skill_files(&skill_dir)?;
        if files.is_empty() {
            anyhow::bail!("Skill '{}' is empty", skill);
        }
        let exec_allowed = opts.include_executable.iter().any(|e| e == skill);
        for abs in files {
            let rel_in_skill = abs
                .strip_prefix(&skill_dir)
                .context("Skill path escape")?
                .to_str()
                .context("Non-UTF8 skill path")?
                .to_string();
            let rel = format!("{}/{}", skill, rel_in_skill);
            validate_rel(&rel)?;
            let bytes = std::fs::read(&abs)?;
            if (bytes.len() as u64) > MAX_FILE_BYTES {
                anyhow::bail!("Skill file '{}' exceeds per-file limit", rel);
            }
            let executable = !is_markdown(&abs);
            if executable && !exec_allowed {
                manifest.held.push(HeldItem {
                    source: format!("skill file {}", rel),
                    reason: "executable content requires --include-executable <skill>".to_string(),
                });
                continue;
            }
            let digest = root_lockfile::compute_sha256(&bytes);
            if !seen_blob.contains(&digest) {
                seen_blob.insert(digest.clone());
                blobs.push((digest.clone(), bytes.clone()));
            }
            if executable {
                manifest.needs_approval.push(NeedsApproval {
                    scope: Scope::SharedSkills,
                    rel: rel.clone(),
                    sha256: digest.clone(),
                    reason: format!("executable skill file {}", rel),
                });
            }
            manifest.files.push(BundleFile {
                scope: Scope::SharedSkills,
                rel,
                sha256: digest,
                size: bytes.len() as u64,
                mode: if executable {
                    "0755".to_string()
                } else {
                    "0644".to_string()
                },
                kind: if executable {
                    FileKind::Executable
                } else {
                    FileKind::PromptContent
                },
                executable,
            });
        }
    }

    // 5. MCP (sanitized, disabled, hash-bound approval).
    for id in &opts.include_mcp {
        if id.is_empty() || id.len() > 128 {
            anyhow::bail!("invalid MCP server id");
        }
        let san = sanitize_mcp_server(&config_path, id)?;
        let cmd_hash =
            crate::manifest::mcp_command_hash(&san.command, &san.args, &san.cwd, &san.env_keys);
        let mut needs_env = san.needs_env.clone();
        needs_env.sort();
        needs_env.dedup();
        manifest.mcp.insert(
            id.clone(),
            McpEntry {
                transport: san.transport,
                enabled: false,
                needs_env: needs_env.clone(),
                command_sha256: Some(cmd_hash.clone()),
                command: san.command.clone(),
                args: san.args.clone(),
                cwd: san.cwd.clone(),
                env_keys: san.env_keys.clone(),
            },
        );
        manifest.needs_approval.push(NeedsApproval {
            scope: Scope::CodexHome,
            rel: format!("config.toml#mcp_servers.{}", id),
            sha256: cmd_hash,
            reason: format!("MCP stdio command for server '{}'", id),
        });
        for env in needs_env {
            if !manifest.needs_env.contains(&env) {
                manifest.needs_env.push(env);
            }
        }
        manifest.needs_env.sort();
    }

    manifest.validate()?;
    write_bundle_dir(bundle_dir, &manifest, &blobs)?;
    Ok(manifest)
}

pub fn export_opencode(bundle_dir: &Path, opts: &ExportOptions) -> Result<Manifest> {
    let bin = find_on_path(crate::opencode::OPENCODE_BINARY)
        .context("opencode executable not found on PATH; refusing export")?;
    let version = crate::opencode::probe_opencode_version(&bin)?;
    if !crate::manifest::SUPPORTED_OPENCODE_VERSIONS.contains(&version.as_str()) {
        anyhow::bail!(
            "unsupported source agent version '{}'. S2 accepts exact OpenCode versions {:?} only",
            version,
            crate::manifest::SUPPORTED_OPENCODE_VERSIONS
        );
    }
    let home = crate::opencode::opencode_home()?;
    let created = if opts.no_timestamp {
        None
    } else {
        Some(utc_now_rfc3339())
    };
    let mut manifest = Manifest::new_for(OPENCODE_ADAPTER_ID, version, created);
    manifest.bundle_version = BUNDLE_VERSION;
    manifest.adapter_schema_version = ADAPTER_SCHEMA_VERSION;
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen_blob: BTreeSet<String> = BTreeSet::new();

    let agents = home.join("AGENTS.md");
    match std::fs::symlink_metadata(&agents) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            manifest.held.push(HeldItem {
                source: "opencode AGENTS.md (absent)".to_string(),
                reason: "absent on source".to_string(),
            });
        }
        Err(e) => return Err(e).context("Failed to stat AGENTS.md"),
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!(
                    "Refusing to export symlink AGENTS.md (bundle v1 rejects all symlinks)"
                );
            }
            let bytes = std::fs::read(&agents)?;
            if (bytes.len() as u64) > MAX_FILE_BYTES {
                anyhow::bail!("AGENTS.md exceeds per-file limit");
            }
            let digest = root_lockfile::compute_sha256(&bytes);
            if !seen_blob.contains(&digest) {
                seen_blob.insert(digest.clone());
                blobs.push((digest.clone(), bytes.clone()));
            }
            manifest.files.push(BundleFile {
                scope: Scope::OpenCodeHome,
                rel: "AGENTS.md".to_string(),
                sha256: digest,
                size: bytes.len() as u64,
                mode: "0644".to_string(),
                kind: FileKind::PromptContent,
                executable: false,
            });
        }
    }

    let config_path = crate::opencode::live_config_path()?;
    if config_path.exists() {
        let settings = crate::opencode::read_allowed_settings(&config_path)?;
        manifest.settings = settings;
    }
    manifest.held.push(HeldItem {
        source: "unknown source fields".to_string(),
        reason: "held by default in bundle v1 (never exported)".to_string(),
    });
    manifest.held.push(HeldItem {
        source: "auth tokens, ~/.local/share/opencode/mcp-auth.json, sessions/, *.sqlite*, logs/"
            .to_string(),
        reason: "secret/volatile: excluded".to_string(),
    });
    manifest.held.push(HeldItem {
        source: "$schema, provider, remote MCP, unknown config keys".to_string(),
        reason: "held by default in bundle v1 (never exported)".to_string(),
    });

    let native_skills = home.join("skills");
    let shared_skills = scope_root(Scope::SharedSkills)?;
    for skill in &opts.skills {
        if !valid_skill_name(skill) {
            anyhow::bail!("invalid skill name '{}'", skill);
        }
        validate_rel(skill)?;
        let (scope, skill_dir, rel_prefix) = {
            let native = native_skills.join(skill);
            match std::fs::symlink_metadata(&native) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    (Scope::OpenCodeHome, native, format!("skills/{}", skill))
                }
                Ok(_) => anyhow::bail!("Skill '{}' is not a directory (symlinks rejected)", skill),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let shared = shared_skills.join(skill);
                    let meta = std::fs::symlink_metadata(&shared)
                        .with_context(|| format!("Skill '{}' not found", skill))?;
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        anyhow::bail!("Skill '{}' is not a directory (symlinks rejected)", skill);
                    }
                    (Scope::SharedSkills, shared, skill.clone())
                }
                Err(e) => return Err(e).context(format!("Failed to stat skill '{}'", skill)),
            }
        };
        let files = collect_skill_files(&skill_dir)?;
        if files.is_empty() {
            anyhow::bail!("Skill '{}' is empty", skill);
        }
        let exec_allowed = opts.include_executable.iter().any(|e| e == skill);
        for abs in files {
            let rel_in_skill = abs
                .strip_prefix(&skill_dir)
                .context("Skill path escape")?
                .to_str()
                .context("Non-UTF8 skill path")?
                .to_string();
            let rel = format!("{}/{}", rel_prefix, rel_in_skill);
            validate_rel(&rel)?;
            let bytes = std::fs::read(&abs)?;
            if (bytes.len() as u64) > MAX_FILE_BYTES {
                anyhow::bail!("Skill file '{}' exceeds per-file limit", rel);
            }
            let executable = !is_markdown(&abs);
            if executable && !exec_allowed {
                manifest.held.push(HeldItem {
                    source: format!("skill file {}", rel),
                    reason: "executable content requires --include-executable <skill>".to_string(),
                });
                continue;
            }
            let digest = root_lockfile::compute_sha256(&bytes);
            if !seen_blob.contains(&digest) {
                seen_blob.insert(digest.clone());
                blobs.push((digest.clone(), bytes.clone()));
            }
            if executable {
                manifest.needs_approval.push(NeedsApproval {
                    scope,
                    rel: rel.clone(),
                    sha256: digest.clone(),
                    reason: format!("executable skill file {}", rel),
                });
            }
            manifest.files.push(BundleFile {
                scope,
                rel,
                sha256: digest,
                size: bytes.len() as u64,
                mode: if executable {
                    "0755".to_string()
                } else {
                    "0644".to_string()
                },
                kind: if executable {
                    FileKind::Executable
                } else {
                    FileKind::PromptContent
                },
                executable,
            });
        }
    }

    for id in &opts.include_mcp {
        if id.is_empty() || id.len() > 128 {
            anyhow::bail!("invalid MCP server id");
        }
        let san = crate::opencode::sanitize_mcp_server(&config_path, id)?;
        let cmd_hash =
            crate::manifest::mcp_command_hash(&san.command, &san.args, &san.cwd, &san.env_keys);
        let mut needs_env = san.needs_env.clone();
        needs_env.sort();
        needs_env.dedup();
        manifest.mcp.insert(
            id.clone(),
            McpEntry {
                transport: san.transport,
                enabled: false,
                needs_env: needs_env.clone(),
                command_sha256: Some(cmd_hash.clone()),
                command: san.command.clone(),
                args: san.args.clone(),
                cwd: san.cwd.clone(),
                env_keys: san.env_keys.clone(),
            },
        );
        let (scope, rel) = mcp_approval_target(OPENCODE_ADAPTER_ID, id)?;
        manifest.needs_approval.push(NeedsApproval {
            scope,
            rel,
            sha256: cmd_hash,
            reason: format!("MCP stdio command for server '{}'", id),
        });
        for env in needs_env {
            if !manifest.needs_env.contains(&env) {
                manifest.needs_env.push(env);
            }
        }
        manifest.needs_env.sort();
    }

    manifest.validate()?;
    write_bundle_dir(bundle_dir, &manifest, &blobs)?;
    Ok(manifest)
}
