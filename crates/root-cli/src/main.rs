use clap::{Parser, Subcommand};
use root_nix::RealNixAdapter;
use root_sandbox::RealSandboxProvider;
use serde::Serialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "root",
    about = "Root - deterministic package manager for developer CLI tools",
    version
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize Root directory structure
    Init {
        /// Install Nix automatically if not detected
        #[arg(long)]
        install_nix: bool,
        /// Skip confirmation prompts (use with --install-nix)
        #[arg(long)]
        yes: bool,
    },
    /// Show the curated package catalog
    Catalog,
    /// Search the curated package catalog
    Search {
        #[arg(value_name = "QUERY")]
        query: String,
    },
    /// Search for a package to install
    Plan {
        #[command(subcommand)]
        subcommand: PlanSubcommands,
    },
    /// Install a package
    Install {
        #[arg(value_name = "PACKAGE")]
        pkg: String,
    },
    /// List managed packages
    List,
    /// Remove a package
    Remove {
        #[arg(value_name = "PACKAGE")]
        pkg: String,
    },
    /// Update one managed package, or all packages in Rootfile
    Update {
        #[arg(value_name = "PACKAGE")]
        pkg: Option<String>,
    },
    /// Show snapshot history
    History {
        /// Show only the most recent N events
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Rollback to the previous state
    Rollback {
        #[arg(long)]
        last: bool,
    },
    /// Check system health and drift
    Doctor {
        /// Exit with a non-zero code if any issue/drift is detected
        #[arg(long)]
        check: bool,
    },
    /// Verify an installed package's binaries are executable
    Verify {
        #[arg(value_name = "PACKAGE")]
        pkg: String,
    },
    /// Import packages from other package managers (e.g., brew)
    Import {
        #[arg(value_name = "SOURCE")]
        source: String,
    },
    /// Regenerate root.lock from current state
    Lock,
    /// Reconcile Nix profile with root.lock
    Sync,
    /// Restore the Root profile from a lockfile
    Restore {
        /// Lockfile to restore from. Defaults to ~/.root/root.lock
        #[arg(long, value_name = "PATH")]
        lock: Option<std::path::PathBuf>,
        /// Show restore plan without mutating anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a Rootfile task, workflow file, or command
    Run {
        #[arg(value_name = "TASK_OR_WORKFLOW")]
        target: Option<String>,
        #[arg(last = true, num_args = 1.., value_name = "COMMAND")]
        command: Vec<OsString>,
    },
    /// Show the active permissions and policy configuration
    Permissions,
    /// Manage Root policy files
    Policy {
        #[command(subcommand)]
        subcommand: PolicySubcommands,
    },
    /// Create or manage isolated sandbox environments
    Sandbox {
        #[command(subcommand)]
        subcommand: SandboxSubcommands,
    },
    /// Show machine status, package drift, and declared agent/model inspection
    Status,
}

#[derive(Subcommand, Debug)]
enum PlanSubcommands {
    /// Show install plan for a package
    Install {
        #[arg(value_name = "PACKAGE")]
        pkg: String,
    },
    /// Preview declared Ollama model actions (read-only)
    Models {
        #[arg(value_name = "NAME")]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum PolicySubcommands {
    /// Validate and activate a policy file
    Apply {
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SandboxSubcommands {
    /// Create a new sandbox
    Create {
        /// Name for the sandbox (default: "default")
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Container image to use (default: ubuntu:latest)
        #[arg(long, value_name = "IMAGE")]
        image: Option<String>,
        /// Memory limit (default: 2g)
        #[arg(long, value_name = "MEMORY")]
        memory: Option<String>,
        /// CPU limit (default: 2.0)
        #[arg(long, value_name = "CPUS")]
        cpus: Option<String>,
    },
    /// Run a command inside a sandbox
    Run {
        /// Sandbox ID or name
        #[arg(value_name = "ID")]
        id: String,
        /// Timeout in seconds (default: 300)
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
        #[arg(last = true, num_args = 1.., value_name = "COMMAND")]
        command: Vec<std::ffi::OsString>,
    },
    /// List all Root-managed sandboxes
    List,
    /// Destroy a sandbox
    Destroy {
        /// Sandbox ID or name
        #[arg(value_name = "ID")]
        id: String,
    },
}

#[derive(Serialize)]
struct GenericOutput {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_stderr: Option<String>,
}

fn print_json<T: Serialize>(output: &T) {
    println!("{}", serde_json::to_string_pretty(output).unwrap());
}

fn append_inventory_section(
    msg: &mut String,
    heading: &str,
    items: &[root_core::inventory::InventoryItem],
) {
    if items.is_empty() {
        return;
    }
    msg.push_str(&format!("\n{heading}:"));
    for item in items {
        msg.push_str(&format!(
            "\n  {}  {}  {}  {}",
            item.name,
            item.desired,
            inventory_presence_label(item.observation),
            inventory_evaluation_label(item.evaluation)
        ));
        if let Some(version) = &item.observed_version {
            msg.push_str(&format!("  {version}"));
        }
        if let Some(digest) = &item.observed_digest {
            msg.push_str(&format!("  {digest}"));
        }
        if let Some(locked) = &item.locked_digest {
            msg.push_str(&format!("  locked {locked}"));
        }
        match item.digest_match {
            Some(true) => msg.push_str("  digest match"),
            Some(false) => msg.push_str("  digest mismatch"),
            None => {}
        }
        if let Some(reason) = &item.reason {
            msg.push_str(&format!(
                "  ({})",
                root_core::inventory::reason_phrase(reason)
            ));
        }
    }
    msg.push('\n');
}

fn inventory_presence_label(presence: root_core::inventory::Presence) -> &'static str {
    match presence {
        root_core::inventory::Presence::Present => "present",
        root_core::inventory::Presence::Absent => "absent",
        root_core::inventory::Presence::Unknown => "unknown",
    }
}

fn inventory_evaluation_label(evaluation: root_core::inventory::EvaluationState) -> &'static str {
    match evaluation {
        root_core::inventory::EvaluationState::Satisfied => "satisfied",
        root_core::inventory::EvaluationState::Missing => "missing",
        root_core::inventory::EvaluationState::Drifted => "drifted",
        root_core::inventory::EvaluationState::Unknown => "unknown",
        root_core::inventory::EvaluationState::Unsupported => "unsupported",
    }
}

fn json_error_output(e: &anyhow::Error) -> GenericOutput {
    let raw_stderr = e
        .downcast_ref::<root_nix::NixError>()
        .and_then(|ne| ne.raw_stderr())
        .map(|s| s.to_string());
    GenericOutput {
        success: false,
        message: format!("{}", e),
        raw_stderr,
    }
}

fn exit_code_for_error(e: &anyhow::Error) -> i32 {
    if let Some(nix_err) = e.downcast_ref::<root_nix::NixError>() {
        return match nix_err {
            root_nix::NixError::NotInstalled => 7,
            root_nix::NixError::NotFound(_) | root_nix::NixError::AttributeMissing(_) => 3,
            root_nix::NixError::PlatformMissing(_) => 8,
            root_nix::NixError::Generic(_) | root_nix::NixError::Internal(_) => 1,
            root_nix::NixError::FlakesDisabled
            | root_nix::NixError::NixCommandDisabled
            | root_nix::NixError::NixpkgsUnavailable
            | root_nix::NixError::ProfileLocked
            | root_nix::NixError::ProfileSymlinkConflict
            | root_nix::NixError::PermissionDenied
            | root_nix::NixError::StorePathNotRealized(_)
            | root_nix::NixError::DerivationPathAsOutput(_) => 1,
        };
    }
    let msg = format!("{:?}", e);
    if msg.contains("Policy denied") {
        9
    } else if msg.contains("Rollback") || msg.contains("rollback") {
        6
    } else if msg.contains("verification")
        || msg.contains("Verification")
        || msg.contains("root.lock does not exist")
        || msg.contains("is not found in root.lock")
    {
        4
    } else if msg.contains("unsupported import source")
        || msg.contains("Only 'brew' is supported")
        || msg.contains("Root does not support")
        || msg.contains("Choose either a task/workflow")
        || msg.contains("Provide a Rootfile task")
        || msg.contains("is not declared in Rootfile")
    {
        2
    } else if msg.contains("Drift") || msg.contains("drift") {
        5
    } else {
        1
    }
}

fn format_user_error(e: &anyhow::Error) -> String {
    if let Some(nix_err) = e.downcast_ref::<root_nix::NixError>() {
        return format!("{}", nix_err);
    }
    let msg = format!("{}", e);
    if msg.contains("Policy denied") {
        format!(
            "{}\n\nRun `root permissions` to inspect the active policy.",
            msg
        )
    } else if msg.contains("No snapshots") {
        "No snapshots available for rollback.\n\n\
         Snapshots are created automatically before every install or remove.\n\
         Run:  root install ffmpeg\n\
         Then: root rollback --last"
            .to_string()
    } else if msg.contains("root.lock does not exist") {
        "No lockfile found.\n\n\
         Run:  root install <package>\n\
         This will create root.lock with deterministic Nix metadata."
            .to_string()
    } else if msg.contains("is not found in root.lock") {
        format!(
            "{}.\n\n\
             Install it first with:  root install {}",
            msg,
            msg.split('\'').nth(1).unwrap_or("the-package")
        )
    } else if msg.contains("does not support") || msg.contains("not support") {
        msg
    } else if msg.contains("stale lockfile") || msg.contains("lockfile is stale") {
        "The lockfile is stale or from a previous version.\n\n\
         Run:  root lock\n\
         This will regenerate the lockfile with current metadata."
            .to_string()
    } else if msg.contains("v2 lockfiles") || msg.contains("does not support v2") {
        msg
    } else if msg.contains("mutation is in progress") {
        "Another Root operation is in progress.\n\n\
         If no other terminal is running Root, delete the lock:\n  rm ~/.root/root.lockfile\nThen try again."
            .to_string()
    } else if msg.contains("Restore validation failed") {
        if msg.contains("Nix is not available") {
            "Restore requires Nix, but Nix is not installed or not in PATH.\n\n\
             Install Nix from https://nixos.org/download.html"
                .to_string()
        } else if msg.contains(".drv path") {
            format!(
                "{}\n\n\
                 The lockfile appears to be corrupted or from an incompatible source.\n\
                 Run:  root lock\n\
                 This will regenerate the lockfile with correct store paths.",
                msg
            )
        } else {
            msg
        }
    } else {
        msg
    }
}

fn handle_structured<T: Serialize>(
    json: bool,
    res: anyhow::Result<T>,
    human_fn: impl FnOnce(&T) -> String,
) -> Option<T> {
    match res {
        Ok(val) => {
            if json {
                print_json(&val);
            } else {
                println!("{}", human_fn(&val));
            }
            Some(val)
        }
        Err(e) => {
            let code = exit_code_for_error(&e);
            if json {
                print_json(&json_error_output(&e));
            } else {
                eprintln!("Error: {}", format_user_error(&e));
            }
            process::exit(code);
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let profile_path = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("~"))
        .join(".root")
        .join("profiles")
        .join("default");
    let adapter = RealNixAdapter::new(profile_path);
    let sandbox_provider = RealSandboxProvider::new();

    match cli.command {
        Commands::Init { install_nix, yes } => {
            let report = handle_structured(cli.json, root_core::init(&adapter), |r| {
                let mut msg = String::from("Root initialized.\n");
                msg.push_str(&format!("\n✓ Root directory created at {}", r.root_dir));
                if r.nix_detected {
                    msg.push_str("\n✓ Nix detected");
                } else {
                    msg.push_str("\n✗ Nix not detected");
                }
                if r.profile_ready {
                    msg.push_str("\n✓ Root profile ready");
                }
                if r.snapshot_enabled {
                    msg.push_str("\n✓ Snapshot system enabled");
                }
                if r.nix_detected {
                    msg.push_str("\n\nRoot is ready.");
                    msg.push_str("\n  Nix provides reproducible builds and package isolation.");
                    msg.push_str("\n  Next steps:");
                    msg.push_str("\n    root doctor           Check system health");
                    msg.push_str("\n    root install ffmpeg   Install your first package");
                    msg.push_str("\n    root history          View history");
                    msg.push_str("\n    root verify ffmpeg    Verify package binaries");
                    msg.push_str("\n    root rollback --last  Undo the install");
                    msg.push_str("\n\n  Run `root catalog` to see all 42 supported packages.");
                }
                msg
            });

            if let Some(mut report) = report {
                if !report.nix_detected {
                    if install_nix {
                        if !cli.json {
                            println!("Installing Nix...");
                        }
                        match root_core::install_nix(yes) {
                            Ok(()) => {
                                report.nix_detected = true;
                                if !cli.json {
                                    println!("✓ Nix installed successfully.");
                                }
                            }
                            Err(e) => {
                                let code = exit_code_for_error(&e);
                                if cli.json {
                                    print_json(&GenericOutput {
                                        success: false,
                                        message: format!("Failed to install Nix: {:?}", e),
                                        raw_stderr: None,
                                    });
                                } else {
                                    eprintln!("Error installing Nix: {}", e);
                                }
                                process::exit(code);
                            }
                        }
                    } else if !cli.json {
                        eprintln!("\nNix is required but was not found. Root uses Nix to build and isolate packages.\n\nTo install Nix, run:\n  root init --install-nix\n\nOr install Nix manually from:\n  https://nixos.org/download/\n\nAfter installing Nix, run:\n  root doctor   Check that everything works\n  root install ffmpeg    Install your first package");
                    }
                }
            }
        }
        Commands::Catalog => {
            let output = root_core::catalog();
            if cli.json {
                print_json(&output);
            } else {
                println!("Root supported packages\n");
                let mut categories: std::collections::BTreeMap<
                    &str,
                    Vec<&root_core::CatalogEntry>,
                > = std::collections::BTreeMap::new();
                for pkg in &output.packages {
                    categories.entry(pkg.category).or_default().push(pkg);
                }
                for (category, pkgs) in &categories {
                    println!("{}", category);
                    for pkg in pkgs {
                        println!("  {:<12} {}", pkg.name, pkg.description);
                    }
                    println!();
                }
            }
        }
        Commands::Search { query } => {
            let output = root_core::search(&query);
            if cli.json {
                print_json(&output);
            } else if output.matches.is_empty() {
                println!("No supported packages matched '{}'.", output.query);
                println!();
                println!(
                    "Root currently supports a curated catalog of {} packages.",
                    output.supported_count
                );
                println!("Run `root catalog` to browse supported packages.");
            } else {
                println!("Search results for '{}'\n", output.query);
                for package in &output.matches {
                    let aliases = if package.aliases.is_empty() {
                        String::new()
                    } else {
                        format!(" aliases: {}", package.aliases.join(", "))
                    };
                    println!(
                        "  {:<14} {:<14} {}{}",
                        package.name, package.category, package.description, aliases
                    );
                    println!("    nix attr: {}", package.nix_attr);
                    println!("    binaries: {}", package.binaries.join(", "));
                }
            }
        }
        Commands::Plan { subcommand } => match subcommand {
            PlanSubcommands::Install { pkg } => {
                let report = handle_structured(cli.json, root_core::plan(&adapter, &pkg), |r| {
                    if r.found {
                        let binaries = r.expected_binaries.join(", ");
                        let title = if let Some(ref input) = r.original_input {
                            format!("Install plan for {} → {}", input, r.package)
                        } else {
                            format!("Install plan for {}", r.package)
                        };
                        format!(
                            "{}\n\
                             \n\
                             Overview\n\
                               Package:     {}\n\
                               Nix attr:    {}\n\
                               Binaries:    {}\n\
                               Verify:      {}\n\
                             \n\
                             Steps that will be performed:\n\
                             1. Supported package check\n\
                             2. Resolve Nix metadata (version, store path, derivation)\n\
                             3. Pin nixpkgs revision in root.lock\n\
                             4. Create pre-install snapshot\n\
                             5. Install to Root-managed profile\n\
                             6. Verify profile contains locked store paths\n\
                             7. Update root.lock with deterministic metadata\n\
                             8. Record history event\n\
                             \n\
                             Rollback available: yes (via root rollback --last)\n\
                             \n\
                             This is a preview. No changes have been made.",
                            title,
                            r.package,
                            r.nix_attr,
                            binaries,
                            r.verify_commands.join(", "),
                        )
                    } else {
                        format!(
                            "Package '{}' was not found in nixpkgs.\n\
                             \n\
                             The package name is on the supported list but Nix could not resolve it.\n\
                             This may mean the attribute does not exist on your platform.",
                            r.package
                        )
                    }
                });
                if let Some(report) = report {
                    if !report.found {
                        process::exit(3);
                    }
                }
            }
            PlanSubcommands::Models { name } => {
                let _ = handle_structured(
                    cli.json,
                    root_core::models::plan_models(name.as_deref()),
                    root_core::models::format_plan_models_human,
                );
            }
        },
        Commands::Install { pkg } => {
            if !cli.json {
                println!("Planning install...");
            }
            let _ = handle_structured(cli.json, root_core::install(&adapter, &pkg), |r| {
                let mut msg = format!("Installed {}.", r.package);
                if !r.changed.is_empty() {
                    msg.push_str(&format!("\nChanged: {}.", r.changed.join(", ")));
                }
                if !r.unchanged.is_empty() {
                    msg.push_str(&format!("\nUnchanged: {}.", r.unchanged.join(", ")));
                }
                msg.push_str(&format!("\nSnapshot saved: {}", r.snapshot_id));
                msg.push_str("\nRollback available with: root rollback --last");
                msg
            });
        }
        Commands::List => match root_core::list(&adapter) {
            Ok(output) => {
                if cli.json {
                    print_json(&output);
                } else {
                    println!("Managed Packages:");
                    if output.packages.is_empty() {
                        println!("  (none)");
                    } else {
                        for pkg in &output.packages {
                            println!("  - {} ({})", pkg.name, pkg.version);
                        }
                    }
                    println!("\nNix Profile State:");
                    println!("{}", output.nix_profile);
                }
            }
            Err(e) => {
                let code = exit_code_for_error(&e);
                if cli.json {
                    print_json(&json_error_output(&e));
                } else {
                    eprintln!("Error: {}", format_user_error(&e));
                }
                process::exit(code);
            }
        },
        Commands::Remove { pkg } => {
            let _ = handle_structured(cli.json, root_core::remove(&adapter, &pkg), |r| {
                let mut msg = format!("Removed {}.", r.package);
                msg.push_str(&format!("\nSnapshot saved: {}", r.snapshot_id));
                msg.push_str("\nRollback available with: root rollback --last");
                msg
            });
        }
        Commands::Update { pkg } => {
            let _ = handle_structured(cli.json, root_core::update(&adapter, pkg.as_deref()), |r| {
                let target = r
                    .requested
                    .as_ref()
                    .map(|pkg| format!("{}.", pkg))
                    .unwrap_or_else(|| "all managed packages.".to_string());
                let mut msg = format!("Updated {}", target);
                if !r.updated.is_empty() {
                    msg.push_str(&format!("\nChanged: {}.", r.updated.join(", ")));
                }
                if !r.unchanged.is_empty() {
                    msg.push_str(&format!("\nUnchanged: {}.", r.unchanged.join(", ")));
                }
                if !r.skipped.is_empty() {
                    msg.push_str(&format!("\nSkipped: {}.", r.skipped.join(", ")));
                }
                if let Some(snapshot_id) = &r.snapshot_id {
                    msg.push_str(&format!("\nSnapshot saved: {}", snapshot_id));
                    msg.push_str("\nRollback available with: root rollback --last");
                }
                if !r.warnings.is_empty() {
                    msg.push_str(&format!("\nWarnings: {}.", r.warnings.join("; ")));
                }
                msg
            });
        }
        Commands::History { limit } => match root_core::history_with_limit(limit) {
            Ok(output) => {
                if cli.json {
                    print_json(&output);
                } else if output.snapshots.is_empty() && output.events.is_empty() {
                    println!("No Root-managed snapshots yet.");
                    println!();
                    println!("Run `root catalog` to see supported packages.");
                    println!();
                    println!("Try:");
                    println!("  root install ffmpeg");
                } else {
                    println!("Root history");
                    println!();
                    if !output.snapshots.is_empty() {
                        println!("Snapshots");
                        for snapshot in &output.snapshots {
                            println!("  {}", snapshot.created_at);
                            println!("    snapshot: {}", snapshot.id);
                            println!("    reason: {}", snapshot.reason);
                            println!("    packages: {}", snapshot.package_count);
                            println!("    lock hash: {}", snapshot.lock_content_hash);
                            println!();
                        }
                    }
                    if !output.events.is_empty() {
                        println!("Events");
                    }
                    for event in &output.events {
                        let type_str = format!("{:?}", event.event_type).to_lowercase();
                        println!("  {}", event.timestamp);
                        println!("    event: {}", type_str);
                        println!("    status: {:?}", event.status);
                        if let Some(ref pkg) = event.package {
                            println!("    package: {}", pkg);
                        }
                        if let Some(ref sid) = event.snapshot_id {
                            println!("    snapshot: {}", sid);
                        }
                        if let Some(ref rsid) = event.restored_snapshot_id {
                            println!("    restored: {}", rsid);
                        }
                        if let Some(ref task) = event.task_name {
                            println!("    task: {}", task);
                        }
                        if let Some(exit_code) = event.exit_code {
                            println!("    exit code: {}", exit_code);
                        }
                        if let Some(duration_ms) = event.duration_ms {
                            println!("    duration: {} ms", duration_ms);
                        }
                        if let Some(ref decision) = event.policy_decision {
                            println!("    policy: {}", decision);
                        }
                        println!();
                    }
                }
            }
            Err(e) => {
                let code = exit_code_for_error(&e);
                if cli.json {
                    print_json(&json_error_output(&e));
                } else {
                    eprintln!("Error: {}", format_user_error(&e));
                }
                process::exit(code);
            }
        },
        Commands::Rollback { last } => {
            if last {
                let _ = handle_structured(cli.json, root_core::rollback_last(&adapter), |r| {
                    let mut msg = format!("Rolled back to {}.", r.from_snapshot);
                    if !r.packages_removed.is_empty() {
                        msg.push_str(&format!("\nRemoved: {}", r.packages_removed.join(", ")));
                    }
                    if !r.packages_restored.is_empty() {
                        msg.push_str(&format!("\nRestored: {}", r.packages_restored.join(", ")));
                    }
                    msg
                });
            } else {
                if cli.json {
                    print_json(&GenericOutput {
                        success: false,
                        message: "Currently only `root rollback --last` is supported".into(),
                        raw_stderr: None,
                    });
                } else {
                    eprintln!("Error: Currently only `root rollback --last` is supported");
                }
                process::exit(2);
            }
        }
        Commands::Doctor { check } => match root_core::doctor(&adapter) {
            Ok(report) => {
                if cli.json {
                    print_json(&report);
                } else {
                    println!("Root health check\n");
                    if report.issues.is_empty() {
                        println!("✓ Nix available — Root uses Nix for deterministic builds");
                        println!("✓ Root profile ready");
                        println!("✓ Event ledger writable");
                        println!("✓ No issues detected");
                        println!("\nRoot is ready.");
                        println!("\nNext steps:");
                        println!("  root install ffmpeg    Install your first package");
                        println!("  root history           View snapshot history");
                        println!("  root verify ffmpeg     Verify package binaries");
                        println!("  root rollback --last   Undo the last change");
                        println!("\nRun `root catalog` to see all 42 supported packages.");
                    } else {
                        for issue in &report.issues {
                            let icon = match issue.severity {
                                root_doctor::IssueSeverity::Error => "✗",
                                root_doctor::IssueSeverity::Warning => "△",
                            };
                            println!("{} {}: {}", icon, issue.category, issue.description);
                            println!("  Suggestion: {}", issue.suggestion);
                            println!();
                        }
                        if report.healthy {
                            println!("System health: healthy with warnings");
                        } else {
                            println!("System health: UNHEALTHY — run `root sync` to repair");
                        }
                    }
                }

                if check && !report.issues.is_empty() {
                    process::exit(5);
                }
            }
            Err(e) => {
                let code = exit_code_for_error(&e);
                if cli.json {
                    print_json(&json_error_output(&e));
                } else {
                    eprintln!("Error running doctor: {}", format_user_error(&e));
                }
                process::exit(code);
            }
        },
        Commands::Verify { pkg } => {
            match root_core::verify(&pkg) {
                Ok(report) => {
                    if cli.json {
                        print_json(&report);
                    } else {
                        println!("Verifying binaries for package '{}'...", pkg);
                        println!();

                        for bin_res in &report.binaries {
                            if bin_res.success {
                                println!(
                                    "  🟢 {} : Executable (Exit code {})",
                                    bin_res.binary,
                                    bin_res.exit_code.unwrap_or(0)
                                );
                            } else {
                                println!("  🔴 {} : FAILED to execute", bin_res.binary);
                                if let Some(ref err) = bin_res.error_message {
                                    println!("     Error: {}", err);
                                } else {
                                    println!("     Exit code: {}", bin_res.exit_code.unwrap_or(-1));
                                }
                                if !bin_res.stderr.is_empty() {
                                    println!("     Stderr: {}", bin_res.stderr.trim());
                                }
                            }
                            if let Some(ref resolved_path) = bin_res.resolved_path {
                                println!("     Path: {}", resolved_path);
                            }
                            if !bin_res.attempted_args.is_empty() {
                                println!("     Args: {}", bin_res.attempted_args.join(" "));
                            }
                        }
                        println!();

                        if report.success {
                            println!(
                                "Verification SUCCESS: All binaries for '{}' are fully functional.",
                                pkg
                            );
                        } else {
                            println!("Verification FAILED: One or more binaries failed execution checks.");
                        }
                    }

                    if !report.success {
                        process::exit(4);
                    }
                }
                Err(e) => {
                    let code = exit_code_for_error(&e);
                    if cli.json {
                        print_json(&json_error_output(&e));
                    } else {
                        eprintln!("Error running verification: {}", format_user_error(&e));
                    }
                    process::exit(code);
                }
            }
        }
        Commands::Import { source } => {
            if source != "brew" {
                if cli.json {
                    print_json(&GenericOutput {
                        success: false,
                        message: format!("Unsupported import source: {}", source),
                        raw_stderr: None,
                    });
                } else {
                    eprintln!("Error: Only 'brew' is supported as an import source currently.");
                }
                process::exit(2);
            }

            let cur_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match root_core::import_brew(&cur_dir) {
                Ok(report) => {
                    if cli.json {
                        print_json(&report);
                    } else {
                        println!("Scanning Homebrew environment...");
                        println!();

                        if !report.brew_detected {
                            println!("❌ Homebrew is not detected on this system or not available in PATH.");
                            process::exit(1);
                        }

                        println!("✅ Homebrew detected!");
                        println!("📦 Formulae found (CLI): {}", report.formulae_found);
                        println!("🖥️  Casks found (GUI/ignored): {}", report.casks_found);
                        println!();

                        println!(
                            "Successfully mapped {} formulae to Nixpkgs attributes.",
                            report.formulae_found
                        );
                        println!("Saved candidate import config to: ./Rootfile.import");
                        println!();

                        println!("👉 Next Steps:");
                        println!("   1. Review the generated './Rootfile.import' candidate file.");
                        println!("   2. Rename it to '~/.root/Rootfile' or merge it into your active config.");
                        println!("   3. Run `root install` (without arguments) in the future to reconcile and lock the state.");
                    }
                }
                Err(e) => {
                    let code = exit_code_for_error(&e);
                    if cli.json {
                        print_json(&json_error_output(&e));
                    } else {
                        eprintln!("Error running brew import: {}", format_user_error(&e));
                    }
                    process::exit(code);
                }
            }
        }
        Commands::Lock => {
            let _ = handle_structured(cli.json, root_core::lock(&adapter), |r| {
                let mut msg = String::from("Locked current state.");
                if !r.packages_locked.is_empty() {
                    msg.push_str(&format!(
                        "\nPackages locked: {}.",
                        r.packages_locked.join(", ")
                    ));
                }
                if !r.packages_removed.is_empty() {
                    msg.push_str(&format!(
                        "\nPackages removed from lock: {}.",
                        r.packages_removed.join(", ")
                    ));
                }
                if let Some(sid) = &r.snapshot_id {
                    msg.push_str(&format!("\nSnapshot saved: {}", sid));
                }
                msg
            });
        }
        Commands::Sync => {
            let _ = handle_structured(cli.json, root_core::sync(&adapter), |r| {
                let mut msg = String::from("Synced Nix profile with root.lock.");
                if !r.installed.is_empty() {
                    msg.push_str(&format!("\nInstalled: {}.", r.installed.join(", ")));
                }
                if !r.removed.is_empty() {
                    msg.push_str(&format!("\nRemoved: {}.", r.removed.join(", ")));
                }
                if !r.unchanged.is_empty() {
                    msg.push_str(&format!("\nUnchanged: {}.", r.unchanged.join(", ")));
                }
                msg.push_str(&format!("\nSnapshot saved: {}", r.snapshot_id));
                msg
            });
        }
        Commands::Restore { lock, dry_run } => {
            if dry_run {
                let _ = handle_structured(
                    cli.json,
                    root_core::restore_dry_run(&adapter, lock.as_deref()),
                    |r| {
                        let mut msg = String::from("Restore plan\n");
                        if !r.will_install.is_empty() {
                            msg.push_str(&format!(
                                "\nWill install:\n  {}\n",
                                r.will_install.join("\n  ")
                            ));
                        }
                        if !r.will_remove.is_empty() {
                            msg.push_str(&format!(
                                "\nWill remove:\n  {}\n",
                                r.will_remove.join("\n  ")
                            ));
                        }
                        if !r.will_keep.is_empty() {
                            msg.push_str(&format!(
                                "\nWill keep:\n  {}\n",
                                r.will_keep.join("\n  ")
                            ));
                        }
                        if !r.will_update.is_empty() {
                            msg.push_str(&format!(
                                "\nWill update:\n  {}\n",
                                r.will_update.join("\n  ")
                            ));
                        }
                        if r.will_install.is_empty()
                            && r.will_remove.is_empty()
                            && r.will_update.is_empty()
                        {
                            msg.push_str("\nNo changes needed.");
                        }
                        msg
                    },
                );
            } else {
                let _ = handle_structured(
                    cli.json,
                    root_core::restore(&adapter, lock.as_deref()),
                    |r| {
                        let mut msg = format!("Restored Root profile from {}.", r.lock_path);
                        if !r.installed.is_empty() {
                            msg.push_str(&format!("\nInstalled: {}.", r.installed.join(", ")));
                        }
                        if !r.removed.is_empty() {
                            msg.push_str(&format!("\nRemoved: {}.", r.removed.join(", ")));
                        }
                        if !r.unchanged.is_empty() {
                            msg.push_str(&format!("\nUnchanged: {}.", r.unchanged.join(", ")));
                        }
                        msg.push_str(&format!("\nSnapshot saved: {}", r.snapshot_id));
                        msg
                    },
                );
            }
        }
        Commands::Run { target, command } => {
            let request = if !command.is_empty() {
                if target.is_some() {
                    let error = anyhow::anyhow!(
                        "Choose either a task/workflow or a command after `--`, not both."
                    );
                    let _ =
                        handle_structured::<GenericOutput>(cli.json, Err(error), |_| String::new());
                    unreachable!();
                }
                root_core::RunRequest::Command(command)
            } else if let Some(target) = target {
                let path = PathBuf::from(&target);
                if path.is_file() {
                    root_core::RunRequest::Workflow(path)
                } else {
                    root_core::RunRequest::Task(target)
                }
            } else {
                let error = anyhow::anyhow!(
                    "Provide a Rootfile task, workflow file, or command after `--`."
                );
                let _ = handle_structured::<GenericOutput>(cli.json, Err(error), |_| String::new());
                unreachable!();
            };

            if let Some(report) = handle_structured(cli.json, root_core::run(request), |r| {
                if !r.stdout.is_empty() {
                    print!("{}", r.stdout);
                    if !r.stdout.ends_with('\n') {
                        println!();
                    }
                }
                if !r.stderr.is_empty() {
                    eprint!("{}", r.stderr);
                    if !r.stderr.ends_with('\n') {
                        eprintln!();
                    }
                }
                format!(
                    "Command exited with code {} in {} ms.",
                    r.exit_code, r.duration_ms
                )
            }) {
                if !report.success {
                    process::exit(report.exit_code.max(1));
                }
            }
        }
        Commands::Permissions => {
            let _ = handle_structured(cli.json, root_core::permissions(), |r| {
                format!(
                    "Active policy: {} ({})\n\
                     Packages: install={:?}, update={:?}, remove={:?}, sync={:?}, restore={:?}\n\
                     Execution: run={:?}\n\
                     Sandboxes: create={:?}, run={:?}, destroy={:?}\n\
                     Resources: network={:?}, filesystem={:?}\n\
                     Agent actions: {:?}",
                    r.path,
                    r.source,
                    r.policy.packages.install,
                    r.policy.packages.update,
                    r.policy.packages.remove,
                    r.policy.packages.sync,
                    r.policy.packages.restore,
                    r.policy.execution.run,
                    r.policy.sandboxes.create,
                    r.policy.sandboxes.run,
                    r.policy.sandboxes.destroy,
                    r.policy.resources.network,
                    r.policy.resources.filesystem,
                    r.policy.approvals.agent
                )
            });
        }
        Commands::Policy { subcommand } => match subcommand {
            PolicySubcommands::Apply { file } => {
                let _ = handle_structured(cli.json, root_core::apply_policy(&file), |r| {
                    format!("Applied policy version {} to {}.", r.version, r.path)
                });
            }
        },
        Commands::Status => {
            let _ = handle_structured(cli.json, root_core::status(&adapter), |r| {
                let mut msg = format!("Root Status — Machine: {}\n", r.machine_id);
                msg.push_str(&format!("State: {}\n", r.state));
                msg.push_str(&format!("Hostname: {}\n", r.hostname));
                msg.push_str(&format!("Rootfile packages: {}\n", r.rootfile_packages));
                msg.push_str(&format!("Lockfile packages: {}\n", r.lockfile_packages));
                msg.push_str(&format!("Profile packages: {}\n", r.profile_packages));
                append_inventory_section(&mut msg, "Agents", &r.inventory.agents);
                append_inventory_section(&mut msg, "Models", &r.inventory.models);
                if r.drift_details.is_empty() {
                    msg.push_str("\n✓ No drift detected. All systems aligned.");
                } else {
                    msg.push_str(&format!("\nDrift issues ({}):", r.drift_details.len()));
                    for issue in &r.drift_details {
                        msg.push_str(&format!("\n  - [{}] {}", issue.category, issue.description));
                        msg.push_str(&format!("\n    Suggestion: {}", issue.suggestion));
                    }
                }
                msg
            });
        }
        Commands::Sandbox { subcommand } => match subcommand {
            SandboxSubcommands::Create {
                name,
                image,
                memory,
                cpus,
            } => {
                let _ = handle_structured(
                    cli.json,
                    root_core::sandbox_create(
                        &sandbox_provider,
                        name.as_deref(),
                        image.as_deref(),
                        memory.as_deref(),
                        cpus.as_deref(),
                    ),
                    |r| {
                        let mut msg = format!(
                            "Created sandbox '{}' (id: {})\n  Image: {}\n  State: {}",
                            r.name, r.id, r.image, r.state
                        );
                        if let Some(ref mem) = r.memory {
                            msg.push_str(&format!("\n  Memory: {}", mem));
                        }
                        if let Some(ref cpus) = r.cpus {
                            msg.push_str(&format!("\n  CPUs: {}", cpus));
                        }
                        msg
                    },
                );
            }
            SandboxSubcommands::Run {
                id,
                timeout,
                command,
            } => {
                let cmd_strings: Vec<String> = command
                    .iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect();
                if let Some(report) = handle_structured(
                    cli.json,
                    root_core::sandbox_run(&sandbox_provider, &id, &cmd_strings, timeout),
                    |r| {
                        let mut output = String::new();
                        if !r.stdout.is_empty() {
                            output.push_str(&r.stdout);
                            if !r.stdout.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                        if !r.stderr.is_empty() {
                            output.push_str(&r.stderr);
                            if !r.stderr.ends_with('\n') {
                                output.push('\n');
                            }
                        }
                        if r.timed_out == Some(true) {
                            output.push_str(&format!(
                                "Command timed out (exit code {}).",
                                r.exit_code
                            ));
                        } else {
                            output.push_str(&format!("Command exited with code {}.", r.exit_code));
                        }
                        if r.cleanup_attempted == Some(true) {
                            output.push_str(" Cleanup was attempted.");
                        }
                        output
                    },
                ) {
                    if !report.success {
                        process::exit(report.exit_code.max(1));
                    }
                }
            }
            SandboxSubcommands::List => {
                let _ =
                    handle_structured(cli.json, root_core::sandbox_list(&sandbox_provider), |r| {
                        if r.sandboxes.is_empty() {
                            "No Root-managed sandboxes.".to_string()
                        } else {
                            let mut msg = format!("Root sandboxes ({}):\n", r.sandboxes.len());
                            for sb in &r.sandboxes {
                                msg.push_str(&format!(
                                    "  {} (id: {}) [{:?}] image: {}\n",
                                    sb.name, sb.id, sb.state, sb.image
                                ));
                            }
                            msg
                        }
                    });
            }
            SandboxSubcommands::Destroy { id } => {
                let _ = handle_structured(
                    cli.json,
                    root_core::sandbox_destroy(&sandbox_provider, &id),
                    |r| format!("Destroyed sandbox '{}'.", r.id),
                );
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_phase_one_commands() {
        let search = Cli::try_parse_from(["root", "search", "rg", "--json"]).unwrap();
        assert!(search.json);
        match search.command {
            Commands::Search { query } => assert_eq!(query, "rg"),
            other => panic!("expected search command, got {:?}", other),
        }

        let update_one = Cli::try_parse_from(["root", "update", "ripgrep"]).unwrap();
        match update_one.command {
            Commands::Update { pkg } => assert_eq!(pkg.as_deref(), Some("ripgrep")),
            other => panic!("expected update command, got {:?}", other),
        }

        let update_all = Cli::try_parse_from(["root", "update"]).unwrap();
        match update_all.command {
            Commands::Update { pkg } => assert_eq!(pkg, None),
            other => panic!("expected update command, got {:?}", other),
        }

        let restore = Cli::try_parse_from(["root", "restore", "--lock", "./root.lock"]).unwrap();
        match restore.command {
            Commands::Restore { lock, dry_run } => {
                assert_eq!(lock.unwrap(), std::path::PathBuf::from("./root.lock"));
                assert!(!dry_run);
            }
            other => panic!("expected restore command, got {:?}", other),
        }

        let restore_dry =
            Cli::try_parse_from(["root", "restore", "--lock", "./root.lock", "--dry-run"]).unwrap();
        match restore_dry.command {
            Commands::Restore { lock, dry_run } => {
                assert_eq!(lock.unwrap(), std::path::PathBuf::from("./root.lock"));
                assert!(dry_run);
            }
            other => panic!("expected restore command, got {:?}", other),
        }

        let run_task = Cli::try_parse_from(["root", "run", "build"]).unwrap();
        match run_task.command {
            Commands::Run { target, command } => {
                assert_eq!(target.as_deref(), Some("build"));
                assert!(command.is_empty());
            }
            other => panic!("expected run command, got {:?}", other),
        }

        let run_command = Cli::try_parse_from(["root", "run", "--", "cargo", "test"]).unwrap();
        match run_command.command {
            Commands::Run { target, command } => {
                assert!(target.is_none());
                assert_eq!(
                    command,
                    vec![OsString::from("cargo"), OsString::from("test")]
                );
            }
            other => panic!("expected run command, got {:?}", other),
        }

        let apply = Cli::try_parse_from(["root", "policy", "apply", "policy.toml"]).unwrap();
        match apply.command {
            Commands::Policy {
                subcommand: PolicySubcommands::Apply { file },
            } => assert_eq!(file, PathBuf::from("policy.toml")),
            other => panic!("expected policy apply command, got {:?}", other),
        }

        let sb_create = Cli::try_parse_from(["root", "sandbox", "create", "test-sb"]).unwrap();
        match sb_create.command {
            Commands::Sandbox {
                subcommand: SandboxSubcommands::Create { name, image, .. },
            } => {
                assert_eq!(name.as_deref(), Some("test-sb"));
                assert!(image.is_none());
            }
            other => panic!("expected sandbox create, got {:?}", other),
        }

        let sb_create_with_resources = Cli::try_parse_from([
            "root",
            "sandbox",
            "create",
            "test-sb-2",
            "--memory",
            "4g",
            "--cpus",
            "4.0",
        ])
        .unwrap();
        match sb_create_with_resources.command {
            Commands::Sandbox {
                subcommand:
                    SandboxSubcommands::Create {
                        name,
                        image,
                        memory,
                        cpus,
                    },
            } => {
                assert_eq!(name.as_deref(), Some("test-sb-2"));
                assert!(image.is_none());
                assert_eq!(memory.as_deref(), Some("4g"));
                assert_eq!(cpus.as_deref(), Some("4.0"));
            }
            other => panic!("expected sandbox create with resources, got {:?}", other),
        }

        let sb_run =
            Cli::try_parse_from(["root", "sandbox", "run", "my-sb", "--", "echo", "hi"]).unwrap();
        match sb_run.command {
            Commands::Sandbox {
                subcommand: SandboxSubcommands::Run { id, command, .. },
            } => {
                assert_eq!(id, "my-sb");
                assert_eq!(command.len(), 2);
            }
            other => panic!("expected sandbox run, got {:?}", other),
        }

        let sb_run_with_timeout = Cli::try_parse_from([
            "root",
            "sandbox",
            "run",
            "my-sb",
            "--timeout",
            "30",
            "--",
            "sleep",
            "10",
        ])
        .unwrap();
        match sb_run_with_timeout.command {
            Commands::Sandbox {
                subcommand:
                    SandboxSubcommands::Run {
                        id,
                        timeout,
                        command,
                    },
            } => {
                assert_eq!(id, "my-sb");
                assert_eq!(timeout, Some(30));
                assert_eq!(command.len(), 2);
            }
            other => panic!("expected sandbox run with timeout, got {:?}", other),
        }

        let sb_list = Cli::try_parse_from(["root", "sandbox", "list"]).unwrap();
        match sb_list.command {
            Commands::Sandbox {
                subcommand: SandboxSubcommands::List,
            } => {}
            other => panic!("expected sandbox list, got {:?}", other),
        }

        let sb_destroy = Cli::try_parse_from(["root", "sandbox", "destroy", "my-sb"]).unwrap();
        match sb_destroy.command {
            Commands::Sandbox {
                subcommand: SandboxSubcommands::Destroy { id },
            } => assert_eq!(id, "my-sb"),
            other => panic!("expected sandbox destroy, got {:?}", other),
        }

        let status = Cli::try_parse_from(["root", "status"]).unwrap();
        match status.command {
            Commands::Status => {}
            other => panic!("expected status, got {:?}", other),
        }
    }

    #[test]
    fn inventory_human_lines_include_locked_digest_overlay() {
        let item = root_core::inventory::InventoryItem {
            name: "qwen3:8b".into(),
            kind: root_core::inventory::ResourceKind::Model,
            desired: "ollama".into(),
            observation: root_core::inventory::Presence::Present,
            evaluation: root_core::inventory::EvaluationState::Drifted,
            observed_version: None,
            observed_digest: Some("C".repeat(64)),
            evidence_source: root_core::inventory::EvidenceSource::OllamaApiTags,
            reason: None,
            locked_digest: Some(format!("sha256:{}", "a".repeat(64))),
            digest_match: Some(false),
        };
        let mut msg = String::new();
        append_inventory_section(&mut msg, "Models", &[item]);
        assert!(msg.contains(&"C".repeat(64)), "raw observed digest: {msg}");
        assert!(
            msg.contains(&format!("locked sha256:{}", "a".repeat(64))),
            "locked digest: {msg}"
        );
        assert!(msg.contains("digest mismatch"), "mismatch label: {msg}");
        assert!(msg.contains("drifted"), "evaluation: {msg}");
    }
}
