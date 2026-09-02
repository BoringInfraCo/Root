use anyhow::{Context, Result};
#[cfg(test)]
use root_lockfile::LockedPackage;
use root_lockfile::{
    get_root_dir, LockedPackageOutput, LockedPackageV2, NixRuntime, NixpkgsConfig, NixpkgsConfigV2,
    RootLock, RootLockV2, Rootfile, ROOT_LOCK_MAX_SUPPORTED_VERSION, ROOT_LOCK_SCHEMA_VERSION,
};
use root_nix::NixAdapter;
use root_sandbox::SandboxProvider;
use root_snapshot::{list_snapshot_summaries, list_snapshots, Snapshot, SnapshotSummary};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub struct VerifyCommand {
    pub binary: &'static str,
    pub args: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub nix_attr: &'static str,
    pub binaries: &'static [&'static str],
    pub verify: &'static [VerifyCommand],
    pub category: &'static str,
    pub description: &'static str,
}

pub const SUPPORTED_PACKAGES: &[PackageSpec] = &[
    // media
    PackageSpec {
        name: "ffmpeg",
        aliases: &[],
        nix_attr: "nixpkgs#ffmpeg",
        binaries: &["ffmpeg"],
        verify: &[VerifyCommand {
            binary: "ffmpeg",
            args: &["-version"],
        }],
        category: "media",
        description: "Video/audio processing",
    },
    PackageSpec {
        name: "imagemagick",
        aliases: &[],
        nix_attr: "nixpkgs#imagemagick",
        binaries: &["magick", "convert"],
        verify: &[VerifyCommand {
            binary: "magick",
            args: &["--version"],
        }],
        category: "media",
        description: "Image manipulation",
    },
    PackageSpec {
        name: "poppler",
        aliases: &[],
        nix_attr: "nixpkgs#poppler",
        binaries: &["pdftotext", "pdfinfo"],
        verify: &[
            VerifyCommand {
                binary: "pdftotext",
                args: &["-v"],
            },
            VerifyCommand {
                binary: "pdfinfo",
                args: &["-v"],
            },
        ],
        category: "media",
        description: "PDF utilities",
    },
    // search
    PackageSpec {
        name: "ripgrep",
        aliases: &["rg"],
        nix_attr: "nixpkgs#ripgrep",
        binaries: &["rg"],
        verify: &[VerifyCommand {
            binary: "rg",
            args: &["--version"],
        }],
        category: "search",
        description: "Fast recursive search",
    },
    PackageSpec {
        name: "fd",
        aliases: &[],
        nix_attr: "nixpkgs#fd",
        binaries: &["fd"],
        verify: &[VerifyCommand {
            binary: "fd",
            args: &["--version"],
        }],
        category: "search",
        description: "Fast file finder",
    },
    PackageSpec {
        name: "fzf",
        aliases: &[],
        nix_attr: "nixpkgs#fzf",
        binaries: &["fzf"],
        verify: &[VerifyCommand {
            binary: "fzf",
            args: &["--version"],
        }],
        category: "search",
        description: "Fuzzy file finder",
    },
    // dev
    PackageSpec {
        name: "bat",
        aliases: &[],
        nix_attr: "nixpkgs#bat",
        binaries: &["bat"],
        verify: &[VerifyCommand {
            binary: "bat",
            args: &["--version"],
        }],
        category: "dev",
        description: "File viewer with syntax highlighting",
    },
    PackageSpec {
        name: "bun",
        aliases: &[],
        nix_attr: "nixpkgs#bun",
        binaries: &["bun"],
        verify: &[VerifyCommand {
            binary: "bun",
            args: &["--version"],
        }],
        category: "dev",
        description: "JavaScript runtime and bundler",
    },
    PackageSpec {
        name: "eza",
        aliases: &[],
        nix_attr: "nixpkgs#eza",
        binaries: &["eza"],
        verify: &[VerifyCommand {
            binary: "eza",
            args: &["--version"],
        }],
        category: "dev",
        description: "Modern ls replacement",
    },
    PackageSpec {
        name: "gh",
        aliases: &[],
        nix_attr: "nixpkgs#gh",
        binaries: &["gh"],
        verify: &[VerifyCommand {
            binary: "gh",
            args: &["--version"],
        }],
        category: "dev",
        description: "GitHub CLI",
    },
    PackageSpec {
        name: "git-lfs",
        aliases: &[],
        nix_attr: "nixpkgs#git-lfs",
        binaries: &["git-lfs"],
        verify: &[VerifyCommand {
            binary: "git-lfs",
            args: &["--version"],
        }],
        category: "dev",
        description: "Git large file storage",
    },
    PackageSpec {
        name: "gnumake",
        aliases: &["make"],
        nix_attr: "nixpkgs#gnumake",
        binaries: &["make"],
        verify: &[VerifyCommand {
            binary: "make",
            args: &["--version"],
        }],
        category: "dev",
        description: "Build automation",
    },
    PackageSpec {
        name: "httpie",
        aliases: &[],
        nix_attr: "nixpkgs#httpie",
        binaries: &["http"],
        verify: &[VerifyCommand {
            binary: "http",
            args: &["--version"],
        }],
        category: "dev",
        description: "HTTP client",
    },
    PackageSpec {
        name: "jq",
        aliases: &[],
        nix_attr: "nixpkgs#jq",
        binaries: &["jq"],
        verify: &[VerifyCommand {
            binary: "jq",
            args: &["--version"],
        }],
        category: "dev",
        description: "JSON processor",
    },
    PackageSpec {
        name: "just",
        aliases: &[],
        nix_attr: "nixpkgs#just",
        binaries: &["just"],
        verify: &[VerifyCommand {
            binary: "just",
            args: &["--version"],
        }],
        category: "dev",
        description: "Command runner",
    },
    PackageSpec {
        name: "nodejs",
        aliases: &["node"],
        nix_attr: "nixpkgs#nodejs",
        binaries: &["node", "npm"],
        verify: &[VerifyCommand {
            binary: "node",
            args: &["--version"],
        }],
        category: "dev",
        description: "JavaScript runtime",
    },
    PackageSpec {
        name: "openssl",
        aliases: &[],
        nix_attr: "nixpkgs#openssl",
        binaries: &["openssl"],
        verify: &[VerifyCommand {
            binary: "openssl",
            args: &["version"],
        }],
        category: "dev",
        description: "Cryptography toolkit",
    },
    PackageSpec {
        name: "pkg-config",
        aliases: &[],
        nix_attr: "nixpkgs#pkg-config",
        binaries: &["pkg-config"],
        verify: &[VerifyCommand {
            binary: "pkg-config",
            args: &["--version"],
        }],
        category: "dev",
        description: "Package configuration",
    },
    PackageSpec {
        name: "python3",
        aliases: &["python"],
        nix_attr: "nixpkgs#python3",
        binaries: &["python3"],
        verify: &[VerifyCommand {
            binary: "python3",
            args: &["--version"],
        }],
        category: "dev",
        description: "Python interpreter",
    },
    PackageSpec {
        name: "sqlite",
        aliases: &[],
        nix_attr: "nixpkgs#sqlite",
        binaries: &["sqlite3"],
        verify: &[VerifyCommand {
            binary: "sqlite3",
            args: &["--version"],
        }],
        category: "dev",
        description: "SQL database engine",
    },
    PackageSpec {
        name: "tree",
        aliases: &[],
        nix_attr: "nixpkgs#tree",
        binaries: &["tree"],
        verify: &[VerifyCommand {
            binary: "tree",
            args: &["--version"],
        }],
        category: "dev",
        description: "Directory tree viewer",
    },
    PackageSpec {
        name: "uv",
        aliases: &[],
        nix_attr: "nixpkgs#uv",
        binaries: &["uv"],
        verify: &[VerifyCommand {
            binary: "uv",
            args: &["--version"],
        }],
        category: "dev",
        description: "Python package manager",
    },
    // net
    PackageSpec {
        name: "wget",
        aliases: &[],
        nix_attr: "nixpkgs#wget",
        binaries: &["wget"],
        verify: &[VerifyCommand {
            binary: "wget",
            args: &["--version"],
        }],
        category: "net",
        description: "URL downloader",
    },
    PackageSpec {
        name: "curl",
        aliases: &[],
        nix_attr: "nixpkgs#curl",
        binaries: &["curl"],
        verify: &[VerifyCommand {
            binary: "curl",
            args: &["--version"],
        }],
        category: "net",
        description: "URL transfer tool",
    },
    // language
    PackageSpec {
        name: "go",
        aliases: &["golang"],
        nix_attr: "nixpkgs#go",
        binaries: &["go"],
        verify: &[VerifyCommand {
            binary: "go",
            args: &["version"],
        }],
        category: "language",
        description: "Go programming language toolchain",
    },
    PackageSpec {
        name: "rustup",
        aliases: &[],
        nix_attr: "nixpkgs#rustup",
        binaries: &["rustup"],
        verify: &[VerifyCommand {
            binary: "rustup",
            args: &["--version"],
        }],
        category: "language",
        description: "Rust toolchain installer and manager",
    },
    // database
    PackageSpec {
        name: "postgresql",
        aliases: &["postgres"],
        nix_attr: "nixpkgs#postgresql",
        binaries: &["psql", "postgres"],
        verify: &[
            VerifyCommand {
                binary: "psql",
                args: &["--version"],
            },
            VerifyCommand {
                binary: "postgres",
                args: &["--version"],
            },
        ],
        category: "database",
        description: "PostgreSQL database server and CLI tools",
    },
    PackageSpec {
        name: "redis",
        aliases: &[],
        nix_attr: "nixpkgs#redis",
        binaries: &["redis-server", "redis-cli"],
        verify: &[
            VerifyCommand {
                binary: "redis-server",
                args: &["--version"],
            },
            VerifyCommand {
                binary: "redis-cli",
                args: &["--version"],
            },
        ],
        category: "database",
        description: "Redis server and command-line client",
    },
    // infrastructure
    PackageSpec {
        name: "terraform",
        aliases: &["tf"],
        nix_attr: "nixpkgs#terraform",
        binaries: &["terraform"],
        verify: &[VerifyCommand {
            binary: "terraform",
            args: &["version"],
        }],
        category: "infrastructure",
        description: "Infrastructure as code CLI",
    },
    PackageSpec {
        name: "kubectl",
        aliases: &["kube"],
        nix_attr: "nixpkgs#kubectl",
        binaries: &["kubectl"],
        verify: &[VerifyCommand {
            binary: "kubectl",
            args: &["version", "--client"],
        }],
        category: "infrastructure",
        description: "Kubernetes command-line tool",
    },
    PackageSpec {
        name: "helm",
        aliases: &[],
        nix_attr: "nixpkgs#kubernetes-helm",
        binaries: &["helm"],
        verify: &[VerifyCommand {
            binary: "helm",
            args: &["version", "--short"],
        }],
        category: "infrastructure",
        description: "Kubernetes package manager",
    },
    PackageSpec {
        name: "k9s",
        aliases: &[],
        nix_attr: "nixpkgs#k9s",
        binaries: &["k9s"],
        verify: &[VerifyCommand {
            binary: "k9s",
            args: &["version"],
        }],
        category: "infrastructure",
        description: "Terminal UI for Kubernetes clusters",
    },
    PackageSpec {
        name: "docker-client",
        aliases: &["docker"],
        nix_attr: "nixpkgs#docker-client",
        binaries: &["docker"],
        verify: &[VerifyCommand {
            binary: "docker",
            args: &["--version"],
        }],
        category: "infrastructure",
        description: "Docker CLI client",
    },
    // security
    PackageSpec {
        name: "age",
        aliases: &[],
        nix_attr: "nixpkgs#age",
        binaries: &["age", "age-keygen"],
        verify: &[
            VerifyCommand {
                binary: "age",
                args: &["--version"],
            },
            VerifyCommand {
                binary: "age-keygen",
                args: &["--version"],
            },
        ],
        category: "security",
        description: "Simple modern file encryption tool",
    },
    PackageSpec {
        name: "sops",
        aliases: &[],
        nix_attr: "nixpkgs#sops",
        binaries: &["sops"],
        verify: &[VerifyCommand {
            binary: "sops",
            args: &["--version"],
        }],
        category: "security",
        description: "Editor for encrypted secrets",
    },
    // editor
    PackageSpec {
        name: "neovim",
        aliases: &["nvim"],
        nix_attr: "nixpkgs#neovim",
        binaries: &["nvim"],
        verify: &[VerifyCommand {
            binary: "nvim",
            args: &["--version"],
        }],
        category: "editor",
        description: "Modern Vim-based text editor",
    },
    // terminal
    PackageSpec {
        name: "tmux",
        aliases: &[],
        nix_attr: "nixpkgs#tmux",
        binaries: &["tmux"],
        verify: &[VerifyCommand {
            binary: "tmux",
            args: &["-V"],
        }],
        category: "terminal",
        description: "Terminal multiplexer",
    },
    // git
    PackageSpec {
        name: "git-delta",
        aliases: &["delta"],
        nix_attr: "nixpkgs#git-delta",
        binaries: &["delta"],
        verify: &[VerifyCommand {
            binary: "delta",
            args: &["--version"],
        }],
        category: "git",
        description: "Syntax-highlighted Git diff viewer.",
    },
    // terminal
    PackageSpec {
        name: "zoxide",
        aliases: &["z"],
        nix_attr: "nixpkgs#zoxide",
        binaries: &["zoxide"],
        verify: &[VerifyCommand {
            binary: "zoxide",
            args: &["--version"],
        }],
        category: "terminal",
        description: "Smarter directory navigation for the terminal.",
    },
    PackageSpec {
        name: "direnv",
        aliases: &[],
        nix_attr: "nixpkgs#direnv",
        binaries: &["direnv"],
        verify: &[VerifyCommand {
            binary: "direnv",
            args: &["version"],
        }],
        category: "terminal",
        description: "Automatically loads and unloads environment variables per directory.",
    },
    PackageSpec {
        name: "starship",
        aliases: &[],
        nix_attr: "nixpkgs#starship",
        binaries: &["starship"],
        verify: &[VerifyCommand {
            binary: "starship",
            args: &["--version"],
        }],
        category: "terminal",
        description: "Cross-shell customizable prompt.",
    },
    // git
    PackageSpec {
        name: "lazygit",
        aliases: &["lg"],
        nix_attr: "nixpkgs#lazygit",
        binaries: &["lazygit"],
        verify: &[VerifyCommand {
            binary: "lazygit",
            args: &["--version"],
        }],
        category: "git",
        description: "Terminal UI for Git workflows.",
    },
];

type UpdateTarget = (String, &'static PackageSpec);

fn resolve_package(name: &str) -> Option<&'static PackageSpec> {
    SUPPORTED_PACKAGES
        .iter()
        .find(|p| p.name == name || p.aliases.contains(&name))
}

/// A file-based mutex guard for mutation commands.
///
/// Acquires the lock by atomically creating `~/.root/root.lockfile`
/// with the current PID and a timestamp. On contention, checks whether
/// the lock-holding process is still alive. If the process is dead,
/// the stale lock is removed and re-acquisition is attempted.
///
/// Released on Drop (which removes the lock file).
#[derive(Debug)]
struct MutationGuard {
    lock_path: PathBuf,
}

impl MutationGuard {
    fn acquire() -> anyhow::Result<Self> {
        let dir = root_lockfile::init_root_dir()?;
        let lock_path = dir.join("root.lockfile");

        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let lock_content = format!("{}\n{}\n", pid, now);

        match Self::try_acquire(&lock_path, &lock_content) {
            Ok(()) => Ok(Self { lock_path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Stale lock recovery: read the lock file, check if the PID is alive
                match Self::check_stale_lock(&lock_path) {
                    Ok(true) => Err(anyhow::anyhow!(
                        "Another Root mutation is in progress (PID {}).\n\
                         If this is unexpected, delete ~/.root/root.lockfile and try again.",
                        pid
                    )),
                    Ok(false) => {
                        // Stale lock — remove it and retry
                        let _ = std::fs::remove_file(&lock_path);
                        Self::try_acquire(&lock_path, &lock_content).with_context(|| {
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

    fn try_acquire(lock_path: &Path, content: &str) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    /// Returns `Ok(true)` if the lock holder's PID is still alive,
    /// `Ok(false)` if the process is dead (stale), or `Err` if the lock
    /// file is unreadable or malformed.
    fn check_stale_lock(lock_path: &Path) -> Result<bool> {
        let mut content = String::new();
        std::fs::File::open(lock_path)
            .and_then(|mut f| f.read_to_string(&mut content))
            .map_err(|_| anyhow::anyhow!("Cannot read lock file"))?;

        let pid_str = content.lines().next().unwrap_or("").trim();
        let lock_pid: u32 = pid_str
            .parse()
            .map_err(|_| anyhow::anyhow!("Malformed lock file (invalid PID)"))?;

        // Check if the PID is alive (signal 0 test, portable on Unix)
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(lock_pid.to_string())
            .output()
            .map_err(|_| anyhow::anyhow!("Cannot check process liveness"))?;

        Ok(status.status.success())
    }
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

pub mod brew;
pub mod events;
pub mod execution;
pub mod inventory;
pub mod ollama;
pub mod policy;

pub use execution::{run, RunReport, RunRequest};

#[derive(Debug, Serialize)]
pub struct ListPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    pub packages: Vec<ListPackage>,
    pub nix_profile: String,
}

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub success: bool,
    pub root_dir: String,
    pub nix_detected: bool,
    pub profile_ready: bool,
    pub snapshot_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct InstallReport {
    pub success: bool,
    pub operation: &'static str,
    pub package: String,
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
    pub snapshot_id: String,
    pub rollback_available: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanReport {
    pub package: String,
    pub original_input: Option<String>,
    pub found: bool,
    pub description: String,
    pub would_create_snapshot: bool,
    pub attributes: Vec<String>,
    pub nix_attr: String,
    pub expected_binaries: Vec<String>,
    pub verify_commands: Vec<String>,
    pub rollback_available: bool,
}

#[derive(Debug, Serialize)]
pub struct SearchMatch {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub category: &'static str,
    pub description: &'static str,
    pub nix_attr: &'static str,
    pub binaries: &'static [&'static str],
    pub verify: Vec<String>,
    pub matched_fields: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct SearchOutput {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub supported_count: usize,
}

#[derive(Debug, Serialize)]
pub struct RemoveReport {
    pub success: bool,
    pub package: String,
    pub snapshot_id: String,
    pub rollback_available: bool,
}

#[derive(Debug, Serialize)]
pub struct RollbackReport {
    pub success: bool,
    pub from_snapshot: String,
    pub packages_removed: Vec<String>,
    pub packages_restored: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryOutput {
    pub snapshots: Vec<SnapshotSummary>,
    pub events: Vec<events::RootEvent>,
}

#[derive(Debug, Serialize)]
pub struct UpdateReport {
    pub success: bool,
    pub requested: Option<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
    pub snapshot_id: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PermissionsReport {
    pub success: bool,
    pub path: String,
    pub source: &'static str,
    pub policy: policy::RootPolicy,
}

#[derive(Debug, Serialize)]
pub struct PolicyApplyReport {
    pub success: bool,
    pub path: String,
    pub version: u32,
}

pub fn permissions() -> Result<PermissionsReport> {
    root_lockfile::init_root_dir()?;
    let path = policy::policy_path()?;
    let (policy, configured) = policy::read_policy()?;
    Ok(PermissionsReport {
        success: true,
        path: path.to_string_lossy().to_string(),
        source: if configured { "configured" } else { "default" },
        policy,
    })
}

pub fn apply_policy(source: &Path) -> Result<PolicyApplyReport> {
    let _guard = MutationGuard::acquire()?;
    let (policy, destination) = policy::apply_policy(source)?;
    events::record_policy_event(
        "root policy apply",
        events::RootEventStatus::Completed,
        "applied",
        format!("Activated policy from {}", source.display()),
    )?;
    Ok(PolicyApplyReport {
        success: true,
        path: destination.to_string_lossy().to_string(),
        version: policy.version,
    })
}

pub(crate) fn enforce_policy(action: policy::PolicyAction, subject: Option<&str>) -> Result<()> {
    let (active_policy, _) = policy::read_policy()?;
    let decision = policy::evaluate(&active_policy, action, subject);
    let status = if decision.allowed {
        events::RootEventStatus::Completed
    } else {
        events::RootEventStatus::Failed
    };
    events::record_policy_event(
        &format!("policy check {}", action.as_str()),
        status,
        if decision.allowed {
            "allowed"
        } else {
            "denied"
        },
        decision.reason.clone(),
    )?;
    if decision.allowed {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Policy denied {}{}: {}",
            action.as_str(),
            subject
                .map(|value| format!(" '{}'", value))
                .unwrap_or_default(),
            decision.reason
        ))
    }
}

/// Platform detected for Nix automatic installation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NixInstallPlatform {
    MacOs,
    Linux,
}

fn detect_nix_install_platform_inner(os: &str) -> Result<NixInstallPlatform> {
    match os {
        "macos" => Ok(NixInstallPlatform::MacOs),
        "linux" => Ok(NixInstallPlatform::Linux),
        other => Err(anyhow::anyhow!(
            "Unsupported platform: '{}'. Nix installation via `root init --install-nix` \
             is only supported on macOS and Linux.\n\n\
             Please install Nix manually from:\n  https://nixos.org/download/\n\n\
             After installing, run:\n  root doctor",
            other
        )),
    }
}

/// Detect whether the current platform supports automatic Nix installation.
pub fn detect_nix_install_platform() -> Result<NixInstallPlatform> {
    detect_nix_install_platform_inner(std::env::consts::OS)
}

/// Build the shell command and arguments used to install Nix.
pub fn build_nix_installer_command() -> (String, Vec<String>) {
    let script = "curl -L https://nixos.org/nix/install | sh".to_string();
    (
        script.clone(),
        vec!["sh".to_string(), "-c".to_string(), script],
    )
}

/// Format the recovery message shown after a failed Nix installation.
pub fn format_nix_installer_failure_message() -> String {
    "Nix installation failed.\n\n\
     You can install Nix manually:\n\
       curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install\n\n\
     Then run:\n  root doctor"
        .to_string()
}

/// Format the message shown when running non-interactively without --yes.
pub fn format_non_interactive_message() -> String {
    "Nix installation requires confirmation, but this session is not interactive.\n\n\
     To install Nix, run:\n  root init --install-nix --yes\n\n\
     Or install Nix manually:\n\
       curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install\n\n\
     Then run:\n  root doctor"
        .to_string()
}

/// Format the recovery message shown when the post-install `nix --version` probe fails.
pub fn format_post_install_version_failure(stderr: &str) -> String {
    let detail = if stderr.is_empty() {
        "unknown error".to_string()
    } else {
        stderr.trim().to_string()
    };
    format!(
        "Nix was installed but 'nix --version' failed: {}.\n\n\
         Try restarting your shell, then run:\n  root doctor",
        detail
    )
}

/// Install Nix using the official multi-user installer.
///
/// # Safety note
///
/// Checksum verification of the installer script is not currently implemented.
/// The installer is downloaded via HTTPS, which provides transport-layer security,
/// but the script contents are not independently verified before execution.
/// This is future work — the script should be downloaded separately, its SHA-256
/// checksum verified against a published value, and only then executed.
///
/// # Arguments
///
/// * `yes` — If `true`, skip the interactive confirmation prompt.
pub fn install_nix(yes: bool) -> Result<()> {
    let platform = detect_nix_install_platform()?;

    // Explanation
    println!("Root needs Nix to manage packages. Root will now download and run the");
    println!("Nix installer from https://nixos.org/nix/install. This may modify your");
    println!("shell profile and create /nix.");
    match platform {
        NixInstallPlatform::MacOs => {
            println!("On macOS, the installer will set up a multi-user Nix installation");
            println!("under /nix and may request administrator privileges.");
        }
        NixInstallPlatform::Linux => {
            println!("On Linux, the installer will set up a multi-user Nix installation");
            println!("under /nix and may request sudo access.");
        }
    }
    println!();

    // Confirmation
    if !yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(format_non_interactive_message());
        }
        print!("Continue? [Y/n] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if !trimmed.is_empty()
            && !trimmed.eq_ignore_ascii_case("y")
            && !trimmed.eq_ignore_ascii_case("yes")
        {
            anyhow::bail!("Nix installation cancelled by user.");
        }
    }

    // Run installer
    let (_, args) = build_nix_installer_command();
    let program = &args[0];
    let program_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    let status = std::process::Command::new(program)
        .args(&program_args)
        .status()
        .context("Failed to run Nix installer")?;

    if !status.success() {
        anyhow::bail!(format_nix_installer_failure_message());
    }

    // Post-install probe
    println!("\nVerifying Nix installation...");

    let version_output = std::process::Command::new("nix")
        .arg("--version")
        .output()
        .map_err(|e| {
            anyhow::anyhow!(
                "Nix was installed but could not be executed: {}.\n\n\
                 Try restarting your shell, or add Nix to your PATH:\n\
                 . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh\n\n\
                 Then run:\n  root doctor",
                e
            )
        })?;

    if version_output.status.success() {
        let version = String::from_utf8_lossy(&version_output.stdout)
            .trim()
            .to_string();
        println!("✓ Nix {} installed and working", version);
    } else {
        let stderr = String::from_utf8_lossy(&version_output.stderr);
        anyhow::bail!(format_post_install_version_failure(&stderr));
    }

    let features_output = std::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--version",
        ])
        .output();

    match features_output {
        Ok(output) if output.status.success() => {
            println!("✓ Nix experimental features (nix-command, flakes) available");
        }
        _ => {
            println!("⚠ Nix experimental features (nix-command, flakes) are not enabled.");
            println!("  To enable them, add to ~/.config/nix/nix.conf:");
            println!("    experimental-features = nix-command flakes");
            println!("  Or edit /etc/nix/nix.conf to include the same line.");
        }
    }

    Ok(())
}

pub fn init(adapter: &impl NixAdapter) -> Result<InitReport> {
    let root_dir =
        root_lockfile::init_root_dir().context("Failed to initialize Root directories")?;
    let nix_detected = adapter
        .check_availability()
        .map_err(|e| anyhow::anyhow!(e))
        .unwrap_or(false);
    Ok(InitReport {
        success: true,
        root_dir: root_dir.to_string_lossy().to_string(),
        nix_detected,
        profile_ready: true,
        snapshot_enabled: true,
    })
}

pub(crate) fn get_or_create_rootfile() -> Result<Rootfile> {
    let dir = get_root_dir()?;
    let path = dir.join("Rootfile");
    if path.exists() {
        Rootfile::read_from_file(&path)
    } else {
        Ok(Rootfile::default())
    }
}

fn save_rootfile(rootfile: &Rootfile) -> Result<()> {
    let path = get_root_dir()?.join("Rootfile");
    rootfile.write_to_file(&path)
}

fn get_or_create_lock() -> Result<RootLock> {
    let dir = get_root_dir()?;
    let path = dir.join("root.lock");
    if path.exists() {
        RootLock::read_from_file(&path)
    } else {
        Ok(RootLock {
            version: ROOT_LOCK_SCHEMA_VERSION,
            platform: root_lockfile::detect_platform()?,
            nixpkgs: NixpkgsConfig {
                rev: "unknown".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: Vec::new(),
        })
    }
}

#[cfg(test)]
fn save_lock(lock: &RootLock) -> Result<()> {
    let path = get_root_dir()?.join("root.lock");
    lock.write_to_file(&path)
}

fn get_or_create_lock_v2() -> Result<RootLockV2> {
    let dir = get_root_dir()?;
    let path = dir.join("root.lock");
    if path.exists() {
        let lock = root_lockfile::read_compatible_lock_v2(&path)?;
        root_lockfile::validate_store_paths(&lock).context(format!(
            "Existing lockfile at {} failed validation",
            path.display()
        ))?;
        root_lockfile::validate_locked_models(&lock).context(format!(
            "Existing lockfile at {} failed models validation",
            path.display()
        ))?;
        Ok(lock)
    } else {
        Ok(get_or_create_lock()?.to_v2())
    }
}

fn refuse_unsupported_lock_at(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read lockfile at {}", path.display()))?;
    let version = root_lockfile::peek_lock_schema_version(&content)?;
    root_lockfile::validate_supported_lock_version(version)
}

fn refuse_unsupported_active_lock() -> Result<()> {
    refuse_unsupported_lock_at(&get_root_dir()?.join("root.lock"))
}

fn save_lock_v2(lock: &RootLockV2) -> Result<()> {
    root_lockfile::validate_locked_models(lock)
        .context("Lockfile models failed validation before write")?;
    let path = get_root_dir()?.join("root.lock");
    lock.write_to_file(&path)
}

fn locked_package_changed(current: &LockedPackageV2, target: &LockedPackageV2) -> bool {
    serde_json::to_value(current).ok() != serde_json::to_value(target).ok()
}

fn locked_installable_for(
    adapter: &impl NixAdapter,
    pkg: &str,
) -> Result<(root_nix::FlakeMetadata, String)> {
    let flake = adapter
        .flake_metadata("nixpkgs")
        .map_err(|e| anyhow::anyhow!(e))?;
    let locked_ref = flake
        .locked_url
        .clone()
        .or_else(|| {
            flake
                .rev
                .as_ref()
                .map(|rev| format!("github:NixOS/nixpkgs/{}", rev))
        })
        .unwrap_or_else(|| "nixpkgs".to_string());
    Ok((flake, format!("{}#{}", locked_ref, pkg)))
}

fn deterministic_package_from_resolution(
    canonical_name: &str,
    requested: &str,
    installable: &str,
    resolution: &root_nix::LockedPackageResolution,
) -> Result<LockedPackageV2> {
    let version = resolution
        .metadata
        .version
        .clone()
        .or_else(|| {
            resolution.metadata.name.as_ref().and_then(|name| {
                name.strip_prefix(&format!("{}-", canonical_name))
                    .map(|value| value.to_string())
            })
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut outputs = BTreeMap::new();
    let mut store_paths = BTreeMap::new();
    for output in &resolution.outputs {
        let path = output.path.to_string_lossy().to_string();
        if path.ends_with(".drv") {
            return Err(anyhow::anyhow!(
                "Root resolved a derivation path but no realized output path for {}. \
                 Expected an output store path, got: {}",
                canonical_name,
                path
            ));
        }
        let path_info = resolution
            .path_info
            .iter()
            .find(|info| info.path == output.path);
        outputs.insert(
            output.output_name.clone(),
            LockedPackageOutput {
                store_path: path.clone(),
                content_hash: None,
                nar_hash: path_info.and_then(|info| info.nar_hash.clone()),
                references: path_info
                    .map(|info| {
                        info.references
                            .iter()
                            .map(|reference| reference.to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            },
        );
        store_paths.insert(output.output_name.clone(), path);
    }

    let primary_store_path = store_paths
        .get("out")
        .cloned()
        .or_else(|| store_paths.values().next().cloned())
        .unwrap_or_default();

    let mut meta = BTreeMap::new();
    if let Some(name) = &resolution.metadata.name {
        meta.insert("name".to_string(), serde_json::Value::String(name.clone()));
    }
    if let Some(description) = &resolution.metadata.description {
        meta.insert(
            "description".to_string(),
            serde_json::Value::String(description.clone()),
        );
    }

    let binaries = resolve_package(canonical_name)
        .map(|spec| {
            spec.binaries
                .iter()
                .map(|binary| (*binary).to_string())
                .collect()
        })
        .unwrap_or_else(|| vec![canonical_name.to_string()]);

    let mut package = LockedPackageV2 {
        name: canonical_name.to_string(),
        requested: requested.to_string(),
        version,
        attribute: canonical_name.to_string(),
        store_path: primary_store_path,
        binaries,
        installable: Some(installable.to_string()),
        flake_attribute: Some(canonical_name.to_string()),
        drv_path: Some(
            resolution
                .derivation
                .derivation_path
                .to_string_lossy()
                .to_string(),
        ),
        outputs,
        store_paths,
        meta,
        content_hash: None,
    };

    let hash_input = serde_json::to_vec(&package).unwrap_or_default();
    package.content_hash = Some(root_lockfile::compute_sha256(&hash_input));
    Ok(package)
}

fn build_v2_lock(
    current: &RootLockV2,
    flake: &root_nix::FlakeMetadata,
    packages: Vec<LockedPackageV2>,
) -> Result<RootLockV2> {
    let now = chrono::Utc::now().to_rfc3339();
    let platform = current.platform.clone();
    let models = current.models.clone();
    Ok(RootLockV2 {
        version: root_lockfile::emit_lock_version(&models),
        root_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        created_at: current.created_at.clone().or(Some(now.clone())),
        updated_at: Some(now),
        platform: platform.clone(),
        nix: NixRuntime {
            version: None,
            system: Some(platform.clone()),
            store_dir: Some("/nix/store".to_string()),
            sandbox: None,
        },
        nixpkgs: NixpkgsConfigV2 {
            rev: flake
                .rev
                .clone()
                .unwrap_or_else(|| current.nixpkgs.rev.clone()),
            source: flake.original_url.clone(),
            flake_ref: flake.locked_url.clone(),
            nar_hash: flake.nar_hash.clone(),
            last_modified: flake.last_modified.map(|value| value.to_string()),
            system: Some(platform),
            config: BTreeMap::new(),
            overlays: Vec::new(),
        },
        profile: current.profile.clone(),
        packages,
        models,
    })
}

fn verify_profile_contains_outputs(
    adapter: &impl NixAdapter,
    outputs: &BTreeMap<String, String>,
) -> Result<()> {
    for store_path in outputs.values() {
        if store_path.ends_with(".drv") {
            return Err(anyhow::anyhow!(
                "Root resolved a derivation path but no realized output path. \
                 Refusing to verify .drv path as an installed output: {}",
                store_path
            ));
        }
    }
    let profile_json = adapter
        .profile_list_json()
        .map_err(|e| anyhow::anyhow!(e))?;
    for store_path in outputs.values() {
        if !profile_json.contains(store_path) {
            return Err(anyhow::anyhow!(
                "Installed profile did not contain locked Nix store path {}",
                store_path
            ));
        }
    }
    Ok(())
}

fn validate_mutation_result(
    adapter: &impl NixAdapter,
    before_generation: Option<u64>,
    packages: &[&str],
    binaries: &[&str],
    store_paths: &[&str],
) -> Result<()> {
    let validation = adapter
        .validate_profile_mutation(before_generation, packages, binaries, store_paths)
        .map_err(|e| anyhow::anyhow!("Profile validation framework error: {}", e))?;

    // Hard error: missing store paths or .drv paths in outputs
    if !validation.expected_packages_present || !validation.drv_paths_found.is_empty() {
        let mut msgs = Vec::new();
        if !validation.missing_output_paths.is_empty() {
            msgs.push(format!(
                "Missing output paths: {}",
                validation.missing_output_paths.join(", ")
            ));
        }
        if !validation.drv_paths_found.is_empty() {
            msgs.push(format!(
                ".drv paths found in outputs: {}",
                validation.drv_paths_found.join(", ")
            ));
        }
        return Err(anyhow::anyhow!(
            "Profile mutation validation failed: {}",
            msgs.join("; ")
        ));
    }

    // Hard error: generation did not change when it should have
    if !validation.generation_changed && (!packages.is_empty() || !store_paths.is_empty()) {
        return Err(anyhow::anyhow!(
            "Profile generation did not change after mutation, \
             which suggests the profile was not actually updated"
        ));
    }

    Ok(())
}

fn parse_attributes(search_output: &str) -> Vec<String> {
    search_output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("* nixpkgs#") {
                rest.split_whitespace().next().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn search_match_for_package(query_lower: &str, spec: &'static PackageSpec) -> Option<SearchMatch> {
    if query_lower.is_empty() {
        return None;
    }

    let mut matched_fields = Vec::new();
    if spec.name.to_lowercase().contains(query_lower) {
        matched_fields.push("name");
    }
    if spec
        .aliases
        .iter()
        .any(|alias| alias.to_lowercase().contains(query_lower))
    {
        matched_fields.push("alias");
    }
    if spec.category.to_lowercase().contains(query_lower) {
        matched_fields.push("category");
    }
    if spec.description.to_lowercase().contains(query_lower) {
        matched_fields.push("description");
    }
    if spec.nix_attr.to_lowercase().contains(query_lower) {
        matched_fields.push("nix_attr");
    }
    if spec
        .binaries
        .iter()
        .any(|binary| binary.to_lowercase().contains(query_lower))
    {
        matched_fields.push("binary");
    }

    if matched_fields.is_empty() {
        return None;
    }

    Some(SearchMatch {
        name: spec.name,
        aliases: spec.aliases,
        category: spec.category,
        description: spec.description,
        nix_attr: spec.nix_attr,
        binaries: spec.binaries,
        verify: spec
            .verify
            .iter()
            .map(|verify| format!("{} {}", verify.binary, verify.args.join(" ")))
            .collect(),
        matched_fields,
    })
}

fn search_rank(query_lower: &str, package: &SearchMatch) -> u8 {
    if package.name.eq_ignore_ascii_case(query_lower) {
        return 0;
    }
    if package
        .aliases
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(query_lower))
    {
        return 1;
    }
    if package
        .binaries
        .iter()
        .any(|binary| binary.eq_ignore_ascii_case(query_lower))
    {
        return 2;
    }
    if package.name.to_lowercase().contains(query_lower) {
        return 3;
    }
    if package
        .aliases
        .iter()
        .any(|alias| alias.to_lowercase().contains(query_lower))
    {
        return 4;
    }
    if package
        .binaries
        .iter()
        .any(|binary| binary.to_lowercase().contains(query_lower))
    {
        return 5;
    }
    if package.category.to_lowercase().contains(query_lower) {
        return 6;
    }
    if package.nix_attr.to_lowercase().contains(query_lower) {
        return 7;
    }
    8
}

pub fn search(query: &str) -> SearchOutput {
    let trimmed = query.trim();
    let query_lower = trimmed.to_lowercase();
    let query_ref: &str = &query_lower;
    let mut matches: Vec<SearchMatch> = SUPPORTED_PACKAGES
        .iter()
        .filter_map(|spec| search_match_for_package(query_ref, spec))
        .collect();
    matches.sort_by(|a, b| {
        search_rank(query_ref, a)
            .cmp(&search_rank(query_ref, b))
            .then_with(|| a.name.cmp(b.name))
    });

    SearchOutput {
        query: trimmed.to_string(),
        matches,
        supported_count: SUPPORTED_PACKAGES.len(),
    }
}

pub fn plan(adapter: &impl NixAdapter, pkg: &str) -> Result<PlanReport> {
    let spec = resolve_package(pkg).ok_or_else(|| {
        anyhow::anyhow!(
            "Root does not support \"{}\" yet.\n\nSupported packages:\n{}\n\nMore packages are coming soon.",
            pkg,
            format_supported_packages()
        )
    })?;
    let canonical = spec.name;
    let original_input = if canonical != pkg {
        Some(pkg.to_string())
    } else {
        None
    };
    match adapter.search(canonical) {
        Ok(description) => {
            let attributes = parse_attributes(&description);
            Ok(PlanReport {
                package: canonical.to_string(),
                original_input,
                found: true,
                description,
                would_create_snapshot: true,
                attributes,
                nix_attr: spec.nix_attr.to_string(),
                expected_binaries: spec.binaries.iter().map(|b| (*b).to_string()).collect(),
                verify_commands: spec
                    .verify
                    .iter()
                    .map(|v| format!("{} {}", v.binary, v.args.join(" ")))
                    .collect(),
                rollback_available: true,
            })
        }
        Err(root_nix::NixError::NotFound(_)) => Ok(PlanReport {
            package: canonical.to_string(),
            original_input,
            found: false,
            description: String::new(),
            would_create_snapshot: true,
            attributes: Vec::new(),
            nix_attr: spec.nix_attr.to_string(),
            expected_binaries: spec.binaries.iter().map(|b| (*b).to_string()).collect(),
            verify_commands: spec
                .verify
                .iter()
                .map(|v| format!("{} {}", v.binary, v.args.join(" ")))
                .collect(),
            rollback_available: true,
        }),
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}

pub fn install(adapter: &impl NixAdapter, pkg: &str) -> Result<InstallReport> {
    let spec = resolve_package(pkg).ok_or_else(|| {
        anyhow::anyhow!(
            "Root does not support \"{}\" yet.\n\nSupported packages:\n{}\n\nMore packages are coming soon.",
            pkg,
            format_supported_packages()
        )
    })?;
    let canonical = spec.name;
    let original = pkg;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    enforce_policy(policy::PolicyAction::Install, Some(canonical))?;
    let _guard = MutationGuard::acquire()?;
    let lock = get_or_create_lock_v2()?;
    let before_packages: Vec<String> = lock.packages.iter().map(|p| p.name.clone()).collect();

    let (flake, installable) = locked_installable_for(adapter, canonical)?;
    let resolution = adapter
        .resolve_locked_package(canonical, Some(&installable))
        .map_err(|e| anyhow::anyhow!(e))?;
    let locked_package =
        deterministic_package_from_resolution(canonical, original, &installable, &resolution)?;

    let snapshot = Snapshot::create_from_v2(&format!("before install {}", canonical), &lock)?;
    let snapshot_id = snapshot.id.clone();

    let before_gen = adapter
        .profile_generation()
        .map_err(|e| anyhow::anyhow!(e))?;

    adapter
        .install_installable(canonical, &installable)
        .map_err(|e| anyhow::anyhow!(e))?;
    verify_profile_contains_outputs(adapter, &locked_package.store_paths)?;
    validate_mutation_result(
        adapter,
        before_gen,
        &[canonical],
        spec.binaries,
        &locked_package
            .store_paths
            .values()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
    )?;

    let mut rootfile = get_or_create_rootfile()?;
    rootfile
        .packages
        .insert(canonical.to_string(), locked_package.version.clone());
    save_rootfile(&rootfile)?;

    let mut v2_packages: Vec<LockedPackageV2> = lock
        .packages
        .iter()
        .filter(|package| package.name != canonical)
        .cloned()
        .collect();
    v2_packages.push(locked_package.clone());
    let v2_lock = build_v2_lock(&lock, &flake, v2_packages)?;
    root_lockfile::validate_store_paths(&v2_lock)
        .context("Newly created lockfile failed validation after install")?;
    root_lockfile::validate_locked_models(&v2_lock)
        .context("Newly created lockfile failed models validation after install")?;
    save_lock_v2(&v2_lock)?;

    let _ = events::record_event(
        events::RootEventType::Install,
        events::RootEventStatus::Verified,
        &format!("root install {}", canonical),
        Some(canonical.to_string()),
        Some(snapshot_id.clone()),
        None,
        Some("Package installed successfully".to_string()),
    )?;

    let after_packages: Vec<String> = v2_lock.packages.iter().map(|p| p.name.clone()).collect();

    let changed: Vec<String> = after_packages
        .iter()
        .filter(|p| !before_packages.contains(p))
        .cloned()
        .collect();
    let unchanged: Vec<String> = before_packages
        .iter()
        .filter(|p| after_packages.contains(p))
        .cloned()
        .collect();

    Ok(InstallReport {
        success: true,
        operation: "install",
        package: canonical.to_string(),
        changed,
        unchanged,
        snapshot_id,
        rollback_available: true,
        warnings: Vec::new(),
    })
}

fn update_requested_package(
    requested: Option<&str>,
    rootfile: &Rootfile,
) -> Result<(Vec<UpdateTarget>, Vec<String>)> {
    if let Some(pkg) = requested {
        let spec = resolve_package(pkg).ok_or_else(|| {
            anyhow::anyhow!(
                "Root does not support \"{}\" yet.\n\nSupported packages:\n{}\n\nMore packages are coming soon.",
                pkg,
                format_supported_packages()
            )
        })?;
        if !rootfile.packages.contains_key(spec.name) && !rootfile.packages.contains_key(pkg) {
            return Err(anyhow::anyhow!(
                "Package '{}' is not listed in Rootfile.\n\nInstall it first with:  root install {}",
                spec.name,
                spec.name
            ));
        }
        return Ok((vec![(pkg.to_string(), spec)], Vec::new()));
    }

    let mut keys: Vec<&str> = rootfile.packages.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for key in keys {
        if let Some(spec) = resolve_package(key) {
            if seen.insert(spec.name) {
                targets.push((key.to_string(), spec));
            }
        } else {
            skipped.push(key.to_string());
        }
    }

    Ok((targets, skipped))
}

pub fn update(adapter: &impl NixAdapter, pkg: Option<&str>) -> Result<UpdateReport> {
    root_lockfile::init_root_dir()?;
    let rootfile = get_or_create_rootfile()?;
    let (targets, skipped) = update_requested_package(pkg, &rootfile)?;

    if targets.is_empty() {
        return Ok(UpdateReport {
            success: true,
            requested: pkg.map(str::to_string),
            updated: Vec::new(),
            unchanged: Vec::new(),
            skipped,
            failed: Vec::new(),
            snapshot_id: None,
            warnings: vec!["No supported packages found in Rootfile.".to_string()],
        });
    }

    refuse_unsupported_active_lock()?;
    for (_, spec) in &targets {
        enforce_policy(policy::PolicyAction::Update, Some(spec.name))?;
    }
    let _guard = MutationGuard::acquire()?;
    let current_lock = get_or_create_lock_v2()?;

    let snapshot = Snapshot::create_from_v2(
        &match pkg {
            Some(pkg) => format!("before update {}", pkg),
            None => "before update all".to_string(),
        },
        &current_lock,
    )?;
    let snapshot_id = snapshot.id.clone();

    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut resolved_packages = Vec::new();
    let mut flake_for_lock = None;

    for (requested_key, spec) in &targets {
        let canonical = spec.name;
        let old_package = current_lock
            .packages
            .iter()
            .find(|package| package.name == canonical);
        let requested_name = old_package
            .map(|package| package.requested.as_str())
            .unwrap_or(requested_key.as_str());

        let (flake, installable) = locked_installable_for(adapter, canonical)?;
        let resolution = adapter
            .resolve_locked_package(canonical, Some(&installable))
            .map_err(|e| anyhow::anyhow!(e))?;
        let locked_package = deterministic_package_from_resolution(
            canonical,
            requested_name,
            &installable,
            &resolution,
        )?;

        if old_package
            .map(|old_package| locked_package_changed(old_package, &locked_package))
            .unwrap_or(true)
        {
            let before_gen = adapter
                .profile_generation()
                .map_err(|e| anyhow::anyhow!(e))?;
            adapter.remove(canonical).map_err(|e| anyhow::anyhow!(e))?;
            adapter
                .install_installable(canonical, &installable)
                .map_err(|e| anyhow::anyhow!(e))?;
            verify_profile_contains_outputs(adapter, &locked_package.store_paths)?;
            validate_mutation_result(
                adapter,
                before_gen,
                &[canonical],
                spec.binaries,
                &locked_package
                    .store_paths
                    .values()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>(),
            )?;
            updated.push(canonical.to_string());
        } else {
            unchanged.push(canonical.to_string());
        }

        flake_for_lock = Some(flake);
        resolved_packages.push(locked_package);
    }

    let target_names: std::collections::BTreeSet<&str> =
        targets.iter().map(|(_, spec)| spec.name).collect();
    let mut v2_packages: Vec<LockedPackageV2> = current_lock
        .packages
        .iter()
        .filter(|package| !target_names.contains(package.name.as_str()))
        .cloned()
        .collect();
    v2_packages.extend(resolved_packages.clone());

    let flake = match flake_for_lock {
        Some(flake) => flake,
        None => adapter
            .flake_metadata("nixpkgs")
            .map_err(|e| anyhow::anyhow!(e))?,
    };
    let new_lock = build_v2_lock(&current_lock, &flake, v2_packages)?;
    root_lockfile::validate_store_paths(&new_lock)
        .context("Newly created lockfile failed validation after update")?;
    root_lockfile::validate_locked_models(&new_lock)
        .context("Newly created lockfile failed models validation after update")?;

    let mut next_rootfile = rootfile;
    for package in &resolved_packages {
        next_rootfile.packages.remove(&package.requested);
        next_rootfile
            .packages
            .insert(package.name.clone(), package.version.clone());
    }
    save_rootfile(&next_rootfile)?;
    save_lock_v2(&new_lock)?;

    let message = format!(
        "Updated: {}. Unchanged: {}. Skipped: {}.",
        updated.join(", "),
        unchanged.join(", "),
        skipped.join(", ")
    );
    let _ = events::record_event(
        events::RootEventType::Update,
        events::RootEventStatus::Completed,
        &match pkg {
            Some(pkg) => format!("root update {}", pkg),
            None => "root update".to_string(),
        },
        if targets.len() == 1 {
            Some(targets[0].1.name.to_string())
        } else {
            None
        },
        Some(snapshot_id.clone()),
        None,
        Some(message),
    )?;

    Ok(UpdateReport {
        success: true,
        requested: pkg.map(str::to_string),
        updated,
        unchanged,
        skipped,
        failed: Vec::new(),
        snapshot_id: Some(snapshot_id),
        warnings: Vec::new(),
    })
}

pub fn list(adapter: &impl NixAdapter) -> Result<ListOutput> {
    root_lockfile::init_root_dir()?;
    let rootfile = get_or_create_rootfile()?;
    let packages: Vec<ListPackage> = rootfile
        .packages
        .into_iter()
        .map(|(name, version)| ListPackage { name, version })
        .collect();

    let nix_profile = adapter.list().map_err(|e| anyhow::anyhow!(e))?;

    Ok(ListOutput {
        packages,
        nix_profile,
    })
}

pub fn remove(adapter: &impl NixAdapter, pkg: &str) -> Result<RemoveReport> {
    root_lockfile::init_root_dir()?;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    enforce_policy(policy::PolicyAction::Remove, Some(pkg))?;
    let _guard = MutationGuard::acquire()?;
    let mut lock = get_or_create_lock_v2()?;

    // Create snapshot before mutation
    let snapshot = Snapshot::create_from_v2(&format!("before remove {}", pkg), &lock)?;
    let snapshot_id = snapshot.id.clone();

    adapter.remove(pkg).map_err(|e| anyhow::anyhow!(e))?;

    let mut rootfile = get_or_create_rootfile()?;
    rootfile.packages.remove(pkg);
    save_rootfile(&rootfile)?;

    lock.packages.retain(|p| p.name != pkg);
    lock.updated_at = Some(chrono::Utc::now().to_rfc3339());
    lock.version = root_lockfile::emit_lock_version(&lock.models);
    save_lock_v2(&lock)?;

    let _ = events::record_event(
        events::RootEventType::Remove,
        events::RootEventStatus::Completed,
        &format!("root remove {}", pkg),
        Some(pkg.to_string()),
        Some(snapshot_id.clone()),
        None,
        Some("Package removed successfully".to_string()),
    )?;

    Ok(RemoveReport {
        success: true,
        package: pkg.to_string(),
        snapshot_id,
        rollback_available: true,
    })
}

pub fn history() -> Result<HistoryOutput> {
    history_with_limit(None)
}

pub fn history_with_limit(limit: Option<usize>) -> Result<HistoryOutput> {
    root_lockfile::init_root_dir()?;
    Ok(HistoryOutput {
        snapshots: list_snapshot_summaries()?,
        events: events::read_events_with_limit(limit)?,
    })
}

pub fn rollback_last(adapter: &impl NixAdapter) -> Result<RollbackReport> {
    root_lockfile::init_root_dir()?;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    let _guard = MutationGuard::acquire()?;
    let snaps = list_snapshots()?;
    if snaps.is_empty() {
        return Err(anyhow::anyhow!("No snapshots available for rollback."));
    }

    let last_snap = &snaps[0];
    let current_lock = get_or_create_lock_v2()?;
    let target_lock = last_snap.restored_lock();
    root_lockfile::validate_store_paths(&target_lock).context(format!(
        "Snapshot '{}' lock failed validation before rollback",
        last_snap.id
    ))?;
    root_lockfile::validate_locked_models(&target_lock).context(format!(
        "Snapshot '{}' lock failed models validation before rollback",
        last_snap.id
    ))?;

    // Step 1: Compute rollback plan
    let mut packages_to_remove = Vec::new();
    for curr_pkg in &current_lock.packages {
        let target_pkg = target_lock
            .packages
            .iter()
            .find(|package| package.name == curr_pkg.name);
        if target_pkg
            .map(|target_pkg| locked_package_changed(curr_pkg, target_pkg))
            .unwrap_or(true)
        {
            packages_to_remove.push(curr_pkg.name.clone());
        }
    }

    let mut packages_to_install = Vec::new();
    for target_pkg in &target_lock.packages {
        let current_pkg = current_lock
            .packages
            .iter()
            .find(|package| package.name == target_pkg.name);
        if current_pkg
            .map(|current_pkg| locked_package_changed(current_pkg, target_pkg))
            .unwrap_or(true)
        {
            packages_to_install.push(target_pkg.name.clone());
        }
    }

    // Step 2: Create a pre-rollback snapshot (for safety)
    let pre_rollback_snap = root_snapshot::Snapshot::create_from_v2(
        &format!("before rollback to {}", last_snap.id),
        &current_lock,
    )?;

    // Step 3: Execute Nix profile changes FIRST
    for pkg in &packages_to_remove {
        adapter.remove(pkg).map_err(|e| {
            let _ = events::record_event(
                events::RootEventType::Rollback,
                events::RootEventStatus::Failed,
                "root rollback --last",
                None,
                Some(pre_rollback_snap.id.clone()),
                Some(last_snap.id.clone()),
                Some(format!("Failed to remove package '{}': {}", pkg, e)),
            );
            anyhow::anyhow!("Rollback failed to remove '{}': {}", pkg, e)
        })?;
    }

    for pkg in &packages_to_install {
        let locked_pkg = target_lock
            .packages
            .iter()
            .find(|package| package.name == *pkg)
            .ok_or_else(|| {
                anyhow::anyhow!("Snapshot package '{}' is missing lock metadata", pkg)
            })?;
        let before_gen = adapter.profile_generation().map_err(|e| {
            let _ = events::record_event(
                events::RootEventType::Rollback,
                events::RootEventStatus::Failed,
                "root rollback --last",
                None,
                Some(pre_rollback_snap.id.clone()),
                Some(last_snap.id.clone()),
                Some(format!("Failed to check generation: {}", e)),
            );
            anyhow::anyhow!("Rollback failed to check generation: {}", e)
        })?;
        let install_result = if let Some(installable) = locked_pkg.installable.as_deref() {
            adapter.install_installable(pkg, installable)
        } else {
            adapter.install(pkg)
        };
        install_result.map_err(|e| {
            let _ = events::record_event(
                events::RootEventType::Rollback,
                events::RootEventStatus::Failed,
                "root rollback --last",
                None,
                Some(pre_rollback_snap.id.clone()),
                Some(last_snap.id.clone()),
                Some(format!("Failed to install package '{}': {}", pkg, e)),
            );
            anyhow::anyhow!("Rollback failed to install '{}': {}", pkg, e)
        })?;

        // Verify profile contains the locked store paths from the snapshot
        verify_profile_contains_outputs(adapter, &locked_pkg.store_paths).map_err(|e| {
            let _ = events::record_event(
                events::RootEventType::Rollback,
                events::RootEventStatus::Failed,
                "root rollback --last",
                None,
                Some(pre_rollback_snap.id.clone()),
                Some(last_snap.id.clone()),
                Some(format!("Rollback verification failed for '{}': {}", pkg, e)),
            );
            anyhow::anyhow!("Rollback verification failed for '{}': {}", pkg, e)
        })?;

        let store_path_strs: Vec<&str> = locked_pkg
            .store_paths
            .values()
            .map(|s| s.as_str())
            .collect();
        let spec = resolve_package(pkg);
        let binaries = spec.map(|s| s.binaries).unwrap_or(&[]);
        validate_mutation_result(
            adapter,
            before_gen,
            &[pkg.as_str()],
            binaries,
            &store_path_strs,
        )
        .map_err(|e| {
            let _ = events::record_event(
                events::RootEventType::Rollback,
                events::RootEventStatus::Failed,
                "root rollback --last",
                None,
                Some(pre_rollback_snap.id.clone()),
                Some(last_snap.id.clone()),
                Some(format!("Rollback validation failed for '{}': {}", pkg, e)),
            );
            anyhow::anyhow!("Rollback validation failed for '{}': {}", pkg, e)
        })?;
    }

    // Step 4: ONLY NOW update Rootfile and root.lock (after Nix succeeded)
    let mut rootfile = get_or_create_rootfile()?;
    rootfile.packages.clear();

    for pkg in &target_lock.packages {
        rootfile
            .packages
            .insert(pkg.name.clone(), pkg.version.clone());
    }

    save_rootfile(&rootfile)?;
    save_lock_v2(&target_lock)?;

    // Step 5: Record rollback event
    let _ = events::record_event(
        events::RootEventType::Rollback,
        events::RootEventStatus::Completed,
        "root rollback --last",
        None,
        Some(pre_rollback_snap.id.clone()),
        Some(last_snap.id.clone()),
        Some(format!(
            "Removed: {}. Restored: {}.",
            packages_to_remove.join(", "),
            packages_to_install.join(", ")
        )),
    )?;

    Ok(RollbackReport {
        success: true,
        from_snapshot: last_snap.id.clone(),
        packages_removed: packages_to_remove,
        packages_restored: packages_to_install,
    })
}

pub fn doctor(adapter: &impl NixAdapter) -> Result<root_doctor::DoctorReport> {
    let report = root_doctor::run_diagnostics(adapter)?;
    if get_root_dir().map(|path| path.exists()).unwrap_or(false) {
        let _ = events::record_event(
            events::RootEventType::Doctor,
            events::RootEventStatus::Completed,
            "root doctor",
            None,
            None,
            None,
            if report.healthy {
                Some("System healthy".to_string())
            } else {
                Some(format!("Issues found: {}", report.issues.len()))
            },
        );
    }
    Ok(report)
}

pub fn verify(pkg: &str) -> Result<root_verify::VerificationReport> {
    match root_verify::verify_package(pkg) {
        Ok(report) => {
            let event_type = if report.success {
                events::RootEventType::Verification
            } else {
                events::RootEventType::VerificationFailed
            };
            let status = if report.success {
                events::RootEventStatus::Verified
            } else {
                events::RootEventStatus::Failed
            };
            let message = if report.success {
                Some(format!("Verified package '{}'.", pkg))
            } else {
                Some(format!(
                    "Verification failed for '{}': {}",
                    pkg,
                    report.errors.join("; ")
                ))
            };
            let _ = events::record_event(
                event_type,
                status,
                &format!("root verify {}", pkg),
                Some(pkg.to_string()),
                None,
                None,
                message,
            );
            Ok(report)
        }
        Err(err) => {
            let _ = events::record_event(
                events::RootEventType::VerificationFailed,
                events::RootEventStatus::Failed,
                &format!("root verify {}", pkg),
                Some(pkg.to_string()),
                None,
                None,
                Some(err.to_string()),
            );
            Err(err)
        }
    }
}

pub fn import_brew(dest_dir: &Path) -> Result<brew::BrewImportReport> {
    brew::import_brew(dest_dir)
}

fn parse_nix_profile_packages(list_output: &str) -> Vec<String> {
    list_output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Match mock adapter format: "Index: 0 - nixpkgs#poppler"
            if let Some(idx) = line.find("nixpkgs#") {
                let pkg = line[idx + 8..].trim().to_string();
                if !pkg.is_empty() {
                    return Some(pkg);
                }
            }
            None
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct LockReport {
    pub success: bool,
    pub packages_locked: Vec<String>,
    pub packages_removed: Vec<String>,
    pub snapshot_id: Option<String>,
}

pub fn lock(adapter: &impl NixAdapter) -> Result<LockReport> {
    root_lockfile::init_root_dir()?;
    refuse_unsupported_active_lock()?;
    let _guard = MutationGuard::acquire()?;
    let rootfile = get_or_create_rootfile()?;
    let current_v2_lock = get_or_create_lock_v2()?;

    // Snapshot existing lockfile state before overwriting
    let snapshot_id = {
        let lock_path = get_root_dir()?.join("root.lock");
        if lock_path.exists() {
            let snapshot = Snapshot::create_from_v2("before lock", &current_v2_lock)?;
            Some(snapshot.id)
        } else {
            None
        }
    };

    let mut packages_locked = Vec::new();
    let mut packages_removed = Vec::new();
    let mut v2_packages = Vec::new();
    let mut flake_for_lock = None;

    // Build a deterministic lock from Rootfile intent by resolving Nix metadata.
    for name in rootfile.packages.keys() {
        let (flake, installable) = locked_installable_for(adapter, name)?;
        let resolution = adapter
            .resolve_locked_package(name, Some(&installable))
            .map_err(|e| anyhow::anyhow!(e))?;
        let locked_package =
            deterministic_package_from_resolution(name, name, &installable, &resolution)?;
        flake_for_lock = Some(flake);
        packages_locked.push(name.clone());
        v2_packages.push(locked_package);
    }

    // Detect packages that were in old lock but not in new lock
    for old_pkg in &current_v2_lock.packages {
        if !v2_packages.iter().any(|p| p.name == old_pkg.name) {
            packages_removed.push(old_pkg.name.clone());
        }
    }

    let flake = match flake_for_lock {
        Some(flake) => flake,
        None => adapter
            .flake_metadata("nixpkgs")
            .map_err(|e| anyhow::anyhow!(e))?,
    };
    let new_lock = build_v2_lock(&current_v2_lock, &flake, v2_packages)?;
    root_lockfile::validate_store_paths(&new_lock)
        .context("Newly created lockfile failed validation after lock")?;
    root_lockfile::validate_locked_models(&new_lock)
        .context("Newly created lockfile failed models validation after lock")?;
    save_lock_v2(&new_lock)?;

    Ok(LockReport {
        success: true,
        packages_locked,
        packages_removed,
        snapshot_id,
    })
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub success: bool,
    pub installed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub snapshot_id: String,
}

#[derive(Debug, Serialize)]
pub struct SandboxCreateReport {
    pub success: bool,
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub state: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SandboxRunReport {
    pub success: bool,
    pub sandbox_id: String,
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_attempted: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct SandboxListReport {
    pub success: bool,
    pub sandboxes: Vec<root_sandbox::SandboxInstance>,
}

#[derive(Debug, Serialize)]
pub struct SandboxDestroyReport {
    pub success: bool,
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct RestorePlanReport {
    pub lock_path: String,
    pub will_install: Vec<String>,
    pub will_remove: Vec<String>,
    pub will_keep: Vec<String>,
    pub will_update: Vec<String>,
    pub total_packages: usize,
}

#[derive(Debug, Serialize)]
pub struct DriftIssue {
    pub category: String,
    pub description: String,
    pub suggestion: String,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub success: bool,
    pub healthy: bool,
    pub state: String,
    pub rootfile_packages: usize,
    pub lockfile_packages: usize,
    pub profile_packages: usize,
    pub machine_id: String,
    pub hostname: String,
    pub drift_details: Vec<DriftIssue>,
    pub inventory: inventory::InventoryReport,
}

#[derive(Debug, Serialize)]
pub struct RestoreReport {
    pub success: bool,
    pub lock_path: String,
    pub installed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub snapshot_id: String,
}

#[derive(Debug, Clone)]
struct ProfilePackageEntry {
    package: String,
    store_paths: Vec<String>,
}

#[derive(Debug)]
struct ProfileReconcileReport {
    installed: Vec<String>,
    removed: Vec<String>,
    unchanged: Vec<String>,
    snapshot_id: String,
}

#[derive(Debug, Serialize)]
pub struct CatalogEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub nix_attr: &'static str,
    pub binaries: &'static [&'static str],
    pub aliases: &'static [&'static str],
    pub verify: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CatalogOutput {
    pub packages: Vec<CatalogEntry>,
}

pub fn catalog() -> CatalogOutput {
    CatalogOutput {
        packages: SUPPORTED_PACKAGES
            .iter()
            .map(|p| CatalogEntry {
                name: p.name,
                description: p.description,
                category: p.category,
                nix_attr: p.nix_attr,
                binaries: p.binaries,
                aliases: p.aliases,
                verify: p
                    .verify
                    .iter()
                    .map(|v| format!("{} {}", v.binary, v.args.join(" ")))
                    .collect(),
            })
            .collect(),
    }
}

fn get_or_create_machine_id() -> Result<String> {
    let dir = root_lockfile::get_root_dir()?;
    let path = dir.join("machine.json");
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string());
    let now = chrono::Utc::now().to_rfc3339();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;
        let id = json["machine_id"].as_str().unwrap_or("unknown").to_string();
        let mut json = json;
        json["last_seen"] = serde_json::Value::String(now);
        if json.get("hostname").and_then(|v| v.as_str()) != Some(&hostname) {
            json["hostname"] = serde_json::Value::String(hostname.clone());
        }
        std::fs::write(&path, serde_json::to_string_pretty(&json)?)?;
        Ok(id)
    } else {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("root-{:016x}", seed as u64);
        let json = serde_json::json!({
            "machine_id": id,
            "hostname": hostname,
            "platform": root_lockfile::detect_platform().unwrap_or_else(|_| "unknown".to_string()),
            "first_seen": now,
            "last_seen": now,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json)?)?;
        Ok(id)
    }
}

fn format_supported_packages() -> String {
    let mut lines = Vec::new();
    let mut categories: Vec<(&str, Vec<&PackageSpec>)> = Vec::new();
    for pkg in SUPPORTED_PACKAGES {
        let idx = categories.iter().position(|(c, _)| *c == pkg.category);
        if let Some(idx) = idx {
            categories[idx].1.push(pkg);
        } else {
            categories.push((pkg.category, vec![pkg]));
        }
    }
    for (category, pkgs) in &categories {
        lines.push(format!("  {}:", category));
        for pkg in pkgs {
            lines.push(format!("    {:<12} {}", pkg.name, pkg.description));
        }
    }
    lines.join("\n")
}

fn package_name_from_installable(installable: &str) -> String {
    installable
        .rsplit_once('#')
        .map(|(_, package)| package)
        .unwrap_or(installable)
        .to_string()
}

fn parse_profile_package_entries_from_json(profile_json: &str) -> Vec<ProfilePackageEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(profile_json) else {
        return Vec::new();
    };

    let values: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(entries) => entries.iter().collect(),
        serde_json::Value::Object(map) => {
            if let Some(elements) = map.get("elements").and_then(|value| value.as_array()) {
                elements.iter().collect()
            } else {
                map.values().collect()
            }
        }
        _ => Vec::new(),
    };

    values
        .into_iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            let installable = object
                .get("installable")
                .or_else(|| object.get("originalUrl"))
                .or_else(|| object.get("original_url"))
                .and_then(|value| value.as_str());
            let attr_path = object
                .get("attrPath")
                .or_else(|| object.get("attr_path"))
                .and_then(|value| value.as_str());
            let package = attr_path
                .filter(|value| !value.is_empty())
                .map(|value| value.rsplit('.').next().unwrap_or(value).to_string())
                .or_else(|| installable.map(package_name_from_installable))?;
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
            Some(ProfilePackageEntry {
                package,
                store_paths,
            })
        })
        .collect()
}

fn profile_packages_or_empty(adapter: &impl NixAdapter) -> Result<Vec<ProfilePackageEntry>> {
    if !adapter.profile_exists() {
        return Ok(Vec::new());
    }
    profile_packages(adapter)
}

fn profile_packages(adapter: &impl NixAdapter) -> Result<Vec<ProfilePackageEntry>> {
    let json_entries = adapter
        .profile_list_json()
        .map(|json| parse_profile_package_entries_from_json(&json))
        .map_err(|e| anyhow::anyhow!(e))?;
    if !json_entries.is_empty() {
        return Ok(json_entries);
    }

    let nix_list = adapter.list().map_err(|e| anyhow::anyhow!(e))?;
    Ok(parse_nix_profile_packages(&nix_list)
        .iter()
        .map(|package| ProfilePackageEntry {
            package: package.clone(),
            store_paths: Vec::new(),
        })
        .collect())
}

fn locked_package_installed(
    profile_entries: &[ProfilePackageEntry],
    locked_package: &LockedPackageV2,
) -> bool {
    profile_entries.iter().any(|entry| {
        if entry.package != locked_package.name {
            return false;
        }
        if locked_package.store_paths.is_empty() {
            return true;
        }
        locked_package
            .store_paths
            .values()
            .all(|store_path| entry.store_paths.iter().any(|path| path == store_path))
    })
}

fn write_rootfile_from_v2_lock(lock: &RootLockV2) -> Result<()> {
    let mut rootfile = get_or_create_rootfile()?;
    rootfile.packages.clear();
    for package in &lock.packages {
        rootfile
            .packages
            .insert(package.name.clone(), package.version.clone());
    }
    save_rootfile(&rootfile)
}

fn reconcile_profile_to_lock(
    adapter: &impl NixAdapter,
    target_lock: &RootLockV2,
    snapshot_reason: &str,
    command: &str,
    event_type: events::RootEventType,
) -> Result<ProfileReconcileReport> {
    let current_lock = get_or_create_lock_v2()?;
    let snapshot = Snapshot::create_from_v2(snapshot_reason, &current_lock)?;
    let snapshot_id = snapshot.id.clone();

    let profile_entries = profile_packages_or_empty(adapter)?;
    let locked_names: std::collections::BTreeSet<&str> = target_lock
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut installed = Vec::new();
    let mut unchanged = Vec::new();

    for package in &target_lock.packages {
        if locked_package_installed(&profile_entries, package) {
            unchanged.push(package.name.clone());
            continue;
        }

        let before_gen = adapter.profile_generation().map_err(|e| {
            let _ = events::record_event(
                event_type.clone(),
                events::RootEventStatus::Failed,
                command,
                Some(package.name.clone()),
                Some(snapshot_id.clone()),
                None,
                Some(format!(
                    "Failed to check generation before installing '{}': {}",
                    package.name, e
                )),
            );
            anyhow::anyhow!(
                "{} failed to check generation for '{}': {}",
                command,
                package.name,
                e
            )
        })?;

        let install_result = if let Some(installable) = package.installable.as_deref() {
            adapter.install_installable(&package.name, installable)
        } else {
            adapter.install(&package.name)
        };
        install_result.map_err(|e| {
            let _ = events::record_event(
                event_type.clone(),
                events::RootEventStatus::Failed,
                command,
                Some(package.name.clone()),
                Some(snapshot_id.clone()),
                None,
                Some(format!(
                    "Failed to install package '{}': {}",
                    package.name, e
                )),
            );
            anyhow::anyhow!("{} failed to install '{}': {}", command, package.name, e)
        })?;

        verify_profile_contains_outputs(adapter, &package.store_paths).map_err(|e| {
            let _ = events::record_event(
                event_type.clone(),
                events::RootEventStatus::Failed,
                command,
                Some(package.name.clone()),
                Some(snapshot_id.clone()),
                None,
                Some(format!(
                    "Profile verification failed for '{}': {}",
                    package.name, e
                )),
            );
            anyhow::anyhow!(
                "{} verification failed for '{}': {}",
                command,
                package.name,
                e
            )
        })?;

        let store_path_strs: Vec<&str> = package.store_paths.values().map(|s| s.as_str()).collect();
        let spec = resolve_package(&package.name);
        let binaries = spec.map(|s| s.binaries).unwrap_or(&[]);
        validate_mutation_result(
            adapter,
            before_gen,
            &[package.name.as_str()],
            binaries,
            &store_path_strs,
        )
        .map_err(|e| {
            let _ = events::record_event(
                event_type.clone(),
                events::RootEventStatus::Failed,
                command,
                Some(package.name.clone()),
                Some(snapshot_id.clone()),
                None,
                Some(format!(
                    "Profile mutation validation failed for '{}': {}",
                    package.name, e
                )),
            );
            anyhow::anyhow!(
                "{} mutation validation failed for '{}': {}",
                command,
                package.name,
                e
            )
        })?;

        installed.push(package.name.clone());
    }

    let mut removed = Vec::new();
    for entry in &profile_entries {
        if !locked_names.contains(entry.package.as_str()) {
            adapter.remove(&entry.package).map_err(|e| {
                let _ = events::record_event(
                    event_type.clone(),
                    events::RootEventStatus::Failed,
                    command,
                    Some(entry.package.clone()),
                    Some(snapshot_id.clone()),
                    None,
                    Some(format!(
                        "Failed to remove package '{}': {}",
                        entry.package, e
                    )),
                );
                anyhow::anyhow!("{} failed to remove '{}': {}", command, entry.package, e)
            })?;
            removed.push(entry.package.clone());
        }
    }

    save_lock_v2(target_lock)?;
    write_rootfile_from_v2_lock(target_lock)?;

    let _ = events::record_event(
        event_type,
        events::RootEventStatus::Completed,
        command,
        None,
        Some(snapshot_id.clone()),
        None,
        Some(format!(
            "Installed: {}. Removed: {}. Unchanged: {}.",
            installed.join(", "),
            removed.join(", "),
            unchanged.join(", ")
        )),
    )?;

    Ok(ProfileReconcileReport {
        installed,
        removed,
        unchanged,
        snapshot_id,
    })
}

pub fn sync(adapter: &impl NixAdapter) -> Result<SyncReport> {
    root_lockfile::init_root_dir()?;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    enforce_policy(policy::PolicyAction::Sync, None)?;
    let policy_lock = get_or_create_lock_v2()?;
    for package in &policy_lock.packages {
        enforce_policy(policy::PolicyAction::Sync, Some(&package.name))?;
    }
    let _guard = MutationGuard::acquire()?;

    let root_dir = get_root_dir()?;
    let lock_path = root_dir.join("root.lock");
    if lock_path.exists() {
        if let Ok(v2_lock) = RootLockV2::read_from_file(&lock_path) {
            if v2_lock.version < ROOT_LOCK_SCHEMA_VERSION {
                return sync_legacy_lock(adapter);
            }
        }
    }

    let lock = get_or_create_lock_v2()?;
    root_lockfile::validate_store_paths(&lock).context("Lockfile failed validation before sync")?;
    root_lockfile::validate_locked_models(&lock)
        .context("Lockfile failed models validation before sync")?;
    let report = reconcile_profile_to_lock(
        adapter,
        &lock,
        "before sync",
        "root sync",
        events::RootEventType::Update,
    )?;

    Ok(SyncReport {
        success: true,
        installed: report.installed,
        removed: report.removed,
        unchanged: report.unchanged,
        snapshot_id: report.snapshot_id,
    })
}

fn sync_legacy_lock(adapter: &impl NixAdapter) -> Result<SyncReport> {
    let lock = get_or_create_lock()?;
    let nix_list = adapter.list().map_err(|e| anyhow::anyhow!(e))?;
    let profile_pkgs = parse_nix_profile_packages(&nix_list);

    let locked_names: Vec<String> = lock.packages.iter().map(|p| p.name.clone()).collect();

    let to_install: Vec<String> = locked_names
        .iter()
        .filter(|p| !profile_pkgs.contains(p))
        .cloned()
        .collect();
    let to_remove: Vec<String> = profile_pkgs
        .iter()
        .filter(|p| !locked_names.contains(p))
        .cloned()
        .collect();
    let unchanged: Vec<String> = locked_names
        .iter()
        .filter(|p| profile_pkgs.contains(p))
        .cloned()
        .collect();

    let snapshot = Snapshot::create("before sync", &lock)?;
    let snapshot_id = snapshot.id.clone();

    for pkg in &to_install {
        adapter.install(pkg).map_err(|e| anyhow::anyhow!(e))?;
    }
    for pkg in &to_remove {
        adapter.remove(pkg).map_err(|e| anyhow::anyhow!(e))?;
    }

    let mut rootfile = get_or_create_rootfile()?;
    rootfile.packages.clear();
    for pkg in &lock.packages {
        rootfile
            .packages
            .insert(pkg.name.clone(), pkg.version.clone());
    }
    save_rootfile(&rootfile)?;

    let _ = events::record_event(
        events::RootEventType::Update,
        events::RootEventStatus::Completed,
        "root sync",
        None,
        Some(snapshot_id.clone()),
        None,
        Some(format!(
            "Installed: {}. Removed: {}. Unchanged: {}.",
            to_install.join(", "),
            to_remove.join(", "),
            unchanged.join(", ")
        )),
    )?;

    Ok(SyncReport {
        success: true,
        installed: to_install,
        removed: to_remove,
        unchanged,
        snapshot_id,
    })
}

fn infer_restore_failure_phase(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string();
    if msg.contains("failed to check generation") {
        "pre-install check"
    } else if msg.contains("failed to install") {
        "package installation"
    } else if msg.contains("verification failed") {
        "profile verification"
    } else if msg.contains("mutation validation failed") {
        "mutation validation"
    } else if msg.contains("failed to remove") {
        "package removal"
    } else if msg.contains("failed validation before restore")
        || msg.contains("Restore validation failed")
    {
        "pre-restore validation"
    } else if msg.contains("Policy denied") {
        "policy check"
    } else if msg.contains("mutation lock")
        || msg.contains("root.lockfile")
        || msg.contains("Another Root mutation")
    {
        "lock acquisition"
    } else {
        "unknown phase"
    }
}

fn attempt_rollback_to_snapshot(adapter: &impl NixAdapter, snapshot: &Snapshot) -> Result<()> {
    let restored = snapshot.restored_lock();
    root_lockfile::validate_store_paths(&restored)
        .context("Rollback validation failed: snapshot lockfile is invalid")?;
    root_lockfile::validate_locked_models(&restored)
        .context("Rollback validation failed: snapshot lockfile models are invalid")?;

    let profile_entries = profile_packages_or_empty(adapter)?;
    let locked_names: std::collections::BTreeSet<&str> = restored
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    for entry in &profile_entries {
        if !locked_names.contains(entry.package.as_str()) {
            adapter.remove(&entry.package).map_err(|e| {
                anyhow::anyhow!("Rollback failed to remove '{}': {}", entry.package, e)
            })?;
        }
    }

    for package in &restored.packages {
        if !locked_package_installed(&profile_entries, package) {
            if let Some(installable) = package.installable.as_deref() {
                adapter
                    .install_installable(&package.name, installable)
                    .map_err(|e| {
                        anyhow::anyhow!("Rollback failed to install '{}': {}", package.name, e)
                    })?;
            } else {
                adapter.install(&package.name).map_err(|e| {
                    anyhow::anyhow!("Rollback failed to install '{}': {}", package.name, e)
                })?;
            }
        }
    }

    save_lock_v2(&restored).context("Rollback failed to restore root.lock")?;
    write_rootfile_from_v2_lock(&restored).context("Rollback failed to restore Rootfile")?;

    Ok(())
}

fn restore_validate(
    adapter: &impl NixAdapter,
    target_lock: &RootLockV2,
    lock_path: &Path,
) -> Result<()> {
    root_lockfile::validate_store_paths(target_lock).map_err(|e| {
        anyhow::anyhow!(
            "Restore validation failed: lockfile at {} contains invalid store paths. {}",
            lock_path.display(),
            e
        )
    })?;
    root_lockfile::validate_locked_models(target_lock).map_err(|e| {
        anyhow::anyhow!(
            "Restore validation failed: lockfile at {} contains invalid models. {}",
            lock_path.display(),
            e
        )
    })?;

    let nix_ok = adapter
        .check_availability()
        .map_err(|_| anyhow::anyhow!("Restore validation failed: Nix is not available"))?;
    if !nix_ok {
        return Err(anyhow::anyhow!(
            "Restore validation failed: Nix is not available.\n\
             Install Nix from https://nixos.org/download.html and try again."
        ));
    }

    let features = adapter.probe_experimental_features().map_err(|_| {
        anyhow::anyhow!("Restore validation failed: could not probe Nix experimental features")
    })?;
    match features {
        root_nix::ExperimentalFeatureStatus::AllAvailable => {}
        root_nix::ExperimentalFeatureStatus::NixNotAvailable => {
            return Err(anyhow::anyhow!(
                "Restore validation failed: Nix is not available."
            ));
        }
        root_nix::ExperimentalFeatureStatus::NixCommandMissing => {
            return Err(anyhow::anyhow!(
                "Restore validation failed: Nix experimental feature 'nix-command' is not enabled.\n\
                 Add '--extra-experimental-features nix-command' to your nix.conf or use Nix 2.13+."
            ));
        }
        root_nix::ExperimentalFeatureStatus::FlakesMissing => {
            return Err(anyhow::anyhow!(
                "Restore validation failed: Nix experimental feature 'flakes' is not enabled.\n\
                 Add '--extra-experimental-features flakes' to your nix.conf or use Nix 2.13+."
            ));
        }
        root_nix::ExperimentalFeatureStatus::BothMissing => {
            return Err(anyhow::anyhow!(
                "Restore validation failed: Nix experimental features 'nix-command' and 'flakes' are not enabled.\n\
                 Add '--extra-experimental-features nix-command flakes' to your nix.conf or use Nix 2.13+."
            ));
        }
        root_nix::ExperimentalFeatureStatus::NixpkgsResolutionFailed => {
            return Err(anyhow::anyhow!(
                "Restore validation failed: could not resolve nixpkgs. Check network connectivity."
            ));
        }
    }

    let platform = root_lockfile::detect_platform().unwrap_or_default();
    if !target_lock.platform.is_empty() && target_lock.platform != platform {
        return Err(anyhow::anyhow!(
            "Restore validation failed: lockfile platform '{}' does not match current platform '{}'.\n\
             The lockfile was created on a different platform and may not be compatible.",
            target_lock.platform,
            platform
        ));
    }

    for package in &target_lock.packages {
        if package.store_path.ends_with(".drv") {
            return Err(anyhow::anyhow!(
                "Restore validation failed: package '{}' has a .drv path as its store path.\n\
                 This lockfile references a derivation instead of a realized output.",
                package.name
            ));
        }
        for path in package.store_paths.values() {
            if path.ends_with(".drv") {
                return Err(anyhow::anyhow!(
                    "Restore validation failed: package '{}' has a .drv path in its outputs.\n\
                     This lockfile references a derivation instead of a realized output.",
                    package.name
                ));
            }
        }
        for (output_name, output) in &package.outputs {
            if output.store_path.ends_with(".drv") {
                return Err(anyhow::anyhow!(
                    "Restore validation failed: package '{}' has a .drv path in output '{}'.\n\
                     This lockfile references a derivation instead of a realized output.",
                    package.name,
                    output_name
                ));
            }
        }
    }

    Ok(())
}

pub fn restore(adapter: &impl NixAdapter, lock_path: Option<&Path>) -> Result<RestoreReport> {
    root_lockfile::init_root_dir()?;
    let selected_lock_path = match lock_path {
        Some(path) => path.to_path_buf(),
        None => get_root_dir()?.join("root.lock"),
    };
    refuse_unsupported_lock_at(&selected_lock_path)?;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    let target_lock = root_lockfile::read_compatible_lock_v2(&selected_lock_path)?;
    if let Err(e) = restore_validate(adapter, &target_lock, &selected_lock_path) {
        let failure_phase = infer_restore_failure_phase(&e);
        let _ = events::record_event(
            events::RootEventType::Restore,
            events::RootEventStatus::Failed,
            "root restore",
            None,
            None,
            None,
            Some(format!(
                "Restore failed during {}. Error: {}",
                failure_phase, e
            )),
        );
        return Err(anyhow::anyhow!(
            "{}\nLockfile failed validation before restore",
            e
        ));
    }
    enforce_policy(policy::PolicyAction::Restore, None)?;
    for package in &target_lock.packages {
        enforce_policy(policy::PolicyAction::Restore, Some(&package.name))?;
    }
    let _guard = MutationGuard::acquire()?;

    let pre_restore_lock = get_or_create_lock_v2().ok();
    let pre_restore_snapshot = pre_restore_lock.as_ref().and_then(|lock| {
        Snapshot::create_from_v2(
            &format!(
                "before restore from {}",
                selected_lock_path.to_string_lossy()
            ),
            lock,
        )
        .ok()
    });

    match reconcile_profile_to_lock(
        adapter,
        &target_lock,
        &format!(
            "before restore from {}",
            selected_lock_path.to_string_lossy()
        ),
        "root restore",
        events::RootEventType::Restore,
    ) {
        Ok(report) => Ok(RestoreReport {
            success: true,
            lock_path: selected_lock_path.to_string_lossy().to_string(),
            installed: report.installed,
            removed: report.removed,
            unchanged: report.unchanged,
            snapshot_id: report.snapshot_id,
        }),
        Err(e) => {
            let failure_phase = infer_restore_failure_phase(&e);
            let _ = events::record_event(
                events::RootEventType::Restore,
                events::RootEventStatus::Failed,
                "root restore",
                None,
                pre_restore_snapshot.as_ref().map(|s| s.id.clone()),
                None,
                Some(format!(
                    "Restore failed during {}. Error: {}",
                    failure_phase, e
                )),
            );

            if let Some(ref snapshot) = pre_restore_snapshot {
                match attempt_rollback_to_snapshot(adapter, snapshot) {
                    Ok(()) => {
                        let _ = events::record_event(
                            events::RootEventType::RestoreRecovered,
                            events::RootEventStatus::Completed,
                            "root restore",
                            None,
                            Some(snapshot.id.clone()),
                            None,
                            Some("Automatic rollback to pre-restore state succeeded.".to_string()),
                        );
                        Err(anyhow::anyhow!(
                            "Restore failed during {}.\n\n\
                             Error: {}\n\n\
                             Root restored Rootfile, root.lock, and the Nix profile to the pre-restore snapshot.\n\n\
                             Next step:\n  Run `root status` to verify the system state.",
                            failure_phase, e
                        ))
                    }
                    Err(recovery_err) => {
                        let _ = events::record_event(
                            events::RootEventType::RestoreRecovered,
                            events::RootEventStatus::Failed,
                            "root restore",
                            None,
                            Some(snapshot.id.clone()),
                            None,
                            Some(format!("Automatic rollback failed: {}", recovery_err)),
                        );
                        Err(anyhow::anyhow!(
                            "Restore failed during {}.\n\n\
                             Error: {}\n\n\
                             Automatic rollback of the Nix profile, Rootfile, and root.lock also failed: {}\n\n\
                             Next steps:\n  Run `root status` to assess the state.\n  Run `root rollback --last` to attempt manual rollback.",
                            failure_phase, e, recovery_err
                        ))
                    }
                }
            } else {
                Err(anyhow::anyhow!(
                    "Restore failed during {}.\n\n\
                     Error: {}\n\n\
                     Your previous Rootfile and root.lock were preserved.\n\n\
                     Next step:\n  Run `root status` to verify the system state.\n  Run `root doctor` for diagnostics.",
                    failure_phase, e
                ))
            }
        }
    }
}

pub fn restore_dry_run(
    adapter: &impl NixAdapter,
    lock_path: Option<&Path>,
) -> Result<RestorePlanReport> {
    root_lockfile::init_root_dir()?;
    let selected_lock_path = match lock_path {
        Some(path) => path.to_path_buf(),
        None => get_root_dir()?.join("root.lock"),
    };
    refuse_unsupported_lock_at(&selected_lock_path)?;
    refuse_unsupported_active_lock()?;
    get_or_create_rootfile()?;
    let target_lock = root_lockfile::read_compatible_lock_v2(&selected_lock_path)?;

    restore_validate(adapter, &target_lock, &selected_lock_path)?;

    let profile_entries = profile_packages_or_empty(adapter)?;
    let locked_names: std::collections::BTreeSet<&str> = target_lock
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    let mut will_install = Vec::new();
    let mut will_keep = Vec::new();
    let mut will_update = Vec::new();
    let mut will_remove = Vec::new();

    for package in &target_lock.packages {
        if locked_package_installed(&profile_entries, package) {
            will_keep.push(package.name.clone());
            continue;
        }
        let current_matches = profile_entries.iter().any(|e| e.package == package.name);
        if current_matches {
            will_update.push(package.name.clone());
        } else {
            will_install.push(package.name.clone());
        }
    }

    for entry in &profile_entries {
        if !locked_names.contains(entry.package.as_str()) {
            will_remove.push(entry.package.clone());
        }
    }

    let _ = events::record_event(
        events::RootEventType::RestorePlanned,
        events::RootEventStatus::Planned,
        "root restore --dry-run",
        None,
        None,
        None,
        Some(format!(
            "Dry-run plan: install {}, remove {}, keep {}, update {}",
            will_install.len(),
            will_remove.len(),
            will_keep.len(),
            will_update.len()
        )),
    );

    Ok(RestorePlanReport {
        lock_path: selected_lock_path.to_string_lossy().to_string(),
        will_install,
        will_remove,
        will_keep,
        will_update,
        total_packages: target_lock.packages.len(),
    })
}

pub fn sandbox_create(
    provider: &impl SandboxProvider,
    name: Option<&str>,
    image: Option<&str>,
    memory: Option<&str>,
    cpus: Option<&str>,
) -> Result<SandboxCreateReport> {
    root_lockfile::init_root_dir()?;
    let sandbox_name = name.unwrap_or("default");
    enforce_policy(policy::PolicyAction::SandboxCreate, Some(sandbox_name))?;

    let available = provider.check_availability()?;
    if !available {
        return Err(anyhow::anyhow!(
            "No sandbox provider is available.\n\n\
             Root requires Docker to create sandboxes.\n\
             Install Docker Desktop from https://docker.com\n\
             Then verify with: docker info"
        ));
    }

    let instance = provider
        .create(sandbox_name, image, memory, cpus)
        .map_err(|e| anyhow::anyhow!("Sandbox create failed: {}", e))?;

    // Post-create validation
    let exists = provider.check_exists(&instance.id).unwrap_or(false);
    let reachable = provider.check_reachable(&instance.id).unwrap_or(false);

    if !exists || !reachable {
        let _ = provider.destroy(&instance.id);
        return Err(anyhow::anyhow!(
            "Sandbox '{}' was created but post-create validation failed \
             (exists: {}, reachable: {}). The sandbox has been destroyed.",
            sandbox_name,
            exists,
            reachable
        ));
    }

    let _ = events::record_event(
        events::RootEventType::Sandbox,
        events::RootEventStatus::Completed,
        &format!("root sandbox create {}", sandbox_name),
        None,
        None,
        None,
        Some(format!(
            "Created sandbox '{}' (id: {})",
            instance.name, instance.id
        )),
    );

    Ok(SandboxCreateReport {
        success: true,
        id: instance.id,
        name: instance.name,
        image: instance.image,
        status: format!("{:?}", instance.state),
        state: format!("{:?}", instance.state),
        created_at: instance.created_at,
        memory: instance.memory,
        cpus: instance.cpus,
    })
}

pub fn sandbox_run(
    provider: &impl SandboxProvider,
    id: &str,
    command: &[String],
    timeout: Option<u64>,
) -> Result<SandboxRunReport> {
    root_lockfile::init_root_dir()?;
    enforce_policy(policy::PolicyAction::SandboxRun, Some(id))?;

    let started_at = events::now_iso_for_event();
    let start_instant = std::time::Instant::now();

    let cmd_str: Vec<&str> = command.iter().map(String::as_str).collect();
    let result = provider
        .run_command(id, &cmd_str, timeout)
        .map_err(|e| anyhow::anyhow!("Sandbox exec failed: {}", e))?;

    let duration_ms = start_instant.elapsed().as_millis() as u64;
    let finished_at = events::now_iso_for_event();
    let is_timeout = result.exit_code == 124;

    let (status, message) = if is_timeout {
        (
            events::RootEventStatus::Timeout,
            format!(
                "Command timed out in sandbox '{}' after {}ms",
                id, duration_ms
            ),
        )
    } else if result.exit_code == 0 {
        (
            events::RootEventStatus::Completed,
            format!(
                "Executed in sandbox '{}': exit code {}",
                id, result.exit_code
            ),
        )
    } else {
        (
            events::RootEventStatus::Failed,
            format!(
                "Executed in sandbox '{}': exit code {}",
                id, result.exit_code
            ),
        )
    };

    let mut cleanup_attempted = false;
    if result.exit_code != 0 || is_timeout {
        let _ = provider.destroy(id);
        cleanup_attempted = true;
        let cleanup_msg = if is_timeout {
            format!("Cleanup attempted after timeout for sandbox '{}'", id)
        } else {
            format!("Cleanup attempted after failed run for sandbox '{}'", id)
        };
        let _ = events::record_event(
            events::RootEventType::Sandbox,
            events::RootEventStatus::Completed,
            &format!("root sandbox cleanup {}", id),
            None,
            None,
            None,
            Some(cleanup_msg),
        );
    }

    let mut event = events::create_event(
        events::RootEventType::Sandbox,
        status,
        &format!("root sandbox run {}", id),
        None,
        None,
        None,
        Some(message),
    );
    event.sandbox_id = Some(id.to_string());
    event.exit_code = Some(result.exit_code);
    event.started_at = Some(started_at);
    event.finished_at = Some(finished_at);
    event.duration_ms = Some(duration_ms);
    let _ = events::append_event(&event);

    Ok(SandboxRunReport {
        success: result.exit_code == 0,
        sandbox_id: id.to_string(),
        command: command.join(" "),
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
        duration_ms: Some(duration_ms),
        timed_out: Some(is_timeout),
        cleanup_attempted: Some(cleanup_attempted),
    })
}

pub fn sandbox_list(provider: &impl SandboxProvider) -> Result<SandboxListReport> {
    root_lockfile::init_root_dir()?;
    let sandboxes = provider
        .list()
        .map_err(|e| anyhow::anyhow!("Sandbox list failed: {}", e))?;
    Ok(SandboxListReport {
        success: true,
        sandboxes,
    })
}

pub fn sandbox_destroy(provider: &impl SandboxProvider, id: &str) -> Result<SandboxDestroyReport> {
    root_lockfile::init_root_dir()?;
    enforce_policy(policy::PolicyAction::SandboxDestroy, Some(id))?;

    let result = provider.destroy(id);

    // Post-destroy check: verify the container is actually gone
    let still_exists = provider.check_exists(id).unwrap_or(false);
    let warning = if still_exists {
        let msg = format!("Sandbox '{}' may still exist after destroy attempt", id);
        let _ = events::record_event(
            events::RootEventType::Sandbox,
            events::RootEventStatus::Failed,
            &format!("root sandbox destroy {}", id),
            None,
            None,
            None,
            Some(msg.clone()),
        );
        Some(msg)
    } else {
        None
    };

    match result {
        Ok(()) => {
            let _ = events::record_event(
                events::RootEventType::Sandbox,
                events::RootEventStatus::Completed,
                &format!("root sandbox destroy {}", id),
                None,
                None,
                None,
                Some(format!("Destroyed sandbox '{}'", id)),
            );
            Ok(SandboxDestroyReport {
                success: true,
                id: id.to_string(),
            })
        }
        Err(e) => {
            let msg = if let Some(ref w) = warning {
                format!("{}. {}", e, w)
            } else {
                format!("{}", e)
            };
            let _ = events::record_event(
                events::RootEventType::Sandbox,
                events::RootEventStatus::Failed,
                &format!("root sandbox destroy {}", id),
                None,
                None,
                None,
                Some(msg.clone()),
            );
            Err(anyhow::anyhow!("Sandbox destroy failed: {}", msg))
        }
    }
}

pub fn status(adapter: &impl root_nix::NixAdapter) -> Result<StatusReport> {
    status_with_probe(adapter, &inventory::SystemInventoryProbe::default())
}

pub fn status_with_probe(
    adapter: &impl root_nix::NixAdapter,
    probe: &impl inventory::InventoryProbe,
) -> Result<StatusReport> {
    root_lockfile::init_root_dir()?;
    let machine_id = get_or_create_machine_id()?;
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string());

    let rootfile = get_or_create_rootfile()?;
    let lock_path = get_root_dir()?.join("root.lock");
    let mut lock_unsupported = false;
    let mut unsupported_lock_version = None;
    let lock = if lock_path.exists() {
        let content = std::fs::read_to_string(&lock_path)
            .with_context(|| format!("Failed to read lockfile at {}", lock_path.display()))?;
        let version = root_lockfile::peek_lock_schema_version(&content)?;
        if version > ROOT_LOCK_MAX_SUPPORTED_VERSION {
            lock_unsupported = true;
            unsupported_lock_version = Some(version);
            None
        } else {
            Some(get_or_create_lock_v2()?)
        }
    } else {
        Some(get_or_create_lock_v2()?)
    };
    let rootfile_count = rootfile.packages.len();
    let lockfile_count = lock.as_ref().map(|lock| lock.packages.len()).unwrap_or(0);

    let mut drift_details = Vec::new();
    let mut healthy = true;

    if lock_unsupported {
        drift_details.push(DriftIssue {
            category: "incompatible-lock-schema".to_string(),
            description: format!(
                "root.lock schema version {} is newer than this Root supports ({ROOT_LOCK_MAX_SUPPORTED_VERSION}). Upgrade Root to use this lockfile.",
                unsupported_lock_version.unwrap_or(0)
            ),
            suggestion: "Install a newer Root release before running mutating commands against this lockfile.".to_string(),
        });
        healthy = false;
    }

    // Local checks first (no Nix calls). Skip comparison against an unsupported lock.
    if let Some(lock) = lock.as_ref() {
        for pkg_name in rootfile.packages.keys() {
            if !lock.packages.iter().any(|p| p.name == *pkg_name) {
                drift_details.push(DriftIssue {
                    category: "rootfile-lockfile-mismatch".to_string(),
                    description: format!(
                        "Package '{}' is in Rootfile but not in root.lock",
                        pkg_name
                    ),
                    suggestion: "Run `root lock` to regenerate root.lock from Rootfile intent"
                        .to_string(),
                });
                healthy = false;
            }
        }
    }

    // If there's no rootfile or lockfile with packages, don't bother calling Nix
    let (profile_entries, profile_count) =
        if !lock_unsupported && (rootfile_count > 0 || lockfile_count > 0) {
            match profile_packages(adapter) {
                Ok(entries) => {
                    let count = entries.len();
                    (entries, count)
                }
                Err(error) => {
                    drift_details.push(DriftIssue {
                        category: "profile-unavailable".to_string(),
                        description: format!(
                            "Could not inspect the Root-managed Nix profile: {}",
                            error
                        ),
                        suggestion: "Run `root doctor` to diagnose Nix and profile availability"
                            .to_string(),
                    });
                    healthy = false;
                    (Vec::new(), 0)
                }
            }
        } else {
            (Vec::new(), 0)
        };

    // Compare lockfile vs profile (only if profile was checked)
    if let Some(lock) = lock.as_ref() {
        for pkg in &lock.packages {
            match profile_entries.iter().find(|e| e.package == pkg.name) {
                None => {
                    drift_details.push(DriftIssue {
                        category: "lockfile-profile-mismatch".to_string(),
                        description: format!(
                            "Package '{}' is in root.lock but not in Nix profile",
                            pkg.name
                        ),
                        suggestion: "Run `root sync` to install the locked package".to_string(),
                    });
                    healthy = false;
                }
                Some(entry) => {
                    if !pkg.store_paths.is_empty() {
                        let all_paths_present = pkg
                            .store_paths
                            .values()
                            .all(|path| entry.store_paths.iter().any(|ep| ep == path));
                        if !all_paths_present {
                            let profile_json = adapter.profile_list_json().ok().unwrap_or_default();
                            let missing: Vec<&String> = pkg
                                .store_paths
                                .values()
                                .filter(|path| !profile_json.contains(path.as_str()))
                                .collect();
                            if !missing.is_empty() {
                                drift_details.push(DriftIssue {
                                    category: "lockfile-output-missing".to_string(),
                                    description: format!(
                                        "Package '{}' is installed but expected output paths are missing from the profile:\n  {}",
                                        pkg.name,
                                        missing.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n  ")
                                    ),
                                    suggestion: "Run `root sync` to restore missing outputs".to_string(),
                                });
                                healthy = false;
                            }
                        }
                    }
                    if !pkg.store_path.is_empty() && pkg.store_path.ends_with(".drv") {
                        drift_details.push(DriftIssue {
                            category: "lockfile-has-drv-path".to_string(),
                            description: format!(
                                "Package '{}' has a .drv path ({}) where an output path is expected",
                                pkg.name, pkg.store_path
                            ),
                            suggestion: "Regenerate root.lock with `root lock`".to_string(),
                        });
                        healthy = false;
                    }
                }
            }
        }

        for entry in &profile_entries {
            if !lock.packages.iter().any(|p| p.name == entry.package) {
                drift_details.push(DriftIssue {
                    category: "profile-lockfile-mismatch".to_string(),
                    description: format!(
                        "Package '{}' is in Nix profile but not in root.lock",
                        entry.package
                    ),
                    suggestion: "Run `root sync` to remove the extra package".to_string(),
                });
                healthy = false;
            }
        }

        if !lock.platform.is_empty() {
            let current_platform = root_lockfile::detect_platform().unwrap_or_default();
            if lock.platform != current_platform {
                drift_details.push(DriftIssue {
                    category: "platform-mismatch".to_string(),
                    description: format!(
                        "root.lock was created on platform '{}' but current platform is '{}'",
                        lock.platform, current_platform
                    ),
                    suggestion: "Regenerate root.lock on the current platform".to_string(),
                });
                healthy = false;
            }
        }
    }

    let package_state = if healthy {
        "Healthy".to_string()
    } else if drift_details.iter().any(|d| {
        d.category == "lockfile-profile-mismatch"
            || d.category == "profile-unavailable"
            || d.category == "lockfile-output-missing"
            || d.category == "platform-mismatch"
            || d.category == "incompatible-lock-schema"
    }) {
        "NeedsAttention".to_string()
    } else {
        "Drifted".to_string()
    };

    let inventory_report = if rootfile.agents.is_empty() && rootfile.models.is_empty() {
        inventory::InventoryReport::default()
    } else {
        inventory::inspect_declarations(&rootfile, probe)
    };

    for item in inventory_report
        .agents
        .iter()
        .chain(inventory_report.models.iter())
    {
        if let Some(category) = inventory::drift_category_for(item) {
            drift_details.push(DriftIssue {
                category: category.to_string(),
                description: format!(
                    "{} '{}' is {} ({})",
                    match item.kind {
                        inventory::ResourceKind::Agent => "Agent",
                        inventory::ResourceKind::Model => "Model",
                    },
                    item.name,
                    match item.evaluation {
                        inventory::EvaluationState::Missing => "missing",
                        inventory::EvaluationState::Unknown => "unknown",
                        inventory::EvaluationState::Unsupported => "unsupported",
                        inventory::EvaluationState::Drifted => "drifted",
                        inventory::EvaluationState::Satisfied => "satisfied",
                    },
                    item.reason
                        .as_deref()
                        .map(inventory::reason_phrase)
                        .unwrap_or("declared environment issue")
                ),
                suggestion: inventory::drift_suggestion_for(category).to_string(),
            });
        }
    }

    let (healthy, state) =
        inventory::combine_environment_state(&package_state, inventory_report.evaluations());

    Ok(StatusReport {
        success: true,
        healthy,
        state,
        rootfile_packages: rootfile_count,
        lockfile_packages: lockfile_count,
        profile_packages: profile_count,
        machine_id,
        hostname,
        drift_details,
        inventory: inventory_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_nix::MockNixAdapter;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-global env vars (ROOT_DIR).
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn test_tmp_dir(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("root_test_{}_{}_{}", name, std::process::id(), n))
    }

    fn write_fake_binary(root_dir: &std::path::Path, name: &str, body: &str) {
        let bin_dir = root_dir.join("profiles").join("default").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
    }

    #[test]
    fn test_snapshots_and_rollback() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("snapshots");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Manually set up an initial package (bypasses allowlist)
        adapter.install("test-pkg-1").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "test-pkg-1".into(),
            requested: "test-pkg-1".into(),
            version: "latest".into(),
            attribute: "test-pkg-1".into(),
            store_path: root_lockfile::derive_store_path("test-pkg-1", "latest"),
            binaries: vec!["test-pkg-1".into()],
        });
        save_lock(&lock).unwrap();
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile
            .packages
            .insert("test-pkg-1".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        // Use core::install for the only supported package
        install(&adapter, "ffmpeg").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let deterministic_lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let ffmpeg = deterministic_lock
            .packages
            .iter()
            .find(|package| package.name == "ffmpeg")
            .unwrap();
        assert_eq!(deterministic_lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert_ne!(ffmpeg.version, "latest");
        assert!(ffmpeg.installable.as_deref().unwrap().contains("#ffmpeg"));
        assert!(ffmpeg.drv_path.as_deref().unwrap().ends_with(".drv"));
        assert!(ffmpeg.store_path.starts_with("/nix/store/"));
        assert!(!ffmpeg.has_placeholder_store_path());

        let hist = history().unwrap();
        assert!(hist
            .snapshots
            .iter()
            .any(|snapshot| snapshot.reason == "before install ffmpeg"));
        assert!(hist
            .events
            .iter()
            .any(|e| e.package.as_deref() == Some("ffmpeg")));

        let res = rollback_last(&adapter).unwrap();
        assert!(res.success);

        let rf = get_or_create_rootfile().unwrap();
        // Rollback reverts the ffmpeg install, leaving test-pkg-1
        assert!(rf.packages.contains_key("test-pkg-1"));
        assert!(!rf.packages.contains_key("ffmpeg"));

        let restored_lock = RootLockV2::read_from_file(&lock_path).unwrap();
        assert_eq!(restored_lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert!(restored_lock
            .packages
            .iter()
            .any(|package| package.name == "test-pkg-1"));
        assert!(!restored_lock
            .packages
            .iter()
            .any(|package| package.name == "ffmpeg"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_preserves_v2_lock_and_records_snapshot() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("remove_v2");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ffmpeg").unwrap();
        let remove_report = remove(&adapter, "ffmpeg").unwrap();
        assert!(remove_report.success);

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        assert_eq!(lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert!(lock.packages.is_empty());

        let hist = history().unwrap();
        assert!(hist
            .snapshots
            .iter()
            .any(|snapshot| snapshot.id == remove_report.snapshot_id));
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Remove
                && event.package.as_deref() == Some("ffmpeg")
        }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_poppler_install_writes_binary_metadata() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("poppler_binaries");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "poppler").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let poppler = lock
            .packages
            .iter()
            .find(|package| package.name == "poppler")
            .unwrap();
        assert_eq!(
            poppler.binaries,
            vec!["pdftotext".to_string(), "pdfinfo".to_string()]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_verify_records_success_event() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("verify_event");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        std::env::set_var(
            "PATH",
            root_dir.join("profiles").join("default").join("bin"),
        );
        write_fake_binary(
            &root_dir,
            "ffmpeg",
            "#!/bin/sh\necho 'ffmpeg version root'\n",
        );

        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "ffmpeg".into(),
            requested: "ffmpeg".into(),
            version: "7.1".into(),
            attribute: "ffmpeg".into(),
            store_path: root_lockfile::derive_store_path("ffmpeg", "7.1"),
            binaries: vec!["ffmpeg".into()],
        });
        save_lock(&lock).unwrap();

        let report = verify("ffmpeg").unwrap();
        assert!(report.success);

        let hist = history().unwrap();
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Verification
                && event.status == events::RootEventStatus::Verified
                && event.package.as_deref() == Some("ffmpeg")
        }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lock_generates_lockfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Install something via adapter so it's in the profile
        adapter.install("ripgrep").unwrap();

        // Add to Rootfile
        let mut rf = get_or_create_rootfile().unwrap();
        rf.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rf).unwrap();

        let report = lock(&adapter).unwrap();
        assert!(report.success);
        assert!(report.packages_locked.contains(&"ripgrep".to_string()));

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let deterministic_lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let locked_package = deterministic_lock
            .packages
            .iter()
            .find(|p| p.name == "ripgrep")
            .unwrap();
        assert_eq!(deterministic_lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert_ne!(deterministic_lock.nixpkgs.rev, "unknown");
        assert_ne!(locked_package.version, "latest");
        assert!(locked_package.store_path.starts_with("/nix/store/"));
        assert!(!locked_package.has_placeholder_store_path());

        let lockfile = get_or_create_lock().unwrap();
        assert!(lockfile.packages.iter().any(|p| p.name == "ripgrep"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lock_creates_snapshot_before_writing() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock_snapshot");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // First lock on fresh root — no existing lockfile, so no snapshot
        let mut rf = get_or_create_rootfile().unwrap();
        rf.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rf).unwrap();

        let report = lock(&adapter).unwrap();
        assert!(report.success);
        assert!(report.snapshot_id.is_none());

        let before_snaps = list_snapshots().unwrap();
        let before_count = before_snaps.len();

        // Second lock — existing lockfile should be snapshotted before overwrite
        let report2 = lock(&adapter).unwrap();
        assert!(report2.success);
        assert!(
            report2.snapshot_id.is_some(),
            "expected a snapshot when an existing lockfile is present"
        );

        let after_snaps = list_snapshots().unwrap();
        assert_eq!(
            after_snaps.len(),
            before_count + 1,
            "expected exactly one new snapshot"
        );
        assert!(after_snaps.iter().any(|s| s.reason == "before lock"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sync_reconciles_profile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("sync");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Setup: lock says pkg-a and pkg-b, but profile only has pkg-a + pkg-c
        adapter.install("pkg-a").unwrap();
        adapter.install("pkg-c").unwrap();

        let mut lock = RootLock {
            version: 1,
            platform: root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into()),
            nixpkgs: NixpkgsConfig {
                rev: "some-rev".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![],
        };
        lock.packages.push(LockedPackage {
            name: "pkg-a".into(),
            requested: "pkg-a".into(),
            version: "latest".into(),
            attribute: "pkg-a".into(),
            store_path: root_lockfile::derive_store_path("pkg-a", "latest"),
            binaries: vec!["pkg-a".into()],
        });
        lock.packages.push(LockedPackage {
            name: "pkg-b".into(),
            requested: "pkg-b".into(),
            version: "latest".into(),
            attribute: "pkg-b".into(),
            store_path: root_lockfile::derive_store_path("pkg-b", "latest"),
            binaries: vec!["pkg-b".into()],
        });
        lock.write_to_file(&root_lockfile::get_root_dir().unwrap().join("root.lock"))
            .unwrap();

        let report = sync(&adapter).unwrap();
        assert!(report.success);
        assert!(report.installed.contains(&"pkg-b".to_string()));
        assert!(report.removed.contains(&"pkg-c".to_string()));
        assert!(report.unchanged.contains(&"pkg-a".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sync_reconciles_v2_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("sync_v2");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("fd").unwrap();
        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let v2_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        v2_lock.write_to_file(&root_dir.join("root.lock")).unwrap();

        let report = sync(&adapter).unwrap();
        assert!(report.success);
        assert!(report.installed.contains(&"ripgrep".to_string()));
        assert!(report.removed.contains(&"fd".to_string()));
        assert!(report.unchanged.is_empty());

        let rf = get_or_create_rootfile().unwrap();
        assert!(rf.packages.contains_key("ripgrep"));
        assert!(!rf.packages.contains_key("fd"));

        let hist = history().unwrap();
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Update
                && event.command == "root sync"
                && event.snapshot_id.as_deref() == Some(report.snapshot_id.as_str())
        }));

        let snaps = list_snapshots().unwrap();
        let snap = snaps
            .iter()
            .find(|snapshot| snapshot.id == report.snapshot_id)
            .unwrap();
        assert_eq!(snap.lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert!(
            snap.lock.packages.iter().any(|p| p.drv_path.is_some()),
            "sync on v2 must keep v2 snapshot metadata (legacy path would drop drv_path)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_from_shared_v2_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_v2");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("fd").unwrap();
        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let report = restore(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(report.success);
        assert_eq!(
            report.lock_path,
            shared_lock_path.to_string_lossy().to_string()
        );
        assert!(report.installed.contains(&"ripgrep".to_string()));
        assert!(report.removed.contains(&"fd".to_string()));

        let active_lock = RootLockV2::read_from_file(&root_dir.join("root.lock")).unwrap();
        assert!(active_lock.packages.iter().any(|p| p.name == "ripgrep"));
        assert!(!active_lock.packages.iter().any(|p| p.name == "fd"));

        let rf = get_or_create_rootfile().unwrap();
        assert!(rf.packages.contains_key("ripgrep"));
        assert!(!rf.packages.contains_key("fd"));

        let hist = history().unwrap();
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Restore
                && event.command == "root restore"
                && event.snapshot_id.as_deref() == Some(report.snapshot_id.as_str())
        }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rollback_v2_verifies_store_paths() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("rollback_v2_verify");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Set up an initial v1 package (simulates pre-v1.2 state)
        adapter.install("ripgrep").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "ripgrep".into(),
            requested: "ripgrep".into(),
            version: "latest".into(),
            attribute: "ripgrep".into(),
            store_path: root_lockfile::derive_store_path("ripgrep", "latest"),
            binaries: vec!["rg".into()],
        });
        save_lock(&lock).unwrap();
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        // Install v2 package
        install(&adapter, "ffmpeg").unwrap();

        // Rollback should succeed (profile contains expected paths)
        let res = rollback_last(&adapter).unwrap();
        assert!(res.success);
        assert!(res.packages_removed.contains(&"ffmpeg".to_string()));

        // Verify final state: ripgrep present, ffmpeg absent
        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let restored_lock = RootLockV2::read_from_file(&lock_path).unwrap();
        assert!(restored_lock.packages.iter().any(|p| p.name == "ripgrep"));
        assert!(!restored_lock.packages.iter().any(|p| p.name == "ffmpeg"));

        let rf = get_or_create_rootfile().unwrap();
        assert!(rf.packages.contains_key("ripgrep"));
        assert!(!rf.packages.contains_key("ffmpeg"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_on_v1_lock_migrates_to_v2() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_migrate_v1");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Create a v1 lock with "latest" version
        adapter.install("test-pkg-1").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "test-pkg-1".into(),
            requested: "test-pkg-1".into(),
            version: "latest".into(),
            attribute: "test-pkg-1".into(),
            store_path: root_lockfile::derive_store_path("test-pkg-1", "latest"),
            binaries: vec!["test-pkg-1".into()],
        });
        save_lock(&lock).unwrap();

        // Install ffmpeg (v2 deterministically)
        install(&adapter, "ffmpeg").unwrap();

        // Verify lock is now v2 with real metadata
        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let migrated_lock = RootLockV2::read_from_file(&lock_path).unwrap();
        assert_eq!(migrated_lock.version, ROOT_LOCK_SCHEMA_VERSION);

        // Old package preserved (v1 fields carry over; v2-only fields may be empty)
        let old_pkg = migrated_lock
            .packages
            .iter()
            .find(|p| p.name == "test-pkg-1")
            .unwrap();
        assert_eq!(old_pkg.version, "latest");
        assert!(!old_pkg.store_path.is_empty());

        // New package has deterministic metadata
        let new_pkg = migrated_lock
            .packages
            .iter()
            .find(|p| p.name == "ffmpeg")
            .unwrap();
        assert_ne!(new_pkg.version, "latest");
        assert!(new_pkg.store_path.starts_with("/nix/store/"));
        assert!(new_pkg.drv_path.as_deref().unwrap().ends_with(".drv"));
        assert!(!new_pkg.store_paths.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rollback_event_recorded_on_success() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("rollback_event_success");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ffmpeg").unwrap();
        let report = rollback_last(&adapter).unwrap();
        assert!(report.success);

        let hist = history().unwrap();
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Rollback
                && event.status == events::RootEventStatus::Completed
        }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rollback_failure_preserves_lockfile_and_rootfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("rollback_failure_preserve");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ffmpeg").unwrap();

        let before_lock = get_or_create_lock_v2().unwrap();
        let before_rootfile = get_or_create_rootfile().unwrap();

        // Break the adapter by making it unavailable
        // We simulate failure by rolling back into a situation where
        // the adapter can't produce required outputs.
        let adapter2 = MockNixAdapter::new(false);

        let result = rollback_last(&adapter2);
        assert!(result.is_err());

        // Lockfile and Rootfile must be unchanged
        let after_lock = get_or_create_lock_v2().unwrap();
        let after_rootfile = get_or_create_rootfile().unwrap();

        assert_eq!(
            serde_json::to_string(&before_lock).unwrap(),
            serde_json::to_string(&after_lock).unwrap(),
            "Lockfile must not change after failed rollback"
        );
        assert_eq!(
            serde_json::to_string(&before_rootfile).unwrap(),
            serde_json::to_string(&after_rootfile).unwrap(),
            "Rootfile must not change after failed rollback"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── Package catalog tests ────────────────────────────────────────────

    #[test]
    fn test_all_packages_have_unique_names() {
        let mut names = std::collections::HashSet::new();
        for pkg in SUPPORTED_PACKAGES {
            assert!(
                names.insert(pkg.name),
                "Duplicate package name: {}",
                pkg.name
            );
        }
    }

    #[test]
    fn test_all_packages_have_nix_attr() {
        for pkg in SUPPORTED_PACKAGES {
            assert!(
                !pkg.nix_attr.is_empty(),
                "Package {} has empty nix_attr",
                pkg.name
            );
        }
    }

    #[test]
    fn test_all_packages_have_at_least_one_binary() {
        for pkg in SUPPORTED_PACKAGES {
            assert!(
                !pkg.binaries.is_empty(),
                "Package {} has no binaries",
                pkg.name
            );
        }
    }

    #[test]
    fn test_all_packages_have_at_least_one_verify_command() {
        for pkg in SUPPORTED_PACKAGES {
            assert!(
                !pkg.verify.is_empty(),
                "Package {} has no verify commands",
                pkg.name
            );
        }
    }

    #[test]
    fn test_verify_binary_matches_expected_binaries() {
        for pkg in SUPPORTED_PACKAGES {
            for verify_cmd in pkg.verify {
                assert!(
                    pkg.binaries.contains(&verify_cmd.binary),
                    "Package {}: verify binary '{}' is not in expected binaries {:?}",
                    pkg.name,
                    verify_cmd.binary,
                    pkg.binaries
                );
            }
        }
    }

    #[test]
    fn test_aliases_dont_collide_with_package_names() {
        let names: std::collections::HashSet<&str> =
            SUPPORTED_PACKAGES.iter().map(|p| p.name).collect();
        let mut all_aliases = std::collections::HashSet::new();
        for pkg in SUPPORTED_PACKAGES {
            for alias in pkg.aliases {
                assert!(
                    !names.contains(alias),
                    "Alias '{}' collides with package name",
                    alias
                );
                assert!(all_aliases.insert(alias), "Duplicate alias: {}", alias);
            }
        }
    }

    #[test]
    fn test_unsupported_package_is_rejected_before_nix_call() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let err = plan(&adapter, "nonexistent_pkg_xyz").unwrap_err();
        assert!(err.to_string().contains("does not support"));
    }

    #[test]
    fn test_unsupported_install_is_rejected_before_nix_call() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let err = install(&adapter, "nonexistent_pkg_xyz").unwrap_err();
        assert!(err.to_string().contains("does not support"));
    }

    #[test]
    fn test_catalog_includes_all_supported_packages() {
        let output = catalog();
        assert_eq!(output.packages.len(), SUPPORTED_PACKAGES.len());
        for entry in &output.packages {
            assert!(SUPPORTED_PACKAGES.iter().any(|p| p.name == entry.name));
        }
    }

    #[test]
    fn test_catalog_entries_have_required_fields() {
        let output = catalog();
        for entry in &output.packages {
            assert!(!entry.name.is_empty());
            assert!(!entry.description.is_empty());
            assert!(!entry.category.is_empty());
            assert!(!entry.nix_attr.is_empty());
            assert!(!entry.binaries.is_empty());
            assert!(!entry.verify.is_empty());
        }
    }

    #[test]
    fn test_search_by_alias_resolves_canonical_package() {
        let output = search("rg");
        assert_eq!(output.query, "rg");
        let ripgrep = output
            .matches
            .iter()
            .find(|package| package.name == "ripgrep")
            .unwrap();
        assert!(ripgrep.aliases.contains(&"rg"));
        assert!(ripgrep.matched_fields.contains(&"alias"));
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["matches"][0]["name"], "ripgrep");
    }

    #[test]
    fn test_search_matches_category_description_binary_and_nix_attr() {
        let category = search("search");
        assert!(category
            .matches
            .iter()
            .any(|package| package.name == "ripgrep"));

        let description = search("recursive");
        assert!(description
            .matches
            .iter()
            .any(|package| package.name == "ripgrep"));

        let binary = search("pdfinfo");
        assert!(binary
            .matches
            .iter()
            .any(|package| package.name == "poppler"));

        let nix_attr = search("nixpkgs#kubectl");
        assert!(nix_attr
            .matches
            .iter()
            .any(|package| package.name == "kubectl"));
    }

    #[test]
    fn test_search_is_case_insensitive_and_reports_no_matches() {
        let output = search("TERRAFORM");
        assert!(output
            .matches
            .iter()
            .any(|package| package.name == "terraform"));

        let no_match = search("definitely-not-a-root-package");
        assert!(no_match.matches.is_empty());
        assert_eq!(no_match.supported_count, SUPPORTED_PACKAGES.len());
    }

    #[test]
    fn test_plan_with_alias_resolves_to_canonical() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "rg").unwrap();
        assert_eq!(report.package, "ripgrep");
        assert_eq!(report.original_input, Some("rg".to_string()));
    }

    #[test]
    fn test_plan_with_canonical_name_has_no_original_input() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "ripgrep").unwrap();
        assert_eq!(report.original_input, None);
    }

    #[test]
    fn test_install_with_alias_stores_canonical_name() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "rg").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let rg_pkg = lock.packages.iter().find(|p| p.name == "ripgrep").unwrap();
        assert_eq!(rg_pkg.name, "ripgrep");
        assert_eq!(rg_pkg.requested, "rg");
        assert_eq!(rg_pkg.attribute, "ripgrep");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_update_with_alias_targets_canonical_package() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("update_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        adapter.install("ripgrep").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "ripgrep".into(),
            requested: "ripgrep".into(),
            version: "latest".into(),
            attribute: "ripgrep".into(),
            store_path: root_lockfile::derive_store_path("ripgrep", "latest"),
            binaries: vec!["rg".into()],
        });
        save_lock(&lock).unwrap();
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        let report = update(&adapter, Some("rg")).unwrap();
        assert!(report.success);
        assert!(report.updated.contains(&"ripgrep".to_string()));
        assert!(report.snapshot_id.is_some());

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let rg_pkg = lock.packages.iter().find(|p| p.name == "ripgrep").unwrap();
        assert_eq!(rg_pkg.name, "ripgrep");
        assert_eq!(rg_pkg.requested, "ripgrep");
        assert_ne!(rg_pkg.version, "latest");
        assert!(rg_pkg.store_path.starts_with("/nix/store/"));

        let hist = history().unwrap();
        assert!(hist.events.iter().any(|event| {
            event.event_type == events::RootEventType::Update
                && event.package.as_deref() == Some("ripgrep")
        }));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_update_all_packages_from_rootfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("update_all");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "latest".into());
        rootfile.packages.insert("fd".into(), "latest".into());
        rootfile
            .packages
            .insert("unsupported-local".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        let report = update(&adapter, None).unwrap();
        assert!(report.success);
        assert!(report.updated.contains(&"ripgrep".to_string()));
        assert!(report.updated.contains(&"fd".to_string()));
        assert!(report.skipped.contains(&"unsupported-local".to_string()));

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        assert!(lock.packages.iter().any(|p| p.name == "ripgrep"));
        assert!(lock.packages.iter().any(|p| p.name == "fd"));
        assert!(!lock.packages.iter().any(|p| p.name == "unsupported-local"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_update_rejects_unsupported_or_unmanaged_package() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("update_rejects");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        let unsupported = update(&adapter, Some("nonexistent_pkg_xyz")).unwrap_err();
        assert!(unsupported.to_string().contains("does not support"));

        let unmanaged = update(&adapter, Some("ripgrep")).unwrap_err();
        assert!(unmanaged.to_string().contains("not listed in Rootfile"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_node_alias_stores_nodejs() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_node_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "node").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let node_pkg = lock.packages.iter().find(|p| p.name == "nodejs").unwrap();
        assert_eq!(node_pkg.name, "nodejs");
        assert_eq!(node_pkg.requested, "node");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_make_alias_stores_gnumake() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_make_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "make").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let make_pkg = lock.packages.iter().find(|p| p.name == "gnumake").unwrap();
        assert_eq!(make_pkg.name, "gnumake");
        assert_eq!(make_pkg.requested, "make");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_python_alias_stores_python3() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_python_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "python").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let py_pkg = lock.packages.iter().find(|p| p.name == "python3").unwrap();
        assert_eq!(py_pkg.name, "python3");
        assert_eq!(py_pkg.requested, "python");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_plan_with_golang_alias_resolves_to_go() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "golang").unwrap();
        assert_eq!(report.package, "go");
        assert_eq!(report.original_input, Some("golang".to_string()));
    }

    #[test]
    fn test_plan_with_postgres_alias_resolves_to_postgresql() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "postgres").unwrap();
        assert_eq!(report.package, "postgresql");
        assert_eq!(report.original_input, Some("postgres".to_string()));
    }

    #[test]
    fn test_plan_with_tf_alias_resolves_to_terraform() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "tf").unwrap();
        assert_eq!(report.package, "terraform");
        assert_eq!(report.original_input, Some("tf".to_string()));
    }

    #[test]
    fn test_plan_with_kube_alias_resolves_to_kubectl() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "kube").unwrap();
        assert_eq!(report.package, "kubectl");
        assert_eq!(report.original_input, Some("kube".to_string()));
    }

    #[test]
    fn test_plan_with_docker_alias_resolves_to_docker_client() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "docker").unwrap();
        assert_eq!(report.package, "docker-client");
        assert_eq!(report.original_input, Some("docker".to_string()));
    }

    #[test]
    fn test_plan_with_nvim_alias_resolves_to_neovim() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "nvim").unwrap();
        assert_eq!(report.package, "neovim");
        assert_eq!(report.original_input, Some("nvim".to_string()));
    }

    #[test]
    fn test_install_with_golang_alias_stores_go() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_golang_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "golang").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock.packages.iter().find(|p| p.name == "go").unwrap();
        assert_eq!(pkg.name, "go");
        assert_eq!(pkg.requested, "golang");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_docker_alias_stores_docker_client() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_docker_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "docker").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock
            .packages
            .iter()
            .find(|p| p.name == "docker-client")
            .unwrap();
        assert_eq!(pkg.name, "docker-client");
        assert_eq!(pkg.requested, "docker");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_tf_alias_stores_terraform() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_tf_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "tf").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock
            .packages
            .iter()
            .find(|p| p.name == "terraform")
            .unwrap();
        assert_eq!(pkg.name, "terraform");
        assert_eq!(pkg.requested, "tf");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_nvim_alias_stores_neovim() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_nvim_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "nvim").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock.packages.iter().find(|p| p.name == "neovim").unwrap();
        assert_eq!(pkg.name, "neovim");
        assert_eq!(pkg.requested, "nvim");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_plan_with_delta_alias_resolves_to_git_delta() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "delta").unwrap();
        assert_eq!(report.package, "git-delta");
        assert_eq!(report.original_input, Some("delta".to_string()));
    }

    #[test]
    fn test_plan_with_z_alias_resolves_to_zoxide() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "z").unwrap();
        assert_eq!(report.package, "zoxide");
        assert_eq!(report.original_input, Some("z".to_string()));
    }

    #[test]
    fn test_plan_with_lg_alias_resolves_to_lazygit() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let report = plan(&adapter, "lg").unwrap();
        assert_eq!(report.package, "lazygit");
        assert_eq!(report.original_input, Some("lg".to_string()));
    }

    #[test]
    fn test_install_with_delta_alias_stores_git_delta() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_delta_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "delta").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock
            .packages
            .iter()
            .find(|p| p.name == "git-delta")
            .unwrap();
        assert_eq!(pkg.name, "git-delta");
        assert_eq!(pkg.requested, "delta");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_z_alias_stores_zoxide() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_z_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "z").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock.packages.iter().find(|p| p.name == "zoxide").unwrap();
        assert_eq!(pkg.name, "zoxide");
        assert_eq!(pkg.requested, "z");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_install_with_lg_alias_stores_lazygit() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_lg_alias");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "lg").unwrap();

        let lock_path = root_lockfile::get_root_dir().unwrap().join("root.lock");
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        let pkg = lock.packages.iter().find(|p| p.name == "lazygit").unwrap();
        assert_eq!(pkg.name, "lazygit");
        assert_eq!(pkg.requested, "lg");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_catalog_json_includes_aliases_and_verify() {
        let output = catalog();
        for entry in &output.packages {
            if let Some(spec) = SUPPORTED_PACKAGES.iter().find(|p| p.name == entry.name) {
                assert_eq!(entry.aliases.len(), spec.aliases.len());
                assert_eq!(entry.verify.len(), spec.verify.len());
            }
        }
    }

    #[test]
    fn test_resolve_package_by_name() {
        assert!(resolve_package("ffmpeg").is_some());
        assert!(resolve_package("ripgrep").is_some());
        assert!(resolve_package("jq").is_some());
        assert!(resolve_package("poppler").is_some());
        assert!(resolve_package("fd").is_some());
        assert!(resolve_package("gh").is_some());
        assert!(resolve_package("go").is_some());
        assert!(resolve_package("rustup").is_some());
        assert!(resolve_package("postgresql").is_some());
        assert!(resolve_package("redis").is_some());
        assert!(resolve_package("terraform").is_some());
        assert!(resolve_package("kubectl").is_some());
        assert!(resolve_package("helm").is_some());
        assert!(resolve_package("k9s").is_some());
        assert!(resolve_package("docker-client").is_some());
        assert!(resolve_package("age").is_some());
        assert!(resolve_package("sops").is_some());
        assert!(resolve_package("neovim").is_some());
        assert!(resolve_package("tmux").is_some());
        assert!(resolve_package("git-delta").is_some());
        assert!(resolve_package("zoxide").is_some());
        assert!(resolve_package("direnv").is_some());
        assert!(resolve_package("starship").is_some());
        assert!(resolve_package("lazygit").is_some());
    }

    #[test]
    fn test_resolve_package_by_alias() {
        assert!(resolve_package("rg").is_some(), "Alias 'rg' should resolve");
        assert!(
            resolve_package("make").is_some(),
            "Alias 'make' should resolve"
        );
        assert!(
            resolve_package("node").is_some(),
            "Alias 'node' should resolve"
        );
        assert!(
            resolve_package("python").is_some(),
            "Alias 'python' should resolve"
        );
        assert!(
            resolve_package("golang").is_some(),
            "Alias 'golang' should resolve"
        );
        assert!(
            resolve_package("postgres").is_some(),
            "Alias 'postgres' should resolve"
        );
        assert!(resolve_package("tf").is_some(), "Alias 'tf' should resolve");
        assert!(
            resolve_package("kube").is_some(),
            "Alias 'kube' should resolve"
        );
        assert!(
            resolve_package("docker").is_some(),
            "Alias 'docker' should resolve"
        );
        assert!(
            resolve_package("nvim").is_some(),
            "Alias 'nvim' should resolve"
        );
        assert!(
            resolve_package("delta").is_some(),
            "Alias 'delta' should resolve"
        );
        assert!(resolve_package("z").is_some(), "Alias 'z' should resolve");
        assert!(resolve_package("lg").is_some(), "Alias 'lg' should resolve");
    }

    #[test]
    fn test_error_message_shows_categories() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let err = plan(&adapter, "nonexistent_pkg_xyz").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("media:"));
        assert!(msg.contains("search:"));
        assert!(msg.contains("dev:"));
        assert!(msg.contains("net:"));
        assert!(msg.contains("language:"));
        assert!(msg.contains("database:"));
        assert!(msg.contains("infrastructure:"));
        assert!(msg.contains("security:"));
        assert!(msg.contains("editor:"));
        assert!(msg.contains("terminal:"));
        assert!(msg.contains("git:"));
    }

    #[test]
    fn test_deterministic_package_stores_drv_only_in_drv_path() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("drv_isolation");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir().unwrap();

        let adapter = MockNixAdapter::new(true);
        adapter.install("ffmpeg").unwrap();
        let (_flake, installable) = locked_installable_for(&adapter, "ffmpeg").unwrap();
        let resolution = adapter
            .resolve_locked_package("ffmpeg", Some(&installable))
            .unwrap();

        let locked =
            deterministic_package_from_resolution("ffmpeg", "ffmpeg", &installable, &resolution)
                .unwrap();

        // drv_path must end in .drv
        assert!(
            locked.drv_path.as_ref().unwrap().ends_with(".drv"),
            "drv_path should end in .drv: {:?}",
            locked.drv_path
        );

        // store_path must NOT end in .drv
        assert!(
            !locked.store_path.ends_with(".drv"),
            "store_path should not end in .drv: {}",
            locked.store_path
        );

        // All store_paths values must NOT end in .drv
        for (output_name, path) in &locked.store_paths {
            assert!(
                !path.ends_with(".drv"),
                "store_paths[{}] should not end in .drv: {}",
                output_name,
                path
            );
        }

        // All outputs store_path values must NOT end in .drv
        for (output_name, output) in &locked.outputs {
            assert!(
                !output.store_path.ends_with(".drv"),
                "outputs[{}].store_path should not end in .drv: {}",
                output_name,
                output.store_path
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_deterministic_package_rejects_drv_output_path() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("drv_reject");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir().unwrap();

        // Build a resolution where outputs contain a .drv path
        let drv_output = root_nix::BuildOutputPath {
            output_name: "out".to_string(),
            path: std::path::PathBuf::from("/nix/store/abc-ffmpeg-8.1.drv"),
        };
        let resolution = root_nix::LockedPackageResolution {
            package: "ffmpeg".to_string(),
            installable: "nixpkgs#ffmpeg".to_string(),
            metadata: root_nix::PackageMetadata {
                package: "ffmpeg".to_string(),
                installable: "nixpkgs#ffmpeg".to_string(),
                name: Some("ffmpeg-8.1".to_string()),
                version: Some("8.1".to_string()),
                description: None,
                raw_json: "{}".to_string(),
            },
            derivation: root_nix::DerivationInfo {
                package: "ffmpeg".to_string(),
                installable: "nixpkgs#ffmpeg".to_string(),
                derivation_path: std::path::PathBuf::from("/nix/store/abc-ffmpeg-8.1.drv"),
                output_paths: vec![drv_output.clone()],
            },
            outputs: vec![drv_output],
            path_info: vec![],
        };

        let result = deterministic_package_from_resolution(
            "ffmpeg",
            "ffmpeg",
            "nixpkgs#ffmpeg",
            &resolution,
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("derivation path"));
        assert!(err_msg.contains("ffmpeg"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_verify_profile_rejects_drv_paths() {
        let adapter = MockNixAdapter::new(true);
        adapter.install("ffmpeg").unwrap();

        let mut outputs = BTreeMap::new();
        outputs.insert(
            "out".to_string(),
            "/nix/store/abc-ffmpeg-8.1.drv".to_string(),
        );

        let result = verify_profile_contains_outputs(&adapter, &outputs);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("derivation path"));
        assert!(err_msg.contains(".drv"));
    }

    #[test]
    fn test_verify_profile_succeeds_with_real_output_path() {
        let adapter = MockNixAdapter::new(true);
        adapter.install("ffmpeg").unwrap();

        // Build store_paths that match what the mock's profile_list_json returns.
        // The mock uses mock_store_path(package) for profile entries.
        // We just need to verify that non-.drv paths pass the guard and the
        // profile check. Use the exact path the mock generates for "ffmpeg".
        let mock_path = {
            let token = format!("{:032x}", {
                "ffmpeg".bytes().fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                })
            });
            let version = {
                let n = "ffmpeg".bytes().fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                });
                format!("0.1.{}", n % 1000)
            };
            format!("/nix/store/{}-ffmpeg-{}", token, version)
        };

        let mut outputs = BTreeMap::new();
        outputs.insert("out".to_string(), mock_path);

        let result = verify_profile_contains_outputs(&adapter, &outputs);
        assert!(result.is_ok(), "verify should succeed: {:?}", result);
    }

    #[test]
    fn test_lockfile_drv_and_output_path_separation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lockfile_separation");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir().unwrap();

        let adapter = MockNixAdapter::new(true);
        adapter.install("ffmpeg").unwrap();
        let (_flake, installable) = locked_installable_for(&adapter, "ffmpeg").unwrap();
        let resolution = adapter
            .resolve_locked_package("ffmpeg", Some(&installable))
            .unwrap();
        let locked =
            deterministic_package_from_resolution("ffmpeg", "ffmpeg", &installable, &resolution)
                .unwrap();

        // Serialize to JSON and verify structure
        let json = serde_json::to_string_pretty(&locked).unwrap();

        // drv_path field must contain .drv
        assert!(
            json.contains("\"drv_path\""),
            "lockfile should have drv_path field"
        );
        let drv_val = locked.drv_path.as_ref().unwrap();
        assert!(drv_val.ends_with(".drv"), "drv_path value must end in .drv");

        // storePath field must NOT contain .drv
        assert!(
            !locked.store_path.ends_with(".drv"),
            "storePath must not end in .drv"
        );

        // storePaths values must NOT contain .drv
        for path in locked.store_paths.values() {
            assert!(!path.ends_with(".drv"), "storePaths must not end in .drv");
        }

        // outputs storePath values must NOT contain .drv
        for output in locked.outputs.values() {
            assert!(
                !output.store_path.ends_with(".drv"),
                "outputs storePath must not end in .drv"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_run_rootfile_task_with_root_profile_path() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("run_task");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        write_fake_binary(
            &tmp,
            "root-test-tool",
            "#!/bin/sh\nprintf 'from-root-profile\\n'\n",
        );

        let mut rootfile = Rootfile::default();
        rootfile
            .tasks
            .insert("profile-check".to_string(), "root-test-tool".to_string());
        rootfile.write_to_file(&tmp.join("Rootfile")).unwrap();

        let report = run(RunRequest::Task("profile-check".to_string())).unwrap();
        assert!(report.success);
        assert_eq!(report.exit_code, 0);
        assert_eq!(report.stdout, "from-root-profile\n");
        assert_eq!(report.task.as_deref(), Some("profile-check"));

        let events = events::read_events().unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == events::RootEventType::Execution
                && event.task_name.as_deref() == Some("profile-check")
                && event.exit_code == Some(0)
        }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_run_workflow_file() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("run_workflow");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let workflow_path = tmp.join("ci.root.toml");
        std::fs::write(
            &workflow_path,
            "version = 1\nname = \"ci\"\ncommand = \"printf workflow-ok\"\n",
        )
        .unwrap();

        let report = run(RunRequest::Workflow(workflow_path)).unwrap();
        assert!(report.success);
        assert_eq!(report.source, "workflow");
        assert_eq!(report.task.as_deref(), Some("ci"));
        assert_eq!(report.stdout, "workflow-ok");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_policy_apply_and_permissions_report() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("policy_apply");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let source = tmp.join("source-policy.toml");
        std::fs::write(&source, "version = 1\n[execution]\nrun = \"deny\"\n").unwrap();

        let applied = apply_policy(&source).unwrap();
        assert_eq!(applied.version, 1);
        let report = permissions().unwrap();
        assert_eq!(report.source, "configured");
        assert_eq!(report.policy.execution.run, policy::PolicyMode::Deny);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_denied_install_creates_no_snapshot() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("policy_denied_install");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        std::fs::write(
            tmp.join("policy.toml"),
            "version = 1\n[packages]\ninstall = \"deny\"\n",
        )
        .unwrap();

        let adapter = MockNixAdapter::new(true);
        let error = install(&adapter, "ripgrep").unwrap_err();
        assert!(error.to_string().contains("Policy denied install"));
        assert!(list_snapshots().unwrap().is_empty());

        let events = events::read_events().unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == events::RootEventType::Policy
                && event.policy_decision.as_deref() == Some("denied")
        }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sandbox_policy_denies_before_provider_mutation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("sandbox_policy");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        std::fs::write(
            tmp.join("policy.toml"),
            "version = 1\n[sandboxes]\ncreate = \"deny\"\n",
        )
        .unwrap();

        let provider = root_sandbox::MockSandboxProvider::new(true);
        let error = sandbox_create(&provider, Some("blocked"), None, None, None).unwrap_err();
        assert!(error.to_string().contains("Policy denied sandbox-create"));
        assert!(provider.list().unwrap().is_empty());

        let events = events::read_events().unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == events::RootEventType::Policy
                && event.policy_decision.as_deref() == Some("denied")
                && event.command == "policy check sandbox-create"
        }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_reports_healthy_and_profile_drift() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_drift");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ripgrep").unwrap();
        let healthy = status(&adapter).unwrap();
        assert!(healthy.healthy);
        assert_eq!(healthy.state, "Healthy");
        assert!(healthy.drift_details.is_empty());
        assert_ne!(healthy.machine_id, "unknown");

        adapter.remove("ripgrep").unwrap();
        let drifted = status(&adapter).unwrap();
        assert!(!drifted.healthy);
        assert_eq!(drifted.state, "NeedsAttention");
        assert_eq!(drifted.machine_id, healthy.machine_id);
        assert!(drifted.drift_details.iter().any(|issue| {
            issue.category == "lockfile-profile-mismatch" && issue.suggestion.contains("root sync")
        }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_reports_unavailable_profile_as_needs_attention() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_unavailable");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(false);

        // Install a package with a working adapter first, then switch to unavailable
        let working_adapter = MockNixAdapter::new(true);
        install(&working_adapter, "ripgrep").unwrap();

        let report = status(&adapter).unwrap();
        assert!(!report.healthy);
        assert_eq!(report.state, "NeedsAttention");
        assert!(report.drift_details.iter().any(|issue| {
            issue.category == "profile-unavailable" && issue.suggestion.contains("root doctor")
        }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_mutation_guard_acquires_and_releases() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("mutation_guard");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();

        let guard = MutationGuard::acquire().unwrap();
        // Second acquire should fail (already held)
        assert!(MutationGuard::acquire().is_err());
        drop(guard);
        // After release, should be able to acquire again
        let guard2 = MutationGuard::acquire().unwrap();
        drop(guard2);
    }

    #[test]
    fn test_mutation_guard_stale_lock_recovery() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("mutation_stale");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();

        // Write a stale lock file with a non-existent PID (PID 999999999 likely dead)
        let lock_path = tmp.join("root.lockfile");
        std::fs::write(&lock_path, "999999999\n0\n").unwrap();
        // Acquire should detect stale lock, remove it, and succeed
        let guard = MutationGuard::acquire().unwrap();
        assert!(lock_path.exists());
        // The lock content should contain our actual PID
        let content = std::fs::read_to_string(&lock_path).unwrap();
        let pid_line = content.lines().next().unwrap().trim();
        assert_eq!(pid_line.parse::<u32>().unwrap(), std::process::id());
        drop(guard);
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_mutation_guard_malformed_lock_fails_safely() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("mutation_malformed");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();

        // Write a malformed lock file
        let lock_path = tmp.join("root.lockfile");
        std::fs::write(&lock_path, "not-a-pid\n").unwrap();
        // Acquire should return an error about the malformed lock
        let err = MutationGuard::acquire().unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("lock") || err_msg.contains("stale") || err_msg.contains("manual"),
            "Expected error about lock file, got: {}",
            err_msg
        );
    }

    // Phase 3: Lockfile unchanged does not rewrite
    #[test]
    fn test_lockfile_unchanged_does_not_rewrite() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lockfile_no_rewrite");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let lock_path = root_dir.join("root.lock");
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ripgrep").unwrap();
        let first_meta = std::fs::metadata(&lock_path).unwrap();
        let first_modified = first_meta.modified().unwrap();

        // Re-save with same content — should not rewrite
        std::thread::sleep(std::time::Duration::from_millis(10));
        let lock = RootLockV2::read_from_file(&lock_path).unwrap();
        lock.write_to_file(&lock_path).unwrap();
        let second_meta = std::fs::metadata(&lock_path).unwrap();
        let second_modified = second_meta.modified().unwrap();

        assert_eq!(
            first_modified, second_modified,
            "Lockfile should not be rewritten when contents are unchanged"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 4: Event ledger with limit
    #[test]
    fn test_history_with_limit_returns_bounded_events() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("history_limit");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let _adapter = MockNixAdapter::new(true);

        // Record multiple events
        for i in 0..5 {
            let _ = events::record_event(
                events::RootEventType::Install,
                events::RootEventStatus::Completed,
                &format!("root install event-{}", i),
                Some(format!("event-{}", i)),
                None,
                None,
                None,
            );
        }

        let all = history().unwrap();
        assert_eq!(all.events.len(), 5);

        let limited = history_with_limit(Some(2)).unwrap();
        assert_eq!(limited.events.len(), 2, "Should return at most 2 events");
        assert!(
            limited.events[0].command.contains("event-4"),
            "Should return most recent events first"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 4: Malformed event handling
    #[test]
    fn test_history_handles_malformed_events_gracefully() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("history_malformed");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let _adapter = MockNixAdapter::new(true);

        // Write a valid event
        let _ = events::record_event(
            events::RootEventType::Install,
            events::RootEventStatus::Completed,
            "root install valid",
            Some("valid".to_string()),
            None,
            None,
            None,
        );

        // Append a malformed line directly
        let events_path = root_lockfile::get_root_dir().unwrap().join("events.jsonl");
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(file, "this is not valid json").unwrap();

        // History should skip the malformed line gracefully
        let hist = history().unwrap();
        assert!(
            hist.events
                .iter()
                .any(|e| e.package.as_deref() == Some("valid")),
            "Valid events should still be returned"
        );
        assert_eq!(hist.events.len(), 1, "Malformed line should be skipped");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 5: Status handles missing files gracefully
    #[test]
    fn test_status_with_missing_rootfile_and_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_missing");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        // No Rootfile, no lockfile — should still return healthy status
        let report = status(&adapter).unwrap();
        assert!(report.success);
        assert!(report.healthy);
        assert_eq!(report.rootfile_packages, 0);
        assert_eq!(report.lockfile_packages, 0);
        assert_eq!(report.profile_packages, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 6: Nix call hygiene — local-only commands should not invoke Nix
    #[test]
    fn test_search_does_not_call_nix() {
        // search operates entirely on SUPPORTED_PACKAGES (local)
        let output = search("ripgrep");
        assert!(!output.matches.is_empty());
        assert_eq!(output.query, "ripgrep");
    }

    #[test]
    fn test_catalog_does_not_call_nix() {
        // catalog operates entirely on SUPPORTED_PACKAGES (local)
        let output = catalog();
        assert_eq!(output.packages.len(), SUPPORTED_PACKAGES.len());
    }

    #[test]
    fn test_history_does_not_call_nix() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("history_no_nix");
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let hist = history().unwrap();
        assert!(hist.events.is_empty());
        assert!(hist.snapshots.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_does_not_call_nix_for_empty_state() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_no_nix");
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(false); // Nix unavailable
        let report = status(&adapter).unwrap();
        assert!(
            report.healthy,
            "With no packages and Nix unavailable, status should still report healthy"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 2: Search result unchanged
    #[test]
    fn test_search_output_format_preserved() {
        let output = search("terraform");
        assert_eq!(output.query, "terraform");
        assert!(output.matches.iter().any(|m| m.name == "terraform"));
        for m in &output.matches {
            // All fields must be present
            assert!(!m.name.is_empty());
            assert!(!m.category.is_empty());
            assert!(!m.description.is_empty());
            assert!(!m.nix_attr.is_empty());
            assert!(!m.binaries.is_empty());
            assert!(!m.matched_fields.is_empty());
        }
        // Verify JSON serialization works
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["query"], "terraform");
        assert!(json["matches"].is_array());
        assert!(json["supported_count"].as_u64().unwrap() > 0);
    }

    // Phase 2: Aliases still resolve
    #[test]
    fn test_search_aliases_resolve_correctly() {
        let by_alias = search("rg");
        assert!(by_alias.matches.iter().any(|m| m.name == "ripgrep"));

        let by_alias2 = search("nvim");
        assert!(by_alias2.matches.iter().any(|m| m.name == "neovim"));

        let by_docker = search("docker");
        assert!(by_docker.matches.iter().any(|m| m.name == "docker-client"));
    }

    // Phase 2: Category searches work
    #[test]
    fn test_search_category_works() {
        let infra = search("infrastructure");
        assert!(infra.matches.iter().any(|m| m.name == "terraform"));
        assert!(infra.matches.iter().any(|m| m.name == "kubectl"));
    }

    // Phase 2: Description searches work
    #[test]
    fn test_search_description_works() {
        let output = search("image");
        assert!(output.matches.iter().any(|m| m.name == "imagemagick"));

        let output2 = search("terminal");
        assert!(output2.matches.iter().any(|m| m.name == "tmux"));
    }

    // Phase 3: Lockfile parsed only once (verify through get_or_create_lock_v2)
    #[test]
    fn test_lockfile_parse_v2_compatibility() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lockfile_v2_compat");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ripgrep").unwrap();
        let lock = get_or_create_lock_v2().unwrap();
        assert_eq!(lock.version, ROOT_LOCK_SCHEMA_VERSION);
        assert!(lock.packages.iter().any(|p| p.name == "ripgrep"));
        // v2 compatibility: verify fields
        let ripgrep = lock.packages.iter().find(|p| p.name == "ripgrep").unwrap();
        assert!(ripgrep.installable.is_some());
        assert!(ripgrep.store_path.starts_with("/nix/store/"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 4: Recent-first ordering
    #[test]
    fn test_history_events_ordered_recent_first() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("history_order");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();

        let _ = events::record_event(
            events::RootEventType::Install,
            events::RootEventStatus::Completed,
            "first",
            Some("pkg-a".to_string()),
            None,
            None,
            None,
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = events::record_event(
            events::RootEventType::Install,
            events::RootEventStatus::Completed,
            "second",
            Some("pkg-b".to_string()),
            None,
            None,
            None,
        );

        let hist = history().unwrap();
        assert_eq!(hist.events.len(), 2);
        assert_eq!(
            hist.events[0].command, "second",
            "Most recent event should be first"
        );
        assert_eq!(
            hist.events[1].command, "first",
            "Older event should be second"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 5: Status handles missing profile
    #[test]
    fn test_status_missing_profile_no_panic() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_no_profile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(false);

        // Should not panic even without Nix
        let report = status(&adapter).unwrap();
        assert!(report.success);
        assert!(report.machine_id.len() > 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 6: Unsupported package rejection before Nix
    #[test]
    fn test_plan_rejects_unsupported_before_nix() {
        use root_nix::MockNixAdapter;
        let adapter = MockNixAdapter::new(true);
        let err = plan(&adapter, "definitely-not-a-real-package").unwrap_err();
        assert!(
            err.to_string().contains("does not support"),
            "Should reject before calling Nix: {}",
            err
        );
    }

    // ── Phase 4: Store Path Validation ────────────────────────────────────

    fn make_invalid_lockfile(lock_dir: &std::path::Path) {
        // Write a lockfile where an output store_path ends in .drv
        let mut pkg = root_lockfile::LockedPackageV2 {
            name: "ffmpeg".into(),
            requested: "ffmpeg".into(),
            version: "8.1".into(),
            attribute: "ffmpeg".into(),
            store_path: "/nix/store/abc-ffmpeg-8.1".into(),
            binaries: vec!["ffmpeg".into()],
            installable: Some("nixpkgs#ffmpeg".into()),
            drv_path: Some("/nix/store/drv-ffmpeg-8.1.drv".into()),
            ..Default::default()
        };
        pkg.outputs.insert(
            "out".into(),
            root_lockfile::LockedPackageOutput {
                store_path: "/nix/store/abc-ffmpeg-8.1.drv".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );
        pkg.store_paths
            .insert("out".into(), "/nix/store/abc-ffmpeg-8.1.drv".into());

        let lock = RootLockV2 {
            platform: root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into()),
            packages: vec![pkg],
            ..RootLockV2::default()
        };
        lock.write_to_file(&lock_dir.join("root.lock")).unwrap();
    }

    fn make_invalid_snapshot(snapshot_dir: &std::path::Path, snapshot_id: &str) {
        let mut pkg = root_lockfile::LockedPackageV2 {
            name: "ffmpeg".into(),
            requested: "ffmpeg".into(),
            version: "8.1".into(),
            attribute: "ffmpeg".into(),
            store_path: "/nix/store/abc-ffmpeg-8.1".into(),
            binaries: vec!["ffmpeg".into()],
            installable: Some("nixpkgs#ffmpeg".into()),
            drv_path: Some("/nix/store/drv-ffmpeg-8.1.drv".into()),
            ..Default::default()
        };
        pkg.outputs.insert(
            "out".into(),
            root_lockfile::LockedPackageOutput {
                store_path: "/nix/store/abc-ffmpeg-8.1.drv".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );
        pkg.store_paths
            .insert("out".into(), "/nix/store/abc-ffmpeg-8.1.drv".into());

        let lock = RootLockV2 {
            version: ROOT_LOCK_SCHEMA_VERSION,
            platform: root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into()),
            packages: vec![pkg],
            ..RootLockV2::default()
        };

        let snapshot = root_snapshot::Snapshot {
            id: snapshot_id.to_string(),
            created_at: chrono::Utc::now(),
            reason: "test invalid snapshot".into(),
            schema_version: ROOT_LOCK_SCHEMA_VERSION,
            package_count: 1,
            lock_content_hash: root_lockfile::compute_sha256(&serde_json::to_vec(&lock).unwrap()),
            lock: lock.clone(),
            packages: vec![],
        };
        let content = serde_json::to_string_pretty(&snapshot).unwrap();
        let snap_path = snapshot_dir.join(format!("{}.json", snapshot_id));
        std::fs::write(&snap_path, content).unwrap();
    }

    #[test]
    fn test_restore_rejects_invalid_lockfile_before_mutation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_rejects_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        make_invalid_lockfile(&tmp);

        let lock_path = tmp.join("root.lock");
        let err = restore(&adapter, Some(&lock_path)).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("failed validation before restore"),
            "Expected store path validation error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rollback_rejects_invalid_snapshot_before_mutation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("rollback_rejects_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        // Install a valid package first so there's something to roll back
        install(&adapter, "ffmpeg").unwrap();

        // Manually inject an invalid snapshot
        let snapshot_dir = root_dir.join("snapshots");
        let snapshot_id = "snap_invalid_00000000_000000_000000";
        make_invalid_snapshot(&snapshot_dir, snapshot_id);

        let err = rollback_last(&adapter).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("derivation path where an output path was expected")
                || err_msg.contains("failed validation"),
            "Expected store path validation error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sync_rejects_invalid_lockfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("sync_rejects_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        // Install a package via mock to get valid profile state
        adapter.install("ffmpeg").unwrap();

        // Write an invalid lockfile
        make_invalid_lockfile(&tmp);

        let err = sync(&adapter).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("derivation path") || err_msg.contains("failed validation"),
            "Expected store path validation error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lock_rejects_invalid_lockfile_on_read() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock_rejects_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        // Install a valid package to create initial state
        adapter.install("ripgrep").unwrap();
        let mut rf = get_or_create_rootfile().unwrap();
        rf.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rf).unwrap();
        lock(&adapter).unwrap();

        // Now replace with invalid lockfile
        make_invalid_lockfile(&tmp);

        // Reading the lockfile should fail validation
        let err = get_or_create_lock_v2().unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("validation"),
            "Expected validation error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Phase 6: Installer validation

    #[test]
    fn test_detect_platform_macos() {
        assert_eq!(
            detect_nix_install_platform_inner("macos").unwrap(),
            NixInstallPlatform::MacOs
        );
    }

    #[test]
    fn test_detect_platform_linux() {
        assert_eq!(
            detect_nix_install_platform_inner("linux").unwrap(),
            NixInstallPlatform::Linux
        );
    }

    #[test]
    fn test_detect_platform_unsupported() {
        let err = detect_nix_install_platform_inner("windows").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Unsupported platform") && msg.contains("windows"),
            "Expected unsupported platform error, got: {}",
            msg
        );
    }

    #[test]
    fn test_detect_platform_unsupported_other() {
        let err = detect_nix_install_platform_inner("freebsd").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Unsupported platform") && msg.contains("freebsd"),
            "Expected unsupported platform error, got: {}",
            msg
        );
    }

    #[test]
    fn test_build_nix_installer_command_structure() {
        let (script, args) = build_nix_installer_command();
        assert!(
            script.contains("curl") && script.contains("nixos.org"),
            "Script should contain curl command with nixos.org URL"
        );
        assert_eq!(args[0], "sh");
        assert_eq!(args[1], "-c");
        assert_eq!(args[2], script);
    }

    #[test]
    fn test_build_nix_installer_command_uses_https() {
        let (script, _) = build_nix_installer_command();
        assert!(
            script.contains("https://nixos.org/nix/install"),
            "Installer URL should use HTTPS"
        );
        assert!(
            !script.contains("http://"),
            "Installer URL should not use plain HTTP"
        );
    }

    #[test]
    fn test_installer_failure_message_format() {
        let msg = format_nix_installer_failure_message();
        assert!(msg.contains("Nix installation failed"));
        assert!(msg.contains("install Nix manually"));
        assert!(msg.contains("install.determinate.systems"));
        assert!(msg.contains("root doctor"));
    }

    #[test]
    fn test_installer_failure_message_includes_curl_command() {
        let msg = format_nix_installer_failure_message();
        assert!(msg.contains("curl --proto '=https' --tlsv1.2 -sSf -L"));
        assert!(msg.contains("install.determinate.systems/nix"));
    }

    #[test]
    fn test_non_interactive_message_format() {
        let msg = format_non_interactive_message();
        assert!(msg.contains("not interactive"));
        assert!(msg.contains("root init --install-nix --yes"));
        assert!(msg.contains("root doctor"));
    }

    #[test]
    fn test_post_install_version_failure_format() {
        let msg = format_post_install_version_failure("nix: command not found");
        assert!(msg.contains("nix --version' failed"));
        assert!(msg.contains("nix: command not found"));
        assert!(msg.contains("restarting your shell"));
        assert!(msg.contains("root doctor"));
    }

    #[test]
    fn test_post_install_version_failure_empty_stderr() {
        let msg = format_post_install_version_failure("");
        assert!(msg.contains("nix --version' failed"));
        assert!(msg.contains("unknown error"));
        assert!(msg.contains("root doctor"));
    }

    #[test]
    fn test_platform_detect_returns_os_on_this_machine() {
        // This test verifies that the platform detection function works
        // on whatever OS this test is run on (macOS or Linux).
        // It should not fail on supported platforms.
        if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
            let result = detect_nix_install_platform();
            assert!(result.is_ok(), "Platform should be detected on macOS/Linux");
        }
    }

    // ─── Phase 3: Profile Validation Integration Tests ────────────────────

    #[test]
    fn test_install_validates_profile_generation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_validate_gen");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "ripgrep").unwrap();

        let gen = adapter.profile_generation().unwrap();
        assert_eq!(
            gen,
            Some(2),
            "Generation should have incremented after install"
        );
    }

    #[test]
    fn test_install_fails_cleanly_when_profile_missing_path() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("install_fail_missing_path");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Override profile list to return JSON that doesn't include the
        // expected store path for ripgrep — simulating a failed install.
        let bad_json = r#"[{"index":0,"attrPath":"other-pkg","originalUrl":"flake:nixpkgs","installable":"nixpkgs#other-pkg","storePaths":["/nix/store/abc-other-pkg"]}]"#.to_string();
        adapter.set_profile_list_json_override(Some(bad_json));

        let err = install(&adapter, "ripgrep").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("validation") || msg.contains("store path") || msg.contains("profile"),
            "Expected validation error mentioning profile, got: {}",
            msg
        );
    }

    #[test]
    fn test_update_validates_profile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("update_validates_profile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Install via raw adapter + set up rootfile
        adapter.install("ripgrep").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "ripgrep".into(),
            requested: "ripgrep".into(),
            version: "latest".into(),
            attribute: "ripgrep".into(),
            store_path: root_lockfile::derive_store_path("ripgrep", "latest"),
            binaries: vec!["rg".into()],
        });
        save_lock(&lock).unwrap();
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        let report = update(&adapter, Some("ripgrep")).unwrap();
        assert!(report.success);
        assert!(report.updated.contains(&"ripgrep".to_string()));
    }

    #[test]
    fn test_rollback_validates_profile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("rollback_validates_profile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Set up initial package
        adapter.install("ripgrep").unwrap();
        let mut lock = get_or_create_lock().unwrap();
        lock.packages.push(LockedPackage {
            name: "ripgrep".into(),
            requested: "ripgrep".into(),
            version: "latest".into(),
            attribute: "ripgrep".into(),
            store_path: root_lockfile::derive_store_path("ripgrep", "latest"),
            binaries: vec!["rg".into()],
        });
        save_lock(&lock).unwrap();
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "latest".into());
        save_rootfile(&rootfile).unwrap();

        // Install another package to create a rollback point
        install(&adapter, "ffmpeg").unwrap();

        // Reset profile JSON override to ensure normal behavior
        adapter.set_profile_list_json_override(None);

        let report = rollback_last(&adapter).unwrap();
        assert!(report.success);
        assert!(report.packages_removed.contains(&"ffmpeg".to_string()));
    }

    #[test]
    fn test_validate_mutation_reports_generation_unchanged_error() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("validate_gen_unchanged");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir();
        let adapter = MockNixAdapter::new(true);

        // Manually configure: don't do any mutation so generation doesn't change
        let before = adapter.profile_generation().unwrap();

        // Set up a mock profile that already has the path
        let mock_path = {
            let token = format!("{:032x}", {
                "ripgrep".bytes().fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                })
            });
            let n = "ripgrep".bytes().fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
            });
            format!("/nix/store/{}-ripgrep-0.1.{}", token, n % 1000)
        };
        adapter.set_profile_list_json_override(Some(
            format!(
                r#"[{{"index":0,"attrPath":"ripgrep","installable":"nixpkgs#ripgrep","storePaths":["{}"]}}]"#,
                mock_path
            )
        ));

        // validate_mutation_result should detect generation didn't change
        let result =
            validate_mutation_result(&adapter, before, &["ripgrep"], &["rg"], &[&mock_path]);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("generation did not change"),
            "Expected generation-not-changed error, got: {}",
            msg
        );
    }

    #[test]
    fn test_validate_mutation_empty_profile_handled() {
        let adapter = MockNixAdapter::new(false);
        let result = validate_mutation_result(&adapter, None, &[], &[], &[]);
        assert!(result.is_ok(), "Empty state should not fail: {:?}", result);
    }

    #[test]
    fn test_restore_dry_run_does_not_mutate() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_dry_run");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("fd").unwrap();
        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let pre_rootfile = get_or_create_rootfile().ok();
        let pre_lock = get_or_create_lock_v2().ok();
        let pre_profile = profile_packages(&adapter).ok();

        let plan = restore_dry_run(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(plan.will_install.contains(&"ripgrep".to_string()));

        let post_rootfile = get_or_create_rootfile().ok();
        let post_lock = get_or_create_lock_v2().ok();
        let post_profile = profile_packages(&adapter).ok();

        assert_eq!(
            pre_rootfile.map(|r| r.packages.len()),
            post_rootfile.map(|r| r.packages.len()),
            "Dry-run should not modify Rootfile"
        );
        assert_eq!(
            pre_lock.map(|l| l.packages.len()),
            post_lock.map(|l| l.packages.len()),
            "Dry-run should not modify root.lock"
        );
        assert_eq!(
            pre_profile.map(|p| p.len()),
            post_profile.map(|p| p.len()),
            "Dry-run should not modify Nix profile"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_dry_run_reports_sets_correctly() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_dry_run_sets");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("ripgrep").unwrap();
        let (flake_rg, installable_rg) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution_rg = adapter
            .resolve_locked_package("ripgrep", Some(&installable_rg))
            .unwrap();
        let locked_rg = deterministic_package_from_resolution(
            "ripgrep",
            "ripgrep",
            &installable_rg,
            &resolution_rg,
        )
        .unwrap();
        let (_flake_fd, installable_fd) = locked_installable_for(&adapter, "fd").unwrap();
        let resolution_fd = adapter
            .resolve_locked_package("fd", Some(&installable_fd))
            .unwrap();
        let locked_fd =
            deterministic_package_from_resolution("fd", "fd", &installable_fd, &resolution_fd)
                .unwrap();
        let (_flake_jq, installable_jq) = locked_installable_for(&adapter, "jq").unwrap();
        let resolution_jq = adapter
            .resolve_locked_package("jq", Some(&installable_jq))
            .unwrap();
        let locked_jq =
            deterministic_package_from_resolution("jq", "jq", &installable_jq, &resolution_jq)
                .unwrap();

        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };

        // Lock has ripgrep, fd, jq. Profile has ripgrep only.
        let shared_lock =
            build_v2_lock(&base, &flake_rg, vec![locked_rg, locked_fd, locked_jq]).unwrap();
        let shared_lock_path = root_dir.join("three-pkg.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let plan = restore_dry_run(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(plan.will_install.contains(&"fd".to_string()));
        assert!(plan.will_install.contains(&"jq".to_string()));
        assert!(
            plan.will_update.contains(&"ripgrep".to_string())
                || plan.will_keep.contains(&"ripgrep".to_string())
        );
        assert!(!plan.will_install.contains(&"ripgrep".to_string()));
        assert_eq!(plan.total_packages, 3);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_dry_run_rejects_invalid_lockfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_dry_run_rejects");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        make_invalid_lockfile(&tmp);
        let lock_path = tmp.join("root.lock");

        let err = restore_dry_run(&adapter, Some(&lock_path)).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Restore validation failed") || err_msg.contains("failed validation"),
            "Expected validation error for invalid lockfile, got: {}",
            err_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_validation_rejects_drv_output_path() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_validates_drv");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let mut pkg = root_lockfile::LockedPackageV2 {
            name: "bad-pkg".into(),
            requested: "bad-pkg".into(),
            version: "1.0".into(),
            attribute: "bad-pkg".into(),
            store_path: "/nix/store/abc-bad-pkg-1.0".into(),
            binaries: vec!["bad-pkg".into()],
            installable: Some("nixpkgs#bad-pkg".into()),
            drv_path: Some("/nix/store/drv-bad-pkg-1.0.drv".into()),
            ..Default::default()
        };
        pkg.outputs.insert(
            "out".into(),
            root_lockfile::LockedPackageOutput {
                store_path: "/nix/store/abc-bad-pkg-1.0.drv".into(),
                content_hash: None,
                nar_hash: None,
                references: vec![],
            },
        );

        let lock = RootLockV2 {
            platform: "aarch64-darwin".into(),
            packages: vec![pkg],
            ..RootLockV2::default()
        };

        let result = restore_validate(&adapter, &lock, &tmp.join("test.lock"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains(".drv path") || msg.contains("derivation path"),
            "Expected .drv rejection, got: {}",
            msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_status_detects_platform_mismatch() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_platform");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.packages.insert("ripgrep".into(), "14.0".into());
        save_rootfile(&rootfile).unwrap();

        let mut lock = get_or_create_lock_v2().unwrap();
        lock.packages.push(LockedPackageV2 {
            name: "ripgrep".into(),
            requested: "ripgrep".into(),
            version: "14.0".into(),
            attribute: "ripgrep".into(),
            store_path: "/nix/store/abc-ripgrep-14.0".into(),
            binaries: vec!["rg".into()],
            ..Default::default()
        });
        let current_platform =
            root_lockfile::detect_platform().unwrap_or_else(|_| "x86_64-linux".into());
        lock.platform = if current_platform == "x86_64-linux" {
            "aarch64-darwin".into()
        } else {
            "x86_64-linux".into()
        };
        save_lock_v2(&lock).unwrap();

        let status = status(&adapter).unwrap();
        let has_platform_drift = status
            .drift_details
            .iter()
            .any(|d| d.category == "platform-mismatch");
        assert!(has_platform_drift, "Status should detect platform mismatch");
        assert!(
            !status.healthy,
            "Platform mismatch should mark the machine unhealthy"
        );
        assert_eq!(
            status.state, "NeedsAttention",
            "Platform mismatch should be NeedsAttention, got {}",
            status.state
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_records_event_on_success() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_event_success");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("fd").unwrap();
        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let report = restore(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(report.success);

        let hist = history().unwrap();
        let has_restore_event = hist.events.iter().any(|e| {
            e.event_type == events::RootEventType::Restore
                && e.status == events::RootEventStatus::Completed
        });
        assert!(
            has_restore_event,
            "Expected Completed Restore event after successful restore"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_records_event_on_validation_failure() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_event_failure");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        make_invalid_lockfile(&tmp);
        let lock_path = tmp.join("root.lock");
        let _ = restore(&adapter, Some(&lock_path));

        let hist = history().unwrap();
        let has_failure_event = hist.events.iter().any(|e| {
            e.event_type == events::RootEventType::Restore
                && e.status == events::RootEventStatus::Failed
        });
        assert!(
            has_failure_event,
            "Expected Failed Restore event after validation failure"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_partial_failure_rolls_back_profile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_partial_rb");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "fd").unwrap();

        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let ripgrep_locked =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();

        let missing_store =
            "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-missing_pkg-1.0".to_string();
        let mut missing_store_paths = BTreeMap::new();
        missing_store_paths.insert("out".to_string(), missing_store.clone());
        let missing_pkg_locked = LockedPackageV2 {
            name: "missing_pkg".into(),
            requested: "missing_pkg".into(),
            version: "1.0".into(),
            attribute: "missing_pkg".into(),
            store_path: missing_store.clone(),
            binaries: vec!["missing_pkg".into()],
            installable: Some("nixpkgs#missing_pkg".into()),
            store_paths: missing_store_paths,
            ..Default::default()
        };

        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock =
            build_v2_lock(&base_v2, &flake, vec![ripgrep_locked, missing_pkg_locked]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let err = restore(&adapter, Some(&shared_lock_path)).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("pre-restore snapshot") && err_msg.contains("package installation"),
            "Expected pre-restore snapshot recovery / package installation, got: {}",
            err_msg
        );

        let active_lock = RootLockV2::read_from_file(&root_dir.join("root.lock")).unwrap();
        assert!(active_lock.packages.iter().any(|p| p.name == "fd"));
        assert!(!active_lock.packages.iter().any(|p| p.name == "missing_pkg"));
        assert!(!active_lock.packages.iter().any(|p| p.name == "ripgrep"));

        let rf = get_or_create_rootfile().unwrap();
        assert!(rf.packages.contains_key("fd"));
        assert!(!rf.packages.contains_key("missing_pkg"));
        assert!(!rf.packages.contains_key("ripgrep"));

        let profile = profile_packages(&adapter).unwrap();
        assert!(profile.iter().any(|e| e.package == "fd"));
        assert!(!profile.iter().any(|e| e.package == "ripgrep"));

        let hist = history().unwrap();
        assert!(
            hist.events.iter().any(|e| {
                e.event_type == events::RootEventType::RestoreRecovered
                    && e.status == events::RootEventStatus::Completed
            }),
            "Expected RestoreRecovered Completed after automatic rollback"
        );
        assert!(
            hist.events.iter().any(|e| {
                e.event_type == events::RootEventType::Restore
                    && e.status == events::RootEventStatus::Failed
            }),
            "Expected Restore Failed after partial restore failure"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_with_no_existing_lockfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_no_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let _ = std::fs::remove_file(root_dir.join("root.lock"));

        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let report = restore(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(report.success);
        assert!(report.installed.contains(&"ripgrep".to_string()));

        let active_lock_path = root_dir.join("root.lock");
        assert!(active_lock_path.exists());
        let active_lock = RootLockV2::read_from_file(&active_lock_path).unwrap();
        assert!(active_lock.packages.iter().any(|p| p.name == "ripgrep"));

        let profile = profile_packages(&adapter).unwrap();
        assert!(profile.iter().any(|e| e.package == "ripgrep"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_from_v1_lockfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_v1");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let derived = root_lockfile::derive_store_path("ripgrep", "14.1.1");
        let store_path = if derived.starts_with("/nix/store/") {
            derived
        } else {
            "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-ripgrep-14.1.1".to_string()
        };
        let v1_lock = RootLock {
            version: 1,
            platform: root_lockfile::detect_platform().unwrap_or_else(|_| "x86_64-linux".into()),
            nixpkgs: NixpkgsConfig {
                rev: "unknown".into(),
                source: "github:NixOS/nixpkgs".into(),
            },
            packages: vec![LockedPackage {
                name: "ripgrep".into(),
                requested: "ripgrep".into(),
                version: "14.1.1".into(),
                attribute: "ripgrep".into(),
                store_path,
                binaries: vec!["rg".into()],
            }],
        };
        let v1_path = root_dir.join("v1-root.lock");
        v1_lock.write_to_file(&v1_path).unwrap();

        let report = restore(&adapter, Some(&v1_path)).unwrap();
        assert!(report.success);

        let profile = profile_packages(&adapter).unwrap();
        assert!(
            profile.iter().any(|e| e.package == "ripgrep"),
            "Profile should include ripgrep after v1 restore"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_recovers_stale_mutation_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_stale_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        adapter.install("fd").unwrap();
        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        std::fs::write(root_dir.join("root.lockfile"), "999999\n0\n").unwrap();

        let report = restore(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(report.success);
        assert!(!root_dir.join("root.lockfile").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_blocked_by_live_mutation_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_live_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let lockfile_path = root_dir.join("root.lockfile");
        std::fs::write(&lockfile_path, format!("{}\n0\n", std::process::id())).unwrap();

        let err = restore(&adapter, Some(&shared_lock_path)).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Another Root mutation is in progress"),
            "Expected live mutation lock error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_file(&lockfile_path);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_creates_missing_profile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_missing_profile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        adapter
            .profile_present
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_pkg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let shared_lock = build_v2_lock(&base_v2, &flake, vec![locked_pkg]).unwrap();
        let shared_lock_path = root_dir.join("shared-root.lock");
        shared_lock.write_to_file(&shared_lock_path).unwrap();

        let report = restore(&adapter, Some(&shared_lock_path)).unwrap();
        assert!(report.success);
        assert!(report.installed.contains(&"ripgrep".to_string()));

        let profile = profile_packages(&adapter).unwrap();
        assert!(profile.iter().any(|e| e.package == "ripgrep"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_rollback_restores_lock_and_rootfile() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_rollback_meta");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let _ = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        install(&adapter, "fd").unwrap();
        let pre_lock = get_or_create_lock_v2().unwrap();
        let snapshot = Snapshot::create_from_v2("before restore metadata test", &pre_lock).unwrap();

        let (flake, installable) = locked_installable_for(&adapter, "ripgrep").unwrap();
        let resolution = adapter
            .resolve_locked_package("ripgrep", Some(&installable))
            .unwrap();
        let locked_rg =
            deterministic_package_from_resolution("ripgrep", "ripgrep", &installable, &resolution)
                .unwrap();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let base_v2 = RootLockV2 {
            platform: platform.clone(),
            ..RootLockV2::default()
        };
        let target_lock = build_v2_lock(&base_v2, &flake, vec![locked_rg]).unwrap();
        save_lock_v2(&target_lock).unwrap();
        adapter.install("ripgrep").unwrap();

        attempt_rollback_to_snapshot(&adapter, &snapshot).unwrap();

        let restored_lock = get_or_create_lock_v2().unwrap();
        assert!(restored_lock.packages.iter().any(|p| p.name == "fd"));
        assert!(!restored_lock.packages.iter().any(|p| p.name == "ripgrep"));

        let rf = get_or_create_rootfile().unwrap();
        assert!(rf.packages.contains_key("fd"));
        assert!(!rf.packages.contains_key("ripgrep"));

        let profile = profile_packages(&adapter).unwrap();
        assert!(profile.iter().any(|e| e.package == "fd"));
        assert!(!profile.iter().any(|e| e.package == "ripgrep"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn seed_declared_inventory() {
        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.agents.insert("codex".into(), "*".into());
        rootfile.models.insert(
            "qwen3:8b".into(),
            root_lockfile::ModelDeclaration {
                runtime: "ollama".into(),
            },
        );
        save_rootfile(&rootfile).unwrap();
    }

    fn assert_declared_inventory_preserved() {
        let rootfile = get_or_create_rootfile().unwrap();
        assert_eq!(rootfile.agents.get("codex").map(String::as_str), Some("*"));
        assert_eq!(
            rootfile
                .models
                .get("qwen3:8b")
                .map(|model| model.runtime.as_str()),
            Some("ollama")
        );
    }

    fn write_future_lock(root_dir: &std::path::Path) -> String {
        let json = r#"{
  "version": 4,
  "platform": "aarch64-darwin",
  "packages": [],
  "models": [{"name": "must-not-be-copied"}]
}"#;
        std::fs::write(root_dir.join("root.lock"), json).unwrap();
        json.to_string()
    }

    fn satisfied_probe() -> inventory::MockInventoryProbe {
        let mut probe = inventory::MockInventoryProbe::default();
        probe.agents.insert(
            "codex".into(),
            inventory::ProbeResult {
                presence: inventory::Presence::Present,
                observed_version: Some("0.42.0".into()),
                observed_digest: None,
                evidence_source: inventory::EvidenceSource::VersionCommand,
                reason: None,
            },
        );
        probe.models.insert(
            "qwen3:8b".into(),
            inventory::ProbeResult {
                presence: inventory::Presence::Present,
                observed_version: Some("0.11.0".into()),
                observed_digest: Some("sha256:abc".into()),
                evidence_source: inventory::EvidenceSource::OllamaApiTags,
                reason: None,
            },
        );
        probe
    }

    #[test]
    fn test_rootfile_inventory_survives_package_rewrites() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("inventory_preserve");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        seed_declared_inventory();
        install(&adapter, "ripgrep").unwrap();
        assert_declared_inventory_preserved();

        update(&adapter, Some("ripgrep")).unwrap();
        assert_declared_inventory_preserved();

        lock(&adapter).unwrap();
        assert_declared_inventory_preserved();

        sync(&adapter).unwrap();
        assert_declared_inventory_preserved();

        remove(&adapter, "ripgrep").unwrap();
        assert_declared_inventory_preserved();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_rootfile_inventory_survives_restore_and_rollback() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("inventory_restore");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        seed_declared_inventory();
        install(&adapter, "ripgrep").unwrap();
        let shared = root_dir.join("shared.lock");
        std::fs::copy(root_dir.join("root.lock"), &shared).unwrap();

        restore(&adapter, Some(&shared)).unwrap();
        assert_declared_inventory_preserved();

        rollback_last(&adapter).unwrap();
        assert_declared_inventory_preserved();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_invalid_inventory_declaration_does_not_write() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("inventory_invalid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let path = root_dir.join("Rootfile");
        std::fs::write(&path, "[packages]\nripgrep = \"14.1.1\"\n").unwrap();
        let before = std::fs::read(&path).unwrap();

        let mut invalid = get_or_create_rootfile().unwrap();
        invalid.agents.insert("codex".into(), "1.2.3".into());
        assert!(save_rootfile(&invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_invalid_rootfile_blocks_mutation_before_nix() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("invalid_rootfile_nix");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        std::fs::write(
            root_dir.join("Rootfile"),
            "[packages]\nripgrep = \"14.1.1\"\n\n[agents]\ncodex = \"1.2.3\"\n",
        )
        .unwrap();
        let rootfile_before = std::fs::read(root_dir.join("Rootfile")).unwrap();

        for result in [
            install(&adapter, "ripgrep").map(|_| ()),
            remove(&adapter, "ripgrep").map(|_| ()),
            sync(&adapter).map(|_| ()),
            rollback_last(&adapter).map(|_| ()),
        ] {
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("version constraints are not supported in v0.2.5"),
                "unexpected error: {err}"
            );
        }

        assert!(adapter.installed_packages.lock().unwrap().is_empty());
        assert!(!root_dir.join("root.lock").exists());
        assert_eq!(
            std::fs::read(root_dir.join("Rootfile")).unwrap(),
            rootfile_before
        );
        assert!(list_snapshots().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_restore_from_v2_refuses_active_future_lock() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("restore_v2_active_v3");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        let original = write_future_lock(&root_dir);
        let shared = root_dir.join("shared-v2.lock");
        std::fs::write(
            &shared,
            r#"{"version":2,"platform":"aarch64-darwin","packages":[]}"#,
        )
        .unwrap();

        let err = restore(&adapter, Some(&shared)).unwrap_err().to_string();
        assert!(
            err.contains("newer than this Root supports"),
            "unexpected error: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(root_dir.join("root.lock")).unwrap(),
            original
        );
        assert!(list_snapshots().unwrap().is_empty());
        assert!(adapter.installed_packages.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_future_lock_is_rejected_before_mutation() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("future_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        let original = write_future_lock(&root_dir);
        let rootfile_path = root_dir.join("Rootfile");
        std::fs::write(&rootfile_path, "[packages]\nripgrep = \"14.1.1\"\n").unwrap();
        let rootfile_before = std::fs::read(&rootfile_path).unwrap();
        let events_before = events::read_events().unwrap().len();
        let snapshots_before = list_snapshots().unwrap().len();
        let installed_before = adapter.installed_packages.lock().unwrap().len();

        for result in [
            install(&adapter, "ripgrep").map(|_| ()),
            update(&adapter, Some("ripgrep")).map(|_| ()),
            remove(&adapter, "ripgrep").map(|_| ()),
            lock(&adapter).map(|_| ()),
            sync(&adapter).map(|_| ()),
            restore(&adapter, None).map(|_| ()),
            restore_dry_run(&adapter, None).map(|_| ()),
            rollback_last(&adapter).map(|_| ()),
        ] {
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("newer than this Root supports"),
                "unexpected error: {err}"
            );
        }

        assert_eq!(
            std::fs::read_to_string(root_dir.join("root.lock")).unwrap(),
            original
        );
        assert_eq!(std::fs::read(&rootfile_path).unwrap(), rootfile_before);
        assert_eq!(events::read_events().unwrap().len(), events_before);
        assert_eq!(list_snapshots().unwrap().len(), snapshots_before);
        assert_eq!(
            adapter.installed_packages.lock().unwrap().len(),
            installed_before
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_reports_future_lock_without_rewrite() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_future_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let original = write_future_lock(&root_dir);
        let adapter = MockNixAdapter::new(true);
        let report = status(&adapter).unwrap();
        assert!(!report.healthy);
        assert_eq!(report.state, "NeedsAttention");
        assert!(report.drift_details.iter().any(|issue| {
            issue.category == "incompatible-lock-schema"
                && issue.description.contains("schema version 4")
        }));
        assert_eq!(
            std::fs::read_to_string(root_dir.join("root.lock")).unwrap(),
            original
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_empty_inventory_json_keeps_legacy_fields() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_json_legacy");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        let report = status(&adapter).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["success"].is_boolean());
        assert!(json["healthy"].is_boolean());
        assert!(json["state"].is_string());
        assert!(json["rootfile_packages"].is_number());
        assert!(json["lockfile_packages"].is_number());
        assert!(json["profile_packages"].is_number());
        assert!(json["machine_id"].is_string());
        assert!(json["hostname"].is_string());
        assert!(json["drift_details"].is_array());
        assert!(json["inventory"].is_object());
        assert!(json["inventory"]["agents"].as_array().unwrap().is_empty());
        assert!(json["inventory"]["models"].as_array().unwrap().is_empty());
        assert!(report.inventory.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_inventory_present_satisfied_and_missing() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_inventory");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        seed_declared_inventory();

        let satisfied = status_with_probe(&adapter, &satisfied_probe()).unwrap();
        assert!(satisfied.healthy);
        assert_eq!(satisfied.state, "Healthy");
        assert_eq!(
            satisfied.inventory.agents[0].evaluation,
            inventory::EvaluationState::Satisfied
        );
        assert_eq!(
            satisfied.inventory.models[0].evaluation,
            inventory::EvaluationState::Satisfied
        );

        let missing =
            status_with_probe(&adapter, &inventory::MockInventoryProbe::default()).unwrap();
        assert!(!missing.healthy);
        assert_eq!(missing.state, "Drifted");
        assert!(missing
            .drift_details
            .iter()
            .any(|issue| issue.category == "agent-missing"));
        assert!(missing
            .drift_details
            .iter()
            .any(|issue| issue.category == "model-missing"));
        assert!(!missing
            .drift_details
            .iter()
            .any(|issue| issue.suggestion.contains("root sync")
                && (issue.category.starts_with("agent-") || issue.category.starts_with("model-"))));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_inventory_unknown_and_unsupported_need_attention() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_inventory_attn");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        let mut rootfile = get_or_create_rootfile().unwrap();
        rootfile.agents.insert("codex".into(), "*".into());
        rootfile.agents.insert("gemini".into(), "*".into());
        save_rootfile(&rootfile).unwrap();

        let mut probe = inventory::MockInventoryProbe::default();
        probe.agents.insert(
            "codex".into(),
            inventory::ProbeResult {
                presence: inventory::Presence::Unknown,
                observed_version: None,
                observed_digest: None,
                evidence_source: inventory::EvidenceSource::VersionCommand,
                reason: Some(inventory::REASON_TIMED_OUT.into()),
            },
        );
        let report = status_with_probe(&adapter, &probe).unwrap();
        assert_eq!(report.state, "NeedsAttention");
        assert!(report
            .drift_details
            .iter()
            .any(|issue| issue.category == "agent-observation-unknown"));
        assert!(report
            .drift_details
            .iter()
            .any(|issue| issue.category == "agent-not-supported-by-this-release"));
        assert!(!probe
            .agent_calls
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "gemini"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_package_needs_attention_dominates_inventory_drift() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_combo");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        install(&adapter, "ripgrep").unwrap();
        adapter.remove("ripgrep").unwrap();
        seed_declared_inventory();

        let report =
            status_with_probe(&adapter, &inventory::MockInventoryProbe::default()).unwrap();
        assert_eq!(report.state, "NeedsAttention");
        assert!(report
            .drift_details
            .iter()
            .any(|issue| issue.category == "lockfile-profile-mismatch"));
        assert!(report
            .drift_details
            .iter()
            .any(|issue| issue.category == "agent-missing"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_status_does_not_probe_without_declarations() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_no_probe");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        let probe = inventory::MockInventoryProbe::default();
        let report = status_with_probe(&adapter, &probe).unwrap();
        assert!(report.healthy);
        assert!(probe.agent_calls.lock().unwrap().is_empty());
        assert!(probe.model_calls.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_emitted_lock_remains_v2_without_inventory() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock_no_inventory");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        seed_declared_inventory();
        install(&adapter, "ripgrep").unwrap();
        let lock = get_or_create_lock_v2().unwrap();
        assert_eq!(lock.version, ROOT_LOCK_SCHEMA_VERSION);
        root_lockfile::validate_supported_lock_version_with_max(lock.version, 2).unwrap();
        let raw = std::fs::read_to_string(get_root_dir().unwrap().join("root.lock")).unwrap();
        assert!(!raw.contains("\"agents\""));
        assert!(!raw.contains("codex"));
        assert!(!raw.contains("qwen3:8b"));
        assert!(!raw.contains("\"models\""));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn canonical_model_digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    fn write_v3_models_lock(root_dir: &std::path::Path) {
        let digest = canonical_model_digest();
        let platform = root_lockfile::detect_platform().unwrap_or_else(|_| "aarch64-darwin".into());
        let json = format!(
            r#"{{
  "version": 3,
  "platform": "{platform}",
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
        std::fs::write(root_dir.join("root.lock"), json).unwrap();
    }

    fn assert_v3_models_preserved() {
        let lock = get_or_create_lock_v2().unwrap();
        assert_eq!(lock.version, 3);
        let model = lock
            .models
            .get("ollama")
            .and_then(|models| models.get("qwen3:8b"))
            .expect("v3 models object map must survive package lock writes");
        assert_eq!(model.runtime, "ollama");
        assert_eq!(model.requested_name, "qwen3:8b");
        assert_eq!(model.observed_digest, canonical_model_digest());
        assert_eq!(model.endpoint.as_deref(), Some("http://127.0.0.1:11434"));
        let raw = std::fs::read_to_string(get_root_dir().unwrap().join("root.lock")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            value["models"].is_object(),
            "v3 models must serialize as an object map, got: {raw}"
        );
        assert!(!value["models"].is_array());
    }

    #[test]
    fn test_lock_models_survive_package_rewrites() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock_models_preserve");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        write_v3_models_lock(&root_dir);
        install(&adapter, "ripgrep").unwrap();
        assert_v3_models_preserved();

        update(&adapter, Some("ripgrep")).unwrap();
        assert_v3_models_preserved();

        lock(&adapter).unwrap();
        assert_v3_models_preserved();

        sync(&adapter).unwrap();
        assert_v3_models_preserved();

        remove(&adapter, "ripgrep").unwrap();
        assert_v3_models_preserved();
        let lock = get_or_create_lock_v2().unwrap();
        assert!(lock.packages.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lock_models_survive_restore_and_rollback() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("lock_models_restore");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);

        write_v3_models_lock(&root_dir);
        install(&adapter, "ripgrep").unwrap();
        assert_v3_models_preserved();
        let shared = root_dir.join("shared.lock");
        std::fs::copy(root_dir.join("root.lock"), &shared).unwrap();

        restore(&adapter, Some(&shared)).unwrap();
        assert_v3_models_preserved();

        rollback_last(&adapter).unwrap();
        assert_v3_models_preserved();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_invalid_locked_models_do_not_rewrite() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("invalid_lock_models");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
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
        "observed_digest": "sha256-{}",
        "locked_at": "2026-09-01T00:00:00Z",
        "verified_at": "2026-09-01T00:00:01Z",
        "verification_method": "inspect_tags_digest",
        "addressability": "verification_record_only"
      }}
    }}
  }}
}}"#,
            "a".repeat(64)
        );
        let path = root_dir.join("root.lock");
        std::fs::write(&path, &json).unwrap();
        let before = std::fs::read(&path).unwrap();

        let err = install(&adapter, "ripgrep").unwrap_err().to_string();
        assert!(
            err.contains("observed_digest must be canonical") || err.contains("models validation"),
            "unexpected error: {err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(adapter.installed_packages.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_unparseable_v3_models_do_not_fall_back_to_v1() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("unparseable_v3_models");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        let adapter = MockNixAdapter::new(true);
        let json = r#"{
  "version": 3,
  "platform": "aarch64-darwin",
  "nixpkgs": {
    "rev": "0123456789abcdef0123456789abcdef01234567",
    "source": "github:NixOS/nixpkgs"
  },
  "packages": [],
  "models": []
}"#;
        let path = root_dir.join("root.lock");
        std::fs::write(&path, json).unwrap();
        let before = std::fs::read(&path).unwrap();

        let err = install(&adapter, "ripgrep").unwrap_err().to_string();
        assert!(
            err.contains("parse") || err.contains("models"),
            "unparseable v3 models must fail closed, got: {err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(adapter.installed_packages.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_v3_lock_is_accepted_by_status() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let tmp = test_tmp_dir("status_v3_models");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ROOT_DIR", &tmp);
        let root_dir = root_lockfile::init_root_dir().unwrap();
        write_v3_models_lock(&root_dir);
        let original = std::fs::read_to_string(root_dir.join("root.lock")).unwrap();
        let adapter = MockNixAdapter::new(true);
        let report = status(&adapter).unwrap();
        assert!(
            !report
                .drift_details
                .iter()
                .any(|issue| issue.category == "incompatible-lock-schema"),
            "v3 models object map must not be treated as a future lock"
        );
        assert_eq!(
            std::fs::read_to_string(root_dir.join("root.lock")).unwrap(),
            original
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
