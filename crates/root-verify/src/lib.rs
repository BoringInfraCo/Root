use anyhow::{Context, Result};
use root_lockfile::{get_root_dir, LockedPackageV2, RootLockV2};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct BinaryResult {
    pub binary: String,
    pub success: bool,
    pub resolved_path: Option<String>,
    pub attempted_args: Vec<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct VerificationReport {
    pub package: String,
    pub success: bool,
    pub binaries: Vec<BinaryResult>,
    pub errors: Vec<String>,
}

/// Verification Strategy override entry
struct VerifyOverride {
    pub args: &'static [&'static str],
    pub expected_contains: Option<&'static str>,
}

/// Get custom verification arguments/assertions for specific tools.
fn get_override(binary: &str) -> Option<VerifyOverride> {
    match binary {
        "pdftotext" | "pdfinfo" => Some(VerifyOverride {
            args: &["-v"],
            expected_contains: None,
        }),
        "ffmpeg" | "ffprobe" => Some(VerifyOverride {
            args: &["-version"],
            expected_contains: Some("ffmpeg"),
        }),
        "rg" => Some(VerifyOverride {
            args: &["--version"],
            expected_contains: Some("ripgrep"),
        }),
        "jq" => Some(VerifyOverride {
            args: &["--version"],
            expected_contains: Some("jq"),
        }),
        "openssl" => Some(VerifyOverride {
            args: &["version"],
            expected_contains: None,
        }),
        "go" => Some(VerifyOverride {
            args: &["version"],
            expected_contains: None,
        }),
        "terraform" => Some(VerifyOverride {
            args: &["version"],
            expected_contains: None,
        }),
        "kubectl" => Some(VerifyOverride {
            args: &["version", "--client"],
            expected_contains: None,
        }),
        "helm" => Some(VerifyOverride {
            args: &["version", "--short"],
            expected_contains: None,
        }),
        "tmux" => Some(VerifyOverride {
            args: &["-V"],
            expected_contains: None,
        }),
        "direnv" => Some(VerifyOverride {
            args: &["version"],
            expected_contains: None,
        }),
        _ => None,
    }
}

fn package_default_binaries(package: &str) -> Option<&'static [&'static str]> {
    match package {
        "ffmpeg" => Some(&["ffmpeg"]),
        "poppler" => Some(&["pdftotext", "pdfinfo"]),
        "ripgrep" => Some(&["rg"]),
        "jq" => Some(&["jq"]),
        "fd" => Some(&["fd"]),
        "bat" => Some(&["bat"]),
        "eza" => Some(&["eza"]),
        "fzf" => Some(&["fzf"]),
        "git-lfs" => Some(&["git-lfs"]),
        "gh" => Some(&["gh"]),
        "httpie" => Some(&["http"]),
        "just" => Some(&["just"]),
        "tree" => Some(&["tree"]),
        "sqlite" => Some(&["sqlite3"]),
        "imagemagick" => Some(&["magick", "convert"]),
        "wget" => Some(&["wget"]),
        "curl" => Some(&["curl"]),
        "gnumake" => Some(&["make"]),
        "pkg-config" => Some(&["pkg-config"]),
        "openssl" => Some(&["openssl"]),
        "python3" => Some(&["python3"]),
        "nodejs" => Some(&["node", "npm"]),
        "bun" => Some(&["bun"]),
        "uv" => Some(&["uv"]),
        "go" => Some(&["go"]),
        "rustup" => Some(&["rustup"]),
        "postgresql" => Some(&["psql", "postgres"]),
        "redis" => Some(&["redis-server", "redis-cli"]),
        "terraform" => Some(&["terraform"]),
        "kubectl" => Some(&["kubectl"]),
        "helm" => Some(&["helm"]),
        "k9s" => Some(&["k9s"]),
        "docker-client" => Some(&["docker"]),
        "age" => Some(&["age", "age-keygen"]),
        "sops" => Some(&["sops"]),
        "neovim" => Some(&["nvim"]),
        "tmux" => Some(&["tmux"]),
        "git-delta" => Some(&["delta"]),
        "zoxide" => Some(&["zoxide"]),
        "direnv" => Some(&["direnv"]),
        "starship" => Some(&["starship"]),
        "lazygit" => Some(&["lazygit"]),
        _ => None,
    }
}

pub fn verify_package(pkg_name: &str) -> Result<VerificationReport> {
    let root_dir = get_root_dir().context("Could not determine ~/.root path")?;
    let lock_path = root_dir.join("root.lock");
    if !lock_path.exists() {
        return Err(anyhow::anyhow!(
            "root.lock does not exist. Run 'root install' first."
        ));
    }

    let lock = read_lock_v2(&lock_path)?;
    let locked_pkg = lock
        .packages
        .iter()
        .find(|p| p.name == pkg_name)
        .ok_or_else(|| anyhow::anyhow!("Package '{}' is not found in root.lock", pkg_name))?;

    let mut binaries_results = Vec::new();
    let mut errors = Vec::new();
    let binaries = binaries_for_package(locked_pkg);

    if binaries.is_empty() {
        errors.push(format!(
            "Package '{}' has no binary metadata in root.lock.",
            pkg_name
        ));
    }

    for binary in &binaries {
        let mut bin_res = BinaryResult {
            binary: binary.clone(),
            success: false,
            resolved_path: None,
            attempted_args: Vec::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error_message: None,
        };

        let Some(binary_path) =
            resolve_binary_path(binary, &root_dir, lock.profile.path.as_deref())
        else {
            let msg = format!(
                "Binary '{}' for package '{}' was not found in Root profile at ~/.root/profiles/default/bin.",
                binary, pkg_name
            );
            bin_res.error_message = Some(msg.clone());
            errors.push(msg);
            binaries_results.push(bin_res);
            continue;
        };
        bin_res.resolved_path = Some(binary_path.to_string_lossy().to_string());

        if let Some(over) = get_override(binary) {
            bin_res.attempted_args = over.args.iter().map(|arg| (*arg).to_string()).collect();
            match run_binary(&binary_path, over.args) {
                Ok((code, out, err)) => {
                    bin_res.exit_code = Some(code);
                    bin_res.stdout = out.clone();
                    bin_res.stderr = err.clone();

                    let mut check = code == 0;
                    if let Some(expected) = over.expected_contains {
                        if out.to_lowercase().contains(expected)
                            || err.to_lowercase().contains(expected)
                        {
                            check = true;
                        }
                    }
                    bin_res.success = check;
                    if !bin_res.success {
                        let msg = format!(
                            "Binary '{}' exited with code {} and did not produce expected verification output.",
                            binary, code
                        );
                        bin_res.error_message = Some(msg.clone());
                        errors.push(msg);
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    bin_res.error_message = Some(msg.clone());
                    errors.push(msg);
                }
            }
        } else {
            let strategies = vec![
                vec!["--version"],
                vec!["-version"],
                vec!["--help"],
                vec!["-h"],
            ];

            let mut last_error = None;
            for args in strategies {
                bin_res.attempted_args = args.iter().map(|arg| (*arg).to_string()).collect();
                match run_binary(&binary_path, &args) {
                    Ok((code, out, err)) => {
                        bin_res.exit_code = Some(code);
                        bin_res.stdout = out.clone();
                        bin_res.stderr = err.clone();

                        let lower_out = out.to_lowercase();
                        let lower_err = err.to_lowercase();

                        let has_help_keywords = lower_out.contains("usage")
                            || lower_out.contains("options")
                            || lower_out.contains("help")
                            || lower_out.contains("version")
                            || lower_err.contains("usage")
                            || lower_err.contains("options")
                            || lower_err.contains("help")
                            || lower_err.contains("version");

                        if code == 0 || has_help_keywords {
                            bin_res.success = true;
                            break;
                        }
                    }
                    Err(e) => {
                        last_error = Some(e.to_string());
                    }
                }
            }

            if !bin_res.success {
                if let Some(err) = last_error {
                    bin_res.error_message = Some(err.clone());
                    errors.push(err);
                } else {
                    let msg = format!(
                        "Binary '{}' did not pass generic verification strategies.",
                        binary
                    );
                    bin_res.error_message = Some(msg.clone());
                    errors.push(msg);
                }
            }
        }

        binaries_results.push(bin_res);
    }

    let all_success = !binaries_results.is_empty() && binaries_results.iter().all(|r| r.success);

    Ok(VerificationReport {
        package: pkg_name.to_string(),
        success: all_success,
        binaries: binaries_results,
        errors,
    })
}

fn read_lock_v2(path: &Path) -> Result<RootLockV2> {
    root_lockfile::read_compatible_lock_v2(path)
}

fn binaries_for_package(locked_pkg: &LockedPackageV2) -> Vec<String> {
    if !locked_pkg.binaries.is_empty() {
        return locked_pkg.binaries.clone();
    }

    package_default_binaries(&locked_pkg.name)
        .map(|binaries| {
            binaries
                .iter()
                .map(|binary| (*binary).to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_binary_path(
    binary: &str,
    root_dir: &Path,
    locked_profile_path: Option<&str>,
) -> Option<PathBuf> {
    let mut profile_bins = Vec::new();
    if let Some(profile_path) = locked_profile_path {
        profile_bins.push(PathBuf::from(profile_path).join("bin"));
    }
    profile_bins.push(root_dir.join("profiles").join("default").join("bin"));

    for profile_bin in profile_bins {
        let profile_binary = profile_bin.join(binary);
        if is_executable_candidate(&profile_binary) {
            return Some(profile_binary);
        }
    }

    None
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
}

fn run_binary(binary: &Path, args: &[&str]) -> Result<(i32, String, String)> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute binary '{}'", binary.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);

    Ok((code, stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_lockfile::{self, LockedPackage, NixpkgsConfig, RootLock};
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

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

    fn write_fake_binary(root_dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let bin_dir = root_dir.join("profiles").join("default").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
        path
    }

    #[test]
    fn test_verify_missing_lockfile() {
        let (_temp_home, _guard) = setup_test_home("test_verify_missing_lockfile");
        let res = verify_package("poppler");
        assert!(res.is_err());
    }

    #[test]
    fn test_verify_missing_package() {
        let (_temp_home, _guard) = setup_test_home("test_verify_missing_package");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let lock = RootLock {
            version: 1,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let res = verify_package("poppler");
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("is not found in root.lock"));
    }

    #[test]
    fn test_verify_nonexistent_binary() {
        let (_temp_home, _guard) = setup_test_home("test_verify_nonexistent_binary");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let lock = RootLock {
            version: 1,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "nonexistent-pkg".to_string(),
                requested: "nonexistent-pkg".to_string(),
                version: "latest".to_string(),
                attribute: "nonexistent-pkg".to_string(),
                store_path: root_lockfile::derive_store_path("nonexistent-pkg", "latest"),
                binaries: vec!["some_garbage_bin_name_123".to_string()],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = verify_package("nonexistent-pkg").unwrap();
        assert!(!report.success);
        assert_eq!(report.binaries.len(), 1);
        assert!(!report.binaries[0].success);
        assert!(report.binaries[0].error_message.is_some());
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn test_verify_poppler_default_binary_metadata() {
        let (_temp_home, _guard) = setup_test_home("test_verify_poppler_default_binary_metadata");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        std::env::set_var(
            "PATH",
            root_dir.join("profiles").join("default").join("bin"),
        );
        write_fake_binary(
            &root_dir,
            "pdftotext",
            "#!/bin/sh\necho 'usage: pdftotext [options] file.pdf'\n",
        );
        write_fake_binary(
            &root_dir,
            "pdfinfo",
            "#!/bin/sh\necho 'usage: pdfinfo [options] file.pdf'\n",
        );

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
                version: "24.08.0".to_string(),
                attribute: "poppler".to_string(),
                store_path: root_lockfile::derive_store_path("poppler", "24.08.0"),
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = verify_package("poppler").unwrap();
        assert!(report.success);
        assert!(report.errors.is_empty());
        assert_eq!(
            report
                .binaries
                .iter()
                .map(|result| result.binary.as_str())
                .collect::<Vec<_>>(),
            vec!["pdftotext", "pdfinfo"]
        );
        assert!(report.binaries.iter().all(|result| {
            result.resolved_path.as_deref().unwrap().contains(".root")
                || result
                    .resolved_path
                    .as_deref()
                    .unwrap()
                    .contains("test_homes")
        }));
    }

    #[test]
    fn test_verify_prefers_root_profile_over_path_shadow() {
        let (temp_home, _guard) = setup_test_home("test_verify_prefers_root_profile");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let shadow_bin_dir = temp_home.join("shadow-bin");
        fs::create_dir_all(&shadow_bin_dir).unwrap();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                shadow_bin_dir.display(),
                root_dir
                    .join("profiles")
                    .join("default")
                    .join("bin")
                    .display()
            ),
        );
        write_fake_binary(
            &root_dir,
            "ffmpeg",
            "#!/bin/sh\necho 'ffmpeg version root'\n",
        );
        let shadow_path = shadow_bin_dir.join("ffmpeg");
        fs::write(&shadow_path, "#!/bin/sh\necho 'ffmpeg version shadow'\n").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&shadow_path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&shadow_path, permissions).unwrap();
        }

        let lock = RootLock {
            version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "ffmpeg".to_string(),
                requested: "ffmpeg".to_string(),
                version: "7.1".to_string(),
                attribute: "ffmpeg".to_string(),
                store_path: root_lockfile::derive_store_path("ffmpeg", "7.1"),
                binaries: vec!["ffmpeg".to_string()],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = verify_package("ffmpeg").unwrap();
        assert!(report.success);
        assert!(report.binaries[0]
            .resolved_path
            .as_deref()
            .unwrap()
            .contains("profiles/default/bin/ffmpeg"));
        assert!(report.binaries[0].stdout.contains("root"));

        // Verify that a missing Root profile binary fails even if a shadow binary exists on PATH
        let shadow_only_bin_dir = temp_home.join("shadow-only");
        fs::create_dir_all(&shadow_only_bin_dir).unwrap();
        let shadow_only = shadow_only_bin_dir.join("missing-from-root");
        fs::write(&shadow_only, "#!/bin/sh\necho 'shadow'\n").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&shadow_only).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&shadow_only, permissions).unwrap();
        }
        std::env::set_var("PATH", shadow_only_bin_dir);
        let mut shadow_lock = lock.clone();
        shadow_lock.packages[0].binaries = vec!["missing-from-root".to_string()];
        shadow_lock
            .write_to_file(&root_dir.join("root.lock"))
            .unwrap();
        let shadow_report = verify_package("ffmpeg").unwrap();
        assert!(!shadow_report.success);
        assert!(shadow_report
            .errors
            .iter()
            .any(|e| e.contains("not found in Root profile")));
    }

    #[test]
    fn test_verify_empty_binaries_list_returns_error() {
        let (_temp_home, _guard) = setup_test_home("test_verify_empty_binaries");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        std::env::set_var(
            "PATH",
            root_dir.join("profiles").join("default").join("bin"),
        );

        let lock = RootLock {
            version: 2,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "empty-pkg".to_string(),
                requested: "empty-pkg".to_string(),
                version: "1.0.0".to_string(),
                attribute: "empty-pkg".to_string(),
                store_path: root_lockfile::derive_store_path("empty-pkg", "1.0.0"),
                binaries: vec![],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = verify_package("empty-pkg").unwrap();
        assert!(!report.success);
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("has no binary metadata")));
        assert!(report.binaries.is_empty());
    }

    #[test]
    fn test_verify_missing_profile_binary_fails_even_if_global_exists() {
        let (_temp_home, _guard) =
            setup_test_home("test_verify_missing_profile_binary_fails_even_if_global_exists");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let global_bin_dir = std::env::temp_dir().join("root_test_global_bin");
        fs::create_dir_all(&global_bin_dir).unwrap();
        std::env::set_var("PATH", &global_bin_dir);
        let global_bin = global_bin_dir.join("ffmpeg");
        fs::write(&global_bin, "#!/bin/sh\necho 'ffmpeg global version'\n").unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&global_bin).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&global_bin, perms).unwrap();
        }

        let lock = RootLock {
            version: root_lockfile::ROOT_LOCK_SCHEMA_VERSION,
            platform: "aarch64-darwin".into(),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "ffmpeg".to_string(),
                requested: "ffmpeg".to_string(),
                version: "7.1".to_string(),
                attribute: "ffmpeg".to_string(),
                store_path: root_lockfile::derive_store_path("ffmpeg", "7.1"),
                binaries: vec!["ffmpeg".to_string()],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = verify_package("ffmpeg").unwrap();
        assert!(!report.success);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("not found in Root profile")),
            "Should fail because ffmpeg is only on global PATH, not in Root profile: {:?}",
            report.errors
        );
        let _ = fs::remove_dir_all(&global_bin_dir);
    }

    #[test]
    fn test_verify_multi_binary_package_reports_each_binary() {
        let (_temp_home, _guard) = setup_test_home("test_verify_multi_binary");
        let root_dir = root_lockfile::init_root_dir().unwrap();
        std::env::set_var(
            "PATH",
            root_dir.join("profiles").join("default").join("bin"),
        );
        write_fake_binary(
            &root_dir,
            "pdftotext",
            "#!/bin/sh\necho 'usage: pdftotext'\n",
        );
        write_fake_binary(&root_dir, "pdfinfo", "#!/bin/sh\necho 'usage: pdfinfo'\n");

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
                version: "24.08.0".to_string(),
                attribute: "poppler".to_string(),
                store_path: root_lockfile::derive_store_path("poppler", "24.08.0"),
                binaries: vec!["pdftotext".to_string(), "pdfinfo".to_string()],
            }],
        };
        lock.write_to_file(&root_dir.join("root.lock")).unwrap();
        let report = verify_package("poppler").unwrap();
        assert_eq!(report.binaries.len(), 2);
        assert_eq!(report.binaries[0].binary, "pdftotext");
        assert_eq!(report.binaries[1].binary, "pdfinfo");
    }

    #[test]
    fn test_verify_non_standard_args_are_correct() {
        let cases: Vec<(&str, &[&str])> = vec![
            ("go", &["version"]),
            ("terraform", &["version"]),
            ("kubectl", &["version", "--client"]),
            ("helm", &["version", "--short"]),
            ("tmux", &["-V"]),
            ("direnv", &["version"]),
        ];
        for (binary, expected_args) in &cases {
            let over = get_override(binary);
            assert!(
                over.is_some(),
                "Expected override for '{}' but none found",
                binary
            );
            assert_eq!(
                over.unwrap().args.to_vec(),
                *expected_args,
                "Args mismatch for '{}'",
                binary
            );
        }
    }
}
