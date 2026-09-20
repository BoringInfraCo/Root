//! Thin coding-agent adapters for Root continuity.
//!
//! Adapters own harness identity, compatibility checks, MCP configuration,
//! Root-specific instructions, and session metadata. They do not own canonical
//! work state; Root's schema remains harness-independent.
//!
//! v0.5 deliberately supports a small matrix: Codex CLI and Claude Code.

use anyhow::{bail, Result};
use serde::Serialize;

/// Supported adapter ids, in stable order.
pub const SUPPORTED: &[&str] = &["codex", "claude"];

fn canonical_id(id: &str) -> Result<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        "codex" => Ok("codex"),
        "claude" => Ok("claude"),
        _ => bail!(
            "Unsupported adapter '{}'. Supported adapters: {}.",
            id.trim(),
            SUPPORTED.join(", ")
        ),
    }
}

fn binary_for(id: &str) -> &'static str {
    match id {
        "claude" => "claude",
        _ => "codex",
    }
}

/// Human-facing adapter name. Unknown ids are returned unchanged.
pub fn display_name(id: &str) -> String {
    match id.trim().to_ascii_lowercase().as_str() {
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        other => other.to_string(),
    }
}

/// List supported adapter ids.
pub fn list() -> Vec<&'static str> {
    SUPPORTED.to_vec()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AdapterDetection {
    pub id: String,
    pub present: bool,
    pub version: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompatibilityReport {
    pub adapter: String,
    pub present: bool,
    pub version: Option<String>,
    pub supported: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct McpConfig {
    pub adapter: String,
    pub format: String,
    pub snippet: String,
    pub instructions: Vec<String>,
}

/// Detect a supported adapter's local binary and version.
pub fn detect(id: &str) -> Result<AdapterDetection> {
    let id = canonical_id(id)?;
    Ok(detect_binary(id, binary_for(id)))
}

/// Detect an arbitrary binary by name. Exposed so tests can probe a
/// definitely-absent binary without depending on the local machine.
pub fn detect_binary(id: &str, binary: &str) -> AdapterDetection {
    let mut warnings = Vec::new();
    match std::process::Command::new(binary).arg("--version").output() {
        Ok(output) => {
            let version = parse_version(&output.stdout);
            if !output.status.success() && version.is_none() {
                warnings.push(format!(
                    "'{binary} --version' exited with status {}; version unknown.",
                    output.status
                ));
            }
            AdapterDetection {
                id: id.to_string(),
                present: true,
                version,
                warnings,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AdapterDetection {
            id: id.to_string(),
            present: false,
            version: None,
            warnings: vec![format!("not installed: '{binary}' was not found on PATH")],
        },
        Err(error) => AdapterDetection {
            id: id.to_string(),
            present: false,
            version: None,
            warnings: vec![format!("not installed: failed to run '{binary}': {error}")],
        },
    }
}

fn parse_version(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Compatibility is intentionally permissive: Root prefers warnings over hard
/// version gates and only marks an adapter unsupported for clearly
/// incompatible cases, which v0.5 does not define.
pub fn compatibility(id: &str) -> Result<CompatibilityReport> {
    let detection = detect(id)?;
    Ok(compatibility_from(&detection))
}

fn compatibility_from(detection: &AdapterDetection) -> CompatibilityReport {
    let mut warnings = detection.warnings.clone();
    if !detection.present {
        warnings.push(
            "adapter is not installed locally; the MCP configuration is provided for reference."
                .to_string(),
        );
    } else if detection.version.is_none() {
        warnings.push(
            "version could not be determined; Root will continue without a version gate."
                .to_string(),
        );
    }
    CompatibilityReport {
        adapter: detection.id.clone(),
        present: detection.present,
        version: detection.version.clone(),
        supported: true,
        warnings,
    }
}

/// MCP configuration snippet that registers Root's stdio server.
pub fn mcp_config(id: &str) -> Result<McpConfig> {
    let id = canonical_id(id)?;
    let (format, snippet, instructions) = if id == "codex" {
        (
            "toml".to_string(),
            "# ~/.codex/config.toml\n\n[mcp_servers.root]\n# Runs: root mcp serve\ncommand = \"root\"\nargs = [\"mcp\", \"serve\"]\n"
                .to_string(),
            vec![
                "Add the [mcp_servers.root] table to ~/.codex/config.toml.".to_string(),
                "Restart Codex so it loads the Root MCP server.".to_string(),
                "Verify the connection with: root mcp status".to_string(),
            ],
        )
    } else {
        (
            "shell + json".to_string(),
            "# Register Root's MCP server with Claude Code\nclaude mcp add root -- root mcp serve\n\n# or add to .mcp.json\n{\n  \"mcpServers\": {\n    \"root\": {\n      \"command\": \"root\",\n      \"args\": [\"mcp\", \"serve\"]\n    }\n  }\n}\n"
                .to_string(),
            vec![
                "Run: claude mcp add root -- root mcp serve".to_string(),
                "Alternatively add a root entry to .mcp.json.".to_string(),
                "Verify the connection with: root mcp status".to_string(),
            ],
        )
    };
    Ok(McpConfig {
        adapter: id.to_string(),
        format,
        snippet,
        instructions,
    })
}

/// Small Root-specific instructions for a supported harness. Root captures
/// durable engineering state, not chain-of-thought.
pub fn root_instructions(id: &str) -> Result<String> {
    let id = canonical_id(id)?;
    Ok(format!(
        "Root continuity instructions ({})\n\
         - Inspect Root before you start: call workspace.status and work.get_goal.\n\
         - Record durable decisions with work.record_decision when a choice must survive this session.\n\
         - Record evidence-backed findings with work.record_finding when you learn something that changes direction.\n\
         - Create a checkpoint with continuity.checkpoint before stopping or switching agents.\n\
         - Resume with continuity.resume from the latest checkpoint, or use continuity.handoff to brief another agent.\n\
         Root captures durable engineering state, not chain-of-thought or raw transcripts.",
        display_name(id)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABSENT_BINARY: &str = "root-definitely-absent-binary-xyz";

    #[test]
    fn absent_binary_is_not_present_and_warns() {
        let detection = detect_binary("codex", ABSENT_BINARY);
        assert!(!detection.present);
        assert!(detection.version.is_none());
        assert!(detection
            .warnings
            .iter()
            .any(|warning| warning.contains("not installed")));
    }

    #[test]
    fn unsupported_adapter_errors_clearly() {
        for id in ["gemini", "cursor", ""] {
            let error = detect(id).unwrap_err().to_string();
            assert!(error.contains("Unsupported adapter"), "{error}");
            assert!(mcp_config(id).is_err());
            assert!(root_instructions(id).is_err());
        }
    }

    #[test]
    fn mcp_config_registers_root_mcp_serve_for_every_adapter() {
        for id in list() {
            let config = mcp_config(id).unwrap();
            assert!(
                config.snippet.contains("root mcp serve"),
                "{} missing command: {}",
                id,
                config.snippet
            );
            assert!(!config.instructions.is_empty());
        }
    }

    #[test]
    fn root_instructions_cover_the_continuity_loop() {
        for id in list() {
            let text = root_instructions(id).unwrap().to_lowercase();
            for token in ["resume", "checkpoint", "decision", "finding"] {
                assert!(text.contains(token), "{id} missing {token}: {text}");
            }
        }
    }

    #[test]
    fn compatibility_is_permissive_for_unknown_or_old_versions() {
        let old = AdapterDetection {
            id: "codex".to_string(),
            present: true,
            version: Some("0.0.1".to_string()),
            warnings: Vec::new(),
        };
        let report = compatibility_from(&old);
        assert!(report.supported);
        assert!(report.warnings.is_empty());

        let unknown = AdapterDetection {
            id: "claude".to_string(),
            present: true,
            version: None,
            warnings: Vec::new(),
        };
        let report = compatibility_from(&unknown);
        assert!(report.supported);
        assert!(report.warnings.iter().any(|w| w.contains("version")));

        let absent = AdapterDetection {
            id: "claude".to_string(),
            present: false,
            version: None,
            warnings: vec!["not installed".to_string()],
        };
        let report = compatibility_from(&absent);
        assert!(report.supported);
        assert!(report.warnings.iter().any(|w| w.contains("not installed")));
    }

    #[test]
    fn display_names_are_friendly() {
        assert_eq!(display_name("codex"), "Codex");
        assert_eq!(display_name("claude"), "Claude Code");
    }
}
