use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum NixError {
    #[error(
        "Nix is not installed or not found on PATH.\n\n\
         Install Nix using the official installer:\n\
           curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install\n\n\
         Then run:\n  root doctor"
    )]
    NotInstalled,
    #[error("The package '{0}' is not available for your system architecture.")]
    PlatformMissing(String),
    #[error(
        "Package '{0}' was not found in the nixpkgs repository.\n\n\
         The package may have been removed or renamed."
    )]
    NotFound(String),
    #[error(
        "Nix is installed but the 'flakes' experimental feature is not enabled.\n\n\
         Root requires flakes to resolve packages.\n\n\
         Enable it in ~/.config/nix/nix.conf:\n  experimental-features = nix-command flakes"
    )]
    FlakesDisabled,
    #[error(
        "Nix is installed but the 'nix-command' experimental feature is not enabled.\n\n\
         Root requires the nix-command feature to manage profiles.\n\n\
         Enable it in ~/.config/nix/nix.conf:\n  experimental-features = nix-command flakes"
    )]
    NixCommandDisabled,
    #[error(
        "Could not reach the Nix package repository (nixpkgs).\n\n\
         Check your internet connection and try again."
    )]
    NixpkgsUnavailable,
    #[error(
        "Package '{0}' was not found in the nixpkgs repository.\n\n\
         The package may have been removed or renamed."
    )]
    AttributeMissing(String),
    #[error(
        "The Nix profile is currently locked by another process.\n\n\
         Wait a moment and try again."
    )]
    ProfileLocked,
    #[error(
        "Nix could not update the profile due to a symlink conflict.\n\n\
         This usually happens when the profile path was pre-created as a directory.\n\
         Run 'root doctor' to fix this."
    )]
    ProfileSymlinkConflict,
    #[error(
        "Root does not have permission to write to the Nix profile.\n\n\
         You may need to run with appropriate permissions or check\n\
         ~/.root/profiles/default ownership."
    )]
    PermissionDenied,
    #[error(
        "The Nix store path for '{0}' has not been built yet.\n\n\
         Run 'root install {0}' again."
    )]
    StorePathNotRealized(String),
    #[error(
        "Internal error: a derivation path (.drv) was found where an output path was expected.\n\n\
         This indicates a lockfile or snapshot is corrupted.\n\
         Run 'root doctor' for recovery steps."
    )]
    DerivationPathAsOutput(String),
    #[error(
        "Nix returned an unexpected error.\n\n\
         Run with the --json flag to see technical details."
    )]
    Generic(String),
    #[error("{0}")]
    Internal(String),
}

impl NixError {
    pub fn raw_stderr(&self) -> Option<&str> {
        match self {
            NixError::Generic(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExperimentalFeatureStatus {
    AllAvailable,
    NixCommandMissing,
    FlakesMissing,
    BothMissing,
    NixpkgsResolutionFailed,
    NixNotAvailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileEntry {
    pub index: u64,
    pub attr_path: Option<String>,
    pub original_url: Option<String>,
    pub installable: Option<String>,
    pub store_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlakeMetadata {
    pub original_url: String,
    pub locked_url: Option<String>,
    pub rev: Option<String>,
    pub nar_hash: Option<String>,
    pub last_modified: Option<u64>,
    pub raw_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageMetadata {
    pub package: String,
    pub installable: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub raw_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOutputPath {
    pub output_name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationInfo {
    pub package: String,
    pub installable: String,
    pub derivation_path: PathBuf,
    pub output_paths: Vec<BuildOutputPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathInfo {
    pub path: PathBuf,
    pub nar_hash: Option<String>,
    pub nar_size: Option<u64>,
    pub closure_size: Option<u64>,
    pub references: Vec<PathBuf>,
    pub raw_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPackageResolution {
    pub package: String,
    pub installable: String,
    pub metadata: PackageMetadata,
    pub derivation: DerivationInfo,
    pub outputs: Vec<BuildOutputPath>,
    pub path_info: Vec<PathInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryCheck {
    pub name: String,
    pub exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileValidation {
    pub profile_exists: bool,
    pub generation_before: Option<u64>,
    pub generation_after: Option<u64>,
    pub generation_changed: bool,
    pub expected_packages_present: bool,
    pub missing_output_paths: Vec<String>,
    pub binaries: Vec<BinaryCheck>,
    pub drv_paths_found: Vec<String>,
    pub errors: Vec<String>,
}

pub trait NixAdapter {
    fn check_availability(&self) -> Result<bool, NixError>;
    fn search(&self, package: &str) -> Result<String, NixError>;
    fn install(&self, package: &str) -> Result<(), NixError>;
    fn install_installable(&self, package: &str, installable: &str) -> Result<(), NixError>;
    fn list(&self) -> Result<String, NixError>;
    fn remove(&self, package_or_index: &str) -> Result<(), NixError>;
    fn profile_list_json(&self) -> Result<String, NixError>;
    fn flake_metadata(&self, flake_ref: &str) -> Result<FlakeMetadata, NixError>;
    fn eval_package_metadata(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<PackageMetadata, NixError>;
    fn build_output_paths(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<Vec<BuildOutputPath>, NixError>;
    fn derivation_path(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<DerivationInfo, NixError>;
    fn path_info(&self, path_or_installable: &str) -> Result<PathInfo, NixError>;
    fn probe_experimental_features(&self) -> Result<ExperimentalFeatureStatus, NixError>;

    fn profile_generation(&self) -> Result<Option<u64>, NixError>;
    fn profile_exists(&self) -> bool;
    fn profile_path(&self) -> Result<PathBuf, NixError>;

    fn validate_profile_mutation(
        &self,
        before_generation: Option<u64>,
        expected_packages: &[&str],
        expected_binaries: &[&str],
        expected_store_paths: &[&str],
    ) -> Result<ProfileValidation, NixError> {
        let mut validation = ProfileValidation {
            profile_exists: self.profile_exists(),
            generation_before: before_generation,
            generation_after: None,
            generation_changed: false,
            expected_packages_present: true,
            missing_output_paths: Vec::new(),
            binaries: Vec::new(),
            drv_paths_found: Vec::new(),
            errors: Vec::new(),
        };

        match self.profile_generation() {
            Ok(Some(gen)) => {
                validation.generation_after = Some(gen);
                validation.generation_changed = before_generation
                    .map(|before| before != gen)
                    .unwrap_or(true);
            }
            Ok(None) => {
                validation.generation_changed = false;
            }
            Err(e) => {
                validation
                    .errors
                    .push(format!("Failed to check profile generation: {}", e));
            }
        }

        if !expected_packages.is_empty() || !expected_store_paths.is_empty() {
            match self.profile_list_json() {
                Ok(profile_json) => {
                    for package in expected_packages {
                        if !profile_json.contains(&format!("\"{}\"", json_escape(package))) {
                            validation.expected_packages_present = false;
                            validation.errors.push(format!(
                                "Profile does not contain expected package: {}",
                                package
                            ));
                        }
                    }
                    for path in expected_store_paths {
                        if path.ends_with(".drv") {
                            validation.drv_paths_found.push(path.to_string());
                            validation
                                .errors
                                .push(format!("Expected output path is a .drv path: {}", path));
                            validation.expected_packages_present = false;
                        } else if !profile_json.contains(path) {
                            validation.expected_packages_present = false;
                            validation.missing_output_paths.push(path.to_string());
                            validation.errors.push(format!(
                                "Profile does not contain expected store path: {}",
                                path
                            ));
                        }
                    }
                }
                Err(e) => {
                    validation
                        .errors
                        .push(format!("Failed to list profile: {}", e));
                }
            }
        }

        if !expected_binaries.is_empty() {
            match self.profile_path() {
                Ok(profile_path) => {
                    let bin_dir = profile_path.join("bin");
                    if bin_dir.exists() {
                        for binary in expected_binaries {
                            let bin_path = bin_dir.join(binary);
                            let exists = bin_path.exists();
                            validation.binaries.push(BinaryCheck {
                                name: binary.to_string(),
                                exists,
                            });
                            if !exists {
                                validation.errors.push(format!(
                                    "Expected binary '{}' not found in profile bin dir",
                                    binary
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    validation
                        .errors
                        .push(format!("Cannot determine profile path: {}", e));
                }
            }
        }

        Ok(validation)
    }

    fn resolve_locked_package(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<LockedPackageResolution, NixError> {
        let installable = installable_for(package, pinned_installable);
        let metadata = self.eval_package_metadata(package, pinned_installable)?;
        let derivation = self.derivation_path(package, pinned_installable)?;
        let outputs = self.build_output_paths(package, pinned_installable)?;
        let path_info = outputs
            .iter()
            .map(|output| self.path_info(&output.path.to_string_lossy()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(LockedPackageResolution {
            package: package.to_string(),
            installable,
            metadata,
            derivation,
            outputs,
            path_info,
        })
    }
}

pub struct RealNixAdapter {
    profile_path: PathBuf,
}

impl Default for RealNixAdapter {
    fn default() -> Self {
        Self::new_default()
    }
}

impl RealNixAdapter {
    pub fn new(profile_path: PathBuf) -> Self {
        Self { profile_path }
    }

    pub fn new_default() -> Self {
        let home = dirs::home_dir().expect("Could not determine home directory");
        Self {
            profile_path: home.join(".root").join("profiles").join("default"),
        }
    }

    fn profile_path_str(&self) -> Result<&str, NixError> {
        self.profile_path
            .to_str()
            .ok_or_else(|| NixError::Internal("Profile path is not valid UTF-8".to_string()))
    }

    fn run_command(
        args: &[&str],
        extra_args: &[&str],
        package_context: Option<&str>,
    ) -> Result<String, NixError> {
        let output = Command::new("nix")
            .arg("--extra-experimental-features")
            .arg("nix-command flakes")
            .args(args)
            .args(extra_args)
            .output()
            .map_err(|_| NixError::NotInstalled)?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(stdout)
        } else {
            Self::normalize_error(&stderr, package_context.unwrap_or("unknown"))
        }
    }

    fn normalize_error(stderr: &str, package_context: &str) -> Result<String, NixError> {
        let trimmed = stderr.trim();
        let lower = stderr.to_lowercase();

        // Unsupported platform (e.g. "attribute 'aarch64-darwin' missing from derivation")
        if lower.contains("attribute") && lower.contains("missing from derivation") {
            return Err(NixError::PlatformMissing(package_context.to_string()));
        }

        // Package not found (e.g. "error: no outputs found")
        if lower.contains("error: no outputs found") {
            return Err(NixError::NotFound(package_context.to_string()));
        }

        // Flakes disabled
        if lower.contains("experimental feature")
            && lower.contains("flakes")
            && lower.contains("is not enabled")
        {
            return Err(NixError::FlakesDisabled);
        }

        // nix-command disabled
        if lower.contains("experimental feature")
            && lower.contains("nix-command")
            && lower.contains("is not enabled")
        {
            return Err(NixError::NixCommandDisabled);
        }

        // Network / nixpkgs unreachable
        if lower.contains("cannot connect")
            || lower.contains("could not resolve")
            || lower.contains("connection refused")
            || lower.contains("temporary failure in name resolution")
            || lower.contains("network is unreachable")
            || (lower.contains("network") && lower.contains("unreachable"))
        {
            return Err(NixError::NixpkgsUnavailable);
        }

        // Attribute not found in nixpkgs (different from platform-missing attribute above)
        if lower.contains("attribute")
            && !lower.contains("missing from derivation")
            && (lower.contains("not found")
                || lower.contains("does not exist")
                || lower.contains("in selection path"))
        {
            return Err(NixError::AttributeMissing(package_context.to_string()));
        }

        // Profile locked / busy
        if (lower.contains("lock") || lower.contains("busy"))
            && (lower.contains("profile")
                || lower.contains("lock file")
                || lower.contains("acquiring"))
        {
            return Err(NixError::ProfileLocked);
        }

        // Profile symlink conflict
        if (lower.contains("reading symbolic link") || lower.contains("invalid argument"))
            || (lower.contains("symlink") && lower.contains("conflict"))
        {
            return Err(NixError::ProfileSymlinkConflict);
        }

        // Permission denied
        if lower.contains("permission denied")
            || lower.contains("not writable")
            || (lower.contains("could not open") && lower.contains("profile"))
        {
            return Err(NixError::PermissionDenied);
        }

        // Store path not realized (not built yet)
        if lower.contains("store path") && lower.contains("does not exist") {
            return Err(NixError::StorePathNotRealized(package_context.to_string()));
        }

        // Derivation path mixed with output path
        if lower.contains(".drv") && lower.contains("output path") {
            return Err(NixError::DerivationPathAsOutput(
                package_context.to_string(),
            ));
        }

        // Fallback: store raw stderr for --json mode
        Err(NixError::Generic(trimmed.to_string()))
    }

    fn eval_json_attr(
        package: &str,
        installable: &str,
        attr: &str,
    ) -> Result<Option<String>, NixError> {
        let expr = format!("{}.{}", installable, attr);
        match Self::run_command(&["eval", "--json", &expr], &[], Some(package)) {
            Ok(stdout) => Ok(json_string_value(stdout.trim())),
            Err(NixError::Generic(_) | NixError::AttributeMissing(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

impl NixAdapter for RealNixAdapter {
    fn profile_generation(&self) -> Result<Option<u64>, NixError> {
        if !self.profile_path.exists() {
            return Ok(None);
        }
        let target = std::fs::read_link(&self.profile_path)
            .map_err(|e| NixError::Generic(format!("Cannot read profile symlink: {}", e)))?;
        let filename = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if let Some(rest) = filename.strip_suffix("-link") {
            if let Some((_, gen_str)) = rest.rsplit_once('-') {
                if let Ok(gen) = gen_str.parse::<u64>() {
                    return Ok(Some(gen));
                }
            }
        }
        Ok(None)
    }

    fn profile_exists(&self) -> bool {
        self.profile_path.exists()
    }

    fn profile_path(&self) -> Result<PathBuf, NixError> {
        Ok(self.profile_path.clone())
    }

    fn check_availability(&self) -> Result<bool, NixError> {
        match Self::run_command(&["--version"], &[], None) {
            Ok(_) => Ok(true),
            Err(NixError::NotInstalled) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn probe_experimental_features(&self) -> Result<ExperimentalFeatureStatus, NixError> {
        let output = Command::new("nix")
            .arg("eval")
            .arg("nixpkgs#hello")
            .output()
            .map_err(|_| NixError::NotInstalled)?;

        if output.status.success() {
            return Ok(ExperimentalFeatureStatus::AllAvailable);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);

        let nix_cmd_missing = stderr.contains("experimental feature 'nix-command'")
            || stderr.contains("'nix-command' is not enabled");
        let flakes_missing = stderr.contains("experimental feature 'flakes'")
            || stderr.contains("'flakes' is not enabled");

        if nix_cmd_missing && flakes_missing {
            Ok(ExperimentalFeatureStatus::BothMissing)
        } else if nix_cmd_missing {
            Ok(ExperimentalFeatureStatus::NixCommandMissing)
        } else if flakes_missing {
            Ok(ExperimentalFeatureStatus::FlakesMissing)
        } else if stderr.contains("cannot find attribute") || stderr.contains("does not exist") {
            Ok(ExperimentalFeatureStatus::NixpkgsResolutionFailed)
        } else {
            Err(NixError::Generic(stderr.trim().to_string()))
        }
    }

    fn search(&self, package: &str) -> Result<String, NixError> {
        Self::run_command(&["search", "nixpkgs", package], &[], Some(package))
    }

    fn install(&self, package: &str) -> Result<(), NixError> {
        let pkg_arg = format!("nixpkgs#{}", package);
        let profile_str = self.profile_path_str()?;
        Self::run_command(
            &["profile", "add", &pkg_arg],
            &["--profile", profile_str],
            Some(package),
        )
        .map(|_| ())
    }

    fn install_installable(&self, package: &str, installable: &str) -> Result<(), NixError> {
        let profile_str = self.profile_path_str()?;
        Self::run_command(
            &["profile", "add", installable],
            &["--profile", profile_str],
            Some(package),
        )
        .map(|_| ())
    }

    fn list(&self) -> Result<String, NixError> {
        let profile_str = self.profile_path_str()?;
        Self::run_command(&["profile", "list"], &["--profile", profile_str], None)
    }

    fn remove(&self, package_or_index: &str) -> Result<(), NixError> {
        let profile_str = self.profile_path_str()?;
        Self::run_command(
            &["profile", "remove", package_or_index],
            &["--profile", profile_str],
            Some(package_or_index),
        )
        .map(|_| ())
    }

    fn profile_list_json(&self) -> Result<String, NixError> {
        let profile_str = self.profile_path_str()?;
        Self::run_command(
            &["profile", "list", "--json"],
            &["--profile", profile_str],
            None,
        )
    }

    fn flake_metadata(&self, flake_ref: &str) -> Result<FlakeMetadata, NixError> {
        let raw_json = Self::run_command(&["flake", "metadata", "--json", flake_ref], &[], None)?;
        Ok(FlakeMetadata {
            original_url: json_field_string(&raw_json, "originalUrl")
                .or_else(|| json_field_string(&raw_json, "url"))
                .unwrap_or_else(|| flake_ref.to_string()),
            locked_url: json_field_string(&raw_json, "lockedUrl"),
            rev: json_field_string(&raw_json, "rev"),
            nar_hash: json_field_string(&raw_json, "narHash"),
            last_modified: json_field_u64(&raw_json, "lastModified"),
            raw_json,
        })
    }

    fn eval_package_metadata(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<PackageMetadata, NixError> {
        let installable = installable_for(package, pinned_installable);
        let raw_json = Self::run_command(
            &["eval", "--json", &format!("{}.meta", installable)],
            &[],
            Some(package),
        )?;
        Ok(PackageMetadata {
            package: package.to_string(),
            installable: installable.clone(),
            name: Self::eval_json_attr(package, &installable, "name")?,
            version: Self::eval_json_attr(package, &installable, "version")?,
            description: json_field_string(&raw_json, "description"),
            raw_json,
        })
    }

    fn build_output_paths(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<Vec<BuildOutputPath>, NixError> {
        let installable = installable_for(package, pinned_installable);
        let raw_json = Self::run_command(
            &[
                "build",
                "--no-link",
                "--print-out-paths",
                "--json",
                &installable,
            ],
            &[],
            Some(package),
        )?;
        let store_paths = json_store_paths(&raw_json);
        if store_paths.is_empty() {
            let all_strings = extract_json_strings(&raw_json);
            let has_drv = all_strings.iter().any(|s| s.ends_with(".drv"));
            if has_drv {
                return Err(NixError::Generic(format!(
                    "Nix build returned only derivation paths for '{}'. \
                     Expected at least one realized output path, but all paths ended in .drv.",
                    package
                )));
            }
            return Err(NixError::Generic(format!(
                "Nix build returned no output paths for '{}'. \
                 Expected at least one realized output store path.",
                package
            )));
        }
        let mut outputs = Vec::new();
        for path in store_paths {
            outputs.push(BuildOutputPath {
                output_name: if outputs.is_empty() {
                    "out".to_string()
                } else {
                    format!("out{}", outputs.len())
                },
                path: PathBuf::from(path),
            });
        }
        Ok(outputs)
    }

    fn derivation_path(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<DerivationInfo, NixError> {
        let installable = installable_for(package, pinned_installable);
        let drv_path = Self::run_command(
            &["eval", "--raw", &format!("{}.drvPath", installable)],
            &[],
            Some(package),
        )?;
        let output_paths = self.build_output_paths(package, pinned_installable)?;
        Ok(DerivationInfo {
            package: package.to_string(),
            installable,
            derivation_path: PathBuf::from(drv_path.trim()),
            output_paths,
        })
    }

    fn path_info(&self, path_or_installable: &str) -> Result<PathInfo, NixError> {
        let raw_json = Self::run_command(
            &["path-info", "--json", "--closure-size", path_or_installable],
            &[],
            Some(path_or_installable),
        )?;
        Ok(PathInfo {
            path: json_store_paths(&raw_json)
                .first()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(path_or_installable)),
            nar_hash: json_field_string(&raw_json, "narHash"),
            nar_size: json_field_u64(&raw_json, "narSize"),
            closure_size: json_field_u64(&raw_json, "closureSize"),
            references: json_array_strings(&raw_json, "references")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            raw_json,
        })
    }
}

pub struct MockNixAdapter {
    pub installed: bool,
    pub installed_packages: std::sync::Mutex<Vec<String>>,
    pub nix_command_enabled: bool,
    pub flakes_enabled: bool,
    pub nixpkgs_accessible: bool,
    pub generation_counter: std::sync::atomic::AtomicU64,
    pub profile_list_json_override: std::sync::Mutex<Option<String>>,
    pub profile_present: std::sync::atomic::AtomicBool,
}

impl MockNixAdapter {
    pub fn new(installed: bool) -> Self {
        Self {
            installed,
            installed_packages: std::sync::Mutex::new(Vec::new()),
            nix_command_enabled: true,
            flakes_enabled: true,
            nixpkgs_accessible: true,
            generation_counter: std::sync::atomic::AtomicU64::new(1),
            profile_list_json_override: std::sync::Mutex::new(None),
            profile_present: std::sync::atomic::AtomicBool::new(installed),
        }
    }

    pub fn increment_generation(&self) -> u64 {
        self.generation_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    pub fn set_profile_list_json_override(&self, json: Option<String>) {
        *self.profile_list_json_override.lock().unwrap() = json;
    }

    fn ensure_available_package(&self, package: &str) -> Result<(), NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        if package == "bad_platform_pkg" {
            return Err(NixError::PlatformMissing(package.to_string()));
        }
        if package == "missing_pkg" {
            return Err(NixError::NotFound(package.to_string()));
        }
        Ok(())
    }
}

impl NixAdapter for MockNixAdapter {
    fn profile_generation(&self) -> Result<Option<u64>, NixError> {
        if !self.installed {
            return Ok(None);
        }
        Ok(Some(
            self.generation_counter
                .load(std::sync::atomic::Ordering::Relaxed),
        ))
    }

    fn profile_exists(&self) -> bool {
        self.profile_present
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn profile_path(&self) -> Result<PathBuf, NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        Ok(PathBuf::from("/tmp/root-mock-profile"))
    }
    fn check_availability(&self) -> Result<bool, NixError> {
        if self.installed {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn probe_experimental_features(&self) -> Result<ExperimentalFeatureStatus, NixError> {
        if !self.installed {
            return Ok(ExperimentalFeatureStatus::NixNotAvailable);
        }
        if !self.nix_command_enabled && !self.flakes_enabled {
            return Ok(ExperimentalFeatureStatus::BothMissing);
        }
        if !self.nix_command_enabled {
            return Ok(ExperimentalFeatureStatus::NixCommandMissing);
        }
        if !self.flakes_enabled {
            return Ok(ExperimentalFeatureStatus::FlakesMissing);
        }
        if !self.nixpkgs_accessible {
            return Ok(ExperimentalFeatureStatus::NixpkgsResolutionFailed);
        }
        Ok(ExperimentalFeatureStatus::AllAvailable)
    }

    fn search(&self, package: &str) -> Result<String, NixError> {
        self.ensure_available_package(package)?;
        Ok(format!("* nixpkgs#{0} (1.0)\n  {0} description", package))
    }

    fn install(&self, package: &str) -> Result<(), NixError> {
        self.ensure_available_package(package)?;
        self.installed_packages
            .lock()
            .unwrap()
            .push(package.to_string());
        self.increment_generation();
        Ok(())
    }

    fn install_installable(&self, package: &str, installable: &str) -> Result<(), NixError> {
        self.ensure_available_package(package)?;
        self.installed_packages
            .lock()
            .unwrap()
            .push(installable.to_string());
        self.increment_generation();
        Ok(())
    }

    fn list(&self) -> Result<String, NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        let pkgs = self.installed_packages.lock().unwrap();
        if pkgs.is_empty() {
            Ok(String::new())
        } else {
            let mut res = String::new();
            for (i, p) in pkgs.iter().enumerate() {
                res.push_str(&format!("Index: {} - {}\n", i, display_installable(p)));
            }
            Ok(res)
        }
    }

    fn remove(&self, package_or_index: &str) -> Result<(), NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        let mut pkgs = self.installed_packages.lock().unwrap();
        pkgs.retain(|p| p != package_or_index && package_from_installable(p) != package_or_index);
        self.increment_generation();
        Ok(())
    }

    fn profile_list_json(&self) -> Result<String, NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        {
            let override_guard = self.profile_list_json_override.lock().unwrap();
            if let Some(ref override_json) = *override_guard {
                return Ok(override_json.clone());
            }
        }
        let pkgs = self.installed_packages.lock().unwrap();
        let entries = pkgs
            .iter()
            .enumerate()
            .map(|(index, package)| {
                format!(
                    "{{\"index\":{},\"attrPath\":\"{}\",\"originalUrl\":\"flake:nixpkgs\",\"installable\":\"{}\",\"storePaths\":[\"{}\"]}}",
                    index,
                    json_escape(&package_from_installable(package)),
                    json_escape(package),
                    json_escape(&mock_store_path(package))
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!("[{}]", entries))
    }

    fn flake_metadata(&self, flake_ref: &str) -> Result<FlakeMetadata, NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        let locked_url = format!("github:NixOS/nixpkgs/{}", mock_rev_for(flake_ref));
        let raw_json = format!(
            "{{\"originalUrl\":\"{}\",\"lockedUrl\":\"{}\",\"rev\":\"{}\",\"narHash\":\"{}\",\"lastModified\":1700000000}}",
            json_escape(flake_ref),
            json_escape(&locked_url),
            mock_rev_for(flake_ref),
            mock_nar_hash(flake_ref)
        );
        Ok(FlakeMetadata {
            original_url: flake_ref.to_string(),
            locked_url: Some(locked_url),
            rev: Some(mock_rev_for(flake_ref)),
            nar_hash: Some(mock_nar_hash(flake_ref)),
            last_modified: Some(1_700_000_000),
            raw_json,
        })
    }

    fn eval_package_metadata(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<PackageMetadata, NixError> {
        self.ensure_available_package(package)?;
        let installable = installable_for(package, pinned_installable);
        let version = mock_version(package);
        let name = format!("{}-{}", package, version);
        let description = format!("Deterministic mock package metadata for {}", package);
        let raw_json = format!(
            "{{\"description\":\"{}\",\"homepage\":\"https://example.invalid/root/mock/{}\",\"license\":{{\"spdxId\":\"MIT\"}}}}",
            json_escape(&description),
            json_escape(package)
        );
        Ok(PackageMetadata {
            package: package.to_string(),
            installable,
            name: Some(name),
            version: Some(version),
            description: Some(description),
            raw_json,
        })
    }

    fn build_output_paths(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<Vec<BuildOutputPath>, NixError> {
        self.ensure_available_package(package)?;
        let installable = installable_for(package, pinned_installable);
        Ok(vec![BuildOutputPath {
            output_name: "out".to_string(),
            path: PathBuf::from(mock_store_path(&installable)),
        }])
    }

    fn derivation_path(
        &self,
        package: &str,
        pinned_installable: Option<&str>,
    ) -> Result<DerivationInfo, NixError> {
        self.ensure_available_package(package)?;
        let installable = installable_for(package, pinned_installable);
        let output_paths = self.build_output_paths(package, pinned_installable)?;
        Ok(DerivationInfo {
            package: package.to_string(),
            installable: installable.clone(),
            derivation_path: PathBuf::from(format!(
                "/nix/store/{}-{}.drv",
                deterministic_token(&installable),
                sanitize_store_name(package)
            )),
            output_paths,
        })
    }

    fn path_info(&self, path_or_installable: &str) -> Result<PathInfo, NixError> {
        if !self.installed {
            return Err(NixError::NotInstalled);
        }
        let path = if path_or_installable.starts_with("/nix/store/") {
            path_or_installable.to_string()
        } else {
            mock_store_path(path_or_installable)
        };
        let nar_hash = mock_nar_hash(&path);
        let raw_json = format!(
            "{{\"path\":\"{}\",\"narHash\":\"{}\",\"narSize\":4096,\"closureSize\":8192,\"references\":[\"{}\"]}}",
            json_escape(&path),
            json_escape(&nar_hash),
            json_escape(&path)
        );
        Ok(PathInfo {
            path: PathBuf::from(&path),
            nar_hash: Some(nar_hash),
            nar_size: Some(4_096),
            closure_size: Some(8_192),
            references: vec![PathBuf::from(&path)],
            raw_json,
        })
    }
}

fn package_from_installable(installable: &str) -> String {
    installable
        .rsplit_once('#')
        .map(|(_, package)| package)
        .unwrap_or(installable)
        .to_string()
}

fn display_installable(value: &str) -> String {
    if value.contains('#') {
        value.to_string()
    } else {
        format!("nixpkgs#{}", value)
    }
}

fn installable_for(package: &str, pinned_installable: Option<&str>) -> String {
    pinned_installable
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("nixpkgs#{}", package))
}

fn json_string_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        Some(unescape_json_string(&trimmed[1..trimmed.len() - 1]))
    } else {
        None
    }
}

fn json_field_string(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{}\"", field);
    let start = json.find(&marker)? + marker.len();
    let after_colon = json[start..].find(':')? + start + 1;
    let value = json[after_colon..].trim_start();
    if !value.starts_with('"') {
        return None;
    }
    let end = find_json_string_end(value)?;
    Some(unescape_json_string(&value[1..end]))
}

fn json_field_u64(json: &str, field: &str) -> Option<u64> {
    let marker = format!("\"{}\"", field);
    let start = json.find(&marker)? + marker.len();
    let after_colon = json[start..].find(':')? + start + 1;
    let value = json[after_colon..].trim_start();
    let digits = value
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn json_array_strings(json: &str, field: &str) -> Vec<String> {
    let marker = format!("\"{}\"", field);
    let Some(start) = json.find(&marker).map(|start| start + marker.len()) else {
        return Vec::new();
    };
    let Some(after_colon) = json[start..].find(':').map(|offset| offset + start + 1) else {
        return Vec::new();
    };
    let value = json[after_colon..].trim_start();
    if !value.starts_with('[') {
        return Vec::new();
    }
    let Some(end) = value.find(']') else {
        return Vec::new();
    };
    extract_json_strings(&value[..=end])
}

fn json_store_paths(json: &str) -> Vec<String> {
    extract_json_strings(json)
        .into_iter()
        .filter(|value| value.starts_with("/nix/store/") && !value.ends_with(".drv"))
        .collect()
}

fn extract_json_strings(json: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut rest = json;
    while let Some(start) = rest.find('"') {
        let after_quote = &rest[start + 1..];
        let Some(end) = find_json_string_end(after_quote) else {
            break;
        };
        strings.push(unescape_json_string(&after_quote[..end]));
        rest = &after_quote[end + 1..];
    }
    strings
}

fn find_json_string_end(value: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in value.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(index);
        }
    }
    None
}

fn unescape_json_string(value: &str) -> String {
    value
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .replace("\\/", "/")
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn mock_version(package: &str) -> String {
    format!("0.1.{}", deterministic_number(package) % 1000)
}

fn mock_store_path(package: &str) -> String {
    format!(
        "/nix/store/{}-{}-{}",
        deterministic_token(package),
        sanitize_store_name(package),
        mock_version(package)
    )
}

fn mock_rev_for(value: &str) -> String {
    format!("{:040x}", deterministic_number(value))
}

fn mock_nar_hash(value: &str) -> String {
    format!("sha256-{:052x}", deterministic_number(value))
}

fn deterministic_token(value: &str) -> String {
    format!("{:032x}", deterministic_number(value))
}

fn deterministic_number(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn sanitize_store_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "package".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Error Normalization Tests ───────────────────────────────────────

    #[test]
    fn test_normalize_platform_missing() {
        let err = RealNixAdapter::normalize_error(
            "error: attribute 'aarch64-darwin' missing from derivation 'xxx'",
            "poppler",
        )
        .unwrap_err();
        assert_eq!(err, NixError::PlatformMissing("poppler".to_string()));
    }

    #[test]
    fn test_normalize_not_found() {
        let err =
            RealNixAdapter::normalize_error("error: no outputs found", "missing_pkg").unwrap_err();
        assert_eq!(err, NixError::NotFound("missing_pkg".to_string()));
    }

    #[test]
    fn test_normalize_flakes_disabled() {
        let err = RealNixAdapter::normalize_error(
            "error: experimental feature 'flakes' is not enabled\n\
             add '--extra-experimental-features flakes' if you want to use it",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::FlakesDisabled);
    }

    #[test]
    fn test_normalize_nix_command_disabled() {
        let err = RealNixAdapter::normalize_error(
            "error: experimental feature 'nix-command' is not enabled",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::NixCommandDisabled);
    }

    #[test]
    fn test_normalize_nix_command_preferred_over_flakes() {
        // When both features are mentioned, nix-command check runs second
        let err = RealNixAdapter::normalize_error(
            "error: experimental feature 'nix-command' is not enabled",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::NixCommandDisabled);
    }

    #[test]
    fn test_normalize_nixpkgs_unreachable_connection_refused() {
        let err = RealNixAdapter::normalize_error(
            "error: cannot connect to daemon at socket\nConnection refused",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::NixpkgsUnavailable);
    }

    #[test]
    fn test_normalize_nixpkgs_unreachable_dns_failure() {
        let err = RealNixAdapter::normalize_error(
            "error: Temporary failure in name resolution",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::NixpkgsUnavailable);
    }

    #[test]
    fn test_normalize_nixpkgs_unreachable_network_unreachable() {
        let err =
            RealNixAdapter::normalize_error("error: Network is unreachable", "ffmpeg").unwrap_err();
        assert_eq!(err, NixError::NixpkgsUnavailable);
    }

    #[test]
    fn test_normalize_attribute_missing() {
        let err = RealNixAdapter::normalize_error(
            "error: attribute 'ffmpeg' in selection path 'nixpkgs#ffmpeg' not found",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::AttributeMissing("ffmpeg".to_string()));
    }

    #[test]
    fn test_normalize_attribute_does_not_exist() {
        let err = RealNixAdapter::normalize_error("error: attribute 'foo' does not exist", "foo")
            .unwrap_err();
        assert_eq!(err, NixError::AttributeMissing("foo".to_string()));
    }

    #[test]
    fn test_normalize_profile_locked() {
        let err = RealNixAdapter::normalize_error(
            "error: opening lock file '/nix/var/nix/profiles/default.lock'",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::ProfileLocked);
    }

    #[test]
    fn test_normalize_profile_busy() {
        let err = RealNixAdapter::normalize_error(
            "error: profile '/nix/var/nix/profiles/per-user/root/profile' is busy",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::ProfileLocked);
    }

    #[test]
    fn test_normalize_symlink_conflict_reading_symlink() {
        let err = RealNixAdapter::normalize_error(
            "error: reading symbolic link '/nix/var/nix/profiles/default': No such file or directory",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::ProfileSymlinkConflict);
    }

    #[test]
    fn test_normalize_symlink_conflict_invalid_argument() {
        let err = RealNixAdapter::normalize_error("error: Invalid argument", "ffmpeg").unwrap_err();
        assert_eq!(err, NixError::ProfileSymlinkConflict);
    }

    #[test]
    fn test_normalize_symlink_conflict_explicit() {
        let err = RealNixAdapter::normalize_error("error: symlink conflict in profile", "ffmpeg")
            .unwrap_err();
        assert_eq!(err, NixError::ProfileSymlinkConflict);
    }

    #[test]
    fn test_normalize_permission_denied() {
        let err =
            RealNixAdapter::normalize_error("error: Permission denied", "ffmpeg").unwrap_err();
        assert_eq!(err, NixError::PermissionDenied);
    }

    #[test]
    fn test_normalize_permission_denied_not_writable() {
        let err =
            RealNixAdapter::normalize_error("error: path '/nix/store' is not writable", "ffmpeg")
                .unwrap_err();
        assert_eq!(err, NixError::PermissionDenied);
    }

    #[test]
    fn test_normalize_permission_denied_could_not_open() {
        let err = RealNixAdapter::normalize_error(
            "error: could not open profile '/nix/var/nix/profiles/default'",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::PermissionDenied);
    }

    #[test]
    fn test_normalize_store_path_not_realized() {
        let err = RealNixAdapter::normalize_error(
            "error: store path '/nix/store/abc-ffmpeg-8.1' does not exist",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::StorePathNotRealized("ffmpeg".to_string()));
    }

    #[test]
    fn test_normalize_derivation_path_as_output() {
        let err = RealNixAdapter::normalize_error(
            "error: path '/nix/store/abc-ffmpeg-8.1.drv' is not an output path",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(err, NixError::DerivationPathAsOutput("ffmpeg".to_string()));
    }

    #[test]
    fn test_normalize_fallback_generic() {
        let err = RealNixAdapter::normalize_error(
            "error: some completely unexpected error happened",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(
            err,
            NixError::Generic("error: some completely unexpected error happened".to_string())
        );
    }

    #[test]
    fn test_normalize_fallback_preserves_raw_stderr() {
        let err = RealNixAdapter::normalize_error(
            "error: some random nix failure\nwith multiple lines",
            "ffmpeg",
        )
        .unwrap_err();
        assert_eq!(
            err.raw_stderr(),
            Some("error: some random nix failure\nwith multiple lines")
        );
    }

    #[test]
    fn test_normalize_typed_variants_have_no_raw_stderr() {
        let err = RealNixAdapter::normalize_error(
            "error: attribute 'aarch64-darwin' missing from derivation",
            "poppler",
        )
        .unwrap_err();
        assert_eq!(err.raw_stderr(), None);

        let err2 = RealNixAdapter::normalize_error("error: no outputs found", "pkg").unwrap_err();
        assert_eq!(err2.raw_stderr(), None);
    }

    #[test]
    fn test_normalize_empty_stderr() {
        let err = RealNixAdapter::normalize_error("", "ffmpeg").unwrap_err();
        assert_eq!(err, NixError::Generic(String::new()));
    }

    #[test]
    fn test_mock_adapter() {
        let mock = MockNixAdapter::new(true);
        assert!(mock.check_availability().unwrap());

        mock.install("poppler").unwrap();
        let list = mock.list().unwrap();
        assert!(list.contains("poppler"));

        let err = mock.install("bad_platform_pkg").unwrap_err();
        assert_eq!(
            err,
            NixError::PlatformMissing("bad_platform_pkg".to_string())
        );

        mock.remove("poppler").unwrap();
        let list2 = mock.list().unwrap();
        assert!(!list2.contains("poppler"));
    }

    #[test]
    fn test_mock_profile_list_json_is_deterministic() {
        let mock = MockNixAdapter::new(true);
        mock.install("ripgrep").unwrap();
        mock.install("fd").unwrap();

        let first = mock.profile_list_json().unwrap();
        let second = mock.profile_list_json().unwrap();

        assert_eq!(first, second);
        assert!(first.contains("\"attrPath\":\"ripgrep\""));
        assert!(first.contains("/nix/store/"));
        assert!(!first.contains("latest"));
        assert!(!first.contains("placeholder"));
    }

    #[test]
    fn test_mock_metadata_methods_are_deterministic() {
        let mock = MockNixAdapter::new(true);
        mock.install("ripgrep").unwrap();

        let metadata = mock
            .eval_package_metadata("ripgrep", Some("github:NixOS/nixpkgs/abcdef#ripgrep"))
            .unwrap();
        assert_eq!(
            metadata.installable,
            "github:NixOS/nixpkgs/abcdef#ripgrep".to_string()
        );
        assert_eq!(metadata.version, Some(mock_version("ripgrep")));
        assert!(!metadata.raw_json.contains("placeholder"));

        let outputs = mock
            .build_output_paths("ripgrep", Some("github:NixOS/nixpkgs/abcdef#ripgrep"))
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].output_name, "out");
        assert!(outputs[0].path.starts_with("/nix/store"));

        let derivation = mock
            .derivation_path("ripgrep", Some("github:NixOS/nixpkgs/abcdef#ripgrep"))
            .unwrap();
        assert_eq!(derivation.output_paths, outputs);
        assert!(derivation
            .derivation_path
            .to_string_lossy()
            .ends_with("ripgrep.drv"));

        let path_info = mock.path_info(outputs[0].path.to_str().unwrap()).unwrap();
        assert_eq!(path_info.path, outputs[0].path);
        assert_eq!(path_info.references, vec![outputs[0].path.clone()]);
    }

    #[test]
    fn test_mock_resolve_locked_package() {
        let mock = MockNixAdapter::new(true);
        let resolution = mock
            .resolve_locked_package("fd", Some("github:NixOS/nixpkgs/abcdef#fd"))
            .unwrap();

        assert_eq!(resolution.package, "fd");
        assert_eq!(resolution.outputs.len(), 1);
        assert_eq!(resolution.path_info.len(), 1);
        assert_eq!(resolution.outputs, resolution.derivation.output_paths);
        assert_eq!(resolution.path_info[0].path, resolution.outputs[0].path);
    }

    #[test]
    fn test_mock_metadata_special_package_errors() {
        let mock = MockNixAdapter::new(true);

        let err = mock.eval_package_metadata("missing_pkg", None).unwrap_err();
        assert_eq!(err, NixError::NotFound("missing_pkg".to_string()));

        let err = mock
            .build_output_paths("bad_platform_pkg", None)
            .unwrap_err();
        assert_eq!(
            err,
            NixError::PlatformMissing("bad_platform_pkg".to_string())
        );
    }

    #[test]
    fn test_json_helpers() {
        let json = r#"{"path":"/nix/store/abc-ripgrep","narHash":"sha256-abcd","narSize":123,"references":["/nix/store/ref-one","/nix/store/ref-two"]}"#;

        assert_eq!(
            json_field_string(json, "narHash"),
            Some("sha256-abcd".to_string())
        );
        assert_eq!(json_field_u64(json, "narSize"), Some(123));
        assert_eq!(
            json_array_strings(json, "references"),
            vec![
                "/nix/store/ref-one".to_string(),
                "/nix/store/ref-two".to_string()
            ]
        );
        assert_eq!(
            json_store_paths(json),
            vec![
                "/nix/store/abc-ripgrep".to_string(),
                "/nix/store/ref-one".to_string(),
                "/nix/store/ref-two".to_string()
            ]
        );
    }

    #[test]
    fn test_json_store_paths_filters_drv_paths() {
        let json = r#"[{"drvPath":"/nix/store/abc-ffmpeg-8.1.drv","outputs":{"out":"/nix/store/xyz-ffmpeg-8.1"}}]"#;

        let paths = json_store_paths(json);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/nix/store/xyz-ffmpeg-8.1");
        assert!(!paths[0].ends_with(".drv"));
    }

    #[test]
    fn test_json_store_paths_multiple_outputs_with_drv() {
        let json = r#"[{"drvPath":"/nix/store/abc-pkg.drv","outputs":{"out":"/nix/store/out-pkg","dev":"/nix/store/dev-pkg"}}]"#;

        let paths = json_store_paths(json);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], "/nix/store/out-pkg");
        assert_eq!(paths[1], "/nix/store/dev-pkg");
        for p in &paths {
            assert!(!p.ends_with(".drv"));
        }
    }

    #[test]
    fn test_json_store_paths_returns_empty_when_all_are_drv() {
        let json = r#"/nix/store/abc-ffmpeg-8.1.drv"#;
        let paths = json_store_paths(json);
        assert!(
            paths.is_empty(),
            "expected no output paths, got {:?}",
            paths
        );
    }

    #[test]
    fn test_json_store_paths_only_drv_in_json_objects() {
        let json = r#"{"drvPath":"/nix/store/abc-pkg.drv","outputs":{}}"#;
        let paths = json_store_paths(json);
        assert!(
            paths.is_empty(),
            "expected no output paths, got {:?}",
            paths
        );
    }

    #[test]
    fn test_mock_probe_nix_not_installed() {
        let mock = MockNixAdapter::new(false);
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::NixNotAvailable
        );
    }

    #[test]
    fn test_mock_probe_all_available() {
        let mock = MockNixAdapter::new(true);
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::AllAvailable
        );
    }

    #[test]
    fn test_mock_probe_nix_command_missing() {
        let mut mock = MockNixAdapter::new(true);
        mock.nix_command_enabled = false;
        mock.flakes_enabled = true;
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::NixCommandMissing
        );
    }

    #[test]
    fn test_mock_probe_flakes_missing() {
        let mut mock = MockNixAdapter::new(true);
        mock.nix_command_enabled = true;
        mock.flakes_enabled = false;
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::FlakesMissing
        );
    }

    #[test]
    fn test_mock_probe_both_missing() {
        let mut mock = MockNixAdapter::new(true);
        mock.nix_command_enabled = false;
        mock.flakes_enabled = false;
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::BothMissing
        );
    }

    #[test]
    fn test_mock_probe_nixpkgs_resolution_failed() {
        let mut mock = MockNixAdapter::new(true);
        mock.nix_command_enabled = true;
        mock.flakes_enabled = true;
        mock.nixpkgs_accessible = false;
        assert_eq!(
            mock.probe_experimental_features().unwrap(),
            ExperimentalFeatureStatus::NixpkgsResolutionFailed
        );
    }

    // ─── Profile Validation Tests ───────────────────────────────────────

    #[test]
    fn test_mock_profile_generation_tracking() {
        let mock = MockNixAdapter::new(true);
        assert_eq!(mock.profile_generation().unwrap(), Some(1));

        mock.install("ripgrep").unwrap();
        assert_eq!(mock.profile_generation().unwrap(), Some(2));

        mock.remove("ripgrep").unwrap();
        assert_eq!(mock.profile_generation().unwrap(), Some(3));
    }

    #[test]
    fn test_mock_profile_generation_not_installed() {
        let mock = MockNixAdapter::new(false);
        assert_eq!(mock.profile_generation().unwrap(), None);
    }

    #[test]
    fn test_mock_profile_exists() {
        let mock = MockNixAdapter::new(true);
        assert!(mock.profile_exists());

        let mock2 = MockNixAdapter::new(false);
        assert!(!mock2.profile_exists());
    }

    #[test]
    fn test_mock_validate_mutation_success() {
        let mock = MockNixAdapter::new(true);
        let before = mock.profile_generation().unwrap();
        mock.install("ripgrep").unwrap();

        // Create mock binary so binary check passes
        std::fs::create_dir_all("/tmp/root-mock-profile/bin").ok();
        let _ = std::fs::write("/tmp/root-mock-profile/bin/rg", "mock binary content");

        let mock_path = mock_store_path("ripgrep");
        let result = mock
            .validate_profile_mutation(before, &["ripgrep"], &["rg"], &[&mock_path])
            .unwrap();

        assert!(result.profile_exists);
        assert!(result.generation_changed);
        assert_eq!(result.generation_before, Some(1));
        assert_eq!(result.generation_after, Some(2));
        assert!(result.expected_packages_present);
        assert!(result.missing_output_paths.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_mock_validate_mutation_fails_on_missing_output_path() {
        let mock = MockNixAdapter::new(true);
        let before = mock.profile_generation().unwrap();
        mock.install("ripgrep").unwrap();

        // Override profile list json to remove the expected path
        mock.set_profile_list_json_override(Some(r#"[{"index":0,"attrPath":"ripgrep","originalUrl":"flake:nixpkgs","installable":"nixpkgs#ripgrep","storePaths":["/nix/store/abc-other-pkg"]}]"#.to_string()));

        let result = mock
            .validate_profile_mutation(
                before,
                &["ripgrep"],
                &[],
                &["/nix/store/expected-missing-path"],
            )
            .unwrap();

        assert!(result.profile_exists);
        assert!(!result.expected_packages_present);
        assert!(!result.missing_output_paths.is_empty());
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("does not contain expected store path")));
    }

    #[test]
    fn test_mock_validate_mutation_rejects_drv_path() {
        let mock = MockNixAdapter::new(true);
        let before = mock.profile_generation().unwrap();
        mock.install("ripgrep").unwrap();

        let result = mock
            .validate_profile_mutation(before, &["ripgrep"], &[], &["/nix/store/abc-ripgrep.drv"])
            .unwrap();

        assert!(!result.drv_paths_found.is_empty());
        assert!(!result.expected_packages_present);
        assert!(result.errors.iter().any(|e| e.contains(".drv path")));
    }

    #[test]
    fn test_mock_validate_mutation_no_profile() {
        let mock = MockNixAdapter::new(false);
        let result = mock.validate_profile_mutation(None, &[], &[], &[]).unwrap();

        assert!(!result.profile_exists);
        assert_eq!(result.generation_before, None);
        assert_eq!(result.generation_after, None);
        assert!(!result.generation_changed);
        assert!(result.expected_packages_present);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_mock_validate_uninstalled_fails_gracefully() {
        let mock = MockNixAdapter::new(false);
        let result = mock
            .validate_profile_mutation(
                Some(5),
                &["missing-pkg"],
                &["binary"],
                &["/nix/store/missing-path"],
            )
            .unwrap();

        assert!(!result.profile_exists);
        assert_eq!(result.generation_before, Some(5));
        assert_eq!(result.generation_after, None);
        assert!(!result.generation_changed);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Failed to list profile")));
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Cannot determine profile path")));
    }

    #[test]
    fn test_mock_validate_generation_unchanged() {
        let mock = MockNixAdapter::new(true);
        // Don't mutate -- generation stays the same
        let before = mock.profile_generation().unwrap();

        let mock_path = mock_store_path("ripgrep");
        // Override profile list json so it contains the path
        mock.set_profile_list_json_override(Some(
            format!(
                r#"[{{"index":0,"attrPath":"ripgrep","originalUrl":"flake:nixpkgs","installable":"nixpkgs#ripgrep","storePaths":["{}"]}}]"#,
                mock_path
            )
        ));

        let result = mock
            .validate_profile_mutation(before, &["ripgrep"], &[], &[&mock_path])
            .unwrap();

        assert!(result.profile_exists);
        assert!(!result.generation_changed);
        assert!(result.expected_packages_present);
    }

    #[test]
    fn test_mock_validate_generation_incremented_on_install() {
        let mock = MockNixAdapter::new(true);
        let before = mock.profile_generation().unwrap();
        assert_eq!(before, Some(1));
        mock.install("ripgrep").unwrap();

        let after = mock.profile_generation().unwrap();
        assert_eq!(after, Some(2));
        assert_ne!(before, after);
    }
}
