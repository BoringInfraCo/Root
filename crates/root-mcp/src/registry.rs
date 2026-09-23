//! Built-in capability registry.
//!
//! Tool discovery reads this registry. The v0.6 workspace, work, continuity,
//! and environment tools are the first registered capabilities. Later
//! connectors register beside them; they are not hard-coded into the session
//! loop.

use crate::tools::{self, ToolDef};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RegisteredCapability {
    pub name: String,
    pub namespace: String,
    pub capability: String,
    pub description: String,
}

/// Capabilities currently registered for discovery and `tools/list`.
pub fn registered() -> Vec<RegisteredCapability> {
    tools::definitions().into_iter().map(register).collect()
}

pub fn find(name: &str) -> Option<RegisteredCapability> {
    registered().into_iter().find(|tool| tool.name == name)
}

pub fn require(name: &str) -> anyhow::Result<RegisteredCapability> {
    find(name).ok_or_else(|| anyhow::anyhow!("unknown capability '{name}'"))
}

fn register(tool: ToolDef) -> RegisteredCapability {
    RegisteredCapability {
        namespace: namespace_of(tool.name).to_string(),
        name: tool.name.to_string(),
        capability: tool.capability.to_string(),
        description: tool.description.to_string(),
    }
}

fn namespace_of(name: &str) -> &str {
    name.split_once('.')
        .map(|(namespace, _)| namespace)
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tools_are_namespaced_and_unique() {
        let tools = registered();
        assert!(tools.len() >= 13);
        let mut names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique);
        assert!(tools.iter().any(|tool| {
            tool.name == "work.record_decision"
                && tool.namespace == "work"
                && tool.capability == "record"
        }));
        assert!(tools.iter().any(|tool| tool.namespace == "continuity"));
        assert!(tools.iter().any(|tool| tool.namespace == "environment"));
        assert!(tools
            .iter()
            .all(|tool| tool.name.starts_with(&format!("{}.", tool.namespace))));
    }
}
