//! Observable Root environment state for checkpoints.
//!
//! Root captures references and digests rather than copying environments. It
//! classifies what it actually observed and never promotes `unknown` to
//! `verified`.

use anyhow::Result;
use root_lockfile::get_root_dir;
use root_work::model::{ENV_MISSING, ENV_OBSERVED, ENV_UNKNOWN};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EnvironmentState {
    pub rootfile_digest: Option<String>,
    pub root_lock_digest: Option<String>,
    pub profile_reference: Option<String>,
    pub status: String,
}

impl EnvironmentState {
    /// Capture the observable Root environment. This does not run verification;
    /// therefore the strongest status it can report is `observed`.
    pub fn capture() -> Result<Self> {
        let root_dir = get_root_dir()?;
        Self::capture_at(&root_dir)
    }

    pub fn capture_at(root_dir: &Path) -> Result<Self> {
        let rootfile = root_dir.join("Rootfile");
        let lock = root_dir.join("root.lock");
        let profile = root_dir.join("profiles").join("default");

        let rootfile_digest = digest_if_file(&rootfile)?;
        let root_lock_digest = digest_if_file(&lock)?;
        let profile_reference = profile_reference(&profile);

        let status = if rootfile_digest.is_none() && root_lock_digest.is_none() {
            ENV_MISSING
        } else if rootfile_digest.is_some() && root_lock_digest.is_some() {
            ENV_OBSERVED
        } else {
            // Partial declaration is observable but not fully resolved.
            ENV_UNKNOWN
        };

        Ok(Self {
            rootfile_digest,
            root_lock_digest,
            profile_reference,
            status: status.to_string(),
        })
    }
}

fn digest_if_file(path: &Path) -> Result<Option<String>> {
    if path.is_file() {
        Ok(Some(root_lockfile::hash_file(path)?))
    } else {
        Ok(None)
    }
}

fn profile_reference(profile: &Path) -> Option<String> {
    match std::fs::read_link(profile) {
        Ok(target) => Some(target.to_string_lossy().to_string()),
        Err(_) if profile.exists() => Some(profile.to_string_lossy().to_string()),
        Err(_) => None,
    }
}

/// Root environment paths used for drift comparison.
pub fn paths(root_dir: &Path) -> EnvironmentPaths {
    EnvironmentPaths {
        rootfile: root_dir.join("Rootfile"),
        root_lock: root_dir.join("root.lock"),
        profile: root_dir.join("profiles").join("default"),
    }
}

#[derive(Debug, Clone)]
pub struct EnvironmentPaths {
    pub rootfile: PathBuf,
    pub root_lock: PathBuf,
    pub profile: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_when_no_rootfile_or_lock() {
        let dir = std::env::temp_dir().join(format!("root_env_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = EnvironmentState::capture_at(&dir).unwrap();
        assert_eq!(state.status, ENV_MISSING);
        assert!(state.rootfile_digest.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn observed_when_rootfile_and_lock_present() {
        let dir = std::env::temp_dir().join(format!("root_env_observed_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Rootfile"), "[packages]\n").unwrap();
        std::fs::write(dir.join("root.lock"), "{}").unwrap();
        let state = EnvironmentState::capture_at(&dir).unwrap();
        assert_eq!(state.status, ENV_OBSERVED);
        assert!(state.rootfile_digest.is_some());
        assert!(state.root_lock_digest.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
