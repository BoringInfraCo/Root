use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The Rootfile TOML format.
#[derive(Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Rootfile {
    #[serde(default)]
    pub packages: BTreeMap<String, String>,
    #[serde(default)]
    pub tasks: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelDeclaration>,
    #[serde(default)]
    pub settings: RootSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelDeclaration {
    pub runtime: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RootSettings {
    #[serde(default = "default_true")]
    pub snapshots: bool,
    #[serde(default = "default_true")]
    pub verify_installs: bool,
}

impl Default for RootSettings {
    fn default() -> Self {
        Self {
            snapshots: true,
            verify_installs: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl Rootfile {
    pub fn read_from_str(content: &str) -> Result<Self> {
        let rootfile: Rootfile =
            toml::from_str(content).context("Failed to parse Rootfile TOML")?;
        rootfile.validate()?;
        Ok(rootfile)
    }

    pub fn read_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).context("Failed to read Rootfile")?;
        Self::read_from_str(&content)
    }

    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let content = toml::to_string_pretty(self).context("Failed to serialize Rootfile")?;
        atomic_write(path, content.as_bytes()).context("Failed to write Rootfile")?;
        Ok(())
    }

    /// Reject empty/control-character identifiers and unsupported agent constraints.
    pub fn validate(&self) -> Result<()> {
        for (name, constraint) in &self.agents {
            validate_rootfile_ident("agent name", name)?;
            validate_rootfile_ident("agent constraint", constraint)?;
            if constraint != "*" {
                anyhow::bail!(
                    "Agent version constraints are not supported in v0.2.5. \
                     Use \"*\" for presence-only declarations (got \"{constraint}\" for agent '{name}')."
                );
            }
        }
        for (name, declaration) in &self.models {
            validate_rootfile_ident("model name", name)?;
            validate_rootfile_ident("model runtime", &declaration.runtime)?;
        }
        Ok(())
    }
}

fn validate_rootfile_ident(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        anyhow::bail!("Rootfile {label} cannot be empty");
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("Rootfile {label} contains control characters");
    }
    Ok(())
}

/// Package-only emit default and missing-`version` peek value. Not the supported-read ceiling.
pub const ROOT_LOCK_SCHEMA_VERSION: u32 = 2;

/// Highest `version` this binary may read or mutate.
/// v3 = namespaced `models` object map. v4+ is refused.
pub const ROOT_LOCK_MAX_SUPPORTED_VERSION: u32 = 3;

/// Read the lock schema version without interpreting package fields.
///
/// A missing `version` field is treated as the current schema, matching
/// `RootLockV2` deserialization defaults.
pub fn peek_lock_schema_version(content: &str) -> Result<u32> {
    let value: serde_json::Value =
        serde_json::from_str(content).context("Failed to parse root.lock JSON")?;
    match value.get("version") {
        None => Ok(ROOT_LOCK_SCHEMA_VERSION),
        Some(version) => {
            let parsed = version
                .as_u64()
                .context("root.lock version must be a non-negative integer")?;
            u32::try_from(parsed).context("root.lock version is out of range")
        }
    }
}

/// Reject lockfiles written by a newer Root than this binary understands.
///
/// Versions below the current schema remain readable (v1 fallback). Versions
/// above `ROOT_LOCK_MAX_SUPPORTED_VERSION` must not be mutated or rewritten.
pub fn validate_supported_lock_version(version: u32) -> Result<()> {
    validate_supported_lock_version_with_max(version, ROOT_LOCK_MAX_SUPPORTED_VERSION)
}

/// Read a lockfile as [`RootLockV2`], taking the v1 fallback only for schema < 2.
///
/// Version 2+ documents that fail to deserialize as v2 are rejected so fields
/// such as `models` cannot be dropped by the v1 shape.
pub fn read_compatible_lock_v2_from_str(content: &str) -> Result<RootLockV2> {
    let version = peek_lock_schema_version(content)?;
    match RootLockV2::read_from_str(content) {
        Ok(lock) => Ok(lock),
        Err(err) => {
            if version < ROOT_LOCK_SCHEMA_VERSION {
                RootLock::read_from_str(content).map(|lock| lock.to_v2())
            } else {
                Err(err)
            }
        }
    }
}

/// File-backed form of [`read_compatible_lock_v2_from_str`].
pub fn read_compatible_lock_v2(path: &Path) -> Result<RootLockV2> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read lockfile at {}", path.display()))?;
    read_compatible_lock_v2_from_str(&content)
}

/// Same guard as [`validate_supported_lock_version`], with an explicit max.
pub fn validate_supported_lock_version_with_max(version: u32, max_supported: u32) -> Result<()> {
    if version > max_supported {
        anyhow::bail!(
            "root.lock schema version {version} is newer than this Root supports ({max_supported}). \
             Upgrade Root to use this lockfile."
        );
    }
    Ok(())
}

/// Schema version to emit for a lock with this `models` map.
/// Package-only locks stay at 2; a non-empty models map requires 3.
pub fn emit_lock_version(models: &BTreeMap<String, BTreeMap<String, LockedModel>>) -> u32 {
    if models.is_empty() {
        ROOT_LOCK_SCHEMA_VERSION
    } else {
        3
    }
}

/// The legacy root.lock JSON format.
///
/// This shape is intentionally kept source-compatible with v0.1 callers that
/// construct locks with struct literals. New deterministic v2 metadata is
/// represented by [`RootLockV2`] and can be reached with [`RootLock::to_v2`].
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct RootLock {
    pub version: u32,
    pub platform: String,
    pub nixpkgs: NixpkgsConfig,
    #[serde(default)]
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct NixpkgsConfig {
    pub rev: String,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct LockedPackage {
    pub name: String,
    pub requested: String,
    pub version: String,
    pub attribute: String,
    #[serde(rename = "storePath")]
    pub store_path: String,
    pub binaries: Vec<String>,
}

/// RootLock v2 JSON format with deterministic Nix resolution metadata.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct RootLockV2 {
    #[serde(default = "default_root_lock_schema_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub platform: String,
    #[serde(default)]
    pub nix: NixRuntime,
    #[serde(default)]
    pub nixpkgs: NixpkgsConfigV2,
    #[serde(default)]
    pub profile: LockProfile,
    #[serde(default)]
    pub packages: Vec<LockedPackageV2>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, BTreeMap<String, LockedModel>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct NixRuntime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct NixpkgsConfigV2 {
    #[serde(default)]
    pub rev: String,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flake_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nar_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct LockProfile {
    #[serde(default = "default_profile_name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct LockedPackageV2 {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub requested: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub attribute: String,
    #[serde(default, rename = "storePath")]
    pub store_path: String,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flake_attribute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drv_path: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, LockedPackageOutput>,
    #[serde(
        default,
        rename = "storePaths",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub store_paths: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// A locked local model under a runtime namespace (`models.<runtime>.<name>`).
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct LockedModel {
    pub runtime: String,
    pub requested_name: String,
    /// Canonical `sha256:<64 lowercase hex>`.
    pub observed_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_version: Option<String>,
    pub locked_at: String,
    pub verified_at: String,
    /// `pull_tag_then_compare_tags_digest` | `inspect_tags_digest`
    pub verification_method: String,
    /// `verification_record_only`
    pub addressability: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct LockedPackageOutput {
    #[serde(default, rename = "storePath")]
    pub store_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nar_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum LegacyLockIssue {
    V1Schema { version: u32 },
    LatestPackageVersion { package: String },
    UnknownNixpkgsRev { rev: String },
    PlaceholderStorePath { package: String, store_path: String },
}

fn default_root_lock_schema_version() -> u32 {
    ROOT_LOCK_SCHEMA_VERSION
}

fn default_profile_name() -> String {
    "default".to_string()
}

impl Default for LockProfile {
    fn default() -> Self {
        Self {
            name: default_profile_name(),
            path: None,
            generation: None,
        }
    }
}

impl RootLock {
    pub fn read_from_str(content: &str) -> Result<Self> {
        let lockfile: RootLock =
            serde_json::from_str(content).context("Failed to parse root.lock JSON")?;
        Ok(lockfile)
    }

    pub fn read_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).context("Failed to read root.lock")?;
        Self::read_from_str(&content)
    }

    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize root.lock")?;
        if path.exists() {
            if let Ok(existing) = fs::read_to_string(path) {
                if existing == content {
                    return Ok(());
                }
            }
        }
        atomic_write(path, content.as_bytes()).context("Failed to write root.lock")?;
        Ok(())
    }

    /// Returns true when this lock predates the v2 deterministic metadata schema.
    pub fn is_legacy_v1(&self) -> bool {
        self.version < ROOT_LOCK_SCHEMA_VERSION
    }

    /// Returns true when the lock's nixpkgs revision is not pinned to a concrete revision.
    pub fn has_unknown_nixpkgs_rev(&self) -> bool {
        is_unknown_nixpkgs_rev(&self.nixpkgs.rev)
    }

    /// Reports all known nondeterministic legacy markers in this lock.
    pub fn nondeterministic_legacy_issues(&self) -> Vec<LegacyLockIssue> {
        let mut issues = Vec::new();

        if self.is_legacy_v1() {
            issues.push(LegacyLockIssue::V1Schema {
                version: self.version,
            });
        }

        if self.has_unknown_nixpkgs_rev() {
            issues.push(LegacyLockIssue::UnknownNixpkgsRev {
                rev: self.nixpkgs.rev.clone(),
            });
        }

        for package in &self.packages {
            if package.has_latest_version() {
                issues.push(LegacyLockIssue::LatestPackageVersion {
                    package: package.name.clone(),
                });
            }

            if package.has_placeholder_store_path() {
                issues.push(LegacyLockIssue::PlaceholderStorePath {
                    package: package.name.clone(),
                    store_path: package.store_path.clone(),
                });
            }
        }

        issues
    }

    /// Returns true if any v1/nondeterministic marker is present.
    pub fn has_nondeterministic_legacy_entries(&self) -> bool {
        !self.nondeterministic_legacy_issues().is_empty()
    }

    /// Converts the source-compatible v1 shape into the richer v2 representation.
    pub fn to_v2(&self) -> RootLockV2 {
        RootLockV2 {
            version: ROOT_LOCK_SCHEMA_VERSION,
            root_version: None,
            created_at: None,
            updated_at: None,
            platform: self.platform.clone(),
            nix: NixRuntime::default(),
            nixpkgs: NixpkgsConfigV2::from(self.nixpkgs.clone()),
            profile: LockProfile::default(),
            packages: self
                .packages
                .iter()
                .cloned()
                .map(LockedPackageV2::from)
                .collect(),
            models: BTreeMap::new(),
        }
    }
}

impl RootLockV2 {
    pub fn read_from_str(content: &str) -> Result<Self> {
        let lockfile: RootLockV2 =
            serde_json::from_str(content).context("Failed to parse root.lock v2 JSON")?;
        Ok(lockfile)
    }

    pub fn read_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).context("Failed to read root.lock")?;
        Self::read_from_str(&content)
    }

    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        let content =
            serde_json::to_string_pretty(self).context("Failed to serialize root.lock v2")?;
        if path.exists() {
            if let Ok(existing) = fs::read_to_string(path) {
                if existing == content {
                    return Ok(());
                }
            }
        }
        atomic_write(path, content.as_bytes()).context("Failed to write root.lock")?;
        Ok(())
    }

    /// Returns true if writing this lock to the given path would change the file contents.
    pub fn would_change_file(&self, path: &Path) -> Result<bool> {
        if !path.exists() {
            return Ok(true);
        }
        let existing = fs::read_to_string(path)?;
        let content = serde_json::to_string_pretty(self)?;
        Ok(existing != content)
    }
}

impl LockedPackage {
    /// Returns true when the package was locked to a floating latest version.
    pub fn has_latest_version(&self) -> bool {
        self.version.trim().eq_ignore_ascii_case("latest")
    }

    /// Returns true when the package store path is a placeholder rather than a concrete store path.
    pub fn has_placeholder_store_path(&self) -> bool {
        is_placeholder_store_path(&self.store_path)
    }
}

impl LockedPackageV2 {
    /// Returns true when the package was locked to a floating latest version.
    pub fn has_latest_version(&self) -> bool {
        self.version.trim().eq_ignore_ascii_case("latest")
    }

    /// Returns true when the primary package store path is a placeholder rather than a concrete store path.
    pub fn has_placeholder_store_path(&self) -> bool {
        is_placeholder_store_path(&self.store_path)
    }
}

impl From<NixpkgsConfig> for NixpkgsConfigV2 {
    fn from(config: NixpkgsConfig) -> Self {
        Self {
            rev: config.rev,
            source: config.source,
            flake_ref: None,
            nar_hash: None,
            last_modified: None,
            system: None,
            config: BTreeMap::new(),
            overlays: Vec::new(),
        }
    }
}

impl From<LockedPackage> for LockedPackageV2 {
    fn from(package: LockedPackage) -> Self {
        let mut outputs = BTreeMap::new();
        let mut store_paths = BTreeMap::new();

        if !package.store_path.is_empty() {
            outputs.insert(
                "out".to_string(),
                LockedPackageOutput {
                    store_path: package.store_path.clone(),
                    content_hash: None,
                    nar_hash: None,
                    references: Vec::new(),
                },
            );
            store_paths.insert("out".to_string(), package.store_path.clone());
        }

        Self {
            installable: Some(package.attribute.clone()),
            flake_attribute: Some(package.attribute.clone()),
            outputs,
            store_paths,
            name: package.name,
            requested: package.requested,
            version: package.version,
            attribute: package.attribute,
            store_path: package.store_path,
            binaries: package.binaries,
            drv_path: None,
            meta: BTreeMap::new(),
            content_hash: None,
        }
    }
}

pub fn is_unknown_nixpkgs_rev(rev: &str) -> bool {
    let normalized = rev.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "" | "unknown" | "latest" | "unstable" | "master" | "main" | "head" | "n/a"
    )
}

fn is_placeholder_store_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("placeholder")
        || lower.contains("dummy")
        || lower.contains("example")
        || lower == "/nix/store/xxx"
    {
        return true;
    }

    let Some(rest) = trimmed.strip_prefix("/nix/store/") else {
        return true;
    };
    let Some((hash_part, _name_part)) = rest.split_once('-') else {
        return true;
    };

    let plausible_legacy_hash =
        hash_part.len() == 10 && hash_part.chars().all(|c| c.is_ascii_hexdigit());
    let plausible_nix_hash = hash_part.len() == 32
        && hash_part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());

    !(plausible_legacy_hash || plausible_nix_hash)
}

/// Returns the ~/.root path (or $ROOT_DIR env var override for testing)
pub fn get_root_dir() -> Result<PathBuf> {
    if let Some(val) = std::env::var_os("ROOT_DIR") {
        return Ok(PathBuf::from(val));
    }
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".root"))
}

/// Initializes the ~/.root directory structure
pub fn init_root_dir() -> Result<PathBuf> {
    let root_dir = get_root_dir()?;
    fs::create_dir_all(&root_dir).context("Failed to create ~/.root directory")?;
    fs::create_dir_all(root_dir.join("snapshots"))
        .context("Failed to create snapshots directory")?;
    fs::create_dir_all(root_dir.join("profiles")).context("Failed to create profiles directory")?;

    // Don't pre-create profiles/default — Nix manages it as a symlink.
    // If it exists as a broken symlink or empty directory, clean it up
    // so Nix can create/replace it on first profile operation.
    let default_profile = root_dir.join("profiles").join("default");
    if let Ok(meta) = fs::symlink_metadata(&default_profile) {
        if meta.file_type().is_symlink() {
            // Broken symlink — remove it so Nix can recreate it
            if fs::read_link(&default_profile).is_err() || !default_profile.exists() {
                let _ = fs::remove_file(&default_profile);
            }
        } else if meta.is_dir() {
            // Plain directory from older Root versions — remove if empty
            // so Nix can manage it as a symlink
            if fs::read_dir(&default_profile)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
            {
                let _ = fs::remove_dir(&default_profile);
            }
        }
    }

    fs::create_dir_all(root_dir.join("logs")).context("Failed to create logs directory")?;
    fs::create_dir_all(root_dir.join("cache")).context("Failed to create cache directory")?;
    Ok(root_dir)
}

/// Write content to a file atomically using temp file + rename.
/// Preserves the existing file if the write or rename fails.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| {
        anyhow::anyhow!("Cannot determine parent directory for {}", path.display())
    })?;
    let tmp_path = dir.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&tmp_path, content).context("Failed to write temp file")?;
    fs::rename(&tmp_path, path).context("Failed to rename temp file to target")?;
    Ok(())
}

/// Compute the SHA-256 hex digest of arbitrary data.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute the SHA-256 hex digest of a file's contents.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).context("Failed to open file for hashing")?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .context("Failed to read file for hashing")?;
    Ok(compute_sha256(&buf))
}

/// Derive a deterministic Nix-like store path from a package name.
///
/// In real Nix the hash is derived from the full derivation; here we produce
/// a stable, reproducible identifier for MVP state-tracking purposes.
pub fn derive_store_path(name: &str, version: &str) -> String {
    let input = format!("root:{}+{}", name, version);
    let hash = compute_sha256(input.as_bytes());
    let short_hash = &hash[..10];
    format!("/nix/store/{}-{}-{}", short_hash, name, version)
}

/// Detect the current platform string for the lockfile.
/// Returns an error if the platform is unsupported.
pub fn detect_platform() -> Result<String> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-darwin".to_string()),
        ("macos", "x86_64") => Ok("x86_64-darwin".to_string()),
        ("linux", "aarch64") => Ok("aarch64-linux".to_string()),
        ("linux", "x86_64") => Ok("x86_64-linux".to_string()),
        (os, arch) => Err(anyhow::anyhow!(
            "Unsupported platform: {}-{}. Root v0.1 supports macOS (Apple Silicon and Intel) and Linux (aarch64 and x86_64).",
            os, arch
        )),
    }
}

/// Compute a content hash for the packages in a RootLock (sorted for determinism).
pub fn compute_lock_content_hash(lock: &RootLock) -> String {
    let mut hasher = Sha256::new();
    for pkg in &lock.packages {
        hasher.update(pkg.name.as_bytes());
        hasher.update(b"\0");
        hasher.update(pkg.version.as_bytes());
        hasher.update(b"\0");
        hasher.update(pkg.requested.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

/// Error type for store path validation failures in Root lockfiles.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum StorePathError {
    #[error(
        "Invalid Root lockfile: package {package} has a derivation path where an output path was expected.\n\nExpected output path:\n  /nix/store/...-{package_short}\n\nFound derivation path:\n  {found}"
    )]
    DrvInOutputField {
        package: String,
        package_short: String,
        found: String,
    },

    #[error(
        "Invalid Root lockfile: package {package} store path does not start with /nix/store/.\n\nFound:\n  {found}"
    )]
    OutputNotInStore { package: String, found: String },
}

fn shorten_package_name(name: &str) -> String {
    // Strip output suffix like ".out", ".dev", etc. for display
    name.rsplit_once('.')
        .map(|(pkg, _)| pkg.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Validate that all store paths in a `RootLockV2` follow the expected conventions.
///
/// Rules:
/// - `drv_path` field must end in `.drv` or be `None`/empty
/// - All `outputs` values must NOT end in `.drv`
/// - All `store_paths` values must NOT end in `.drv`
/// - `store_path` field must NOT end in `.drv`
/// - Output paths must start with `/nix/store/`
pub fn validate_store_paths(lock: &RootLockV2) -> Result<()> {
    for package in &lock.packages {
        // drv_path: must end in .drv or be None/empty
        if let Some(ref drv_path) = package.drv_path {
            if !drv_path.is_empty() && !drv_path.ends_with(".drv") {
                anyhow::bail!(StorePathError::OutputNotInStore {
                    package: format!("{}.drv_path", package.name),
                    found: drv_path.clone(),
                });
            }
        }

        // outputs.*.store_path: must NOT end in .drv
        for (output_name, output) in &package.outputs {
            let path = &output.store_path;
            if path.is_empty() {
                continue;
            }
            if path.ends_with(".drv") {
                anyhow::bail!(StorePathError::DrvInOutputField {
                    package: format!("{}.{}", package.name, output_name),
                    package_short: shorten_package_name(&package.name),
                    found: path.clone(),
                });
            }
            if !path.starts_with("/nix/store/") {
                anyhow::bail!(StorePathError::OutputNotInStore {
                    package: format!("{}.{}.store_path", package.name, output_name),
                    found: path.clone(),
                });
            }
        }

        // store_paths.*: must NOT end in .drv
        for (output_name, path) in &package.store_paths {
            if path.is_empty() {
                continue;
            }
            if path.ends_with(".drv") {
                anyhow::bail!(StorePathError::DrvInOutputField {
                    package: format!("{}.{}", package.name, output_name),
                    package_short: shorten_package_name(&package.name),
                    found: path.clone(),
                });
            }
            if !path.starts_with("/nix/store/") {
                anyhow::bail!(StorePathError::OutputNotInStore {
                    package: format!("{}.store_paths.{}", package.name, output_name),
                    found: path.clone(),
                });
            }
        }

        // store_path: must NOT end in .drv
        if !package.store_path.is_empty() {
            if package.store_path.ends_with(".drv") {
                anyhow::bail!(StorePathError::DrvInOutputField {
                    package: package.name.clone(),
                    package_short: package.name.clone(),
                    found: package.store_path.clone(),
                });
            }
            if !package.store_path.starts_with("/nix/store/") {
                anyhow::bail!(StorePathError::OutputNotInStore {
                    package: format!("{}.store_path", package.name),
                    found: package.store_path.clone(),
                });
            }
        }

        // binaries: no path validation needed, they're just binary names
        // meta, content_hash, etc: not paths, skip
    }
    Ok(())
}

/// Validate namespaced `models` entries on a lock.
///
/// Empty maps are accepted. When entries are present, each digest must already
/// be canonical `sha256:<64 lowercase hex>`, `runtime` must be `ollama`,
/// credential-looking strings are rejected, and any `endpoint` must be loopback.
pub fn validate_locked_models(lock: &RootLockV2) -> Result<()> {
    if lock.models.is_empty() {
        return Ok(());
    }

    for (runtime_ns, models) in &lock.models {
        if runtime_ns != "ollama" {
            anyhow::bail!(
                "invalid locked model '{runtime_ns}': runtime namespace must be \"ollama\""
            );
        }
        reject_secret_string("model runtime namespace", runtime_ns)?;
        for (name, model) in models {
            let label = format!("{runtime_ns}/{name}");
            reject_secret_string(&format!("locked model '{label}' name"), name)?;
            validate_one_locked_model(&label, model)?;
        }
    }
    Ok(())
}

fn validate_one_locked_model(label: &str, model: &LockedModel) -> Result<()> {
    if model.runtime != "ollama" {
        anyhow::bail!("invalid locked model '{label}': runtime must be \"ollama\"");
    }
    reject_secret_string(&format!("locked model '{label}' runtime"), &model.runtime)?;
    reject_secret_string(
        &format!("locked model '{label}' requested_name"),
        &model.requested_name,
    )?;
    reject_secret_string(
        &format!("locked model '{label}' observed_digest"),
        &model.observed_digest,
    )?;
    if let Some(endpoint) = &model.endpoint {
        reject_secret_string(&format!("locked model '{label}' endpoint"), endpoint)?;
        if !endpoint_is_loopback(endpoint) {
            anyhow::bail!(
                "invalid locked model '{label}': endpoint must be loopback (127.0.0.1 or ::1)"
            );
        }
    }
    if let Some(backend_version) = &model.backend_version {
        reject_secret_string(
            &format!("locked model '{label}' backend_version"),
            backend_version,
        )?;
    }
    reject_secret_string(
        &format!("locked model '{label}' verification_method"),
        &model.verification_method,
    )?;
    reject_secret_string(
        &format!("locked model '{label}' addressability"),
        &model.addressability,
    )?;
    if !is_canonical_sha256_digest(&model.observed_digest) {
        anyhow::bail!(
            "invalid locked model '{label}': observed_digest must be canonical sha256:<64 lowercase hex>"
        );
    }
    Ok(())
}

fn is_canonical_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn looks_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("canary")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("bearer")
        || lower.contains("api_key")
}

fn reject_secret_string(label: &str, value: &str) -> Result<()> {
    if looks_secret(value) {
        anyhow::bail!("{label} contains a credential-looking string");
    }
    Ok(())
}

fn endpoint_is_loopback(endpoint: &str) -> bool {
    matches!(
        endpoint_host(endpoint).as_deref(),
        Some("127.0.0.1") | Some("::1")
    )
}

fn endpoint_host(endpoint: &str) -> Option<String> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = without_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        return rest
            .split(']')
            .next()
            .filter(|h| !h.is_empty())
            .map(str::to_string);
    }
    if authority == "::1" {
        return Some("::1".to_string());
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) {
            return Some(host.to_string());
        }
    }
    Some(authority.to_string())
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_compute_sha256() {
        let hash = compute_sha256(b"hello");
        assert_eq!(hash.len(), 64);
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_derive_store_path() {
        let path = derive_store_path("poppler", "24.08.0");
        assert!(path.starts_with("/nix/store/"));
        assert!(path.contains("poppler-24.08.0"));
        assert_eq!(derive_store_path("poppler", "24.08.0"), path);
    }

    #[test]
    fn test_compute_lock_content_hash() {
        let lock = RootLock {
            version: 1,
            platform: "test".into(),
            nixpkgs: NixpkgsConfig {
                rev: "abc".into(),
                source: "test".into(),
            },
            packages: vec![LockedPackage {
                name: "poppler".into(),
                requested: "poppler".into(),
                version: "24.08.0".into(),
                attribute: "poppler".into(),
                store_path: "/nix/store/xxx".into(),
                binaries: vec!["pdftotext".into()],
            }],
        };
        let hash = compute_lock_content_hash(&lock);
        assert_eq!(hash.len(), 64);
        assert_eq!(compute_lock_content_hash(&lock), hash);
    }

    #[test]
    fn parse_rootfile() {
        let toml_str = r#"
        [packages]
        node = "22.11.0"
        poppler = "24.08.0"

        [tasks]
        build = "cargo build"

        [settings]
        snapshots = true
        verify_installs = false
        "#;
        let rootfile = Rootfile::read_from_str(toml_str).unwrap();
        assert_eq!(rootfile.packages.get("node").unwrap(), "22.11.0");
        assert_eq!(rootfile.tasks.get("build").unwrap(), "cargo build");
        assert!(rootfile.settings.snapshots);
        assert!(!rootfile.settings.verify_installs);
    }

    #[test]
    fn parse_rootlock() {
        let json_str = r#"
        {
          "version": 1,
          "platform": "aarch64-darwin",
          "nixpkgs": {
            "rev": "some-hash",
            "source": "github:NixOS/nixpkgs"
          },
          "packages": [
            {
              "name": "poppler",
              "requested": "poppler",
              "version": "24.08.0",
              "attribute": "poppler",
              "storePath": "/nix/store/123-poppler-24.08.0",
              "binaries": ["pdftotext", "pdfinfo"]
            }
          ]
        }
        "#;
        let lock = RootLock::read_from_str(json_str).unwrap();
        assert_eq!(lock.version, 1);
        assert_eq!(lock.nixpkgs.rev, "some-hash");
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages[0].name, "poppler");
        assert_eq!(lock.packages[0].binaries, vec!["pdftotext", "pdfinfo"]);
    }

    #[test]
    fn parse_rootlock_v2_metadata() {
        let json_str = r#"
        {
          "version": 2,
          "root_version": "0.1.2",
          "created_at": "2026-06-03T00:00:00Z",
          "updated_at": "2026-06-03T00:00:01Z",
          "platform": "x86_64-linux",
          "nix": {
            "version": "2.24.0",
            "system": "x86_64-linux",
            "store_dir": "/nix/store",
            "sandbox": true
          },
          "nixpkgs": {
            "rev": "0123456789abcdef0123456789abcdef01234567",
            "source": "github:NixOS/nixpkgs",
            "flake_ref": "github:NixOS/nixpkgs/0123456789abcdef0123456789abcdef01234567",
            "nar_hash": "sha256-deadbeef",
            "last_modified": "2026-06-01T00:00:00Z",
            "system": "x86_64-linux",
            "config": { "allowUnfree": true },
            "overlays": ["github:example/overlay"]
          },
          "profile": {
            "name": "default",
            "path": "/home/alice/.root/profiles/default",
            "generation": 7
          },
          "packages": [
            {
              "name": "poppler",
              "requested": "poppler",
              "version": "24.08.0",
              "attribute": "poppler",
              "storePath": "/nix/store/0123456789abcdef0123456789abcdef-poppler-24.08.0",
              "binaries": ["pdftotext"],
              "installable": "nixpkgs#poppler",
              "flake_attribute": "legacyPackages.x86_64-linux.poppler",
              "drv_path": "/nix/store/0123456789abcdef0123456789abcdef-poppler-24.08.0.drv",
              "outputs": {
                "out": {
                  "storePath": "/nix/store/0123456789abcdef0123456789abcdef-poppler-24.08.0",
                  "content_hash": "sha256-feedface",
                  "nar_hash": "sha256-cafebabe",
                  "references": []
                }
              },
              "storePaths": {
                "out": "/nix/store/0123456789abcdef0123456789abcdef-poppler-24.08.0"
              },
              "meta": { "description": "PDF rendering tools" },
              "content_hash": "sha256-feedface"
            }
          ]
        }
        "#;

        let lock = RootLockV2::read_from_str(json_str).unwrap();

        assert_eq!(lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert_eq!(lock.root_version.as_deref(), Some("0.1.2"));
        assert_eq!(lock.nix.version.as_deref(), Some("2.24.0"));
        assert_eq!(
            lock.nixpkgs.config.get("allowUnfree"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(lock.profile.generation, Some(7));
        assert_eq!(
            lock.packages[0].installable.as_deref(),
            Some("nixpkgs#poppler")
        );
        assert_eq!(
            lock.packages[0].outputs["out"].content_hash.as_deref(),
            Some("sha256-feedface")
        );
    }

    #[test]
    fn detect_nondeterministic_legacy_entries() {
        let lock = RootLock {
            version: 1,
            platform: "test".into(),
            nixpkgs: NixpkgsConfig {
                rev: "unknown".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "node".into(),
                requested: "node".into(),
                version: "latest".into(),
                attribute: "nodejs".into(),
                store_path: "/nix/store/xxx".into(),
                binaries: vec!["node".into()],
            }],
        };

        let issues = lock.nondeterministic_legacy_issues();

        assert!(lock.has_nondeterministic_legacy_entries());
        assert!(issues.contains(&LegacyLockIssue::V1Schema { version: 1 }));
        assert!(issues.contains(&LegacyLockIssue::UnknownNixpkgsRev {
            rev: "unknown".into()
        }));
        assert!(issues.contains(&LegacyLockIssue::LatestPackageVersion {
            package: "node".into()
        }));
        assert!(issues.contains(&LegacyLockIssue::PlaceholderStorePath {
            package: "node".into(),
            store_path: "/nix/store/xxx".into()
        }));
    }

    #[test]
    fn v1_lock_converts_to_v2_defaults() {
        let lock = RootLock {
            version: 1,
            platform: "x86_64-linux".into(),
            nixpkgs: NixpkgsConfig {
                rev: "0123456789abcdef0123456789abcdef01234567".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "ripgrep".into(),
                requested: "ripgrep".into(),
                version: "14.1.1".into(),
                attribute: "ripgrep".into(),
                store_path: derive_store_path("ripgrep", "14.1.1"),
                binaries: vec!["rg".into()],
            }],
        };

        let v2 = lock.to_v2();

        assert_eq!(v2.version, ROOT_LOCK_SCHEMA_VERSION);
        assert!(v2.models.is_empty());
        assert_eq!(v2.platform, "x86_64-linux");
        assert_eq!(v2.profile, LockProfile::default());
        assert_eq!(v2.nixpkgs.rev, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(v2.packages[0].installable.as_deref(), Some("ripgrep"));
        assert_eq!(
            v2.packages[0].store_paths["out"],
            lock.packages[0].store_path
        );
        assert_eq!(
            lock.nondeterministic_legacy_issues(),
            vec![LegacyLockIssue::V1Schema { version: 1 }]
        );
    }

    #[test]
    fn test_atomic_write_creates_and_reads() {
        let tmp = std::env::temp_dir().join(format!("root_atomic_write_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let path = tmp.join("test.json");
        let content = b"{\"hello\": \"world\"}";
        atomic_write(&path, content).unwrap();
        assert!(path.exists());
        let read_back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, "{\"hello\": \"world\"}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_atomic_write_preserves_existing_on_invalid_parent() {
        // Writing to a path without a valid parent should fail
        let result = atomic_write(Path::new("/nonexistent_dir/file.json"), b"data");
        assert!(result.is_err());
    }

    #[test]
    fn test_v2_serialized_can_be_read_after_atomic_write() {
        let tmp = std::env::temp_dir().join(format!("root_atomic_v2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let path = tmp.join("root.lock");
        let lock = RootLockV2 {
            version: 2,
            root_version: Some("0.2.0".into()),
            created_at: Some("2026-06-11T00:00:00Z".into()),
            updated_at: None,
            platform: "x86_64-darwin".into(),
            nix: NixRuntime::default(),
            nixpkgs: NixpkgsConfigV2::default(),
            profile: LockProfile::default(),
            packages: vec![],
            models: BTreeMap::new(),
        };
        lock.write_to_file(&path).unwrap();
        let read_back = RootLockV2::read_from_file(&path).unwrap();
        assert_eq!(read_back.version, 2);
        assert_eq!(read_back.platform, "x86_64-darwin");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rootfile_serialization_deterministic_order() {
        let rootfile = Rootfile {
            packages: vec![
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
                ("c".to_string(), "3".to_string()),
            ]
            .into_iter()
            .collect(),
            tasks: vec![
                ("z".to_string(), "echo z".to_string()),
                ("build".to_string(), "cargo build".to_string()),
            ]
            .into_iter()
            .collect(),
            settings: RootSettings::default(),
            ..Rootfile::default()
        };
        let serialized = toml::to_string_pretty(&rootfile).unwrap();
        let parsed: Rootfile = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.packages.len(), 3);
        // Serialization should round-trip successfully
        let reserialized = toml::to_string_pretty(&parsed).unwrap();
        assert_eq!(
            serialized, reserialized,
            "Rootfile serialization must be deterministic"
        );
    }

    // ── Store path validation tests ─────────────────────────────────────

    fn make_valid_package() -> LockedPackageV2 {
        LockedPackageV2 {
            name: "ffmpeg".into(),
            requested: "ffmpeg".into(),
            version: "8.1".into(),
            attribute: "ffmpeg".into(),
            store_path: "/nix/store/abc123ffmpeg-ffmpeg-8.1".into(),
            binaries: vec!["ffmpeg".into()],
            installable: Some("nixpkgs#ffmpeg".into()),
            drv_path: Some("/nix/store/drv456ffmpeg-ffmpeg-8.1.drv".into()),
            outputs: {
                let mut m = BTreeMap::new();
                m.insert(
                    "out".into(),
                    LockedPackageOutput {
                        store_path: "/nix/store/abc123ffmpeg-ffmpeg-8.1".into(),
                        content_hash: None,
                        nar_hash: None,
                        references: vec![],
                    },
                );
                m
            },
            store_paths: {
                let mut m = BTreeMap::new();
                m.insert("out".into(), "/nix/store/abc123ffmpeg-ffmpeg-8.1".into());
                m
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_store_paths_accepts_valid_lock() {
        let lock = RootLockV2 {
            packages: vec![make_valid_package()],
            ..RootLockV2::default()
        };
        assert!(validate_store_paths(&lock).is_ok());
    }

    #[test]
    fn test_validate_store_paths_rejects_drv_in_outputs() {
        let mut pkg = make_valid_package();
        pkg.outputs.insert(
            "out".into(),
            LockedPackageOutput {
                store_path: "/nix/store/abc-ffmpeg-8.1.drv".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        let err = validate_store_paths(&lock).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("derivation path where an output path was expected"));
        assert!(msg.contains("ffmpeg"));
        assert!(msg.contains(".drv"));
    }

    #[test]
    fn test_validate_store_paths_rejects_drv_in_store_paths() {
        let mut pkg = make_valid_package();
        pkg.store_paths
            .insert("dev".into(), "/nix/store/abc-ffmpeg-dev.drv".into());
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        let err = validate_store_paths(&lock).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("derivation path"));
    }

    #[test]
    fn test_validate_store_paths_rejects_drv_in_store_path() {
        let mut pkg = make_valid_package();
        pkg.store_path = "/nix/store/abc-ffmpeg-8.1.drv".into();
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        let err = validate_store_paths(&lock).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("derivation path where an output path was expected"));
    }

    #[test]
    fn test_validate_store_paths_accepts_drv_in_drv_path() {
        let mut pkg = make_valid_package();
        pkg.drv_path = Some("/nix/store/abc-ffmpeg-8.1.drv".into());
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        assert!(validate_store_paths(&lock).is_ok());
    }

    #[test]
    fn test_validate_store_paths_accepts_empty_drv_path() {
        let mut pkg = make_valid_package();
        pkg.drv_path = None;
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        assert!(validate_store_paths(&lock).is_ok());
    }

    #[test]
    fn test_validate_store_paths_rejects_non_store_path() {
        let mut pkg = make_valid_package();
        pkg.outputs.insert(
            "out".into(),
            LockedPackageOutput {
                store_path: "/tmp/some-local-path".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        let err = validate_store_paths(&lock).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not start with /nix/store/"));
    }

    #[test]
    fn test_validate_store_paths_skips_empty_paths() {
        let mut pkg = make_valid_package();
        pkg.outputs.insert(
            "dev".into(),
            LockedPackageOutput {
                store_path: "".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );
        pkg.store_paths.insert("dev".into(), "".into());
        let lock = RootLockV2 {
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        assert!(validate_store_paths(&lock).is_ok());
    }

    #[test]
    fn test_validate_store_paths_multiple_packages() {
        let mut pkg1 = make_valid_package();
        pkg1.name = "good-pkg".into();
        let mut pkg2 = make_valid_package();
        pkg2.name = "bad-pkg".into();
        pkg2.store_path = "/nix/store/bad-pkg.drv".into();
        let lock = RootLockV2 {
            packages: vec![pkg1, pkg2],
            ..RootLockV2::default()
        };
        let err = validate_store_paths(&lock).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad-pkg"));
        assert!(msg.contains("derivation path where an output path was expected"));
    }

    #[test]
    fn parse_rootfile_without_inventory_omits_empty_sections() {
        let toml_str = r#"
        [packages]
        node = "22.11.0"

        [settings]
        snapshots = true
        verify_installs = true
        "#;
        let rootfile = Rootfile::read_from_str(toml_str).unwrap();
        assert!(rootfile.agents.is_empty());
        assert!(rootfile.models.is_empty());
        let serialized = toml::to_string_pretty(&rootfile).unwrap();
        assert!(
            !serialized.contains("[agents]"),
            "empty agents must not be serialized: {serialized}"
        );
        assert!(
            !serialized.contains("[models]"),
            "empty models must not be serialized: {serialized}"
        );
    }

    #[test]
    fn parse_rootfile_inventory_round_trips() {
        let toml_str = r#"
        [packages]
        ripgrep = "14.1.1"

        [agents]
        codex = "*"
        claude = "*"

        [models."qwen3:8b"]
        runtime = "ollama"
        "#;
        let rootfile = Rootfile::read_from_str(toml_str).unwrap();
        assert_eq!(rootfile.agents.get("codex").unwrap(), "*");
        assert_eq!(rootfile.agents.get("claude").unwrap(), "*");
        assert_eq!(rootfile.models["qwen3:8b"].runtime, "ollama");

        let serialized = toml::to_string_pretty(&rootfile).unwrap();
        let parsed = Rootfile::read_from_str(&serialized).unwrap();
        assert_eq!(parsed, rootfile);
        assert!(serialized.contains("[agents]"));
        assert!(serialized.contains("qwen3:8b"));
    }

    #[test]
    fn parse_rootfile_rejects_unknown_model_fields() {
        let err = Rootfile::read_from_str(
            r#"
            [models."qwen3:8b"]
            runtime = "ollama"
            digest = "sha256:abc"
            "#,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown field") || msg.contains("digest"),
            "{msg}"
        );
    }

    #[test]
    fn parse_rootfile_rejects_agent_version_constraint() {
        let err = Rootfile::read_from_str(
            r#"
            [agents]
            codex = "0.42.0"
            "#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("version constraints are not supported in v0.2.5"));
        assert!(msg.contains("codex"));
    }

    #[test]
    fn parse_rootfile_rejects_empty_and_control_characters() {
        let mut empty_agent = Rootfile::default();
        empty_agent.agents.insert(String::new(), "*".into());
        let empty_err = empty_agent.validate().unwrap_err();
        assert!(empty_err.to_string().contains("cannot be empty"));

        let mut control = Rootfile::default();
        control
            .agents
            .insert("codex".into(), format!("*{}", '\u{0007}'));
        let control_err = control.validate().unwrap_err();
        assert!(control_err.to_string().contains("control characters"));

        let empty_runtime = Rootfile::read_from_str(
            r#"
            [models."qwen3:8b"]
            runtime = ""
            "#,
        )
        .unwrap_err();
        assert!(empty_runtime.to_string().contains("cannot be empty"));
    }

    #[test]
    fn peek_and_validate_lock_schema_version() {
        assert_eq!(
            peek_lock_schema_version(r#"{"platform":"aarch64-darwin","packages":[]}"#).unwrap(),
            ROOT_LOCK_SCHEMA_VERSION
        );
        assert_eq!(
            peek_lock_schema_version(r#"{"version":2,"platform":"aarch64-darwin"}"#).unwrap(),
            2
        );
        assert_eq!(
            peek_lock_schema_version(r#"{"version":3,"platform":"aarch64-darwin"}"#).unwrap(),
            3
        );
        assert_eq!(
            peek_lock_schema_version(r#"{"version":4,"platform":"aarch64-darwin"}"#).unwrap(),
            4
        );
        validate_supported_lock_version(1).unwrap();
        validate_supported_lock_version(2).unwrap();
        validate_supported_lock_version(3).unwrap();
        let err = validate_supported_lock_version(4).unwrap_err();
        assert!(err
            .to_string()
            .contains("newer than this Root supports (3)"));
        assert!(err.to_string().contains("Upgrade Root"));
    }

    #[test]
    fn max_2_validator_still_rejects_3() {
        let err = validate_supported_lock_version_with_max(3, 2).unwrap_err();
        assert!(err
            .to_string()
            .contains("newer than this Root supports (2)"));
        validate_supported_lock_version_with_max(2, 2).unwrap();
    }

    #[test]
    fn future_lock_version_still_deserializes_but_is_rejected_by_guard() {
        let json = r#"{"version":4,"platform":"aarch64-darwin","packages":[]}"#;
        let parsed = RootLockV2::read_from_str(json).unwrap();
        assert_eq!(parsed.version, 4);
        assert!(validate_supported_lock_version(parsed.version).is_err());
    }

    fn canonical_digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    fn sample_locked_model() -> LockedModel {
        LockedModel {
            runtime: "ollama".into(),
            requested_name: "qwen3:8b".into(),
            observed_digest: canonical_digest(),
            size_bytes: Some(42),
            endpoint: Some("http://127.0.0.1:11434".into()),
            backend_version: Some("0.11.0".into()),
            locked_at: "2026-09-01T00:00:00Z".into(),
            verified_at: "2026-09-01T00:00:01Z".into(),
            verification_method: "inspect_tags_digest".into(),
            addressability: "verification_record_only".into(),
        }
    }

    fn sample_v3_models() -> BTreeMap<String, BTreeMap<String, LockedModel>> {
        let mut inner = BTreeMap::new();
        inner.insert("qwen3:8b".into(), sample_locked_model());
        let mut models = BTreeMap::new();
        models.insert("ollama".into(), inner);
        models
    }

    #[test]
    fn emit_lock_version_stays_2_when_models_empty() {
        assert_eq!(
            emit_lock_version(&BTreeMap::new()),
            ROOT_LOCK_SCHEMA_VERSION
        );
    }

    #[test]
    fn emit_lock_version_is_3_when_models_present() {
        assert_eq!(emit_lock_version(&sample_v3_models()), 3);
    }

    #[test]
    fn parse_rootlock_v3_models_object_map() {
        let digest = canonical_digest();
        let json = format!(
            r#"{{
              "version": 3,
              "platform": "aarch64-darwin",
              "packages": [],
              "models": {{
                "ollama": {{
                  "qwen3:8b": {{
                    "runtime": "ollama",
                    "requested_name": "qwen3:8b",
                    "observed_digest": "{digest}",
                    "size_bytes": 42,
                    "endpoint": "http://127.0.0.1:11434",
                    "backend_version": "0.11.0",
                    "locked_at": "2026-09-01T00:00:00Z",
                    "verified_at": "2026-09-01T00:00:01Z",
                    "verification_method": "inspect_tags_digest",
                    "addressability": "verification_record_only"
                  }}
                }}
              }}
            }}"#
        );
        let lock = RootLockV2::read_from_str(&json).unwrap();
        assert_eq!(lock.version, 3);
        validate_supported_lock_version(lock.version).unwrap();
        validate_locked_models(&lock).unwrap();
        let model = &lock.models["ollama"]["qwen3:8b"];
        assert_eq!(model.runtime, "ollama");
        assert_eq!(model.observed_digest, digest);
        assert_eq!(model.endpoint.as_deref(), Some("http://127.0.0.1:11434"));

        let serialized = serde_json::to_value(&lock).unwrap();
        assert!(serialized["models"].is_object());
        assert!(!serialized["models"].is_array());
    }

    #[test]
    fn empty_models_are_omitted_from_serialized_lock() {
        let lock = RootLockV2 {
            version: 2,
            platform: "aarch64-darwin".into(),
            ..RootLockV2::default()
        };
        let serialized = serde_json::to_value(&lock).unwrap();
        assert!(serialized.get("models").is_none());
        assert!(validate_locked_models(&lock).is_ok());
    }

    #[test]
    fn v3_models_array_does_not_deserialize_as_object_map() {
        let json = r#"{"version":3,"platform":"aarch64-darwin","packages":[],"models":[]}"#;
        assert!(RootLockV2::read_from_str(json).is_err());
    }

    #[test]
    fn compatible_reader_rejects_unparseable_v3_models() {
        let json = r#"{
          "version": 3,
          "platform": "aarch64-darwin",
          "nixpkgs": {"rev": "abc", "source": "github:NixOS/nixpkgs"},
          "packages": [],
          "models": []
        }"#;
        assert!(read_compatible_lock_v2_from_str(json).is_err());
        let v1 = r#"{
          "version": 1,
          "platform": "aarch64-darwin",
          "nixpkgs": {"rev": "abc", "source": "github:NixOS/nixpkgs"},
          "packages": []
        }"#;
        let lock = read_compatible_lock_v2_from_str(v1).unwrap();
        assert!(lock.models.is_empty());
    }

    #[test]
    fn validate_locked_models_rejects_non_canonical_digests() {
        let cases = [
            format!("sha256-{}", "a".repeat(64)),
            "a".repeat(64),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "a".repeat(63)),
        ];
        for digest in cases {
            let mut models = sample_v3_models();
            models
                .get_mut("ollama")
                .unwrap()
                .get_mut("qwen3:8b")
                .unwrap()
                .observed_digest = digest.clone();
            let lock = RootLockV2 {
                version: 3,
                models,
                ..RootLockV2::default()
            };
            let err = validate_locked_models(&lock).unwrap_err().to_string();
            assert!(
                err.contains("observed_digest must be canonical"),
                "digest {digest} should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn validate_locked_models_rejects_non_ollama_runtime() {
        let mut models = sample_v3_models();
        models
            .get_mut("ollama")
            .unwrap()
            .get_mut("qwen3:8b")
            .unwrap()
            .runtime = "llama.cpp".into();
        let lock = RootLockV2 {
            version: 3,
            models,
            ..RootLockV2::default()
        };
        let err = validate_locked_models(&lock).unwrap_err().to_string();
        assert!(err.contains("runtime must be \"ollama\""));
    }

    #[test]
    fn validate_locked_models_rejects_non_ollama_namespace() {
        let mut models = sample_v3_models();
        let inner = models.remove("ollama").unwrap();
        models.insert("llama.cpp".into(), inner);
        let lock = RootLockV2 {
            version: 3,
            models,
            ..RootLockV2::default()
        };
        let err = validate_locked_models(&lock).unwrap_err().to_string();
        assert!(err.contains("runtime namespace must be \"ollama\""));
    }

    #[test]
    fn validate_locked_models_rejects_non_loopback_endpoint() {
        let mut models = sample_v3_models();
        models
            .get_mut("ollama")
            .unwrap()
            .get_mut("qwen3:8b")
            .unwrap()
            .endpoint = Some("http://192.168.1.10:11434".into());
        let lock = RootLockV2 {
            version: 3,
            models,
            ..RootLockV2::default()
        };
        let err = validate_locked_models(&lock).unwrap_err().to_string();
        assert!(err.contains("endpoint must be loopback"));
    }

    #[test]
    fn validate_locked_models_accepts_ipv6_loopback() {
        let mut models = sample_v3_models();
        models
            .get_mut("ollama")
            .unwrap()
            .get_mut("qwen3:8b")
            .unwrap()
            .endpoint = Some("http://[::1]:11434".into());
        let lock = RootLockV2 {
            version: 3,
            models,
            ..RootLockV2::default()
        };
        validate_locked_models(&lock).unwrap();
    }

    #[test]
    fn validate_locked_models_rejects_credential_looking_strings() {
        let mut models = sample_v3_models();
        models
            .get_mut("ollama")
            .unwrap()
            .get_mut("qwen3:8b")
            .unwrap()
            .requested_name = "secret-model".into();
        let lock = RootLockV2 {
            version: 3,
            models,
            ..RootLockV2::default()
        };
        let err = validate_locked_models(&lock).unwrap_err().to_string();
        assert!(err.contains("credential-looking string"));
    }
}
