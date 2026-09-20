//! Workspace discovery metadata: a local registry under Root ownership.
//!
//! The registry (`~/.root/work/index.json`) maps workspace IDs to observable
//! repository facts. Root deliberately does not write mutable metadata into the
//! repository working tree, so initializing a workspace never dirties Git.

use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const METADATA_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceEntry {
    pub id: String,
    pub repo_path: String,
    pub repo_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkIndex {
    pub schema: u32,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}

impl Default for WorkIndex {
    fn default() -> Self {
        Self {
            schema: METADATA_SCHEMA,
            workspaces: Vec::new(),
        }
    }
}

impl WorkIndex {
    pub fn load(root_dir: &Path) -> Result<Self> {
        let path = paths::index_path(root_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read workspace index at {}", path.display()))?;
        let index: WorkIndex = serde_json::from_str(&content).with_context(|| {
            format!(
                "Workspace metadata is unreadable at {}.\n\n\
                 The Root work index is corrupt. Remove it and re-run `root workspace init`.",
                path.display()
            )
        })?;
        Ok(index)
    }

    pub fn save(&self, root_dir: &Path) -> Result<()> {
        let work = paths::work_dir(root_dir);
        std::fs::create_dir_all(&work)
            .with_context(|| format!("Failed to create {}", work.display()))?;
        let path = paths::index_path(root_dir);
        let content = serde_json::to_vec_pretty(self)?;
        root_lockfile::atomic_write(&path, &content)
            .with_context(|| format!("Failed to write workspace index at {}", path.display()))?;
        Ok(())
    }

    pub fn find_by_identity(&self, identity: &str) -> Option<&WorkspaceEntry> {
        self.workspaces
            .iter()
            .find(|entry| entry.repo_identity == identity)
    }

    pub fn find_by_path(&self, repo_path: &str) -> Option<&WorkspaceEntry> {
        self.workspaces
            .iter()
            .find(|entry| entry.repo_path == repo_path)
    }

    pub fn upsert(&mut self, entry: WorkspaceEntry) {
        if let Some(existing) = self.workspaces.iter_mut().find(|e| e.id == entry.id) {
            *existing = entry;
        } else {
            self.workspaces.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_and_finds_entries() {
        let root = std::env::temp_dir().join(format!("root_work_index_{}", std::process::id()));
        let index = WorkIndex {
            schema: METADATA_SCHEMA,
            workspaces: vec![WorkspaceEntry {
                id: "root_ws_TEST".into(),
                repo_path: "/tmp/repo".into(),
                repo_identity: "path:/tmp/repo".into(),
            }],
        };
        index.save(&root).unwrap();

        let loaded = WorkIndex::load(&root).unwrap();
        assert_eq!(loaded.workspaces.len(), 1);
        assert!(loaded.find_by_identity("path:/tmp/repo").is_some());
        assert!(loaded.find_by_path("/tmp/repo").is_some());
        assert!(loaded.find_by_identity("missing").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_index_is_empty() {
        let root = std::env::temp_dir().join(format!("root_work_noindex_{}", std::process::id()));
        let loaded = WorkIndex::load(&root).unwrap();
        assert!(loaded.workspaces.is_empty());
    }
}
