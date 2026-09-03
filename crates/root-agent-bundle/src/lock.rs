//! Global mutation lock reuse.
//!
//! Operates on the same `~/.root/root.lockfile` as `root-core::MutationGuard`
//! (same path, same `{pid}\n{now_secs}\n` format, same stale recovery via
//! `kill -0`). This guarantees mutual exclusion between agent-bundle
//! mutations and all other Root mutations even though the guard type lives
//! in another crate.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct GlobalMutationLock {
    lock_path: PathBuf,
}

impl GlobalMutationLock {
    pub fn acquire() -> Result<Self> {
        let dir = root_lockfile::init_root_dir()?;
        let lock_path = dir.join("root.lockfile");
        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let content = format!("{}\n{}\n", pid, now);
        match Self::try_acquire(&lock_path, &content) {
            Ok(()) => Ok(Self { lock_path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match Self::holder_alive(&lock_path) {
                    Ok(true) => Err(anyhow::anyhow!(
                        "Another Root mutation is in progress.\n\
                         If this is unexpected, delete ~/.root/root.lockfile and try again."
                    )),
                    Ok(false) => {
                        let _ = std::fs::remove_file(&lock_path);
                        Self::try_acquire(&lock_path, &content).with_context(|| {
                            "Failed to acquire mutation lock after recovering stale lock"
                        })?;
                        Ok(Self { lock_path })
                    }
                    Err(_) => Err(anyhow::anyhow!(
                        "Lock file ~/.root/root.lockfile exists and could not be read.\n\
                         Delete it manually and try again."
                    )),
                }
            }
            Err(e) => Err(anyhow::anyhow!("Failed to acquire mutation lock: {}", e)),
        }
    }

    fn try_acquire(lock_path: &std::path::Path, content: &str) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    fn holder_alive(lock_path: &std::path::Path) -> Result<bool> {
        let mut content = String::new();
        std::fs::File::open(lock_path)
            .and_then(|mut f| f.read_to_string(&mut content))
            .map_err(|_| anyhow::anyhow!("Cannot read lock file"))?;
        let pid_str = content.lines().next().unwrap_or("").trim();
        let lock_pid: u32 = pid_str
            .parse()
            .map_err(|_| anyhow::anyhow!("Malformed lock file (invalid PID)"))?;
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(lock_pid.to_string())
            .output()
            .map_err(|_| anyhow::anyhow!("Cannot check process liveness"))?;
        Ok(status.status.success())
    }
}

impl Drop for GlobalMutationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}
