//! Opt-in project pointer: `<repo>/.root/workspace.json`.
//!
//! The pointer is a *hint*, not canonical state. It carries exactly a workspace
//! id and an optional Root-dir hint — never work data, secrets, or digests.
//! Callers that read it fail closed when the id is unknown to `index.json`;
//! Root never auto-creates a divergent workspace from a stale pointer.
//!
//! Symlinked `.root` directories or pointer files are rejected on both read and
//! write. Writes are atomic (temp file + rename) like `registry.rs`.

use crate::paths;
use crate::registry::WorkIndex;
use crate::repository::Repository;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspacePointer {
    pub workspace_id: String,
    pub root_dir_hint: Option<String>,
}

/// Load the project pointer. A missing file is `Ok(None)`; a malformed or
/// symlinked pointer is an error (fail closed).
pub fn load(repo_root: &Path) -> Result<Option<WorkspacePointer>> {
    reject_symlinked_root_dir(repo_root)?;
    let path = paths::workspace_pointer_path(repo_root);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to stat workspace pointer {}", path.display()))
        }
    };
    if meta.file_type().is_symlink() {
        bail!(
            "Refusing workspace pointer: {} is a symlink.",
            path.display()
        );
    }
    if !meta.is_file() {
        bail!(
            "Refusing workspace pointer: {} is not a regular file.",
            path.display()
        );
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read workspace pointer {}", path.display()))?;
    let pointer: WorkspacePointer = serde_json::from_str(&content).with_context(|| {
        format!(
            "Workspace pointer is unreadable at {}.\n\n\
             Fix or remove it, then run `root restore` to bind this repository.",
            path.display()
        )
    })?;
    Ok(Some(pointer))
}

/// Write the project pointer atomically, creating `.root/` as needed.
pub fn save(repo_root: &Path, pointer: &WorkspacePointer) -> Result<()> {
    reject_symlinked_root_dir(repo_root)?;
    let dir = repo_root.join(".root");
    if dir.exists() {
        let meta = std::fs::symlink_metadata(&dir)
            .with_context(|| format!("Failed to stat {}", dir.display()))?;
        if meta.file_type().is_symlink() {
            bail!(
                "Refusing workspace pointer: {} is a symlink.",
                dir.display()
            );
        }
        if !meta.is_dir() {
            bail!(
                "Refusing workspace pointer: {} is not a directory.",
                dir.display()
            );
        }
    } else {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
    }

    let path = paths::workspace_pointer_path(repo_root);
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            bail!(
                "Refusing workspace pointer: {} is a symlink.",
                path.display()
            );
        }
        if !meta.is_file() {
            bail!(
                "Refusing workspace pointer: {} is not a regular file.",
                path.display()
            );
        }
    }
    let content =
        serde_json::to_vec_pretty(pointer).context("Failed to serialize the workspace pointer")?;
    root_lockfile::atomic_write(&path, &content)
        .with_context(|| format!("Failed to write workspace pointer at {}", path.display()))?;
    Ok(())
}

/// Repair the project pointer for `repository` by resolving the workspace
/// through the Root index (identity first, then path).
///
/// Writes a fresh pointer when a workspace is found and returns `Some(id)`;
/// returns `Ok(None)` when the repository is not bound to any workspace. This
/// is the remediation behind `root restore --rebind`: it replaces a malformed
/// or stale pointer without ever renaming or recreating work state.
pub fn rebind(root_dir: &Path, repository: &Repository) -> Result<Option<String>> {
    reject_symlinked_root_dir(&repository.root)?;
    let index = WorkIndex::load(root_dir)?;
    let repo_path = repository.root.display().to_string();
    let entry = index
        .find_by_identity(&repository.identity())
        .or_else(|| index.find_by_path(&repo_path));
    let Some(entry) = entry else {
        return Ok(None);
    };
    let pointer = WorkspacePointer {
        workspace_id: entry.id.clone(),
        root_dir_hint: Some(root_dir.display().to_string()),
    };
    save(&repository.root, &pointer)?;
    Ok(Some(entry.id.clone()))
}

fn reject_symlinked_root_dir(repo_root: &Path) -> Result<()> {
    let dir = repo_root.join(".root");
    if let Ok(meta) = std::fs::symlink_metadata(&dir) {
        if meta.file_type().is_symlink() {
            bail!(
                "Refusing workspace pointer: {} is a symlink.",
                dir.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_repo(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("root_work_pointer_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    #[test]
    fn missing_pointer_is_none() {
        let repo = temp_repo("missing");
        assert_eq!(load(&repo).unwrap(), None);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn round_trips_atomically() {
        let repo = temp_repo("roundtrip");
        let pointer = WorkspacePointer {
            workspace_id: "root_ws_TEST".to_string(),
            root_dir_hint: Some("/home/dev/.root".to_string()),
        };
        save(&repo, &pointer).unwrap();
        assert_eq!(load(&repo).unwrap(), Some(pointer));
        assert!(paths::workspace_pointer_path(&repo).exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn malformed_pointer_is_error() {
        let repo = temp_repo("malformed");
        let path = paths::workspace_pointer_path(&repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&repo).is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pointer_is_rejected() {
        use std::os::unix::fs::symlink;
        let repo = temp_repo("symlink");
        let dir = repo.join(".root");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = paths::workspace_pointer_path(&repo);
        symlink(&target, &link).unwrap();
        assert!(load(&repo).is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "root_work_pointer_root_{}_{}",
            tag,
            std::process::id()
        ))
    }

    fn bind(root: &Path, repository: &Repository, id: &str) {
        let mut index = WorkIndex::load(root).unwrap();
        index.upsert(crate::registry::WorkspaceEntry {
            id: id.to_string(),
            repo_path: repository.root.display().to_string(),
            repo_identity: repository.identity(),
        });
        index.save(root).unwrap();
    }

    #[test]
    fn rebind_repairs_malformed_pointer() {
        let repo = temp_repo("rebind_repair");
        let root = temp_root("rebind_repair");
        let repository = Repository::discover(&repo).unwrap();
        bind(&root, &repository, "root_ws_BOUND");

        let path = paths::workspace_pointer_path(&repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        let id = rebind(&root, &repository).unwrap();
        assert_eq!(id.as_deref(), Some("root_ws_BOUND"));
        assert_eq!(load(&repo).unwrap().unwrap().workspace_id, "root_ws_BOUND");

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rebind_returns_none_when_unknown() {
        let repo = temp_repo("rebind_unknown");
        let root = temp_root("rebind_unknown");
        let repository = Repository::discover(&repo).unwrap();

        assert_eq!(rebind(&root, &repository).unwrap(), None);
        assert!(!paths::workspace_pointer_path(&repo).exists());

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn rebind_refuses_symlinked_root_dir() {
        use std::os::unix::fs::symlink;
        let repo = temp_repo("rebind_symlink");
        let root = temp_root("rebind_symlink");
        let repository = Repository::discover(&repo).unwrap();
        bind(&root, &repository, "root_ws_BOUND");

        let real = repo.join("real_root");
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, repo.join(".root")).unwrap();
        assert!(rebind(&root, &repository).is_err());

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_dir_is_rejected() {
        use std::os::unix::fs::symlink;
        let repo = temp_repo("rootlink");
        let real = repo.join("real_root");
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, repo.join(".root")).unwrap();
        assert!(load(&repo).is_err());
        assert!(save(
            &repo,
            &WorkspacePointer {
                workspace_id: "root_ws_TEST".to_string(),
                root_dir_hint: None,
            }
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
