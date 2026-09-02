use anyhow::Result;
use root_lockfile::{get_root_dir, is_unknown_nixpkgs_rev, RootLock, RootLockV2, Rootfile};
use root_nix::NixAdapter;
use serde::Serialize;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum IssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct DoctorIssue {
    pub severity: IssueSeverity,
    pub category: String,
    pub description: String,
    pub suggestion: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct DoctorReport {
    pub healthy: bool,
    pub nix_installed: bool,
    pub root_initialized: bool,
    pub issues: Vec<DoctorIssue>,
}

pub fn run_diagnostics(adapter: &impl NixAdapter) -> Result<DoctorReport> {
    let mut report = DoctorReport {
        healthy: true,
        nix_installed: false,
        root_initialized: false,
        issues: Vec::new(),
    };

    // 1. Check Nix Status
    match adapter.check_availability() {
        Ok(available) => {
            report.nix_installed = available;
            if !available {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description: "Nix is not installed or not available on PATH.\n\nRoot uses Nix to build packages from source in isolated environments.\nNix provides reproducible, deterministic builds that Root pins in its lockfile."
                        .to_string(),
                    suggestion:
                        "Install Nix with: root init --install-nix\nOr install manually from:\n  https://nixos.org/download/\n\nAfter installing, run: root doctor"
                            .to_string(),
                });
            }
        }
        Err(e) => {
            let msg = format!("{}", e);
            if msg.contains("experimental feature") && msg.contains("not enabled") {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description: "Nix is installed but experimental features are not enabled.\nRoot needs 'nix-command' and 'flakes' experimental features."
                        .to_string(),
                    suggestion:
                        "Add this to ~/.config/nix/nix.conf:\n  experimental-features = nix-command flakes\n\nThen run: root doctor"
                            .to_string(),
                });
            } else {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description: format!("Failed to check Nix availability: {}", e),
                    suggestion:
                        "Run `nix --version` to verify Nix is working, then run `root doctor` again."
                            .to_string(),
                });
            }
        }
    }

    // 1b. Probe experimental features (only if Nix is installed)
    if report.nix_installed {
        match adapter.probe_experimental_features() {
            Ok(root_nix::ExperimentalFeatureStatus::AllAvailable) => {}
            Ok(root_nix::ExperimentalFeatureStatus::NixNotAvailable) => {
                report.nix_installed = false;
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description: "Nix is not installed or not available on PATH.\n\nRoot uses Nix to build packages from source in isolated environments.\nNix provides reproducible, deterministic builds that Root pins in its lockfile."
                        .to_string(),
                    suggestion:
                        "Install Nix with: root init --install-nix\nOr install manually from:\n  https://nixos.org/download/\n\nAfter installing, run: root doctor"
                            .to_string(),
                });
            }
            Ok(root_nix::ExperimentalFeatureStatus::NixCommandMissing)
            | Ok(root_nix::ExperimentalFeatureStatus::FlakesMissing)
            | Ok(root_nix::ExperimentalFeatureStatus::BothMissing) => {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description:
                        "Nix is installed, but required experimental features are not enabled.\n\nRoot requires:\n  nix-command\n  flakes\n\nYou can enable them by adding this to ~/.config/nix/nix.conf:\n\n  experimental-features = nix-command flakes\n\nThen run:\n  root doctor".to_string(),
                    suggestion:
                        "Add this to ~/.config/nix/nix.conf:\n  experimental-features = nix-command flakes\n\nThen run: root doctor".to_string(),
                });
            }
            Ok(root_nix::ExperimentalFeatureStatus::NixpkgsResolutionFailed) => {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description:
                        "Nix and experimental features are available, but nixpkgs could not be resolved.\n\nThis may indicate a network issue, a missing nixpkgs channel, or a problem with your Nix installation."
                            .to_string(),
                    suggestion:
                        "Run 'nix-channel --update' and try 'root doctor' again, or check your network connection.\nIf the issue persists, try: nix eval nixpkgs#hello".to_string(),
                });
            }
            Err(e) => {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Error,
                    category: "Nix".to_string(),
                    description: format!("Failed to probe Nix experimental features: {}", e),
                    suggestion:
                        "Run `nix eval nixpkgs#hello` to diagnose the issue, then run `root doctor` again."
                            .to_string(),
                });
            }
        }
    }

    // 2. Check Root Directory Status
    let root_dir_res = get_root_dir();
    let mut has_root_dir = false;
    if let Ok(ref root_dir) = root_dir_res {
        if root_dir.exists() && root_dir.is_dir() {
            has_root_dir = true;
            report.root_initialized = true;

            // Check subdirectories
            let subdirs = ["snapshots", "profiles", "logs", "cache"];
            for sub in &subdirs {
                let sub_path = root_dir.join(sub);
                if !sub_path.exists() || !sub_path.is_dir() {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Warning,
                        category: "Repository".to_string(),
                        description: format!("Subdirectory ~/.root/{} is missing.", sub),
                        suggestion: "Run `root init` to recreate missing directories safely."
                            .to_string(),
                    });
                }
            }

            // Specifically check the default profile subdirectory
            let default_profile = root_dir.join("profiles").join("default");
            let profile_usable = fs::symlink_metadata(&default_profile)
                .map(|meta| meta.file_type().is_symlink() || meta.is_dir())
                .unwrap_or(false);
            if !profile_usable {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Warning,
                    category: "Repository".to_string(),
                    description:
                        "Root managed profile directory (~/.root/profiles/default) is missing."
                            .to_string(),
                    suggestion: "Run `root init` to recreate the profile directory.".to_string(),
                });
            }

            let default_profile_bin = default_profile.join("bin");
            let bin_usable = fs::symlink_metadata(&default_profile_bin)
                .map(|meta| meta.file_type().is_symlink() || meta.is_dir())
                .unwrap_or(false);
            if !bin_usable {
                report.issues.push(DoctorIssue {
                    severity: IssueSeverity::Warning,
                    category: "Repository".to_string(),
                    description:
                        "Root profile binary directory (~/.root/profiles/default/bin) is missing."
                            .to_string(),
                    suggestion:
                        "Run `root sync` after packages are locked to recreate profile links."
                            .to_string(),
                });
            }
        } else {
            report.issues.push(DoctorIssue {
                severity: IssueSeverity::Error,
                category: "Repository".to_string(),
                description: "Root app directory (~/.root) does not exist.".to_string(),
                suggestion: "Run `root init` to initialize the Root environment.".to_string(),
            });
        }
    } else {
        report.issues.push(DoctorIssue {
            severity: IssueSeverity::Error,
            category: "Repository".to_string(),
            description: "Unable to determine the Root directory (~/.root).".to_string(),
            suggestion: "Ensure your HOME environment variable is set correctly.".to_string(),
        });
    }

    // 3. Lockfile and Configuration Status
    let mut rootfile_opt = None;
    let mut lockfile_opt: Option<RootLockV2> = None;

    if has_root_dir {
        let root_dir = root_dir_res.unwrap();
        let rootfile_path = root_dir.join("Rootfile");
        let lock_path = root_dir.join("root.lock");

        // Read Rootfile
        if rootfile_path.exists() {
            match Rootfile::read_from_file(&rootfile_path) {
                Ok(rf) => rootfile_opt = Some(rf),
                Err(e) => {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Error,
                        category: "Config".to_string(),
                        description: format!("Rootfile is corrupted or unparseable: {}", e),
                        suggestion:
                            "Fix the syntax errors in ~/.root/Rootfile or delete and recreate it."
                                .to_string(),
                    });
                }
            }
        } else {
            report.issues.push(DoctorIssue {
                severity: IssueSeverity::Warning,
                category: "Config".to_string(),
                description: "Rootfile (~/.root/Rootfile) is missing.".to_string(),
                suggestion:
                    "Run `root install ffmpeg` to install your first package and auto-create a Rootfile."
                        .to_string(),
            });
        }

        // Read Lockfile
        if lock_path.exists() {
            match read_lock_v2(&lock_path) {
                Ok(lock) => {
                    if lock.version < root_lockfile::ROOT_LOCK_SCHEMA_VERSION {
                        report.issues.push(DoctorIssue {
                            severity: IssueSeverity::Warning,
                            category: "Config".to_string(),
                            description: format!(
                                "root.lock uses legacy schema version {}.",
                                lock.version
                            ),
                            suggestion:
                                "Run `root lock` to regenerate deterministic lock metadata."
                                    .to_string(),
                        });
                    }

                    // Detect nondeterministic legacy markers
                    for pkg in &lock.packages {
                        if pkg.has_latest_version() {
                            report.issues.push(DoctorIssue {
                                severity: IssueSeverity::Warning,
                                category: "Config".to_string(),
                                description: format!(
                                    "Package '{}' has floating 'latest' version in root.lock.",
                                    pkg.name
                                ),
                                suggestion: format!(
                                    "Run `root lock` or `root install {}` to lock a concrete version.",
                                    pkg.name
                                ),
                            });
                        }
                        if pkg.has_placeholder_store_path() {
                            report.issues.push(DoctorIssue {
                                severity: IssueSeverity::Warning,
                                category: "Config".to_string(),
                                description: format!(
                                    "Package '{}' has placeholder store path '{}' in root.lock.",
                                    pkg.name,
                                    pkg.store_path
                                ),
                                suggestion: format!(
                                    "Run `root lock` or `root install {}` to resolve the real Nix store path.",
                                    pkg.name
                                ),
                            });
                        }
                    }
                    if is_unknown_nixpkgs_rev(&lock.nixpkgs.rev) {
                        report.issues.push(DoctorIssue {
                            severity: IssueSeverity::Warning,
                            category: "Config".to_string(),
                            description: format!(
                                "root.lock nixpkgs revision '{}' is not a concrete pinned revision.",
                                lock.nixpkgs.rev
                            ),
                            suggestion:
                                "Run `root lock` to pin the current nixpkgs revision with real metadata."
                                    .to_string(),
                        });
                    }

                    lockfile_opt = Some(lock);
                }
                Err(e) => {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Error,
                        category: "Config".to_string(),
                        description: format!("root.lock is corrupted or unparseable: {}", e),
                        suggestion:
                            "Run `root init` or reinstall packages to rebuild the lockfile."
                                .to_string(),
                    });
                }
            }
        } else {
            report.issues.push(DoctorIssue {
                severity: IssueSeverity::Warning,
                category: "Config".to_string(),
                description: "root.lock (~/.root/root.lock) is missing.".to_string(),
                suggestion:
                    "Run `root install ffmpeg` to create root.lock with deterministic Nix metadata."
                        .to_string(),
            });
        }
    }

    // 4. Reconcile Configuration and Nix profile (Drift Detection)
    if report.nix_installed && report.root_initialized {
        if let (Some(ref rootfile), Some(ref lockfile)) = (&rootfile_opt, &lockfile_opt) {
            // Check for Rootfile packages missing from lockfile
            for pkg_name in rootfile.packages.keys() {
                let Some(locked_pkg) = lockfile.packages.iter().find(|p| p.name == *pkg_name)
                else {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Warning,
                        category: "Drift".to_string(),
                        description: format!(
                            "Package '{}' in Rootfile is not locked in root.lock.",
                            pkg_name
                        ),
                        suggestion: format!(
                            "Run `root install {}` to lock and install the package.",
                            pkg_name
                        ),
                    });
                    continue;
                };

                if let Some(requested_version) = rootfile.packages.get(pkg_name) {
                    if requested_version != &locked_pkg.version {
                        report.issues.push(DoctorIssue {
                            severity: IssueSeverity::Warning,
                            category: "Drift".to_string(),
                            description: format!(
                                "Package '{}' has Rootfile version '{}' but root.lock version '{}'.",
                                pkg_name, requested_version, locked_pkg.version
                            ),
                            suggestion: format!(
                                "Run `root lock` to refresh deterministic metadata for '{}'.",
                                pkg_name
                            ),
                        });
                    }
                }
            }

            for locked_pkg in &lockfile.packages {
                if !rootfile.packages.contains_key(&locked_pkg.name) {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Warning,
                        category: "Drift".to_string(),
                        description: format!(
                            "Package '{}' is locked but not listed in Rootfile.",
                            locked_pkg.name
                        ),
                        suggestion: format!(
                            "Add '{}' to Rootfile or run `root lock` after updating Rootfile.",
                            locked_pkg.name
                        ),
                    });
                }
            }

            // Fetch actual Nix profile packages
            match adapter.profile_list_json() {
                Ok(profile_json) => {
                    let profile_entries = parse_profile_entries(&profile_json);
                    let profile_store_paths: BTreeSet<String> = profile_entries
                        .iter()
                        .flat_map(|entry| entry.store_paths.iter().cloned())
                        .collect();
                    let locked_names: BTreeSet<String> = lockfile
                        .packages
                        .iter()
                        .map(|pkg| pkg.name.clone())
                        .collect();

                    for locked_pkg in &lockfile.packages {
                        let expected_paths = locked_pkg
                            .store_paths
                            .values()
                            .cloned()
                            .chain(std::iter::once(locked_pkg.store_path.clone()))
                            .filter(|path| !path.is_empty())
                            .collect::<BTreeSet<_>>();

                        let missing_paths = expected_paths
                            .difference(&profile_store_paths)
                            .cloned()
                            .collect::<Vec<_>>();
                        if !missing_paths.is_empty() {
                            report.issues.push(DoctorIssue {
                                severity: IssueSeverity::Error,
                                category: "Drift".to_string(),
                                description: format!(
                                    "Locked package '{}' is missing expected store output(s) from the Root profile: {}.",
                                    locked_pkg.name,
                                    missing_paths.join(", ")
                                ),
                                suggestion: format!(
                                    "Run `root sync` or `root install {}` to repair the Root profile.",
                                    locked_pkg.name
                                ),
                            });
                        }
                    }

                    for entry in &profile_entries {
                        if let Some(name) = entry.package_name() {
                            if !locked_names.contains(&name) {
                                report.issues.push(DoctorIssue {
                                    severity: IssueSeverity::Error,
                                    category: "Drift".to_string(),
                                    description: format!(
                                        "Root profile contains '{}' but it is absent from root.lock.",
                                        name
                                    ),
                                    suggestion:
                                        "Run `root sync` to reconcile the Root profile with root.lock."
                                            .to_string(),
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    report.issues.push(DoctorIssue {
                        severity: IssueSeverity::Warning,
                        category: "Nix".to_string(),
                        description: format!("Failed to read Nix profile JSON: {}", e),
                        suggestion: "Ensure nix profile operations are functioning normally."
                            .to_string(),
                    });
                }
            }
        }
    }

    // 5. Check PATH & Shadows
    if let Some(ref lockfile) = lockfile_opt {
        let root_profile_bin = get_root_dir()
            .ok()
            .map(|root_dir| root_dir.join("profiles").join("default").join("bin"));

        let mut root_profile_in_path = false;
        if let (Some(paths), Some(root_bin)) = (env::var_os("PATH"), root_profile_bin.as_ref()) {
            for path in env::split_paths(&paths) {
                if path == *root_bin {
                    root_profile_in_path = true;
                    break;
                }
            }
        }

        if !root_profile_in_path {
            report.issues.push(DoctorIssue {
                severity: IssueSeverity::Warning,
                category: "Environment".to_string(),
                description: "Root profile binary path (~/.root/profiles/default/bin) is not found in PATH."
                    .to_string(),
                suggestion:
                    "Add '~/.root/profiles/default/bin' before Homebrew and system package paths in your shell PATH."
                        .to_string(),
            });
        }

        for locked_pkg in &lockfile.packages {
            for binary in &locked_pkg.binaries {
                match find_on_path(binary) {
                    Some(bin_path) => {
                        let is_root_profile = root_profile_bin
                            .as_ref()
                            .map(|root_bin| bin_path.starts_with(root_bin))
                            .unwrap_or(false);
                        let is_locked_store = locked_pkg
                            .store_paths
                            .values()
                            .chain(std::iter::once(&locked_pkg.store_path))
                            .any(|store_path| bin_path.starts_with(store_path));

                        if !is_root_profile && !is_locked_store {
                            report.issues.push(DoctorIssue {
                                severity: IssueSeverity::Warning,
                                category: "Conflict".to_string(),
                                description: format!(
                                    "Binary '{}' from package '{}' is shadowed by another installation at '{}'.",
                                    binary, locked_pkg.name, bin_path.display()
                                ),
                                suggestion: format!(
                                    "Move '~/.root/profiles/default/bin' before '{}' in PATH or remove the conflicting binary.",
                                    bin_path.parent().unwrap_or(Path::new("")).display()
                                ),
                            });
                        }
                    }
                    None => {
                        report.issues.push(DoctorIssue {
                            severity: IssueSeverity::Warning,
                            category: "Environment".to_string(),
                            description: format!(
                                "Binary '{}' from package '{}' is not accessible in PATH.",
                                binary, locked_pkg.name
                            ),
                            suggestion:
                                "Run `root sync` and ensure '~/.root/profiles/default/bin' is in PATH."
                                    .to_string(),
                        });
                    }
                }
            }
        }
    }

    // Determine healthy status: no errors or warnings
    report.healthy = report
        .issues
        .iter()
        .all(|issue| issue.severity != IssueSeverity::Error);

    Ok(report)
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    if let Some(paths) = env::var_os("PATH") {
        for path in env::split_paths(&paths) {
            let bin_path = path.join(binary);
            if bin_path.is_file() {
                return Some(bin_path);
            }
        }
    }
    None
}

fn read_lock_v2(path: &Path) -> Result<RootLockV2> {
    RootLockV2::read_from_file(path)
        .or_else(|_| RootLock::read_from_file(path).map(|lock| lock.to_v2()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileJsonEntry {
    attr_path: Option<String>,
    installable: Option<String>,
    store_paths: Vec<String>,
}

impl ProfileJsonEntry {
    fn package_name(&self) -> Option<String> {
        self.attr_path
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|value| value.rsplit('.').next().unwrap_or(value).to_string())
            .or_else(|| {
                self.installable.as_ref().and_then(|installable| {
                    installable
                        .rsplit_once('#')
                        .map(|(_, name)| name.to_string())
                })
            })
    }
}

fn parse_profile_entries(profile_json: &str) -> Vec<ProfileJsonEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(profile_json) else {
        return Vec::new();
    };

    match value {
        serde_json::Value::Array(entries) => {
            entries.iter().filter_map(parse_profile_entry).collect()
        }
        serde_json::Value::Object(map) => {
            if let Some(elements) = map.get("elements").and_then(|value| value.as_array()) {
                elements.iter().filter_map(parse_profile_entry).collect()
            } else {
                map.values().filter_map(parse_profile_entry).collect()
            }
        }
        _ => Vec::new(),
    }
}

fn parse_profile_entry(value: &serde_json::Value) -> Option<ProfileJsonEntry> {
    let object = value.as_object()?;
    let attr_path = object
        .get("attrPath")
        .or_else(|| object.get("attr_path"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string);
    let installable = object
        .get("installable")
        .or_else(|| object.get("originalUrl"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string);
    let store_paths = object
        .get("storePaths")
        .or_else(|| object.get("store_paths"))
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(ProfileJsonEntry {
        attr_path,
        installable,
        store_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_lockfile::{self, LockedPackage, NixpkgsConfig};
    use root_nix::MockNixAdapter;
    use std::fs;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn setup_test_home(test_name: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = TEST_MUTEX.lock().unwrap();
        let temp_dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test_homes")
            .join(test_name);
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir).unwrap();
        std::env::set_var("HOME", &temp_dir);
        (temp_dir, guard)
    }

    fn first_mock_store_path(adapter: &MockNixAdapter) -> String {
        let profile_json = adapter.profile_list_json().unwrap();
        parse_profile_entries(&profile_json)
            .first()
            .and_then(|entry| entry.store_paths.first())
            .cloned()
            .unwrap()
    }

    #[test]
    fn test_diagnostics_healthy_with_packages() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_healthy_with_packages");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let profile_bin = root_dir.join("profiles").join("default").join("bin");
        fs::create_dir_all(&profile_bin).unwrap();
        std::env::set_var("PATH", &profile_bin);

        let adapter = MockNixAdapter::new(true);
        adapter.install("poppler").unwrap();
        let store_path = first_mock_store_path(&adapter);

        // Create standard Rootfile and root.lock
        let mut rf = Rootfile::default();
        rf.packages
            .insert("poppler".to_string(), "latest".to_string());
        rf.write_to_file(&root_dir.join("Rootfile")).unwrap();

        let lock = RootLock {
            version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "poppler".to_string(),
                requested: "poppler".to_string(),
                version: "latest".to_string(),
                attribute: "poppler".to_string(),
                store_path,
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        // Run diagnostics
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        assert!(report.root_initialized);
        let error_count = report
            .issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .count();
        assert_eq!(error_count, 0);
    }

    #[test]
    fn test_diagnostics_does_not_initialize_missing_root_dir() {
        let (temp_home, _guard) = setup_test_home("test_diagnostics_does_not_initialize");
        let root_dir = temp_home.join(".root");
        assert!(!root_dir.exists());

        let adapter = MockNixAdapter::new(true);
        let report = run_diagnostics(&adapter).unwrap();

        assert!(!report.root_initialized);
        assert!(!root_dir.exists());
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.category == "Repository"));
    }

    #[test]
    fn test_diagnostics_no_nix() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_no_nix");
        let adapter = MockNixAdapter::new(false);
        let report = run_diagnostics(&adapter).unwrap();
        assert!(!report.nix_installed);
        assert!(!report.healthy);

        let nix_issue = report.issues.iter().find(|i| i.category == "Nix").unwrap();
        assert_eq!(nix_issue.severity, IssueSeverity::Error);
    }

    #[test]
    fn test_diagnostics_drift() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_drift");
        let root_dir = root_lockfile::init_root_dir().unwrap();

        // Mock nix adapter is installed, but we DO NOT install poppler in its actual profile.
        let adapter = MockNixAdapter::new(true);

        // Create standard Rootfile and root.lock claiming poppler is locked
        let mut rf = Rootfile::default();
        rf.packages
            .insert("poppler".to_string(), "latest".to_string());
        rf.write_to_file(&root_dir.join("Rootfile")).unwrap();

        let lock = RootLock {
            version: 1,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "poppler".to_string(),
                requested: "poppler".to_string(),
                version: "latest".to_string(),
                attribute: "poppler".to_string(),
                store_path: root_lockfile::derive_store_path("poppler", "latest"),
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        // Run diagnostics
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        assert!(report.root_initialized);

        // We should detect drift because poppler is locked but missing from active Nix profile
        let drift_issue = report
            .issues
            .iter()
            .find(|i| i.category == "Drift")
            .unwrap();
        assert_eq!(drift_issue.severity, IssueSeverity::Error);
        assert!(drift_issue
            .description
            .contains("missing expected store output"));
    }

    #[test]
    fn test_diagnostics_detects_extra_profile_package() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_extra_profile_package");
        let root_dir = root_lockfile::init_root_dir().unwrap();

        let adapter = MockNixAdapter::new(true);
        adapter.install("fd").unwrap();

        let rf = Rootfile::default();
        rf.write_to_file(&root_dir.join("Rootfile")).unwrap();

        let lock = RootLock {
            version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.issues.iter().any(|issue| {
            issue.severity == IssueSeverity::Error
                && issue.category == "Drift"
                && issue.description.contains("absent from root.lock")
        }));
    }

    #[test]
    fn test_diagnostics_detects_nondeterministic_lock() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_nondeterministic");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let profile_bin = root_dir.join("profiles").join("default").join("bin");
        fs::create_dir_all(&profile_bin).unwrap();
        std::env::set_var("PATH", &profile_bin);

        let adapter = MockNixAdapter::new(true);
        adapter.install("poppler").unwrap();

        // Create v1-style lock with all nondeterministic markers
        let mut rf = Rootfile::default();
        rf.packages
            .insert("poppler".to_string(), "latest".to_string());
        rf.write_to_file(&root_dir.join("Rootfile")).unwrap();

        let lock = RootLock {
            version: 1,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "unknown".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "poppler".to_string(),
                requested: "poppler".to_string(),
                version: "latest".to_string(),
                attribute: "poppler".to_string(),
                store_path: "/nix/store/xxx".into(),
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = run_diagnostics(&adapter).unwrap();

        // Should flag legacy schema version
        assert!(report.issues.iter().any(|issue| {
            issue.category == "Config" && issue.description.contains("legacy schema version")
        }));

        // Should flag floating "latest" version
        assert!(report.issues.iter().any(|issue| {
            issue.category == "Config"
                && issue.description.contains("floating")
                && issue.description.contains("latest")
        }));

        // Should flag placeholder store path
        assert!(report.issues.iter().any(|issue| {
            issue.category == "Config" && issue.description.contains("placeholder store path")
        }));

        // Should flag unknown nixpkgs revision
        assert!(report.issues.iter().any(|issue| {
            issue.category == "Config"
                && issue.description.contains("not a concrete pinned revision")
        }));
    }

    #[test]
    fn test_diagnostics_does_not_warn_on_v2() {
        let (_temp_home, _guard) = setup_test_home("test_diagnostics_v2_not_legacy");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let profile_bin = root_dir.join("profiles").join("default").join("bin");
        fs::create_dir_all(&profile_bin).unwrap();
        std::env::set_var("PATH", &profile_bin);

        let adapter = MockNixAdapter::new(true);
        adapter.install("poppler").unwrap();
        let store_path = first_mock_store_path(&adapter);

        let mut rf = Rootfile::default();
        rf.packages
            .insert("poppler".to_string(), "24.08.0".to_string());
        rf.write_to_file(&root_dir.join("Rootfile")).unwrap();

        let lock = RootLock {
            version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "0123456789abcdef0123456789abcdef01234567".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "poppler".to_string(),
                requested: "poppler".to_string(),
                version: "24.08.0".to_string(),
                attribute: "poppler".to_string(),
                store_path,
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = run_diagnostics(&adapter).unwrap();
        assert!(!report.issues.iter().any(|issue| {
            issue.category == "Config" && issue.description.contains("legacy schema version")
        }));
    }

    #[test]
    fn test_probe_all_available() {
        let (_temp_home, _guard) = setup_test_home("test_probe_all_available");
        let mut adapter = MockNixAdapter::new(true);
        adapter.nix_command_enabled = true;
        adapter.flakes_enabled = true;
        adapter.nixpkgs_accessible = true;
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        assert!(!report
            .issues
            .iter()
            .any(|i| i.category == "Nix" && i.description.contains("experimental features")));
    }

    #[test]
    fn test_probe_nix_command_missing() {
        let (_temp_home, _guard) = setup_test_home("test_probe_nix_command_missing");
        let mut adapter = MockNixAdapter::new(true);
        adapter.nix_command_enabled = false;
        adapter.flakes_enabled = true;
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        let nix_issue = report.issues.iter().find(|i| i.category == "Nix").unwrap();
        assert!(nix_issue.description.contains("experimental features"));
    }

    #[test]
    fn test_probe_flakes_missing() {
        let (_temp_home, _guard) = setup_test_home("test_probe_flakes_missing");
        let mut adapter = MockNixAdapter::new(true);
        adapter.nix_command_enabled = true;
        adapter.flakes_enabled = false;
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        let nix_issue = report.issues.iter().find(|i| i.category == "Nix").unwrap();
        assert!(nix_issue.description.contains("experimental features"));
    }

    #[test]
    fn test_probe_both_missing() {
        let (_temp_home, _guard) = setup_test_home("test_probe_both_missing");
        let mut adapter = MockNixAdapter::new(true);
        adapter.nix_command_enabled = false;
        adapter.flakes_enabled = false;
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        let nix_issue = report.issues.iter().find(|i| i.category == "Nix").unwrap();
        assert!(nix_issue.description.contains("experimental features"));
    }

    #[test]
    fn test_probe_nixpkgs_resolution_failed() {
        let (_temp_home, _guard) = setup_test_home("test_probe_nixpkgs_resolution_failed");
        let mut adapter = MockNixAdapter::new(true);
        adapter.nix_command_enabled = true;
        adapter.flakes_enabled = true;
        adapter.nixpkgs_accessible = false;
        let report = run_diagnostics(&adapter).unwrap();
        assert!(report.nix_installed);
        let nix_issue = report.issues.iter().find(|i| i.category == "Nix").unwrap();
        assert!(nix_issue
            .description
            .contains("nixpkgs could not be resolved"));
    }
}
