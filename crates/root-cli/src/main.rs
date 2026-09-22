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
    /// Restore the Root profile from a lockfile, then bind durable work state
    ///
    /// Ordering: the deterministic environment (Rootfile, root.lock, Nix
    /// profile) is restored first; the durable work state is then bound
    /// (read-only) and reported. A genuinely absent workspace is reported as
    /// unavailable; a malformed or stale project pointer fails closed so a
    /// divergent workspace is never bound by accident. `--rebind` repairs the
    /// pointer only after environment restoration succeeds; under `--dry-run`
    /// the repair is only reported, never written.
    Restore {
        /// Lockfile to restore from. Defaults to ~/.root/root.lock
        #[arg(long, value_name = "PATH")]
        lock: Option<std::path::PathBuf>,
        /// Show the restore plan and work bind without mutating anything
        #[arg(long)]
        dry_run: bool,
        /// Repair the project pointer from the Root index (dry-run reports the proposal without writing; a real restore writes it only after env restore succeeds)
        #[arg(long)]
        rebind: bool,
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
    /// Pull and verify declared Ollama models
    Models {
        #[command(subcommand)]
        subcommand: ModelsSubcommands,
    },
    /// Transfer Codex or OpenCode working configuration between machines (explicit bundles only)
    #[command(name = "agent-bundle")]
    AgentBundle {
        #[command(subcommand)]
        subcommand: AgentBundleSubcommands,
    },
    /// Inspect canonical agent environment and cross-harness plan/diff (read-only, no writes)
    #[command(name = "agent")]
    Agent {
        #[command(subcommand)]
        subcommand: AgentSubcommands,
    },
    /// Initialize and inspect the durable Root workspace for this repository
    Workspace {
        #[command(subcommand)]
        subcommand: WorkspaceSubcommands,
    },
    /// Set or show the active work goal
    Goal {
        #[command(subcommand)]
        subcommand: GoalSubcommands,
    },
    /// Record and inspect durable engineering decisions
    Decision {
        #[command(subcommand)]
        subcommand: DecisionSubcommands,
    },
    /// Record and inspect evidence-backed findings
    Finding {
        #[command(subcommand)]
        subcommand: FindingSubcommands,
    },
    /// Record and inspect artifact references
    Artifact {
        #[command(subcommand)]
        subcommand: ArtifactSubcommands,
    },
    /// Create and inspect durable continuation checkpoints
    Checkpoint {
        #[command(subcommand)]
        subcommand: CheckpointSubcommands,
    },
    /// Produce a continuation package from the most recent checkpoint
    Resume {
        /// Resume from a specific checkpoint ID instead of the latest
        #[arg(long, value_name = "ID")]
        checkpoint: Option<String>,
        /// Assemble a harness-aware package for an agent. Without a value,
        /// resolves the repo Rootfile [agents].default_target.
        #[arg(long, value_name = "AGENT", num_args = 0..=1)]
        with: Option<Option<String>>,
    },
    /// Produce a portable handoff package for another agent or human
    Handoff {
        /// Target adapter id. Without a value, resolves the repo Rootfile
        /// [agents].default_target.
        #[arg(long, value_name = "AGENT", num_args = 0..=1)]
        to: Option<Option<String>>,
    },
    /// Report durable state after an interruption and what Root can continue from
    Recover,
    /// Serve and inspect the local MCP interface
    Mcp {
        #[command(subcommand)]
        subcommand: McpSubcommands,
    },
    /// Inspect supported coding-agent adapters
    Adapters {
        #[command(subcommand)]
        subcommand: AdaptersSubcommands,
    },
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
enum ModelsSubcommands {
    /// Pull missing declared models and write a v3 verification record
    Pull {
        #[arg(value_name = "NAME")]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum AgentBundleSubcommands {
    /// Read-only inspection of the local Codex or OpenCode configuration (no writes)
    Inspect {
        /// Agent id (S3 supports `codex`, `opencode`, and `claude`)
        #[arg(long, default_value = "codex")]
        agent: String,
    },
    /// Export a versioned bundle directory (manifest.json + blobs/)
    Export {
        /// Agent id (S3 supports `codex`, `opencode`, and `claude`)
        #[arg(long, default_value = "codex")]
        agent: String,
        /// Output bundle directory (must not exist)
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Shared skill to include (repeatable)
        #[arg(long, value_name = "SKILL")]
        skill: Vec<String>,
        /// MCP server id to include as a disabled declaration (repeatable)
        #[arg(long, value_name = "ID")]
        include_mcp: Vec<String>,
        /// Skill or MCP id whose executable content may be included (repeatable)
        #[arg(long, value_name = "ID")]
        include_executable: Vec<String>,
        /// Omit timestamp for deterministic bundles
        #[arg(long)]
        no_timestamp: bool,
    },
    /// Show apply plan with target preconditions and plan hash (read-only)
    Plan {
        /// Bundle directory
        #[arg(long, value_name = "DIR")]
        bundle: PathBuf,
    },
    /// Apply a bundle (requires --apply, --plan-hash, and per-item --approve hashes)
    Apply {
        /// Bundle directory
        #[arg(long, value_name = "DIR")]
        bundle: PathBuf,
        /// Confirm mutation (plan is default-safe; this flag is required to write)
        #[arg(long)]
        apply: bool,
        /// Plan hash from a current `plan` output
        #[arg(long, value_name = "HASH")]
        plan_hash: String,
        /// Exact sha256 approval per executable item (repeatable; no global approval)
        #[arg(long, value_name = "SHA256")]
        approve: Vec<String>,
    },
    /// Post-apply verification (read-only, secret-safe)
    Verify {
        /// Agent id (S3 supports `codex`, `opencode`, and `claude`)
        #[arg(long, default_value = "codex")]
        agent: String,
    },
    /// Roll back the most recent agent-bundle snapshot
    Rollback {
        #[arg(long)]
        last: bool,
    },
    /// Preview enabling an MCP server (read-only enable plan + descriptor hash)
    EnablePlan {
        /// Agent id (S3 supports `codex`, `opencode`, and `claude`)
        #[arg(long, default_value = "codex")]
        agent: String,
        /// MCP server id
        #[arg(long, value_name = "ID")]
        server: String,
    },
    /// Enable a previously applied (disabled) MCP server (protected mutation)
    Enable {
        /// Agent id (S3 supports `codex`, `opencode`, and `claude`)
        #[arg(long, default_value = "codex")]
        agent: String,
        /// MCP server id
        #[arg(long, value_name = "ID")]
        server: String,
        /// Plan hash from a current `enable-plan` output
        #[arg(long, value_name = "HASH")]
        plan_hash: String,
        /// Exact descriptor sha256 approval (no global approval)
        #[arg(long, value_name = "SHA256")]
        approve: Vec<String>,
    },
    /// Delete agent snapshots (requires --yes)
    Purge {
        /// Snapshot id (default: all)
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        /// Explicit confirmation (required)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AgentSubcommands {
    /// Read-only inspection of the canonical agent environment (no writes)
    Inspect {
        /// Agent id (codex, opencode, or claude)
        #[arg(value_name = "AGENT")]
        agent: String,
    },
    /// Read-only translation plan from one harness to another (no writes)
    Plan {
        /// Source agent id (defaults to the loaded --env's source_agent)
        #[arg(long, value_name = "AGENT")]
        from: Option<String>,
        /// Target agent id (defaults to the repo Rootfile [agents].default_target)
        #[arg(long, value_name = "AGENT")]
        to: Option<String>,
        /// Canonical environment JSON/TOML file (instead of live --from state)
        #[arg(long, value_name = "FILE")]
        env: Option<PathBuf>,
    },
    /// Read-only symmetric diff between two harnesses (no writes)
    Diff {
        /// First agent id
        #[arg(value_name = "AGENT_A")]
        a: String,
        /// Second agent id
        #[arg(value_name = "AGENT_B")]
        b: String,
    },
    /// Plan and apply a canonical agent environment to a harness.
    ///
    /// Without --apply this is a read-only preflight (exit 2, no writes).
    /// With --apply --plan-hash <hash> --approve <sha256>... it mutates the
    /// target harness. MCP servers always apply disabled; enable them
    /// afterwards with the existing per-harness `root agent-bundle enable`
    /// flow (no canonical `enable` command exists).
    Apply {
        /// Target agent id (defaults to the repo Rootfile [agents].default_target)
        #[arg(long, value_name = "AGENT")]
        to: Option<String>,
        /// Plan hash from a current preflight output (required with --apply)
        #[arg(long, value_name = "HASH")]
        plan_hash: Option<String>,
        /// Exact sha256 approval per MCP/executable item (repeatable; no global approval)
        #[arg(long, value_name = "SHA256")]
        approve: Vec<String>,
        /// Confirm mutation (preflight is default-safe; this flag is required to write)
        #[arg(long)]
        apply: bool,
        /// Canonical environment file (.json or .toml; else repo .root/agent.toml discovery)
        #[arg(long, value_name = "FILE")]
        env: Option<PathBuf>,
    },
    /// Post-apply verification for one harness (read-only, secret-safe)
    Verify {
        /// Agent id (codex, opencode, or claude)
        #[arg(long, value_name = "AGENT")]
        agent: String,
    },
    /// Propose or write a checked-in canonical agent.toml for a repo
    Capture {
        /// Source agent id (codex, opencode, or claude)
        #[arg(long, value_name = "AGENT")]
        from: String,
        /// Output agent.toml path (required with --apply; defaults inside .root/)
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Confirm write (proposal is default-safe; this flag is required to write)
        #[arg(long)]
        apply: bool,
        /// Overwrite an existing agent.toml
        #[arg(long)]
        force: bool,
        /// Allow writing outside a .root/ directory
        #[arg(long)]
        allow_outside_repo: bool,
    },
    /// Roll back the most recent agent snapshot
    Rollback {
        /// Roll back the most recent snapshot (required)
        #[arg(long)]
        last: bool,
    },
    /// Delete agent snapshots (requires --yes; --id XOR --all)
    Purge {
        /// Snapshot id to delete (mutually exclusive with --all)
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        /// Delete all agent snapshots (mutually exclusive with --id)
        #[arg(long)]
        all: bool,
        /// Explicit confirmation (required; no deletion without it)
        #[arg(long)]
        yes: bool,
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

#[derive(Subcommand, Debug)]
enum WorkspaceSubcommands {
    /// Initialize a durable Root workspace inside the current Git repository
    Init {
        /// Also write the opt-in project pointer <repo>/.root/workspace.json
        /// (workspace id + Root directory hint only; no work data, no secrets)
        #[arg(long)]
        write_pointer: bool,
    },
    /// Show workspace identity, active goal, and work counts
    Status,
    /// Export this workspace's recorded work state to a portable document
    Export {
        /// Export the state as of a specific checkpoint ID
        #[arg(long, value_name = "ID")]
        checkpoint: Option<String>,
        /// Output workspace transfer document
        #[arg(long, short = 'o', value_name = "PATH")]
        out: PathBuf,
        /// Overwrite an existing output file
        #[arg(long)]
        force: bool,
    },
    /// Import a workspace transfer document into a fresh Root directory
    Import {
        /// Workspace transfer document
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Project directory to bind (defaults to the current directory)
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
        /// Also write the opt-in project pointer in the project
        #[arg(long)]
        write_pointer: bool,
    },
}

#[derive(Subcommand, Debug)]
enum GoalSubcommands {
    /// Set the active goal (supersedes the previous goal without deleting it)
    Set {
        #[arg(value_name = "GOAL")]
        goal: String,
    },
    /// Show the active goal
    Show,
}

#[derive(Subcommand, Debug)]
enum DecisionSubcommands {
    /// Record a durable decision
    Add {
        #[arg(value_name = "STATEMENT")]
        statement: String,
        /// Optional rationale for the decision
        #[arg(long, value_name = "TEXT")]
        rationale: Option<String>,
    },
    /// List recorded decisions
    List,
    /// Show a decision by ID
    Show {
        #[arg(value_name = "ID")]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum FindingSubcommands {
    /// Record an evidence-backed finding
    Add {
        #[arg(value_name = "STATEMENT")]
        statement: String,
        /// Optional evidence reference (path, command, or note)
        #[arg(long, value_name = "REF")]
        evidence: Option<String>,
    },
    /// List recorded findings
    List,
    /// Show a finding by ID
    Show {
        #[arg(value_name = "ID")]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum ArtifactSubcommands {
    /// Record a reference to an existing file artifact
    Add {
        #[arg(value_name = "PATH")]
        path: String,
    },
    /// List recorded artifacts
    List,
}

#[derive(Subcommand, Debug)]
enum CheckpointSubcommands {
    /// Create an immutable checkpoint of current work, Git, and environment state
    Create {
        /// Optional continuation message for the checkpoint
        #[arg(long, value_name = "MESSAGE")]
        message: Option<String>,
    },
    /// List checkpoints
    List,
    /// Show a checkpoint by ID, or the most recent with --last
    Show {
        #[arg(value_name = "ID")]
        id: Option<String>,
        /// Show the most recent checkpoint
        #[arg(long)]
        last: bool,
    },
}

#[derive(Subcommand, Debug)]
enum McpSubcommands {
    /// Serve MCP over stdio (newline-delimited JSON-RPC 2.0)
    Serve,
    /// Show MCP workspace, capabilities, and exposed tools
    Status,
}

#[derive(Subcommand, Debug)]
enum AdaptersSubcommands {
    /// List supported adapters and their local detection status
    List,
    /// Inspect one adapter's compatibility, MCP configuration, and instructions
    Inspect {
        /// Adapter id (codex or claude)
        #[arg(long, value_name = "AGENT")]
        agent: String,
    },
}

#[derive(Serialize)]
struct GenericOutput {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_stderr: Option<String>,
}

#[derive(Serialize)]
struct AdaptersListReport {
    adapters: Vec<root_adapters::CompatibilityReport>,
}

#[derive(Serialize)]
struct AdapterInspectReport {
    detection: root_adapters::AdapterDetection,
    compatibility: root_adapters::CompatibilityReport,
    mcp_config: root_adapters::McpConfig,
    instructions: String,
}

#[derive(Serialize)]
struct AgentInspectOutput {
    agent: String,
    present: bool,
    version: Option<String>,
    version_supported: bool,
    warnings: Vec<String>,
    report: serde_json::Value,
    canonical: root_agent_bundle::canonical::CanonicalEnv,
    mutated: bool,
}

fn agent_exit_code(e: &anyhow::Error) -> i32 {
    let dbg = format!("{:?}", e);
    if dbg.contains("unsupported bundle adapter") || dbg.contains("invalid canonical env") {
        return 2;
    }
    match exit_code_for_error(e) {
        4 | 5 | 6 | 0 => 1,
        c => c,
    }
}

fn handle_agent_structured<T: Serialize>(
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
            let code = agent_exit_code(&e);
            if json {
                print_json(&json_error_output(&e));
            } else {
                eprintln!("Error: {}", format_user_error(&e));
            }
            process::exit(code);
        }
    }
}

fn agent_inspect_report(agent: &str) -> anyhow::Result<AgentInspectOutput> {
    let norm = root_agent_bundle::canonical::canonical_adapter_id(agent)?;
    let canonical = root_agent_bundle::translate::build_canonical_from_inspect(norm)?;
    let (present, version, report) = match norm {
        root_agent_bundle::manifest::ADAPTER_ID => {
            let r = root_agent_bundle::codex::inspect()?;
            let v = serde_json::to_value(&r).unwrap_or(serde_json::Value::Null);
            (r.present, r.version, v)
        }
        root_agent_bundle::manifest::OPENCODE_ADAPTER_ID => {
            let r = root_agent_bundle::opencode::inspect()?;
            let v = serde_json::to_value(&r).unwrap_or(serde_json::Value::Null);
            (r.present, r.version, v)
        }
        root_agent_bundle::manifest::CLAUDE_ADAPTER_ID => {
            let r = root_agent_bundle::claude::inspect()?;
            let v = serde_json::to_value(&r).unwrap_or(serde_json::Value::Null);
            (r.present, r.version, v)
        }
        other => {
            return Err(root_agent_bundle::manifest::unsupported_adapter_error(
                other,
            ))
        }
    };
    let version_supported = match &version {
        Some(v) => root_agent_bundle::canonical::version_supported(norm, v),
        None => false,
    };
    let mut warnings = Vec::new();
    if present {
        match &version {
            Some(v) if !version_supported => {
                let supported = root_agent_bundle::manifest::supported_versions_for(norm)
                    .map(|s| s.join(", "))
                    .unwrap_or_default();
                warnings.push(format!(
                    "unsupported version '{}'; supported versions: [{}], live: {}",
                    v, supported, v
                ));
            }
            None => warnings.push("version probe failed; version_supported=false".to_string()),
            _ => {}
        }
    }
    Ok(AgentInspectOutput {
        agent: norm.to_string(),
        present,
        version,
        version_supported,
        warnings,
        report,
        canonical,
        mutated: false,
    })
}

fn report_str(report: &serde_json::Value, key: &str) -> Option<String> {
    report
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn report_bool(report: &serde_json::Value, key: &str) -> Option<bool> {
    report.get(key).and_then(|v| v.as_bool())
}

fn report_str_list(report: &serde_json::Value, key: &str) -> Vec<String> {
    report
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn format_agent_inspect(r: &AgentInspectOutput) -> String {
    let label = match r.agent.as_str() {
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "claude" => "Claude",
        other => other,
    };
    let mut msg = format!("{} present: {}\n", label, r.present);
    msg.push_str(&format!(
        "Version: {}\n",
        r.version.as_deref().unwrap_or("(absent)")
    ));
    msg.push_str(&format!("Supported: {}\n", r.version_supported));
    msg.push_str(&format!(
        "Instructions: {}\n",
        r.canonical.instructions.main
    ));
    let skills: Vec<&str> = r.canonical.skills.iter().map(|s| s.name.as_str()).collect();
    msg.push_str(&format!(
        "Skills: {}\n",
        if skills.is_empty() {
            "(none)".to_string()
        } else {
            skills.join(", ")
        }
    ));
    let mut mcp: Vec<&str> = r.canonical.mcp_servers.keys().map(|s| s.as_str()).collect();
    mcp.sort();
    msg.push_str(&format!(
        "MCP servers: {}\n",
        if mcp.is_empty() {
            "(none)".to_string()
        } else {
            mcp.join(", ")
        }
    ));
    // Adapter-native report fields (mirrored verbatim under JSON `report`).
    if let Some(path) =
        report_str(&r.report, "codex_home").or_else(|| report_str(&r.report, "config_dir"))
    {
        msg.push_str(&format!("Config path: {}\n", path));
    }
    if let Some(dir) = report_str(&r.report, "global_state_dir") {
        msg.push_str(&format!("Global state dir: {}\n", dir));
    }
    let mut presence: Vec<String> = Vec::new();
    for key in [
        "config_present",
        "agents_md_present",
        "settings_present",
        "claude_md_present",
    ] {
        if let Some(b) = report_bool(&r.report, key) {
            presence.push(format!("{}: {}", key, b));
        }
    }
    if !presence.is_empty() {
        msg.push_str(&format!("Config presence: {}\n", presence.join(", ")));
    }
    let adapter_mcp = report_str_list(&r.report, "mcp_servers");
    msg.push_str(&format!(
        "Adapter MCP servers: {}\n",
        if adapter_mcp.is_empty() {
            "(none)".to_string()
        } else {
            adapter_mcp.join(", ")
        }
    ));
    if let Some(held) = r.report.get("held").and_then(|v| v.as_array()) {
        if held.is_empty() {
            msg.push_str("Adapter held: (none)\n");
        } else {
            msg.push_str(&format!("Adapter held ({})\n", held.len()));
            for h in held {
                let source = h.get("source").and_then(|s| s.as_str()).unwrap_or("?");
                let reason = h.get("reason").and_then(|s| s.as_str()).unwrap_or("");
                if reason.is_empty() {
                    msg.push_str(&format!("  {}\n", source));
                } else {
                    msg.push_str(&format!("  {} ({})\n", source, reason));
                }
            }
        }
    }
    if !r.warnings.is_empty() {
        msg.push_str("\nWarnings\n");
        for w in &r.warnings {
            msg.push_str(&format!("  {}\n", w));
        }
    }
    if !r.canonical.policies.is_empty() {
        msg.push_str("\nHeld\n");
        for p in &r.canonical.policies {
            msg.push_str(&format!("  {} ({})\n", p.key, p.disposition));
        }
    }
    msg.push_str(&format!(
        "\n{}\n",
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    ));
    msg.push_str("No changes made.");
    msg
}

fn format_agent_plan(r: &root_agent_bundle::translate::TranslationPlan) -> String {
    let mut msg = format!("Agent plan {} -> {} (dry-run, no writes)\n", r.from, r.to);
    msg.push_str(&format!("\nPlan hash: {}\n", r.plan_hash));
    msg.push_str(&format!("\nPortable ({})\n", r.portable.len()));
    for item in &r.portable {
        msg.push_str(&format!("  {}\n", item));
    }
    msg.push_str(&format!(
        "\nRequires review ({})\n",
        r.requires_review.len()
    ));
    for item in &r.requires_review {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!("\nUnsupported ({})\n", r.unsupported.len()));
    for item in &r.unsupported {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!(
        "\nSecrets required ({})\n",
        r.secrets_required.len()
    ));
    if r.secrets_required.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for s in &r.secrets_required {
            msg.push_str(&format!("  {}\n", s));
        }
    }
    msg.push_str(&format!("\nNeeds approval ({})\n", r.needs_approval.len()));
    if r.needs_approval.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for a in &r.needs_approval {
            msg.push_str(&format!("  {} {} ({})\n", a.sha256, a.target, a.reason));
        }
    }
    msg.push_str(&format!("\nHeld ({})\n", r.held.len()));
    for h in &r.held {
        msg.push_str(&format!("  {} ({})\n", h.source, h.reason));
    }
    if !r.warnings.is_empty() {
        msg.push_str(&format!("\nWarnings ({})\n", r.warnings.len()));
        for w in &r.warnings {
            msg.push_str(&format!("  {}\n", w));
        }
    }
    msg.push_str(&format!(
        "\n{}\n",
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    ));
    msg.push_str("No changes made.");
    msg
}

fn format_agent_diff(r: &root_agent_bundle::translate::TranslationDiff) -> String {
    let mut msg = format!("Agent diff {} <-> {} (dry-run, no writes)\n", r.a, r.b);
    msg.push_str(&format!("\nPortable ({})\n", r.portable.len()));
    for item in &r.portable {
        msg.push_str(&format!("  {}\n", item));
    }
    msg.push_str(&format!(
        "\nRequires review ({})\n",
        r.requires_review.len()
    ));
    for item in &r.requires_review {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!("\nUnsupported ({})\n", r.unsupported.len()));
    for item in &r.unsupported {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!(
        "\nSecrets required ({})\n",
        r.secrets_required.len()
    ));
    if r.secrets_required.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for s in &r.secrets_required {
            msg.push_str(&format!("  {}\n", s));
        }
    }
    msg.push_str(&format!("\nHeld ({})\n", r.held.len()));
    for h in &r.held {
        msg.push_str(&format!("  {} ({})\n", h.source, h.reason));
    }
    if !r.warnings.is_empty() {
        msg.push_str(&format!("\nWarnings ({})\n", r.warnings.len()));
        for w in &r.warnings {
            msg.push_str(&format!("  {}\n", w));
        }
    }
    msg.push_str(&format!(
        "\n{}\n",
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    ));
    msg.push_str("No changes made.");
    msg
}

fn load_env_explicit(
    path: &std::path::Path,
) -> anyhow::Result<root_agent_bundle::canonical::CanonicalEnv> {
    let ext_is_toml = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("toml"))
        .unwrap_or(false);
    if ext_is_toml {
        let meta = std::fs::symlink_metadata(path).map_err(|e| {
            anyhow::anyhow!(
                "invalid canonical env: cannot stat {}: {}",
                path.display(),
                e
            )
        })?;
        if meta.file_type().is_symlink() {
            anyhow::bail!("invalid canonical env: symlinks are rejected");
        }
        if !meta.is_file() {
            anyhow::bail!("invalid canonical env: path must be a regular file");
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("invalid canonical env: failed to read file: {}", e))?;
        root_agent_bundle::project::parse_agent_toml(&text)
    } else {
        root_agent_bundle::canonical::load_canonical_file(path)
    }
}

fn resolve_agent_env(
    explicit: Option<&std::path::Path>,
) -> anyhow::Result<(PathBuf, root_agent_bundle::canonical::CanonicalEnv)> {
    if let Some(p) = explicit {
        let env = load_env_explicit(p)?;
        return Ok((p.to_path_buf(), env));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    root_agent_bundle::project::resolve_env_source(None, &cwd)
}

/// Resolve the target harness id: explicit `--to` (normalized), else the repo
/// Rootfile `[agents].default_target`, else an error naming supported targets.
fn resolve_agent_target(to: Option<&str>) -> anyhow::Result<String> {
    match to {
        Some(t) => Ok(root_agent_bundle::canonical::canonical_adapter_id(t)?.to_string()),
        None => {
            let cwd = current_dir();
            match root_agent_bundle::project::resolve_default_target(&cwd)? {
                Some(t) => Ok(t),
                None => anyhow::bail!(
                    "no target agent available: pass --to <codex|opencode|claude> or set [agents].default_target in the repo Rootfile"
                ),
            }
        }
    }
}

/// Invalid CLI arguments for `root agent` exit 2 (JSON-aware). Used for
/// argument-resolution failures that have no library error sentinel.
fn fail_agent_args(cli_json: bool, e: &anyhow::Error) -> ! {
    if cli_json {
        print_json(&json_error_output(e));
    } else {
        eprintln!("Error: {}", format_user_error(e));
    }
    process::exit(2);
}

/// Resolve an optional-value target flag (Sprint 013). `Some(value)` uses the
/// explicit value; `None` (flag present without a value) resolves the repo
/// Rootfile `[agents].default_target` and fails closed when none is set.
fn resolve_optional_target(
    explicit: Option<String>,
    cwd: &std::path::Path,
    flag: &str,
) -> anyhow::Result<String> {
    match explicit {
        Some(target) => Ok(target),
        None => match root_agent_bundle::project::resolve_default_target(cwd)? {
            Some(target) => Ok(target),
            None => anyhow::bail!(
                "no {flag} target available: set [agents].default_target in the repo Rootfile \
                 or pass {flag} <codex|opencode|claude>"
            ),
        },
    }
}

fn format_agent_apply_preflight(
    plan: &root_agent_bundle::canonical_apply::CanonicalApplyPlan,
    translation: &root_agent_bundle::translate::TranslationPlan,
) -> String {
    let mut msg = format!(
        "Agent apply {} -> {} (dry-run, no writes)\n",
        translation.from, plan.to
    );
    msg.push_str(&format!("\nPlan hash: {}\n", plan.plan_hash));
    msg.push_str(&format!("\nPortable ({})\n", translation.portable.len()));
    for item in &translation.portable {
        msg.push_str(&format!("  {}\n", item));
    }
    msg.push_str(&format!(
        "\nRequires review ({})\n",
        translation.requires_review.len()
    ));
    for item in &translation.requires_review {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!(
        "\nUnsupported ({})\n",
        translation.unsupported.len()
    ));
    for item in &translation.unsupported {
        msg.push_str(&format!("  {}: {}\n", item.item, item.reason));
    }
    msg.push_str(&format!(
        "\nSecrets required ({})\n",
        translation.secrets_required.len()
    ));
    if translation.secrets_required.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for s in &translation.secrets_required {
            msg.push_str(&format!("  {}\n", s));
        }
    }
    if !plan.will_create.is_empty() {
        msg.push_str(&format!(
            "\nWill create:\n  {}\n",
            plan.will_create.join("\n  ")
        ));
    }
    if !plan.will_update.is_empty() {
        msg.push_str(&format!(
            "\nWill update:\n  {}\n",
            plan.will_update.join("\n  ")
        ));
    }
    if !plan.will_keep.is_empty() {
        msg.push_str(&format!(
            "\nWill keep:\n  {}\n",
            plan.will_keep.join("\n  ")
        ));
    }
    if plan.will_create.is_empty() && plan.will_update.is_empty() && plan.will_keep.is_empty() {
        msg.push_str("\nNo changes needed.\n");
    }
    if !plan.needs_env.is_empty() {
        msg.push_str(&format!(
            "\nNeeds env on target: {}\n",
            plan.needs_env.join(", ")
        ));
    }
    msg.push_str(&format!(
        "\nNeeds approval ({})\n",
        plan.needs_approval.len()
    ));
    if plan.needs_approval.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for a in &plan.needs_approval {
            msg.push_str(&format!("  {} {} ({})\n", a.sha256, a.target, a.reason));
        }
    }
    msg.push_str(&format!("\nHeld ({})\n", plan.held.len()));
    for h in &plan.held {
        msg.push_str(&format!("  {} ({})\n", h.source, h.reason));
    }
    if plan
        .held
        .iter()
        .any(|h| h.reason.contains("absent on source"))
    {
        msg.push_str(
            "\nNote: source content is not available on this machine; apply will refuse (Sprint 012 is same-machine). Cross-machine transfer is Sprint 013.\n",
        );
    }
    let mut warnings = translation.warnings.clone();
    for w in &plan.warnings {
        if !warnings.contains(w) {
            warnings.push(w.clone());
        }
    }
    if !warnings.is_empty() {
        msg.push_str(&format!("\nWarnings ({})\n", warnings.len()));
        for w in &warnings {
            msg.push_str(&format!("  {}\n", w));
        }
    }
    msg.push_str(&format!(
        "\n{}\n",
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    ));
    msg.push_str(
        "MCP servers apply disabled; enable afterwards with `root agent-bundle enable --agent ",
    );
    msg.push_str(&plan.to);
    msg.push_str(" --server <id>` after providing secrets.\n");
    msg.push_str(&format!(
        "Plan only: no writes performed. Re-run with --apply --plan-hash {} to mutate.",
        plan.plan_hash
    ));
    msg
}

fn format_agent_apply_report(r: &root_agent_bundle::apply::ApplyReport) -> String {
    let mut msg = format!(
        "Applied canonical environment.\nSnapshot: {}\n",
        r.snapshot_id
    );
    msg.push_str(&format!(
        "Applied: {}\n",
        if r.applied.is_empty() {
            "(none)".to_string()
        } else {
            r.applied.join(", ")
        }
    ));
    msg.push_str(&format!(
        "Skipped identical: {}\n",
        if r.skipped_identical.is_empty() {
            "(none)".to_string()
        } else {
            r.skipped_identical.join(", ")
        }
    ));
    msg.push_str(&format!(
        "MCP imported (disabled): {}\n",
        if r.mcp_imported.is_empty() {
            "(none)".to_string()
        } else {
            r.mcp_imported.join(", ")
        }
    ));
    msg.push_str(&format!("Plan hash: {}\n", r.plan_hash));
    msg.push_str("Rollback available with: root agent rollback --last\n");
    msg.push_str("MCP servers apply disabled; enable afterwards with the existing per-harness `root agent-bundle enable` flow after providing secrets.");
    msg
}

fn format_agent_verify_report(r: &root_agent_bundle::verify::VerifyReport) -> String {
    let label = match r.agent.as_str() {
        "codex" => "Codex verification",
        "opencode" => "OpenCode verification",
        "claude" => "Claude verification",
        other => other,
    };
    let mut msg = format!("{}\n", label);
    for c in &r.checks {
        msg.push_str(&format!(
            "  {} {}: {}\n",
            if c.passed { "✓" } else { "✗" },
            c.name,
            c.detail
        ));
    }
    if r.success {
        msg.push_str("Verification passed.");
    } else {
        msg.push_str("Verification failed.");
    }
    msg
}

fn format_capture_proposal(r: &root_agent_bundle::capture::CaptureProposal) -> String {
    let mut msg = format!("Capture proposal from {} (dry-run, no writes)\n", r.from);
    msg.push_str(&format!(
        "Version: {}\nSupported: {}\n",
        r.version.as_deref().unwrap_or("(unknown)"),
        r.supported
    ));
    msg.push_str(&format!("\nSkills ({})\n", r.skills.len()));
    if r.skills.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for s in &r.skills {
            msg.push_str(&format!("  {}\n", s));
        }
    }
    msg.push_str(&format!("\nMCP servers ({})\n", r.mcp_servers.len()));
    if r.mcp_servers.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for s in &r.mcp_servers {
            msg.push_str(&format!("  {}\n", s));
        }
    }
    msg.push_str("\nInstructions\n");
    match &r.instructions {
        Some(i) => msg.push_str(&format!(
            "  role={} size={} sha256={}\n",
            i.role, i.size, i.sha256
        )),
        None => msg.push_str("  (absent)\n"),
    }
    msg.push_str(&format!("\nEnv vars ({})\n", r.env_vars.len()));
    if r.env_vars.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for e in &r.env_vars {
            msg.push_str(&format!("  {}\n", e));
        }
    }
    msg.push_str(&format!("\nPolicies ({})\n", r.policies.len()));
    if r.policies.is_empty() {
        msg.push_str("  (none)\n");
    } else {
        for p in &r.policies {
            msg.push_str(&format!("  {}\n", p));
        }
    }
    msg.push_str(&format!("\nHeld ({})\n", r.held.len()));
    for h in &r.held {
        msg.push_str(&format!("  {} ({})\n", h.source, h.reason));
    }
    if !r.warnings.is_empty() {
        msg.push_str(&format!("\nWarnings ({})\n", r.warnings.len()));
        for w in &r.warnings {
            msg.push_str(&format!("  {}\n", w));
        }
    }
    msg.push_str(&format!(
        "\n{}\n",
        root_agent_bundle::manifest::SECRET_DISCLOSURE
    ));
    msg.push_str("No changes made. Re-run with --apply --out <file> to write.");
    msg
}

fn format_capture_applied(
    out: &std::path::Path,
    env: &root_agent_bundle::canonical::CanonicalEnv,
) -> String {
    format!(
        "Wrote canonical agent environment to {}.\nSource: {} {}\nNames only (no secret values); review before committing.",
        out.display(),
        env.source_agent,
        env.source_agent_version
    )
}

fn adapters_list() -> anyhow::Result<AdaptersListReport> {
    let mut adapters = Vec::new();
    for id in root_adapters::list() {
        adapters.push(root_adapters::compatibility(id)?);
    }
    Ok(AdaptersListReport { adapters })
}

fn adapter_inspect(agent: &str) -> anyhow::Result<AdapterInspectReport> {
    Ok(AdapterInspectReport {
        detection: root_adapters::detect(agent)?,
        compatibility: root_adapters::compatibility(agent)?,
        mcp_config: root_adapters::mcp_config(agent)?,
        instructions: root_adapters::root_instructions(agent)?,
    })
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

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn format_workspace_init(r: &root_work::WorkspaceInitReport, write_pointer: bool) -> String {
    let mut msg = format!(
        "Root workspace initialized.\n\nWorkspace\n  {}\n  {}\n\nRepository\n  {}\n\nState\n  {}",
        r.workspace.name, r.workspace.id, r.workspace.repo_path, r.database,
    );
    if write_pointer {
        msg.push_str(
            "\n\nPointer\n  .root/workspace.json written (workspace id + Root directory hint only)",
        );
    }
    msg
}

fn format_workspace_status(r: &root_work::WorkspaceStatusReport) -> String {
    let mut msg = format!(
        "Workspace\n  {}\n  {}\n\nRepository\n  {}\n  {}\n",
        r.workspace.name,
        r.workspace.id,
        r.repository.path,
        r.repository.branch.as_deref().unwrap_or("(unknown branch)")
    );
    match &r.goal {
        Some(goal) => msg.push_str(&format!("\nGoal\n  {}\n", goal.statement)),
        None => msg.push_str("\nGoal\n  (none)\n"),
    }
    msg.push_str(&format!(
        "\nWork\n  Decisions   {}\n  Findings    {}\n  Artifacts   {}\n",
        r.counts.decisions, r.counts.findings, r.counts.artifacts
    ));
    if let Some(activity) = &r.last_activity {
        msg.push_str(&format!(
            "\nLast activity\n  {} {}\n",
            activity.description, activity.age_human
        ));
    }
    msg
}

fn format_workspace_export(r: &root_work::ExportReport) -> String {
    format!(
        "Workspace exported.\n\nDocument\n  {}\n  {} bytes\n  payload sha256: {}\n\nWorkspace\n  {}\n\nIncluded\n  Goals        {}\n  Decisions    {}\n  Findings     {}\n  Artifacts    {}\n  Checkpoints  {}\n  Events       {}\n",
        r.path,
        r.bytes,
        r.payload_sha256,
        r.workspace_id,
        r.goals,
        r.decisions,
        r.findings,
        r.artifacts,
        r.checkpoints,
        r.events
    )
}

fn format_workspace_import(r: &root_work::ImportReport, write_pointer: bool) -> String {
    let mut msg = format!(
        "Workspace imported.\n\nWorkspace\n  {} ({})\n\nImported\n  Goals        {}\n  Decisions    {}\n  Findings     {}\n  Artifacts    {}\n  Checkpoints  {}\n  Events       {}\n",
        r.workspace_name,
        r.workspace_id,
        r.goals,
        r.decisions,
        r.findings,
        r.artifacts,
        r.checkpoints,
        r.events
    );
    match &r.latest_checkpoint_id {
        Some(id) => msg.push_str(&format!("\nLatest checkpoint\n  {id}\n")),
        None => msg.push_str("\nLatest checkpoint\n  (none)\n"),
    }
    if write_pointer {
        msg.push_str(
            "\nPointer\n  .root/workspace.json written (workspace id + Root directory hint only)\n",
        );
    }
    msg
}

fn format_goal(r: &root_work::GoalReport) -> String {
    format!(
        "Goal\n  {}\n  {}\n  status: {}\n  created: {}",
        r.goal.id, r.goal.statement, r.goal.status, r.goal.created_at
    )
}

fn format_decision(r: &root_work::DecisionRecord) -> String {
    let mut msg = format!(
        "Decision\n  {}\n  {}\n  status: {}\n  created: {}",
        r.id, r.statement, r.status, r.created_at
    );
    if let Some(rationale) = &r.rationale {
        msg.push_str(&format!("\n  rationale: {}", rationale));
    }
    msg
}

fn format_decision_list(r: &root_work::DecisionListReport) -> String {
    if r.decisions.is_empty() {
        return "No decisions recorded.".to_string();
    }
    let mut msg = format!("Decisions ({})\n", r.decisions.len());
    for decision in &r.decisions {
        msg.push_str(&format!(
            "  {}  [{}]  {}\n",
            decision.id, decision.status, decision.statement
        ));
    }
    msg
}

fn format_finding(r: &root_work::FindingRecord) -> String {
    let mut msg = format!(
        "Finding\n  {}\n  {}\n  status: {}\n  created: {}",
        r.id, r.statement, r.status, r.created_at
    );
    if let Some(evidence) = &r.evidence_ref {
        msg.push_str(&format!("\n  evidence: {}", evidence));
    }
    msg
}

fn format_finding_list(r: &root_work::FindingListReport) -> String {
    if r.findings.is_empty() {
        return "No findings recorded.".to_string();
    }
    let mut msg = format!("Findings ({})\n", r.findings.len());
    for finding in &r.findings {
        msg.push_str(&format!(
            "  {}  [{}]  {}\n",
            finding.id, finding.status, finding.statement
        ));
    }
    msg
}

fn format_artifact(r: &root_work::ArtifactRecord) -> String {
    let mut msg = format!(
        "Artifact\n  {}\n  {} ({})\n  created: {}",
        r.id, r.uri, r.kind, r.created_at
    );
    if let Some(fingerprint) = &r.fingerprint {
        msg.push_str(&format!("\n  fingerprint: {}", fingerprint));
    }
    msg
}

fn format_artifact_list(r: &root_work::ArtifactListReport) -> String {
    if r.artifacts.is_empty() {
        return "No artifacts recorded.".to_string();
    }
    let mut msg = format!("Artifacts ({})\n", r.artifacts.len());
    for artifact in &r.artifacts {
        msg.push_str(&format!(
            "  {}  {} ({})\n",
            artifact.id, artifact.uri, artifact.kind
        ));
    }
    msg
}

fn format_checkpoint(checkpoint: &root_work::CheckpointRecord) -> String {
    let mut msg = format!("Checkpoint\n  {}\n", checkpoint.id);
    if let Some(message) = &checkpoint.message {
        msg.push_str(&format!("  message: {}\n", message));
    }
    msg.push_str(&format!(
        "  work revision: {}\n  environment: {}\n  created: {}\n",
        checkpoint.work_revision, checkpoint.environment_status, checkpoint.created_at
    ));

    let branch = checkpoint.git_branch.as_deref().unwrap_or("(no branch)");
    let head = checkpoint
        .git_head
        .as_deref()
        .map(|sha| sha.chars().take(7).collect::<String>())
        .unwrap_or_else(|| "(no commits)".to_string());
    let dirty = if checkpoint.git_dirty_fingerprint.is_none() {
        "unknown"
    } else if checkpoint.git_dirty {
        "dirty"
    } else {
        "clean"
    };
    msg.push_str(&format!(
        "\nRepository\n  {} @ {}\n  {}\n",
        branch, head, dirty
    ));

    if !checkpoint.continuation_summary.is_empty() {
        msg.push_str("\nContinuation\n");
        for line in checkpoint.continuation_summary.lines() {
            msg.push_str(&format!("  {}\n", line));
        }
    }
    if let Some(raw) = &checkpoint.agent_env_ref {
        match parse_agent_env_summary(raw) {
            Some(summary) if !summary.summary_is_empty() => {
                msg.push_str(&format_agent_environment(&summary));
            }
            Some(_) => {}
            None => msg.push_str("\nAgent environment\n  (unreadable agent environment summary)\n"),
        }
    }
    msg
}

/// Parse the names-only `AgentEnvSummary` JSON stored on a checkpoint. Secret
/// values are never present in this projection, so nothing sensitive is read.
fn parse_agent_env_summary(raw: &str) -> Option<root_continuity::AgentEnvSummary> {
    serde_json::from_str::<root_continuity::AgentEnvSummary>(raw).ok()
}

fn format_agent_environment(summary: &root_continuity::AgentEnvSummary) -> String {
    let mut msg = String::from("\nAgent environment\n");
    match (&summary.adapter, &summary.source_agent_version) {
        (Some(adapter), Some(version)) => {
            msg.push_str(&format!("  adapter: {adapter} ({version})\n"))
        }
        (Some(adapter), None) => msg.push_str(&format!("  adapter: {adapter}\n")),
        _ => msg.push_str("  adapter: (none)\n"),
    }
    if let Some(instructions) = &summary.instructions {
        msg.push_str(&format!("  instructions: {instructions}\n"));
    }
    if !summary.skills.is_empty() {
        msg.push_str(&format!("  skills: {}\n", summary.skills.join(", ")));
    }
    if !summary.mcp_servers.is_empty() {
        msg.push_str(&format!(
            "  mcp servers: {}\n",
            summary.mcp_servers.join(", ")
        ));
    }
    if !summary.credential_refs.is_empty() {
        msg.push_str(&format!(
            "  credential refs: {} (names only)\n",
            summary.credential_refs.join(", ")
        ));
    }
    if !summary.policies.is_empty() {
        msg.push_str(&format!("  policies: {}\n", summary.policies.join(", ")));
    }
    msg
}

fn format_checkpoint_list(r: &root_work::CheckpointListReport) -> String {
    if r.checkpoints.is_empty() {
        return "No checkpoints recorded.".to_string();
    }
    let mut msg = format!("Checkpoints ({})\n", r.checkpoints.len());
    for checkpoint in &r.checkpoints {
        let dirty = if checkpoint.git_dirty {
            "dirty"
        } else {
            "clean"
        };
        msg.push_str(&format!(
            "  {}  rev {}  {}  {}",
            checkpoint.id, checkpoint.work_revision, dirty, checkpoint.created_at
        ));
        if let Some(agent) = &checkpoint.provenance_agent {
            msg.push_str(&format!("  from={agent}"));
        }
        let environment_adapter = checkpoint
            .agent_env_ref
            .as_deref()
            .and_then(parse_agent_env_summary)
            .and_then(|summary| summary.adapter);
        if let Some(adapter) = environment_adapter {
            msg.push_str(&format!("  env={adapter}"));
        }
        msg.push('\n');
        if let Some(message) = &checkpoint.message {
            msg.push_str(&format!("    {}\n", message));
        }
    }
    msg
}

fn environment_status_label(status: &str) -> &'static str {
    match status {
        root_work::model::ENV_VERIFIED => "Root environment verified.",
        root_work::model::ENV_OBSERVED => {
            "Root environment observed (Rootfile and root.lock present)."
        }
        root_work::model::ENV_UNKNOWN => "Root environment partially observed.",
        root_work::model::ENV_MISSING => "No Root environment declared.",
        _ => "Root environment status unknown.",
    }
}

fn format_resume(r: &root_continuity::ResumeReport) -> String {
    let mut msg = String::from("Root Resume\n");

    msg.push_str("\nGoal\n");
    match &r.goal {
        Some(goal) => msg.push_str(&format!("  {}\n", goal.statement)),
        None => msg.push_str("  (none)\n"),
    }

    msg.push_str("\nCheckpoint\n");
    msg.push_str(&format!("  {}\n", r.checkpoint.id));
    if let Some(message) = &r.checkpoint.message {
        msg.push_str(&format!("  {}\n", message));
    }

    msg.push_str("\nCurrent state\n");
    msg.push_str(&format!("  {}\n", r.current_state.summary));

    if !r.findings.is_empty() {
        msg.push_str("\nFindings\n");
        for finding in &r.findings {
            msg.push_str(&format!("  {}\n", finding.statement));
        }
        if r.findings_omitted > 0 {
            msg.push_str(&format!(
                "  ({} older findings omitted)\n",
                r.findings_omitted
            ));
        }
    }

    if !r.decisions.is_empty() {
        msg.push_str("\nDecisions\n");
        for decision in &r.decisions {
            msg.push_str(&format!("  {}\n", decision.statement));
        }
        if r.decisions_omitted > 0 {
            msg.push_str(&format!(
                "  ({} older decisions omitted)\n",
                r.decisions_omitted
            ));
        }
    }

    if !r.artifacts.is_empty() {
        msg.push_str("\nRelevant artifacts\n");
        for artifact in &r.artifacts {
            msg.push_str(&format!("  {}\n", artifact.uri));
        }
        if r.artifacts_omitted > 0 {
            msg.push_str(&format!(
                "  ({} older artifacts omitted)\n",
                r.artifacts_omitted
            ));
        }
    }

    msg.push_str("\nEnvironment\n");
    msg.push_str(&format!(
        "  {}\n",
        environment_status_label(&r.environment_state.status)
    ));

    msg.push_str("\nDrift\n");
    if r.drift.items.is_empty() {
        msg.push_str("  None detected.\n");
    } else {
        for item in &r.drift.items {
            msg.push_str(&format!("  [{}] {}\n", item.level, item.detail));
        }
    }

    msg.push_str("\nSuggested continuation\n");
    for suggestion in &r.suggested_continuation {
        msg.push_str(&format!("  Suggestion (not verified): {}\n", suggestion));
    }

    msg
}

#[derive(serde::Deserialize)]
struct ResumePackageWorkspaceView {
    id: String,
    name: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageGoalView {
    statement: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageCheckpointView {
    id: String,
    #[serde(default)]
    message: Option<String>,
}

#[derive(serde::Deserialize)]
struct ResumePackageStatementView {
    statement: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageArtifactView {
    uri: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageEnvironmentView {
    status: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageDriftItemView {
    level: String,
    detail: String,
}

#[derive(serde::Deserialize)]
struct ResumePackageDriftView {
    #[serde(default)]
    items: Vec<ResumePackageDriftItemView>,
}

#[derive(serde::Deserialize)]
struct ResumePackageView {
    #[serde(default)]
    workspace: Option<ResumePackageWorkspaceView>,
    #[serde(default)]
    goal: Option<ResumePackageGoalView>,
    checkpoint: ResumePackageCheckpointView,
    #[serde(default)]
    decisions: Vec<ResumePackageStatementView>,
    #[serde(default)]
    decisions_omitted: usize,
    #[serde(default)]
    findings: Vec<ResumePackageStatementView>,
    #[serde(default)]
    findings_omitted: usize,
    #[serde(default)]
    artifacts: Vec<ResumePackageArtifactView>,
    #[serde(default)]
    artifacts_omitted: usize,
    environment_state: ResumePackageEnvironmentView,
    drift: ResumePackageDriftView,
    #[serde(default)]
    suggested_continuation: Vec<String>,
}

fn resume_target_label(target: &str) -> String {
    match target {
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        "opencode" => "OpenCode".to_string(),
        other => other.to_string(),
    }
}

/// Render the harness-aware `resume --with` package: the seven observable
/// steps followed by the same continuation sections as `format_resume`, plus
/// the target `Instructions` and `Next` commands.
fn format_resume_with(report: &root_continuity::ResumeWithReport) -> String {
    let label = resume_target_label(&report.target);
    let mut msg = format!("Root Resume --with {label}\n");
    let view = serde_json::from_value::<ResumePackageView>(report.package.clone()).ok();
    if let Some(view) = &view {
        match &view.workspace {
            Some(workspace) => msg.push_str(&format!(
                "Workspace {} ({}) · checkpoint {}\n",
                workspace.name, workspace.id, view.checkpoint.id
            )),
            None => msg.push_str(&format!("Checkpoint {}\n", view.checkpoint.id)),
        }
    }

    msg.push_str("\nSteps\n");
    for (index, step) in report.steps.iter().enumerate() {
        let status = if step.ok { "ok" } else { "failed" };
        msg.push_str(&format!(
            "  [{}/{}] {:<24} {} ({})\n",
            index + 1,
            report.steps.len(),
            step.name,
            status,
            step.detail
        ));
    }

    if !report.mapping_available {
        let reason =
            report.warnings.first().cloned().unwrap_or_else(|| {
                "agent environment was not captured at this checkpoint".to_string()
            });
        msg.push_str(&format!("\nAgent mapping: unavailable — {reason}\n"));
    }

    if let Some(view) = &view {
        msg.push_str("\nGoal\n");
        match &view.goal {
            Some(goal) => msg.push_str(&format!("  {}\n", goal.statement)),
            None => msg.push_str("  (none)\n"),
        }

        msg.push_str("\nCheckpoint\n");
        msg.push_str(&format!("  {}\n", view.checkpoint.id));
        if let Some(message) = &view.checkpoint.message {
            msg.push_str(&format!("  {message}\n"));
        }

        if !view.decisions.is_empty() {
            msg.push_str("\nDecisions\n");
            for decision in &view.decisions {
                msg.push_str(&format!("  {}\n", decision.statement));
            }
            if view.decisions_omitted > 0 {
                msg.push_str(&format!(
                    "  ({} older decisions omitted)\n",
                    view.decisions_omitted
                ));
            }
        }

        if !view.findings.is_empty() {
            msg.push_str("\nFindings\n");
            for finding in &view.findings {
                msg.push_str(&format!("  {}\n", finding.statement));
            }
            if view.findings_omitted > 0 {
                msg.push_str(&format!(
                    "  ({} older findings omitted)\n",
                    view.findings_omitted
                ));
            }
        }

        if !view.artifacts.is_empty() {
            msg.push_str("\nRelevant artifacts\n");
            for artifact in &view.artifacts {
                msg.push_str(&format!("  {}\n", artifact.uri));
            }
            if view.artifacts_omitted > 0 {
                msg.push_str(&format!(
                    "  ({} older artifacts omitted)\n",
                    view.artifacts_omitted
                ));
            }
        }

        msg.push_str("\nEnvironment\n");
        msg.push_str(&format!(
            "  {}\n",
            environment_status_label(&view.environment_state.status)
        ));

        msg.push_str("\nDrift\n");
        if view.drift.items.is_empty() {
            msg.push_str("  None detected.\n");
        } else {
            for item in &view.drift.items {
                msg.push_str(&format!("  [{}] {}\n", item.level, item.detail));
            }
        }

        msg.push_str("\nSuggested continuation\n");
        for suggestion in &view.suggested_continuation {
            msg.push_str(&format!("  {suggestion}\n"));
        }
    }

    if !report.instructions.is_empty() {
        msg.push_str(&format!("\nInstructions ({label})\n"));
        for line in &report.instructions {
            msg.push_str(&format!("  {line}\n"));
        }
    }

    if !report.next.is_empty() {
        msg.push_str("\nNext\n");
        for (index, command) in report.next.iter().enumerate() {
            msg.push_str(&format!("  {}. {command}\n", index + 1));
        }
    }

    if !report.warnings.is_empty() {
        msg.push_str("\nWarnings\n");
        for warning in &report.warnings {
            msg.push_str(&format!("  {warning}\n"));
        }
    }

    msg
}

#[derive(Serialize)]
struct RestorePlanWithBind {
    #[serde(flatten)]
    restore: root_core::RestorePlanReport,
    work_bind: root_continuity::WorkBindReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    rebind: Option<RebindReport>,
}

#[derive(Serialize)]
struct RestoreWithBind {
    #[serde(flatten)]
    restore: root_core::RestoreReport,
    work_bind: root_continuity::WorkBindReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    rebind: Option<RebindReport>,
}

/// Proposed or applied project-pointer rebind, reported by `root restore
/// --rebind`.
///
/// Under `--dry-run` this is a pure in-memory plan: `applied` stays false and
/// the pointer file is never written. A real restore sets `applied` only after
/// environment restoration has succeeded, so a failed environment restore
/// leaves the pointer exactly as it was.
#[derive(Debug, Clone, Serialize)]
struct RebindReport {
    /// Project pointer file this (re)bind targets (`<repo>/.root/workspace.json`).
    pointer_path: String,
    /// Pointer state observed before any write: `absent`, `malformed`,
    /// `stale` (workspace id unknown to the Root index), `divergent` (points
    /// at a different known workspace), or `bound`.
    current_state: String,
    /// Workspace id currently readable from the pointer, when it parses.
    current_workspace_id: Option<String>,
    /// Workspace id the pointer is (or would be) rebound to.
    workspace_id: String,
    /// Whether the pointer file was written. Always false under `--dry-run`.
    applied: bool,
}

/// Plan the `--rebind` pointer repair read-only: resolve the target workspace
/// from the Root index and classify the current project pointer. Writes
/// nothing, so it is safe under `--dry-run` and can run before environment
/// restoration.
///
/// No known workspace is a hard error raised before any mutation: the operator
/// must initialize one explicitly.
fn plan_rebind(cwd: &std::path::Path) -> anyhow::Result<RebindReport> {
    let repository = root_work::Repository::discover(cwd)?;
    let root_dir = root_lockfile::get_root_dir()?;
    let index = root_work::registry::WorkIndex::load(&root_dir)?;
    let repo_path = repository.root.display().to_string();
    let entry = index
        .find_by_identity(&repository.identity())
        .or_else(|| index.find_by_path(&repo_path))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No known workspace for this repository; run `root workspace init --write-pointer`"
            )
        })?;
    let (current_state, current_workspace_id) = match root_work::pointer::load(&repository.root) {
        Err(_) => ("malformed".to_string(), None),
        Ok(None) => ("absent".to_string(), None),
        Ok(Some(pointer)) => {
            let state = if pointer.workspace_id == entry.id {
                "bound"
            } else if index
                .workspaces
                .iter()
                .any(|e| e.id == pointer.workspace_id)
            {
                "divergent"
            } else {
                "stale"
            };
            (state.to_string(), Some(pointer.workspace_id))
        }
    };
    Ok(RebindReport {
        pointer_path: root_work::paths::workspace_pointer_path(&repository.root)
            .display()
            .to_string(),
        current_state,
        current_workspace_id,
        workspace_id: entry.id.clone(),
        applied: false,
    })
}

/// Persist a previously planned rebind by writing the project pointer.
///
/// Called only after environment restoration has succeeded, so a failed
/// environment restore leaves the pointer unchanged (still valid-or-invalid as
/// it was, never newly rewritten).
fn persist_rebind(plan: &RebindReport, cwd: &std::path::Path) -> anyhow::Result<RebindReport> {
    let repository = root_work::Repository::discover(cwd)?;
    let root_dir = root_lockfile::get_root_dir()?;
    let mut report = plan.clone();
    report.workspace_id = match root_continuity::rebind_workspace(&root_dir, &repository)? {
        Some(id) => id,
        None => anyhow::bail!(
            "No known workspace for this repository; run `root workspace init --write-pointer`"
        ),
    };
    report.applied = true;
    Ok(report)
}

/// Work bind for `restore --dry-run`: inspect normally, except when `--rebind`
/// was requested and the pointer is invalid — exactly the state the repair
/// targets. In that case report the pending repair instead of failing closed on
/// the state the operator already asked to fix, still without writing anything.
/// Without `--rebind`, invalid pointers keep failing closed (exit 2).
fn dry_run_work_bind(
    cwd: &std::path::Path,
    rebind: Option<&RebindReport>,
) -> anyhow::Result<root_continuity::WorkBindReport> {
    if let Some(plan) = rebind {
        let repository = root_work::Repository::discover(cwd)?;
        let root_dir = root_lockfile::get_root_dir()?;
        if let root_work::WorkspaceLookup::PointerInvalid(_) =
            root_work::lookup_workspace(&root_dir, &repository)?
        {
            return Ok(root_continuity::WorkBindReport {
                workspace_id: None,
                workspace_name: None,
                latest_checkpoint_id: None,
                work_revision: None,
                available: false,
                drift: None,
                notes: vec![format!(
                    "project pointer is {}; a real `root restore --rebind` would repair it to {} (dry-run does not write)",
                    plan.current_state, plan.workspace_id
                )],
            });
        }
    }
    root_continuity::bind_workspace(cwd)
}

fn append_rebind_note(msg: &mut String, rebind: &Option<RebindReport>) {
    let Some(rebind) = rebind else {
        return;
    };
    if rebind.applied {
        msg.push_str(&format!(
            "\nRebind\n  project pointer repaired for workspace {} (was {})\n",
            rebind.workspace_id, rebind.current_state
        ));
    } else {
        msg.push_str(&format!(
            "\nRebind (proposed, not written)\n  current pointer: {} ({})\n  proposed target: {}\n  change: rewrite the pointer to the proposed target\n",
            rebind.current_state, rebind.pointer_path, rebind.workspace_id
        ));
    }
}

fn format_work_bind(bind: &root_continuity::WorkBindReport) -> String {
    let mut msg = String::from("\nWork state\n");
    match (&bind.workspace_id, &bind.workspace_name) {
        (Some(id), Some(name)) => msg.push_str(&format!("  workspace {name} ({id})\n")),
        (Some(id), None) => msg.push_str(&format!("  workspace {id}\n")),
        _ => msg.push_str("  workspace (none)\n"),
    }
    match &bind.latest_checkpoint_id {
        Some(id) => {
            let revision = bind
                .work_revision
                .map(|revision| format!(" (revision {revision})"))
                .unwrap_or_default();
            msg.push_str(&format!("  latest checkpoint {id}{revision}\n"));
        }
        None => msg.push_str("  latest checkpoint (none)\n"),
    }
    if let Some(revision) = bind.work_revision {
        msg.push_str(&format!("  work revision: {revision}\n"));
    }
    msg.push_str(&format!("  available: {}\n", bind.available));
    if let Some(drift) = &bind.drift {
        if drift.items.is_empty() {
            msg.push_str("  drift: none\n");
        } else {
            for item in &drift.items {
                msg.push_str(&format!("  drift [{}] {}\n", item.level, item.detail));
            }
        }
    }
    for note in &bind.notes {
        msg.push_str(&format!("  note: {note}\n"));
    }
    msg
}

fn format_restore_plan_with_bind(report: &RestorePlanWithBind) -> String {
    let r = &report.restore;
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
        msg.push_str(&format!("\nWill keep:\n  {}\n", r.will_keep.join("\n  ")));
    }
    if !r.will_update.is_empty() {
        msg.push_str(&format!(
            "\nWill update:\n  {}\n",
            r.will_update.join("\n  ")
        ));
    }
    if r.will_install.is_empty() && r.will_remove.is_empty() && r.will_update.is_empty() {
        msg.push_str("\nNo changes needed.");
    }
    if r.models_restored.is_some() {
        msg.push_str("\nModels will not be pulled, restored, or deleted. Ollama weights will be left unchanged.");
    }
    append_rebind_note(&mut msg, &report.rebind);
    msg.push_str(&format_work_bind(&report.work_bind));
    msg
}

fn format_restore_with_bind(report: &RestoreWithBind) -> String {
    let r = &report.restore;
    let mut msg = format!("Restored Root profile from {}.", r.lock_path);
    if r.models_restored.is_some() {
        msg.push_str("\nModels were not restored. Any model lock entries were copied from the lockfile; Ollama weights were left unchanged.");
    }
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
    append_rebind_note(&mut msg, &report.rebind);
    msg.push_str(&format_work_bind(&report.work_bind));
    msg
}

fn format_recover(r: &root_continuity::RecoverReport) -> String {
    root_continuity::render_recover(r)
}

fn format_mcp_status(r: &root_mcp::McpStatusReport) -> String {
    let mut msg = String::from("Root MCP\n");
    match &r.workspace {
        Some(workspace) => {
            msg.push_str(&format!(
                "\nWorkspace\n  {} ({})\n",
                workspace.name, workspace.id
            ));
        }
        None => msg.push_str("\nWorkspace\n  (none)\n"),
    }
    msg.push_str("\nCapabilities\n");
    msg.push_str(&format!("  read: {}\n", r.capabilities.read));
    msg.push_str(&format!("  record: {}\n", r.capabilities.record));
    msg.push_str(&format!("  checkpoint: {}\n", r.capabilities.checkpoint));
    msg.push_str(&format!(
        "  environment_verify: {}\n",
        r.capabilities.environment_verify
    ));
    msg.push_str(&format!("\nPolicy source: {}\n", r.policy_source));
    msg.push_str(&format!("Protocol: {}\n", r.protocol_version));
    msg.push_str(&format!("\nTools ({})\n", r.tools.len()));
    for tool in &r.tools {
        msg.push_str(&format!("  {}\n", tool));
    }
    msg
}

fn format_adapters_list(r: &AdaptersListReport) -> String {
    let mut msg = String::from("Adapters\n");
    for adapter in &r.adapters {
        let presence = if adapter.present {
            "installed"
        } else {
            "not installed"
        };
        msg.push_str(&format!(
            "\n{}\n  {}\n  supported: {}\n",
            root_adapters::display_name(&adapter.adapter),
            presence,
            adapter.supported
        ));
        if let Some(version) = &adapter.version {
            msg.push_str(&format!("  version: {}\n", version));
        }
        for warning in &adapter.warnings {
            msg.push_str(&format!("  warning: {}\n", warning));
        }
    }
    msg
}

fn format_adapter_inspect(r: &AdapterInspectReport) -> String {
    let mut msg = format!(
        "Adapter\n  {}\n  installed: {}\n  supported: {}\n",
        root_adapters::display_name(&r.compatibility.adapter),
        r.detection.present,
        r.compatibility.supported
    );
    if let Some(version) = &r.detection.version {
        msg.push_str(&format!("  version: {}\n", version));
    }
    msg.push_str(&format!("\nMCP configuration ({})\n", r.mcp_config.format));
    msg.push_str(&format!("{}\n", r.mcp_config.snippet));
    msg.push_str(&format!("\nInstructions\n{}\n", r.instructions));
    if !r.compatibility.warnings.is_empty() {
        msg.push_str("\nWarnings\n");
        for warning in &r.compatibility.warnings {
            msg.push_str(&format!("  {}\n", warning));
        }
    }
    msg
}

fn exit_code_for_error(e: &anyhow::Error) -> i32 {
    if let Some(model_err) = e.downcast_ref::<root_core::models::ModelError>() {
        return model_err.exit_code();
    }
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
    } else if msg.contains("Rollback failed") {
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
        || msg.contains("unsupported bundle adapter")
        || msg.contains("Choose either a task/workflow")
        || msg.contains("Provide a Rootfile task")
        || msg.contains("is not declared in Rootfile")
        || msg.contains("Missing hash-bound approval")
        || msg.contains("Unknown approval hash")
        || msg.contains("unsupported target agent version")
        || msg.contains("unsupported source agent version")
        || msg.contains("MCP is held")
        || msg.contains("invalid canonical env")
        || msg.contains("no canonical environment found")
        || msg.contains("refusing to write agent environment")
        || msg.contains("refusing to overwrite existing agent environment")
        || msg.contains("outside repo")
        || msg.contains("without --force")
        || msg.contains("--out is required")
        || msg.contains("Currently only `root agent rollback --last` is supported")
        || msg.contains("Currently only `root rollback --last` is supported")
        || msg.contains("--id and --all are mutually exclusive")
        || msg.contains("requires one of --id or --all")
        || msg.contains("Unsupported --with target")
        || msg.contains("no --with target available")
        || msg.contains("no --to target available")
        || msg.contains("Unsupported handoff target")
        || msg.contains("invalid Rootfile [agents].default_target")
        || msg.contains("project pointer")
        || msg.contains("Workspace pointer")
        || msg.contains("Unknown workspace id")
        || msg.contains("No known workspace")
        || msg.contains("capture agent environment")
    {
        2
    } else if msg.contains("Drift") || msg.contains("drift") {
        5
    } else {
        1
    }
}

fn unsupported_bundle_agent(agent: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "unsupported bundle adapter '{}'. S3 supports 'codex', 'opencode', and 'claude' only (no cross-agent translation)",
        agent
    )
}

fn format_user_error(e: &anyhow::Error) -> String {
    if let Some(model_err) = e.downcast_ref::<root_core::models::ModelError>() {
        return model_err.to_string();
    }
    if let Some(nix_err) = e.downcast_ref::<root_nix::NixError>() {
        return format!("{}", nix_err);
    }
    let msg = format!("{}", e);
    if msg.contains("Policy denied") {
        format!(
            "{}\n\nRun `root permissions` to inspect the active policy.",
            msg
        )
    } else if msg.contains("Snapshot purge requires explicit confirmation") {
        msg
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
                        if let Some(ref model) = event.model {
                            println!("    model: {}", model);
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
                    if r.models_restored.is_some() {
                        msg.push_str("\nModels were not restored. Ollama weights were retained.");
                    }
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
        Commands::Restore {
            lock,
            dry_run,
            rebind,
        } => {
            let cwd = current_dir();
            if dry_run {
                let result = (|| -> anyhow::Result<RestorePlanWithBind> {
                    // Plan the pointer repair read-only: --dry-run never
                    // writes the pointer, it only reports the proposal.
                    let rebind = if rebind {
                        Some(plan_rebind(&cwd)?)
                    } else {
                        None
                    };
                    let restore = root_core::restore_dry_run(&adapter, lock.as_deref())?;
                    let work_bind = dry_run_work_bind(&cwd, rebind.as_ref())?;
                    Ok(RestorePlanWithBind {
                        restore,
                        work_bind,
                        rebind,
                    })
                })();
                let _ = handle_structured(cli.json, result, format_restore_plan_with_bind);
            } else {
                let result = (|| -> anyhow::Result<RestoreWithBind> {
                    // Resolve the rebind target read-only before mutating
                    // anything; an unknown workspace fails here, before
                    // environment restore.
                    let plan = if rebind {
                        Some(plan_rebind(&cwd)?)
                    } else {
                        None
                    };
                    // Environment restore resolves paths from ROOT_DIR, not
                    // the project pointer, so it runs first; a failure here
                    // leaves the pointer untouched.
                    let restore = root_core::restore(&adapter, lock.as_deref())?;
                    // Persist the pointer repair only after env restore
                    // succeeded, then bind work state against it.
                    let rebind = plan.map(|plan| persist_rebind(&plan, &cwd)).transpose()?;
                    let work_bind = root_continuity::bind_workspace(&cwd)?;
                    Ok(RestoreWithBind {
                        restore,
                        work_bind,
                        rebind,
                    })
                })();
                let _ = handle_structured(cli.json, result, format_restore_with_bind);
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
                     Agent actions: {:?}\n\
                     Models: pull={:?}",
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
                    r.policy.approvals.agent,
                    r.policy.models.pull
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
        Commands::Models { subcommand } => match subcommand {
            ModelsSubcommands::Pull { name } => {
                match root_core::models::pull_models(name.as_deref()) {
                    Ok(report) => {
                        if cli.json {
                            print_json(&report);
                        } else {
                            println!("{}", root_core::models::format_pull_models_human(&report));
                        }
                        let code = root_core::models::models_pull_exit_code(&report);
                        if code != 0 {
                            process::exit(code);
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
                }
            }
        },
        Commands::AgentBundle { subcommand } => match subcommand {
            AgentBundleSubcommands::Inspect { agent } => match agent.as_str() {
                "codex" => {
                    let _ = handle_structured(cli.json, root_agent_bundle::codex::inspect(), |r| {
                        format!(
                            "Codex present: {}\nVersion: {}\nSupported: {}\nHome: {}\nConfig: {}\nAGENTS.md: {}\nSkills: {}\nMCP servers: {}",
                            r.present,
                            r.version.as_deref().unwrap_or("(absent)"),
                            r.version_supported,
                            r.codex_home,
                            r.config_present,
                            r.agents_md_present,
                            if r.skills.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.skills.join(", ")
                            },
                            if r.mcp_servers.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.mcp_servers.join(", ")
                            },
                        )
                    });
                }
                "opencode" => {
                    let _ = handle_structured(
                        cli.json,
                        root_agent_bundle::opencode::inspect(),
                        |r| {
                            format!(
                            "OpenCode present: {}\nVersion: {}\nSupported: {}\nConfig dir: {}\nConfig: {}\nAGENTS.md: {}\nSkills: {}\nMCP servers: {}",
                            r.present,
                            r.version.as_deref().unwrap_or("(absent)"),
                            r.version_supported,
                            r.config_dir,
                            r.config_present,
                            r.agents_md_present,
                            if r.skills.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.skills.join(", ")
                            },
                            if r.mcp_servers.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.mcp_servers.join(", ")
                            },
                        )
                        },
                    );
                }
                "claude" => {
                    let _ = handle_structured(
                        cli.json,
                        root_agent_bundle::claude::inspect(),
                        |r| {
                            format!(
                            "Claude present: {}\nVersion: {}\nSupported: {}\nConfig dir: {}\nGlobal state dir: {}\nsettings.json: {}\nCLAUDE.md: {}\nSkills: {}\nMCP servers (held): {}",
                            r.present,
                            r.version.as_deref().unwrap_or("(absent)"),
                            r.version_supported,
                            r.config_dir,
                            r.global_state_dir,
                            r.settings_present,
                            r.claude_md_present,
                            if r.skills.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.skills.join(", ")
                            },
                            if r.mcp_servers.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.mcp_servers.join(", ")
                            },
                        )
                        },
                    );
                }
                other => {
                    let _ = handle_structured::<GenericOutput>(
                        cli.json,
                        Err(unsupported_bundle_agent(other)),
                        |_| String::new(),
                    );
                    unreachable!();
                }
            },
            AgentBundleSubcommands::Export {
                agent,
                out,
                skill,
                include_mcp,
                include_executable,
                no_timestamp,
            } => {
                let opts = root_agent_bundle::export::ExportOptions {
                    skills: skill,
                    include_mcp,
                    include_executable,
                    no_timestamp,
                };
                let export = match agent.as_str() {
                    "codex" => root_agent_bundle::export::export_codex(&out, &opts),
                    "opencode" => root_agent_bundle::export::export_opencode(&out, &opts),
                    "claude" => root_agent_bundle::export::export_claude(&out, &opts),
                    other => Err(unsupported_bundle_agent(other)),
                };
                let _ = handle_structured(cli.json, export, |m| {
                    format!(
                            "Exported {} bundle v{} (agent {}).\nFiles: {}\nMCP disabled entries: {}\nNeeds env: {}\nNeeds approval: {}\n\n{}",
                            m.adapter,
                            m.bundle_version,
                            m.source_agent_version,
                            m.files.len(),
                            m.mcp.len(),
                            if m.needs_env.is_empty() {
                                "(none)".to_string()
                            } else {
                                m.needs_env.join(", ")
                            },
                            m.needs_approval.len(),
                            m.disclosure,
                        )
                });
            }
            AgentBundleSubcommands::Plan { bundle } => {
                let res: anyhow::Result<root_agent_bundle::plan::PlanReport> = (|| {
                    let manifest = root_agent_bundle::manifest::load_bundle(&bundle)?;
                    root_agent_bundle::plan::compute_plan(&bundle, &manifest)
                })();
                let _ = handle_structured(cli.json, res, |r| {
                    let mut msg = String::from("AgentBundle plan (dry-run, no writes)\n");
                    msg.push_str(&format!("\nPlan hash: {}\n", r.plan_hash));
                    if !r.will_create.is_empty() {
                        msg.push_str(&format!(
                            "\nWill create:\n  {}\n",
                            r.will_create.join("\n  ")
                        ));
                    }
                    if !r.will_update.is_empty() {
                        msg.push_str(&format!(
                            "\nWill update:\n  {}\n",
                            r.will_update.join("\n  ")
                        ));
                    }
                    if !r.settings_changes.is_empty() {
                        msg.push_str("\nconfig.toml settings changes:\n");
                        for c in &r.settings_changes {
                            msg.push_str(&format!(
                                "  {}: {} -> {}\n",
                                c.key,
                                c.old.as_deref().unwrap_or("(absent)"),
                                c.new
                            ));
                        }
                    }
                    if !r.mcp_to_add.is_empty() {
                        msg.push_str("\nMCP declarations to add (disabled):\n");
                        for m in &r.mcp_to_add {
                            msg.push_str(&format!(
                                "  {} [{}] command={:?} args={:?} {}\n",
                                m.id,
                                m.transport,
                                m.command,
                                m.args,
                                if m.exists {
                                    "(exists: will update)"
                                } else {
                                    ""
                                },
                            ));
                        }
                    }
                    if r.will_create.is_empty()
                        && r.will_update.is_empty()
                        && r.settings_changes.is_empty()
                        && r.mcp_to_add.is_empty()
                    {
                        msg.push_str("\nNo changes needed.\n");
                    }
                    if !r.needs_env.is_empty() {
                        msg.push_str(&format!(
                            "\nNeeds env on target: {}\n",
                            r.needs_env.join(", ")
                        ));
                    }
                    if !r.needs_approval.is_empty() {
                        msg.push_str(
                            "\nNeeds hash-bound approval (--approve <sha256> per item):\n",
                        );
                        for a in &r.needs_approval {
                            msg.push_str(&format!("  {} {}\n", a.sha256, a.target));
                        }
                    }
                    if !r.held.is_empty() {
                        msg.push_str("\nHeld (not exported):\n");
                        for h in &r.held {
                            msg.push_str(&format!("  {} ({})\n", h.source, h.reason));
                        }
                    }
                    msg.push_str(&format!(
                        "\n{}\n",
                        root_agent_bundle::manifest::SECRET_DISCLOSURE
                    ));
                    msg
                });
            }
            AgentBundleSubcommands::Apply {
                bundle,
                apply,
                plan_hash,
                approve,
            } => {
                if !apply {
                    if cli.json {
                        print_json(&GenericOutput {
                            success: false,
                            message: "Plan only: no writes performed. Re-run with --apply --plan-hash <hash> to mutate.".into(),
                            raw_stderr: None,
                        });
                    } else {
                        eprintln!("Plan only: no writes performed. Re-run with --apply --plan-hash <hash> to mutate.");
                    }
                    process::exit(2);
                }
                let _ = handle_structured(
                    cli.json,
                    root_agent_bundle::apply::apply_bundle(&bundle, &plan_hash, &approve),
                    |r| {
                        format!(
                            "Applied bundle (op {}).\nSnapshot: {}\nApplied: {}\nMCP imported (disabled): {}",
                            r.op_id,
                            r.snapshot_id,
                            if r.applied.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.applied.join(", ")
                            },
                            if r.mcp_imported.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.mcp_imported.join(", ")
                            },
                        )
                    },
                );
            }
            AgentBundleSubcommands::Verify { agent } => {
                let (res, label) = match agent.as_str() {
                    "codex" => (
                        root_agent_bundle::verify::verify_codex(),
                        "Codex verification",
                    ),
                    "opencode" => (
                        root_agent_bundle::verify::verify_opencode(),
                        "OpenCode verification",
                    ),
                    "claude" => (
                        root_agent_bundle::verify::verify_claude(),
                        "Claude verification",
                    ),
                    other => (Err(unsupported_bundle_agent(other)), "Agent verification"),
                };
                if let Some(report) = handle_structured(cli.json, res, |r| {
                    let mut msg = format!("{}\n", label);
                    for c in &r.checks {
                        msg.push_str(&format!(
                            "  {} {}: {}\n",
                            if c.passed { "✓" } else { "✗" },
                            c.name,
                            c.detail
                        ));
                    }
                    msg
                }) {
                    if !report.success {
                        process::exit(4);
                    }
                }
            }
            AgentBundleSubcommands::Rollback { last } => {
                if !last {
                    if cli.json {
                        print_json(&GenericOutput {
                            success: false,
                            message:
                                "Currently only `root agent-bundle rollback --last` is supported"
                                    .into(),
                            raw_stderr: None,
                        });
                    } else {
                        eprintln!(
                            "Error: Currently only `root agent-bundle rollback --last` is supported"
                        );
                    }
                    process::exit(2);
                }
                let _ =
                    handle_structured(cli.json, root_agent_bundle::apply::rollback_last(), |r| {
                        format!("Rolled back snapshot {}.", r.snapshot_id)
                    });
            }
            AgentBundleSubcommands::EnablePlan { agent, server } => {
                let plan = match agent.as_str() {
                    "codex" => root_agent_bundle::codex::enable_plan(&server),
                    "opencode" => root_agent_bundle::opencode::enable_plan(&server),
                    "claude" => Err(root_agent_bundle::claude::claude_mcp_held_error()),
                    other => Err(unsupported_bundle_agent(other)),
                };
                let _ = handle_structured(cli.json, plan, |r| {
                    format!(
                            "Enable plan for MCP server '{}'\nPlan hash: {}\nDescriptor sha256: {}\nNeeds env: {}",
                            r.server,
                            r.plan_hash,
                            r.descriptor_hash,
                            if r.needs_env.is_empty() {
                                "(none)".to_string()
                            } else {
                                r.needs_env.join(", ")
                            },
                        )
                });
            }
            AgentBundleSubcommands::Enable {
                agent,
                server,
                plan_hash,
                approve,
            } => {
                let enable = match agent.as_str() {
                    "codex" => {
                        root_agent_bundle::apply::enable_server(&server, &plan_hash, &approve)
                    }
                    "opencode" => root_agent_bundle::apply::enable_opencode_server(
                        &server, &plan_hash, &approve,
                    ),
                    "claude" => Err(root_agent_bundle::claude::claude_mcp_held_error()),
                    other => Err(unsupported_bundle_agent(other)),
                };
                let _ = handle_structured(cli.json, enable, |r| {
                    format!(
                        "Enabled MCP server '{}'. Snapshot: {}.",
                        server, r.snapshot_id
                    )
                });
            }
            AgentBundleSubcommands::Purge { id, yes } => {
                let _ = handle_structured(
                    cli.json,
                    root_agent_bundle::apply::purge_snapshots(id.as_deref(), yes),
                    |deleted| {
                        if deleted.is_empty() {
                            "No agent snapshots deleted.".to_string()
                        } else {
                            format!("Deleted snapshots: {}.", deleted.join(", "))
                        }
                    },
                );
            }
        },
        Commands::Agent { subcommand } => match subcommand {
            AgentSubcommands::Inspect { agent } => {
                let _ = handle_agent_structured(
                    cli.json,
                    agent_inspect_report(&agent),
                    format_agent_inspect,
                );
            }
            AgentSubcommands::Plan { from, to, env } => {
                let to_norm = match resolve_agent_target(to.as_deref()) {
                    Ok(t) => t,
                    Err(e) => fail_agent_args(cli.json, &e),
                };
                if env.is_none() && from.is_none() {
                    let e = anyhow::anyhow!(
                        "no source agent available: pass --from <codex|opencode|claude> or --env <file>"
                    );
                    fail_agent_args(cli.json, &e);
                }
                let res: anyhow::Result<root_agent_bundle::translate::TranslationPlan> = (|| {
                    let env_loaded = match &env {
                        Some(p) => load_env_explicit(p)?,
                        None => {
                            let from_norm = root_agent_bundle::canonical::canonical_adapter_id(
                                from.as_deref().unwrap_or_default(),
                            )?;
                            root_agent_bundle::translate::build_canonical_from_inspect(from_norm)?
                        }
                    };
                    let from_norm = match &from {
                        Some(f) => root_agent_bundle::canonical::canonical_adapter_id(f)?,
                        None => root_agent_bundle::canonical::canonical_adapter_id(
                            &env_loaded.source_agent,
                        )?,
                    };
                    root_agent_bundle::translate::plan_translation(from_norm, &env_loaded, &to_norm)
                })(
                );
                let _ = handle_agent_structured(cli.json, res, format_agent_plan);
            }
            AgentSubcommands::Diff { a, b } => {
                let res: anyhow::Result<root_agent_bundle::translate::TranslationDiff> = (|| {
                    let a_norm = root_agent_bundle::canonical::canonical_adapter_id(&a)?;
                    let b_norm = root_agent_bundle::canonical::canonical_adapter_id(&b)?;
                    let a_env = root_agent_bundle::translate::build_canonical_from_inspect(a_norm)?;
                    let b_env = root_agent_bundle::translate::build_canonical_from_inspect(b_norm)?;
                    root_agent_bundle::translate::diff_canonical(a_norm, &a_env, b_norm, &b_env)
                })(
                );
                let _ = handle_agent_structured(cli.json, res, format_agent_diff);
            }
            AgentSubcommands::Apply {
                to,
                plan_hash,
                approve,
                apply,
                env,
            } => {
                let to_norm = match resolve_agent_target(to.as_deref()) {
                    Ok(t) => t,
                    Err(e) => fail_agent_args(cli.json, &e),
                };
                let env_res = resolve_agent_env(env.as_deref());
                let (env_path, env_loaded) = match env_res {
                    Ok(v) => v,
                    Err(e) => {
                        let code = exit_code_for_error(&e);
                        if cli.json {
                            print_json(&json_error_output(&e));
                        } else {
                            eprintln!("Error: {}", format_user_error(&e));
                        }
                        process::exit(code);
                    }
                };
                let _ = env_path;
                let preflight: anyhow::Result<(
                    root_agent_bundle::canonical_apply::CanonicalApplyPlan,
                    root_agent_bundle::translate::TranslationPlan,
                )> = (|| {
                    let plan = root_agent_bundle::canonical_apply::plan_canonical_apply(
                        &env_loaded,
                        &to_norm,
                    )?;
                    let translation = root_agent_bundle::translate::plan_translation(
                        &env_loaded.source_agent,
                        &env_loaded,
                        &to_norm,
                    )?;
                    Ok((plan, translation))
                })();
                let (plan, translation) = match preflight {
                    Ok(v) => v,
                    Err(e) => {
                        let code = exit_code_for_error(&e);
                        if cli.json {
                            print_json(&json_error_output(&e));
                        } else {
                            eprintln!("Error: {}", format_user_error(&e));
                        }
                        process::exit(code);
                    }
                };
                if !apply || plan_hash.is_none() {
                    if cli.json {
                        print_json(&plan);
                    } else {
                        println!("{}", format_agent_apply_preflight(&plan, &translation));
                    }
                    process::exit(2);
                }
                let plan_hash = plan_hash.unwrap();
                let res = root_agent_bundle::canonical_apply::apply_canonical(
                    &env_loaded,
                    &to_norm,
                    &plan_hash,
                    &approve,
                    true,
                );
                match res {
                    Ok(report) => {
                        if cli.json {
                            print_json(&report);
                        } else {
                            println!("{}", format_agent_apply_report(&report));
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
                }
            }
            AgentSubcommands::Verify { agent } => {
                let res = root_agent_bundle::canonical_apply::verify_agent(&agent);
                match res {
                    Ok(report) => {
                        if cli.json {
                            print_json(&report);
                        } else {
                            println!("{}", format_agent_verify_report(&report));
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
                            eprintln!("Error: {}", format_user_error(&e));
                        }
                        process::exit(code);
                    }
                }
            }
            AgentSubcommands::Capture {
                from,
                out,
                apply,
                force,
                allow_outside_repo,
            } => {
                if !apply {
                    let res = root_agent_bundle::capture::propose_capture(&from);
                    match res {
                        Ok(proposal) => {
                            if cli.json {
                                print_json(&proposal);
                            } else {
                                println!("{}", format_capture_proposal(&proposal));
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
                    }
                } else {
                    let out_path = match out {
                        Some(p) => p,
                        None => {
                            let e = anyhow::anyhow!("--out is required with --apply");
                            let code = exit_code_for_error(&e);
                            if cli.json {
                                print_json(&json_error_output(&e));
                            } else {
                                eprintln!("Error: {}", format_user_error(&e));
                            }
                            process::exit(code);
                        }
                    };
                    match root_agent_bundle::capture::write_agent_toml(
                        &from,
                        &out_path,
                        force,
                        allow_outside_repo,
                    ) {
                        Ok(env_written) => {
                            if cli.json {
                                print_json(&env_written);
                            } else {
                                println!("{}", format_capture_applied(&out_path, &env_written));
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
                    }
                }
            }
            AgentSubcommands::Rollback { last } => {
                if !last {
                    let e =
                        anyhow::anyhow!("Currently only `root agent rollback --last` is supported");
                    let code = exit_code_for_error(&e);
                    if cli.json {
                        print_json(&json_error_output(&e));
                    } else {
                        eprintln!("Error: {}", format_user_error(&e));
                    }
                    process::exit(code);
                }
                let res = root_agent_bundle::apply::rollback_last();
                match res {
                    Ok(report) => {
                        if cli.json {
                            print_json(&report);
                        } else {
                            println!("Rolled back snapshot {}.", report.snapshot_id);
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
                }
            }
            AgentSubcommands::Purge { id, all, yes } => {
                let id_is_some = id.is_some();
                if id_is_some == all {
                    let e = if id_is_some {
                        anyhow::anyhow!("--id and --all are mutually exclusive")
                    } else {
                        anyhow::anyhow!("requires one of --id or --all")
                    };
                    let code = exit_code_for_error(&e);
                    if cli.json {
                        print_json(&json_error_output(&e));
                    } else {
                        eprintln!("Error: {}", format_user_error(&e));
                    }
                    process::exit(code);
                }
                let res = root_agent_bundle::apply::purge_snapshots(id.as_deref(), yes);
                match res {
                    Ok(deleted) => {
                        if cli.json {
                            print_json(&deleted);
                        } else if deleted.is_empty() {
                            println!("No agent snapshots deleted.");
                        } else {
                            println!("Deleted snapshots: {}.", deleted.join(", "));
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
                }
            }
        },
        Commands::Workspace { subcommand } => match subcommand {
            WorkspaceSubcommands::Init { write_pointer } => {
                let result =
                    root_work::Repository::discover(&current_dir()).and_then(|repository| {
                        root_work::workspace_init_with(&repository, write_pointer)
                    });
                let _ = handle_structured(cli.json, result, |r| {
                    format_workspace_init(r, write_pointer)
                });
            }
            WorkspaceSubcommands::Status => {
                let _ = handle_structured(
                    cli.json,
                    root_work::workspace_status(&current_dir()),
                    format_workspace_status,
                );
            }
            WorkspaceSubcommands::Export {
                checkpoint,
                out,
                force,
            } => {
                let result = (|| {
                    let repository = root_work::Repository::discover(&current_dir())?;
                    let root_dir = root_lockfile::get_root_dir()?;
                    root_work::export_workspace(
                        &root_dir,
                        &repository,
                        checkpoint.as_deref(),
                        &out,
                        force,
                    )
                })();
                let _ = handle_structured(cli.json, result, format_workspace_export);
            }
            WorkspaceSubcommands::Import {
                file,
                project,
                write_pointer,
            } => {
                let result = (|| {
                    let project = project.unwrap_or_else(current_dir);
                    let root_dir = root_lockfile::get_root_dir()?;
                    root_work::import_workspace(&root_dir, &project, &file, write_pointer)
                })();
                let _ = handle_structured(cli.json, result, |r| {
                    format_workspace_import(r, write_pointer)
                });
            }
        },
        Commands::Goal { subcommand } => match subcommand {
            GoalSubcommands::Set { goal } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::goal_set(&current_dir(), &goal),
                    format_goal,
                );
            }
            GoalSubcommands::Show => {
                let _ =
                    handle_structured(cli.json, root_work::goal_show(&current_dir()), format_goal);
            }
        },
        Commands::Decision { subcommand } => match subcommand {
            DecisionSubcommands::Add {
                statement,
                rationale,
            } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::decision_add(&current_dir(), &statement, rationale.as_deref()),
                    |r| format_decision(&r.decision),
                );
            }
            DecisionSubcommands::List => {
                let _ = handle_structured(
                    cli.json,
                    root_work::decision_list(&current_dir()),
                    format_decision_list,
                );
            }
            DecisionSubcommands::Show { id } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::decision_show(&current_dir(), &id),
                    |r| format_decision(&r.decision),
                );
            }
        },
        Commands::Finding { subcommand } => match subcommand {
            FindingSubcommands::Add {
                statement,
                evidence,
            } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::finding_add(&current_dir(), &statement, evidence.as_deref()),
                    |r| format_finding(&r.finding),
                );
            }
            FindingSubcommands::List => {
                let _ = handle_structured(
                    cli.json,
                    root_work::finding_list(&current_dir()),
                    format_finding_list,
                );
            }
            FindingSubcommands::Show { id } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::finding_show(&current_dir(), &id),
                    |r| format_finding(&r.finding),
                );
            }
        },
        Commands::Artifact { subcommand } => match subcommand {
            ArtifactSubcommands::Add { path } => {
                let _ = handle_structured(
                    cli.json,
                    root_work::artifact_add(&current_dir(), &path),
                    |r| format_artifact(&r.artifact),
                );
            }
            ArtifactSubcommands::List => {
                let _ = handle_structured(
                    cli.json,
                    root_work::artifact_list(&current_dir()),
                    format_artifact_list,
                );
            }
        },
        Commands::Checkpoint { subcommand } => match subcommand {
            CheckpointSubcommands::Create { message } => {
                let _ = handle_structured(
                    cli.json,
                    root_continuity::create(&current_dir(), message.as_deref()),
                    |r| format_checkpoint(&r.checkpoint),
                );
            }
            CheckpointSubcommands::List => {
                let _ = handle_structured(
                    cli.json,
                    root_continuity::list(&current_dir()),
                    format_checkpoint_list,
                );
            }
            CheckpointSubcommands::Show { id, last } => {
                if last {
                    let _ = handle_structured(
                        cli.json,
                        root_continuity::show_last(&current_dir()),
                        |r| format_checkpoint(&r.checkpoint),
                    );
                } else if let Some(id) = id {
                    let _ = handle_structured(
                        cli.json,
                        root_continuity::show(&current_dir(), &id),
                        |r| format_checkpoint(&r.checkpoint),
                    );
                } else {
                    let error = anyhow::anyhow!("Provide a checkpoint ID or use --last.");
                    let _ =
                        handle_structured::<GenericOutput>(cli.json, Err(error), |_| String::new());
                    unreachable!();
                }
            }
        },
        Commands::Resume { checkpoint, with } => {
            let cwd = current_dir();
            match with {
                None => {
                    let _ = handle_structured(
                        cli.json,
                        root_continuity::resume(&cwd, checkpoint.as_deref()),
                        format_resume,
                    );
                }
                Some(explicit) => {
                    let resolved = resolve_optional_target(explicit, &cwd, "--with");
                    let result = resolved.and_then(|target| {
                        root_continuity::resume_with(&cwd, checkpoint.as_deref(), &target)
                    });
                    let _ = handle_structured(cli.json, result, format_resume_with);
                }
            }
        }
        Commands::Handoff { to } => {
            let cwd = current_dir();
            let resolved = match to {
                None => Ok(None),
                Some(explicit) => resolve_optional_target(explicit, &cwd, "--to").map(Some),
            };
            let result =
                resolved.and_then(|target| root_continuity::handoff(&cwd, target.as_deref()));
            let _ = handle_structured(cli.json, result, root_continuity::render_handoff);
        }
        Commands::Recover => {
            let _ = handle_structured(
                cli.json,
                root_continuity::recover(&current_dir()),
                format_recover,
            );
        }
        Commands::Mcp { subcommand } => match subcommand {
            McpSubcommands::Serve => {
                if let Err(e) = root_mcp::serve() {
                    eprintln!("Error: {}", format_user_error(&e));
                    process::exit(exit_code_for_error(&e));
                }
            }
            McpSubcommands::Status => {
                let _ = handle_structured(cli.json, root_mcp::status(), format_mcp_status);
            }
        },
        Commands::Adapters { subcommand } => match subcommand {
            AdaptersSubcommands::List => {
                let _ = handle_structured(cli.json, adapters_list(), format_adapters_list);
            }
            AdaptersSubcommands::Inspect { agent } => {
                let _ =
                    handle_structured(cli.json, adapter_inspect(&agent), format_adapter_inspect);
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
            Commands::Restore {
                lock,
                dry_run,
                rebind,
            } => {
                assert_eq!(lock.unwrap(), std::path::PathBuf::from("./root.lock"));
                assert!(!dry_run);
                assert!(!rebind);
            }
            other => panic!("expected restore command, got {:?}", other),
        }

        let restore_dry =
            Cli::try_parse_from(["root", "restore", "--lock", "./root.lock", "--dry-run"]).unwrap();
        match restore_dry.command {
            Commands::Restore {
                lock,
                dry_run,
                rebind,
            } => {
                assert_eq!(lock.unwrap(), std::path::PathBuf::from("./root.lock"));
                assert!(dry_run);
                assert!(!rebind);
            }
            other => panic!("expected restore command, got {:?}", other),
        }

        let restore_rebind =
            Cli::try_parse_from(["root", "restore", "--rebind", "--dry-run"]).unwrap();
        match restore_rebind.command {
            Commands::Restore {
                lock,
                dry_run,
                rebind,
            } => {
                assert!(lock.is_none());
                assert!(dry_run);
                assert!(rebind);
            }
            other => panic!("expected restore command, got {:?}", other),
        }

        let ws_export =
            Cli::try_parse_from(["root", "workspace", "export", "--out", "out.rootws.json"])
                .unwrap();
        match ws_export.command {
            Commands::Workspace {
                subcommand:
                    WorkspaceSubcommands::Export {
                        checkpoint,
                        out,
                        force,
                    },
            } => {
                assert!(checkpoint.is_none());
                assert_eq!(out, PathBuf::from("out.rootws.json"));
                assert!(!force);
            }
            other => panic!("expected workspace export, got {:?}", other),
        }

        let ws_import = Cli::try_parse_from([
            "root",
            "workspace",
            "import",
            "in.rootws.json",
            "--project",
            ".",
            "--write-pointer",
        ])
        .unwrap();
        match ws_import.command {
            Commands::Workspace {
                subcommand:
                    WorkspaceSubcommands::Import {
                        file,
                        project,
                        write_pointer,
                    },
            } => {
                assert_eq!(file, PathBuf::from("in.rootws.json"));
                assert_eq!(project, Some(PathBuf::from(".")));
                assert!(write_pointer);
            }
            other => panic!("expected workspace import, got {:?}", other),
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

        let models_pull = Cli::try_parse_from(["root", "models", "pull"]).unwrap();
        match models_pull.command {
            Commands::Models {
                subcommand: ModelsSubcommands::Pull { name },
            } => assert_eq!(name, None),
            other => panic!("expected models pull, got {:?}", other),
        }

        let models_pull_one = Cli::try_parse_from(["root", "models", "pull", "qwen3:8b"]).unwrap();
        match models_pull_one.command {
            Commands::Models {
                subcommand: ModelsSubcommands::Pull { name },
            } => assert_eq!(name.as_deref(), Some("qwen3:8b")),
            other => panic!("expected models pull NAME, got {:?}", other),
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
