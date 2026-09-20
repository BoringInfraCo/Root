//! Git repository discovery and observable repository facts.
//!
//! Root never mutates Git. This module only walks the filesystem and, when
//! available, reads facts through the `git` CLI. A repository without a
//! remote or without commits must still work.

use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repository {
    /// Absolute path to the Git work tree root.
    pub root: PathBuf,
    /// Configured `origin` remote URL, when present.
    pub remote_origin: Option<String>,
    /// Current `HEAD` commit, when the repository has commits.
    pub head: Option<String>,
    /// Current branch name, when HEAD is on a branch.
    pub branch: Option<String>,
}

impl Repository {
    /// Walk up from `start` until a `.git` entry is found.
    pub fn discover(start: &Path) -> Result<Self> {
        let start = canonicalize_lossy(start);
        let mut cursor: Option<&Path> = Some(&start);
        while let Some(dir) = cursor {
            if dir.join(".git").exists() {
                return Ok(Self::inspect(dir));
            }
            cursor = dir.parent();
        }
        bail!(
            "Not inside a Git repository.\n\n\
             Root workspaces are associated with a Git repository. Run this \
             command from inside a checked-out repository, or initialize one \
             with `git init`."
        )
    }

    /// Capture observable facts for a known Git root without mutating it.
    pub fn inspect(root: &Path) -> Self {
        let root = canonicalize_lossy(root);
        Self {
            remote_origin: git_output(&root, &["remote", "get-url", "origin"]),
            head: git_output(&root, &["rev-parse", "HEAD"]),
            branch: git_output(&root, &["rev-parse", "--abbrev-ref", "HEAD"])
                .filter(|value| value != "HEAD"),
            root,
        }
    }

    /// Display name for the workspace, derived from the repository directory.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "workspace".to_string())
    }

    /// Stable identity derived from observable repository facts.
    ///
    /// Prefers the configured remote (survives local moves and clones), and
    /// otherwise falls back to the canonical work-tree path.
    pub fn identity(&self) -> String {
        match &self.remote_origin {
            Some(remote) => format!("remote:{}", normalize_remote(remote)),
            None => format!("path:{}", self.root.display()),
        }
    }
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_remote(remote: &str) -> String {
    let trimmed = remote.trim().trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_string()
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_suffixes() {
        assert_eq!(
            normalize_remote("https://github.com/boring/campfire.git"),
            "https://github.com/boring/campfire"
        );
        assert_eq!(
            normalize_remote("git@github.com:boring/campfire.git"),
            "git@github.com:boring/campfire"
        );
    }

    #[test]
    fn discovers_git_root_from_nested_directory() {
        let base = std::env::temp_dir().join(format!("root_work_repo_{}", std::process::id()));
        let nested = base.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();

        let repo = Repository::discover(&nested).unwrap();
        assert_eq!(repo.root, canonicalize_lossy(&base));
        assert_eq!(repo.name(), base.file_name().unwrap().to_string_lossy());
        assert!(repo
            .identity()
            .starts_with(&format!("path:{}", canonicalize_lossy(&base).display())));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn errors_outside_repository() {
        let base = std::env::temp_dir().join(format!("root_work_norepo_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let err = Repository::discover(&base).unwrap_err().to_string();
        assert!(err.contains("Not inside a Git repository"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
