//! Canonical on-disk layout for Root work state.
//!
//! Canonical state lives under Root ownership (`~/.root/work/`). Root does not
//! write mutable work state into the repository working tree.

use std::path::{Path, PathBuf};

pub const INDEX_FILE: &str = "index.json";

/// `~/.root/work`
pub fn work_dir(root_dir: &Path) -> PathBuf {
    root_dir.join("work")
}

/// `~/.root/work/<workspace-id>`
pub fn workspace_dir(root_dir: &Path, workspace_id: &str) -> PathBuf {
    work_dir(root_dir).join(workspace_id)
}

/// `~/.root/work/<workspace-id>/state.db`
pub fn database_path(root_dir: &Path, workspace_id: &str) -> PathBuf {
    workspace_dir(root_dir, workspace_id).join("state.db")
}

/// `~/.root/work/<workspace-id>/exports`
pub fn exports_dir(root_dir: &Path, workspace_id: &str) -> PathBuf {
    workspace_dir(root_dir, workspace_id).join("exports")
}

/// `~/.root/work/index.json`
pub fn index_path(root_dir: &Path) -> PathBuf {
    work_dir(root_dir).join(INDEX_FILE)
}

/// `<repo>/.root/workspace.json` — opt-in project pointer.
///
/// Contains only `{ workspace_id, root_dir_hint }`: no work data, no secrets,
/// no digests. Root's canonical state still lives under `~/.root/work/`.
pub fn workspace_pointer_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".root").join("workspace.json")
}
