//! Content-addressed blob store helpers.
//!
//! Bundle layout: `<bundle>/manifest.json` + `<bundle>/blobs/<sha256hex>`.
//! No symlinks, no hardlinks, no device files. Bounded reads everywhere.

use crate::manifest::{is_hex_sha256, MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES};
use anyhow::{Context, Result};
use std::path::Path;

/// Write a new bundle directory (must not exist). Creates `manifest.json`
/// (0600) + `blobs/` entries. Validates caps before writing.
pub fn write_bundle_dir(
    bundle_dir: &Path,
    manifest: &crate::manifest::Manifest,
    blobs: &[(String, Vec<u8>)],
) -> Result<()> {
    if bundle_dir.exists() {
        anyhow::bail!(
            "Refusing to write bundle: {} already exists",
            bundle_dir.display()
        );
    }
    if blobs.len() > MAX_FILES {
        anyhow::bail!("invalid bundle: too many files");
    }
    let mut total: u64 = 0;
    for (sha, bytes) in blobs {
        if !is_hex_sha256(sha) {
            anyhow::bail!("invalid blob name: malformed sha256");
        }
        if (bytes.len() as u64) > MAX_FILE_BYTES {
            anyhow::bail!("invalid bundle: blob exceeds per-file limit");
        }
        total += bytes.len() as u64;
    }
    if total > MAX_TOTAL_BYTES {
        anyhow::bail!("invalid bundle: total size exceeds limit");
    }
    std::fs::create_dir_all(bundle_dir.join("blobs")).context("Failed to create bundle dir")?;
    for (sha, bytes) in blobs {
        let dest = bundle_dir.join("blobs").join(sha);
        // create_new: never overwrite; 0600: bundles may contain prompt
        // content that embeds unrecognized secrets (see disclosure).
        use std::os::unix::fs::PermissionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        let mut f = opts
            .open(&dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        use std::io::Write;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    let manifest_bytes =
        serde_json::to_vec_pretty(manifest).context("Failed to serialize manifest")?;
    let manifest_path = bundle_dir.join("manifest.json");
    {
        use std::os::unix::fs::PermissionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        let mut f = opts.open(&manifest_path)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        use std::io::Write;
        f.write_all(&manifest_bytes)?;
        f.sync_all()?;
    }
    // fsync parent dir for durability.
    if let Ok(dirf) = std::fs::File::open(bundle_dir) {
        let _ = dirf.sync_all();
    }
    Ok(())
}

/// Read a blob by sha256 (bounded).
pub fn read_blob(bundle_dir: &Path, sha256: &str) -> Result<Vec<u8>> {
    if !is_hex_sha256(sha256) {
        anyhow::bail!("invalid blob reference: malformed sha256");
    }
    let path = bundle_dir.join("blobs").join(sha256.to_lowercase());
    let meta = std::fs::symlink_metadata(&path).context("Missing blob")?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("invalid bundle: symlinks are rejected in bundle v1");
    }
    if !meta.is_file() {
        anyhow::bail!("invalid bundle: blobs must be regular files");
    }
    if meta.len() > MAX_FILE_BYTES {
        anyhow::bail!("invalid bundle: blob exceeds size limit");
    }
    std::fs::read(&path).context("Failed to read blob")
}
