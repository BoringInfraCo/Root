//! Local capability policy for the MCP interface.
//!
//! Policy is enforced server-side. An absent configuration grants every
//! capability; an explicit `deny` removes one.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CAP_READ: &str = "read";
pub const CAP_RECORD: &str = "record";
pub const CAP_CHECKPOINT: &str = "checkpoint";
pub const CAP_ENVIRONMENT_VERIFY: &str = "environment_verify";

pub const CAPABILITIES: &[&str] = &[CAP_READ, CAP_RECORD, CAP_CHECKPOINT, CAP_ENVIRONMENT_VERIFY];

const ALLOW: &str = "allow";
const DENY: &str = "deny";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CapabilitiesView {
    pub read: String,
    pub record: String,
    pub checkpoint: String,
    pub environment_verify: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    read: bool,
    record: bool,
    checkpoint: bool,
    environment_verify: bool,
    source: String,
}

impl Policy {
    pub fn default_allow() -> Self {
        Self {
            read: true,
            record: true,
            checkpoint: true,
            environment_verify: true,
            source: "default".to_string(),
        }
    }

    pub fn load_at(root_dir: &Path) -> Self {
        let path = root_dir.join("mcp.toml");
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return Self::default_allow(),
        };
        let config = match toml::from_str::<McpFile>(&content) {
            Ok(config) => config,
            Err(_) => return Self::default_allow(),
        };
        let mut policy = Self::default_allow();
        policy.source = "configured".to_string();
        if let Some(value) = config.capabilities.read {
            policy.read = value == ALLOW;
        }
        if let Some(value) = config.capabilities.record {
            policy.record = value == ALLOW;
        }
        if let Some(value) = config.capabilities.checkpoint {
            policy.checkpoint = value == ALLOW;
        }
        if let Some(value) = config.capabilities.environment_verify {
            policy.environment_verify = value == ALLOW;
        }
        policy
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn allowed(&self, capability: &str) -> bool {
        match capability {
            CAP_READ => self.read,
            CAP_RECORD => self.record,
            CAP_CHECKPOINT => self.checkpoint,
            CAP_ENVIRONMENT_VERIFY => self.environment_verify,
            _ => false,
        }
    }

    pub fn view(&self) -> CapabilitiesView {
        CapabilitiesView {
            read: label(self.read),
            record: label(self.record),
            checkpoint: label(self.checkpoint),
            environment_verify: label(self.environment_verify),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::default_allow()
    }
}

fn label(allowed: bool) -> String {
    if allowed { ALLOW } else { DENY }.to_string()
}

#[derive(Debug, Deserialize, Default)]
struct McpFile {
    #[serde(default)]
    capabilities: CapabilityConfig,
}

#[derive(Debug, Deserialize, Default)]
struct CapabilityConfig {
    read: Option<String>,
    record: Option<String>,
    checkpoint: Option<String>,
    environment_verify: Option<String>,
}

/// Resolve the Root state directory, honoring `ROOT_DIR` for isolation.
pub fn root_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("ROOT_DIR") {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME").context("Could not determine home directory")?;
    Ok(PathBuf::from(home).join(".root"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("root_mcp_policy_{}_{}", tag, std::process::id()))
    }

    #[test]
    fn missing_file_allows_everything() {
        let dir = temp("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let policy = Policy::load_at(&dir);
        assert!(policy.allowed(CAP_READ));
        assert!(policy.allowed(CAP_RECORD));
        assert!(policy.allowed(CAP_CHECKPOINT));
        assert!(policy.allowed(CAP_ENVIRONMENT_VERIFY));
        assert_eq!(policy.source(), "default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_deny_is_enforced() {
        let dir = temp("deny");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mcp.toml"),
            "[capabilities]\nread = \"allow\"\nrecord = \"deny\"\n",
        )
        .unwrap();
        let policy = Policy::load_at(&dir);
        assert!(policy.allowed(CAP_READ));
        assert!(!policy.allowed(CAP_RECORD));
        assert!(policy.allowed(CAP_CHECKPOINT));
        assert_eq!(policy.source(), "configured");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
