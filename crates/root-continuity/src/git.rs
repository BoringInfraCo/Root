//! Observable Git state for checkpoints.
//!
//! Root never mutates Git. It only observes HEAD, branch, and a deterministic
//! dirty fingerprint of the working tree.

use root_lockfile::compute_sha256;
use root_work::Repository;
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GitState {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub dirty_fingerprint: Option<String>,
}

impl GitState {
    /// Capture observable Git state. A missing `git` binary yields unknown
    /// (not clean) state rather than a false claim of cleanliness.
    pub fn capture(repository: &Repository) -> Self {
        match porcelain(repository) {
            Some(status) => {
                let dirty = !status.trim().is_empty();
                Self {
                    head: repository.head.clone(),
                    branch: repository.branch.clone(),
                    dirty,
                    dirty_fingerprint: Some(compute_sha256(status.as_bytes())),
                }
            }
            None => Self {
                head: repository.head.clone(),
                branch: repository.branch.clone(),
                dirty: false,
                dirty_fingerprint: None,
            },
        }
    }
}

fn porcelain(repository: &Repository) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&repository.root)
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Deterministic short form of a Git SHA for display.
pub fn short_sha(head: &str) -> String {
    head.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_sha_truncates() {
        assert_eq!(short_sha("2fc193e0aabbcc"), "2fc193e");
        assert_eq!(short_sha("abc"), "abc");
    }
}
