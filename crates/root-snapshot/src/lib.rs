use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use root_lockfile::{compute_sha256, get_root_dir, LockedPackage, RootLock, RootLockV2};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub reason: String,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub package_count: usize,
    #[serde(default)]
    pub lock_content_hash: String,
    #[serde(default = "default_snapshot_lock")]
    pub lock: RootLockV2,
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotSummary {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub reason: String,
    pub schema_version: u32,
    pub package_count: usize,
    pub lock_content_hash: String,
}

impl From<&Snapshot> for SnapshotSummary {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            created_at: snapshot.created_at,
            reason: snapshot.reason.clone(),
            schema_version: snapshot.schema_version,
            package_count: snapshot.package_count,
            lock_content_hash: snapshot.lock_content_hash.clone(),
        }
    }
}

fn default_snapshot_lock() -> RootLockV2 {
    RootLock {
        version: 0,
        platform: "unknown".to_string(),
        nixpkgs: root_lockfile::NixpkgsConfig {
            rev: "unknown".to_string(),
            source: "github:NixOS/nixpkgs".to_string(),
        },
        packages: Vec::new(),
    }
    .to_v2()
}

impl Snapshot {
    pub fn create(reason: &str, current_lock: &RootLock) -> Result<Self> {
        Self::create_from_v2(reason, &current_lock.to_v2())
    }

    pub fn create_from_v2(reason: &str, current_lock: &RootLockV2) -> Result<Self> {
        let now = Utc::now();
        let id = format!("snap_{}", now.format("%Y%m%d_%H%M%S_%f"));
        let packages: Vec<LockedPackage> = current_lock
            .packages
            .iter()
            .map(|package| LockedPackage {
                name: package.name.clone(),
                requested: package.requested.clone(),
                version: package.version.clone(),
                attribute: package.attribute.clone(),
                store_path: package.store_path.clone(),
                binaries: package.binaries.clone(),
            })
            .collect();
        let lock_content = serde_json::to_vec(current_lock)?;

        let snapshot = Snapshot {
            id: id.clone(),
            created_at: now,
            reason: reason.to_string(),
            schema_version: current_lock.version,
            package_count: current_lock.packages.len(),
            lock_content_hash: compute_sha256(&lock_content),
            lock: current_lock.clone(),
            packages,
        };

        let snapshots_dir = get_root_dir()?.join("snapshots");
        fs::create_dir_all(&snapshots_dir)?;

        let path = snapshots_dir.join(format!("{}.json", id));
        let content = serde_json::to_string_pretty(&snapshot)?;
        fs::write(path, content)?;

        Ok(snapshot)
    }

    pub fn read(id: &str) -> Result<Self> {
        let path = get_root_dir()?
            .join("snapshots")
            .join(format!("{}.json", id));
        let content = fs::read_to_string(path).context("Snapshot not found")?;
        let snapshot: Snapshot = serde_json::from_str(&content)?;
        // Validate lock content hash if one was stored
        if !snapshot.lock_content_hash.is_empty() {
            let lock_content = serde_json::to_vec(&snapshot.lock)
                .context("Failed to serialize snapshot lock for hash verification")?;
            let computed = compute_sha256(&lock_content);
            if computed != snapshot.lock_content_hash {
                anyhow::bail!(
                    "Snapshot '{}' lock content hash mismatch: expected {}, got {}. \
                     The snapshot may be corrupted or tampered with.",
                    id,
                    snapshot.lock_content_hash,
                    computed
                );
            }
        }
        Ok(snapshot)
    }

    pub fn restored_lock(&self) -> RootLockV2 {
        if self.lock.version == 0 && !self.packages.is_empty() {
            RootLock {
                version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
                platform: root_lockfile::detect_platform().unwrap_or_else(|_| "unknown".into()),
                nixpkgs: root_lockfile::NixpkgsConfig {
                    rev: "unknown".into(),
                    source: "github:NixOS/nixpkgs".into(),
                },
                packages: self.packages.clone(),
            }
            .to_v2()
        } else {
            self.lock.clone()
        }
    }
}

pub fn list_snapshots() -> Result<Vec<Snapshot>> {
    let snapshots_dir = get_root_dir()?.join("snapshots");
    if !snapshots_dir.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    for entry in fs::read_dir(snapshots_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let content = fs::read_to_string(&path)?;
            match serde_json::from_str::<Snapshot>(&content) {
                Ok(snap) => {
                    // Validate lock content hash; skip corrupt snapshots with a warning
                    if !snap.lock_content_hash.is_empty() {
                        if let Ok(lock_bytes) = serde_json::to_vec(&snap.lock) {
                            let computed = compute_sha256(&lock_bytes);
                            if computed != snap.lock_content_hash {
                                eprintln!(
                                    "Warning: Snapshot '{}' is corrupted (hash mismatch), skipping",
                                    snap.id
                                );
                                continue;
                            }
                        }
                    }
                    snapshots.push(snap);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Skipping corrupt snapshot file '{}': {}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }

    snapshots.sort_by_key(|b| std::cmp::Reverse(b.created_at)); // newest first
    Ok(snapshots)
}

pub fn list_snapshot_summaries() -> Result<Vec<SnapshotSummary>> {
    Ok(list_snapshots()?
        .iter()
        .map(SnapshotSummary::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_create_and_list() {
        let _guard = TEST_MUTEX.lock().unwrap();
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("root_test_snap_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let lock = RootLock {
            version: 1,
            platform: "test".into(),
            nixpkgs: root_lockfile::NixpkgsConfig {
                rev: "abc".into(),
                source: "test".into(),
            },
            packages: vec![],
        };

        let snap = Snapshot::create("test snapshot", &lock).unwrap();
        assert!(snap.id.starts_with("snap_"));
        assert_eq!(snap.schema_version, root_lockfile::ROOT_LOCK_SCHEMA_VERSION);
        assert_eq!(snap.package_count, 0);
        assert!(!snap.lock_content_hash.is_empty());

        let list = list_snapshots().unwrap();
        assert!(list.iter().any(|s| s.id == snap.id));

        let read_snap = Snapshot::read(&snap.id).unwrap();
        assert_eq!(read_snap.reason, "test snapshot");
    }

    #[test]
    fn test_snapshot_hash_validation_passes() {
        let _guard = TEST_MUTEX.lock().unwrap();
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp =
            std::env::temp_dir().join(format!("root_test_snap_hash_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let lock = RootLock {
            version: 1,
            platform: "test".into(),
            nixpkgs: root_lockfile::NixpkgsConfig {
                rev: "abc".into(),
                source: "test".into(),
            },
            packages: vec![],
        };
        let snap = Snapshot::create("test hash", &lock).unwrap();
        // Reading back should pass hash validation
        let read_snap = Snapshot::read(&snap.id).unwrap();
        assert_eq!(read_snap.reason, "test hash");
    }

    #[test]
    fn test_snapshot_hash_validation_fails_on_corrupted_content() {
        let _guard = TEST_MUTEX.lock().unwrap();
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "root_test_snap_corrupt_{}_{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let lock = RootLock {
            version: 1,
            platform: "test".into(),
            nixpkgs: root_lockfile::NixpkgsConfig {
                rev: "abc".into(),
                source: "test".into(),
            },
            packages: vec![],
        };
        let snap = Snapshot::create("test corrupt", &lock).unwrap();

        // Corrupt the snapshot file on disk
        let snap_path = tmp.join("snapshots").join(format!("{}.json", snap.id));
        let mut content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&snap_path).unwrap()).unwrap();
        content["lock"]["platform"] = serde_json::json!("corrupted");
        std::fs::write(&snap_path, serde_json::to_string_pretty(&content).unwrap()).unwrap();

        // Reading the corrupted snapshot should fail hash validation
        let err = Snapshot::read(&snap.id).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("hash mismatch"),
            "Expected hash mismatch error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_snapshot_package_count_ignores_models() {
        let _guard = TEST_MUTEX.lock().unwrap();
        use root_lockfile::LockedModel;
        use std::collections::BTreeMap;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "root_test_snap_models_{}_{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();

        let mut lock = RootLockV2 {
            version: 3,
            platform: "test".into(),
            ..RootLockV2::default()
        };
        let mut ollama = BTreeMap::new();
        ollama.insert(
            "qwen3:8b".into(),
            LockedModel {
                runtime: "ollama".into(),
                requested_name: "qwen3:8b".into(),
                observed_digest: format!("sha256:{}", "a".repeat(64)),
                locked_at: "2026-09-01T00:00:00Z".into(),
                verified_at: "2026-09-01T00:00:01Z".into(),
                verification_method: "inspect_tags_digest".into(),
                addressability: "verification_record_only".into(),
                ..Default::default()
            },
        );
        lock.models.insert("ollama".into(), ollama);
        lock.version = root_lockfile::emit_lock_version(&lock.models);

        let snap = Snapshot::create_from_v2("models copy", &lock).unwrap();
        assert_eq!(snap.package_count, lock.packages.len());
        assert_eq!(snap.package_count, 0);
        assert_eq!(snap.schema_version, 3);
        assert!(snap
            .lock
            .models
            .get("ollama")
            .and_then(|models| models.get("qwen3:8b"))
            .is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
