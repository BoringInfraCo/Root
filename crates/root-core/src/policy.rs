use anyhow::{Context, Result};
use root_lockfile::{get_root_dir, init_root_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const POLICY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    #[default]
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PackagePolicy {
    #[serde(default)]
    pub install: PolicyMode,
    #[serde(default)]
    pub update: PolicyMode,
    #[serde(default)]
    pub remove: PolicyMode,
    #[serde(default)]
    pub sync: PolicyMode,
    #[serde(default)]
    pub restore: PolicyMode,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl Default for PackagePolicy {
    fn default() -> Self {
        Self {
            install: PolicyMode::Allow,
            update: PolicyMode::Allow,
            remove: PolicyMode::Allow,
            sync: PolicyMode::Allow,
            restore: PolicyMode::Allow,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionPolicy {
    #[serde(default)]
    pub run: PolicyMode,
    #[serde(default)]
    pub allow_commands: Vec<String>,
    #[serde(default)]
    pub deny_commands: Vec<String>,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            run: PolicyMode::Allow,
            allow_commands: Vec::new(),
            deny_commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxPolicy {
    #[serde(default)]
    pub create: PolicyMode,
    #[serde(default)]
    pub run: PolicyMode,
    #[serde(default)]
    pub destroy: PolicyMode,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            create: PolicyMode::Allow,
            run: PolicyMode::Allow,
            destroy: PolicyMode::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ResourcePolicy {
    #[serde(default)]
    pub network: PolicyMode,
    #[serde(default)]
    pub filesystem: PolicyMode,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            network: PolicyMode::Allow,
            filesystem: PolicyMode::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalPolicy {
    #[serde(default)]
    pub agent: PolicyMode,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            agent: PolicyMode::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelPolicy {
    #[serde(default)]
    pub pull: PolicyMode,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        Self {
            pull: PolicyMode::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RootPolicy {
    #[serde(default = "default_policy_version")]
    pub version: u32,
    #[serde(default)]
    pub packages: PackagePolicy,
    #[serde(default)]
    pub execution: ExecutionPolicy,
    #[serde(default)]
    pub sandboxes: SandboxPolicy,
    #[serde(default)]
    pub resources: ResourcePolicy,
    #[serde(default)]
    pub approvals: ApprovalPolicy,
    #[serde(default)]
    pub models: ModelPolicy,
}

impl Default for RootPolicy {
    fn default() -> Self {
        Self {
            version: POLICY_SCHEMA_VERSION,
            packages: PackagePolicy::default(),
            execution: ExecutionPolicy::default(),
            sandboxes: SandboxPolicy::default(),
            resources: ResourcePolicy::default(),
            approvals: ApprovalPolicy::default(),
            models: ModelPolicy::default(),
        }
    }
}

fn default_policy_version() -> u32 {
    POLICY_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Install,
    Update,
    Remove,
    Run,
    Sync,
    Restore,
    SandboxCreate,
    SandboxRun,
    SandboxDestroy,
    ModelPull,
}

impl PolicyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Remove => "remove",
            Self::Run => "run",
            Self::Sync => "sync",
            Self::Restore => "restore",
            Self::SandboxCreate => "sandbox-create",
            Self::SandboxRun => "sandbox-run",
            Self::SandboxDestroy => "sandbox-destroy",
            Self::ModelPull => "model-pull",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub action: String,
    pub subject: Option<String>,
    pub reason: String,
}

pub fn policy_path() -> Result<PathBuf> {
    Ok(get_root_dir()?.join("policy.toml"))
}

pub fn read_policy() -> Result<(RootPolicy, bool)> {
    let path = policy_path()?;
    if !path.exists() {
        return Ok((RootPolicy::default(), false));
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read policy file at {}", path.display()))?;
    let policy: RootPolicy =
        toml::from_str(&content).context("Failed to parse Root policy TOML")?;
    validate_policy(&policy)?;
    Ok((policy, true))
}

pub fn apply_policy(source: &Path) -> Result<(RootPolicy, PathBuf)> {
    let content = fs::read_to_string(source)
        .with_context(|| format!("Failed to read policy file at {}", source.display()))?;
    let policy: RootPolicy =
        toml::from_str(&content).context("Failed to parse Root policy TOML")?;
    validate_policy(&policy)?;
    init_root_dir()?;
    let destination = policy_path()?;
    let serialized = toml::to_string_pretty(&policy).context("Failed to serialize Root policy")?;
    let temporary = destination.with_extension("toml.tmp");
    fs::write(&temporary, serialized)
        .with_context(|| format!("Failed to write policy file at {}", temporary.display()))?;
    fs::rename(&temporary, &destination)
        .with_context(|| format!("Failed to activate policy at {}", destination.display()))?;
    Ok((policy, destination))
}

pub fn validate_policy(policy: &RootPolicy) -> Result<()> {
    if policy.version != POLICY_SCHEMA_VERSION {
        return Err(anyhow::anyhow!(
            "Unsupported policy schema version {}. Expected {}.",
            policy.version,
            POLICY_SCHEMA_VERSION
        ));
    }
    Ok(())
}

fn matches_rule(rules: &[String], subject: &str) -> bool {
    rules.iter().any(|rule| {
        rule == subject
            || subject
                .split_whitespace()
                .next()
                .map(|command| rule == command)
                .unwrap_or(false)
    })
}

fn agent_mode_enabled() -> bool {
    std::env::var("ROOT_ACTOR")
        .map(|value| value.eq_ignore_ascii_case("agent"))
        .or_else(|_| {
            std::env::var("ROOT_AGENT")
                .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        })
        .unwrap_or(false)
}

pub fn evaluate(
    policy: &RootPolicy,
    action: PolicyAction,
    subject: Option<&str>,
) -> PolicyDecision {
    if agent_mode_enabled() && policy.approvals.agent == PolicyMode::Deny {
        return PolicyDecision {
            allowed: false,
            action: action.as_str().to_string(),
            subject: subject.map(ToString::to_string),
            reason: "agent-initiated actions are denied by policy".to_string(),
        };
    }

    if policy.resources.filesystem == PolicyMode::Deny {
        return PolicyDecision {
            allowed: false,
            action: action.as_str().to_string(),
            subject: subject.map(ToString::to_string),
            reason: "filesystem access is denied by policy".to_string(),
        };
    }

    if policy.resources.network == PolicyMode::Deny
        && matches!(
            action,
            PolicyAction::Install
                | PolicyAction::Update
                | PolicyAction::Run
                | PolicyAction::Sync
                | PolicyAction::Restore
                | PolicyAction::SandboxCreate
                | PolicyAction::SandboxRun
                | PolicyAction::ModelPull
        )
    {
        return PolicyDecision {
            allowed: false,
            action: action.as_str().to_string(),
            subject: subject.map(ToString::to_string),
            reason: "network-capable actions are denied by policy".to_string(),
        };
    }

    let mode = match action {
        PolicyAction::Install => policy.packages.install,
        PolicyAction::Update => policy.packages.update,
        PolicyAction::Remove => policy.packages.remove,
        PolicyAction::Run => policy.execution.run,
        PolicyAction::Sync => policy.packages.sync,
        PolicyAction::Restore => policy.packages.restore,
        PolicyAction::SandboxCreate => policy.sandboxes.create,
        PolicyAction::SandboxRun => policy.sandboxes.run,
        PolicyAction::SandboxDestroy => policy.sandboxes.destroy,
        PolicyAction::ModelPull => policy.models.pull,
    };
    if mode == PolicyMode::Deny {
        return PolicyDecision {
            allowed: false,
            action: action.as_str().to_string(),
            subject: subject.map(ToString::to_string),
            reason: format!("{} actions are denied by policy", action.as_str()),
        };
    }

    if action == PolicyAction::ModelPull {
        return PolicyDecision {
            allowed: true,
            action: action.as_str().to_string(),
            subject: subject.map(ToString::to_string),
            reason: "allowed by active policy".to_string(),
        };
    }

    if let Some(subject) = subject {
        let (allow, deny) = if action == PolicyAction::Run {
            (
                &policy.execution.allow_commands,
                &policy.execution.deny_commands,
            )
        } else {
            (&policy.packages.allow, &policy.packages.deny)
        };
        if matches_rule(deny, subject) {
            return PolicyDecision {
                allowed: false,
                action: action.as_str().to_string(),
                subject: Some(subject.to_string()),
                reason: format!("'{}' is denied by policy", subject),
            };
        }
        if !allow.is_empty() && !matches_rule(allow, subject) {
            return PolicyDecision {
                allowed: false,
                action: action.as_str().to_string(),
                subject: Some(subject.to_string()),
                reason: format!("'{}' is not in the policy allow list", subject),
            };
        }
    }

    PolicyDecision {
        allowed: true,
        action: action.as_str().to_string(),
        subject: subject.map(ToString::to_string),
        reason: "allowed by active policy".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_allows_actions() {
        let decision = evaluate(
            &RootPolicy::default(),
            PolicyAction::Install,
            Some("ripgrep"),
        );
        assert!(decision.allowed);
    }

    #[test]
    fn deny_list_overrides_allow_mode() {
        let mut policy = RootPolicy::default();
        policy.packages.deny.push("ripgrep".to_string());
        let decision = evaluate(&policy, PolicyAction::Install, Some("ripgrep"));
        assert!(!decision.allowed);
    }

    #[test]
    fn command_allow_list_matches_executable() {
        let mut policy = RootPolicy::default();
        policy.execution.allow_commands.push("cargo".to_string());
        assert!(evaluate(&policy, PolicyAction::Run, Some("cargo test")).allowed);
        assert!(!evaluate(&policy, PolicyAction::Run, Some("sh script.sh")).allowed);
    }

    #[test]
    fn resource_denials_are_conservative() {
        let mut policy = RootPolicy::default();
        policy.resources.network = PolicyMode::Deny;
        assert!(!evaluate(&policy, PolicyAction::Run, Some("cargo test")).allowed);
        assert!(evaluate(&policy, PolicyAction::Remove, Some("ripgrep")).allowed);

        policy.resources.filesystem = PolicyMode::Deny;
        assert!(!evaluate(&policy, PolicyAction::Remove, Some("ripgrep")).allowed);
    }

    #[test]
    fn sandbox_actions_use_sandbox_policy() {
        let mut policy = RootPolicy::default();
        policy.sandboxes.create = PolicyMode::Deny;
        assert!(!evaluate(&policy, PolicyAction::SandboxCreate, Some("build")).allowed);
        assert!(evaluate(&policy, PolicyAction::SandboxRun, Some("build")).allowed);
    }

    #[test]
    fn model_pull_defaults_allow() {
        let decision = evaluate(
            &RootPolicy::default(),
            PolicyAction::ModelPull,
            Some("qwen3:8b"),
        );
        assert!(decision.allowed);
        assert_eq!(decision.action, "model-pull");
    }

    #[test]
    fn policy_packages_allow_does_not_filter_models() {
        let mut policy = RootPolicy::default();
        policy.packages.allow.push("ripgrep".to_string());
        assert!(evaluate(&policy, PolicyAction::ModelPull, Some("qwen3:8b")).allowed);
        assert!(!evaluate(&policy, PolicyAction::Install, Some("qwen3:8b")).allowed);
    }

    #[test]
    fn policy_packages_deny_does_not_block_model_pull() {
        let mut policy = RootPolicy::default();
        policy.packages.deny.push("qwen3:8b".to_string());
        assert!(evaluate(&policy, PolicyAction::ModelPull, Some("qwen3:8b")).allowed);
        assert!(!evaluate(&policy, PolicyAction::Install, Some("qwen3:8b")).allowed);
    }

    #[test]
    fn policy_network_deny_blocks_model_pull() {
        let mut policy = RootPolicy::default();
        policy.resources.network = PolicyMode::Deny;
        assert!(!evaluate(&policy, PolicyAction::ModelPull, Some("qwen3:8b")).allowed);
    }

    #[test]
    fn policy_models_pull_deny_blocks_model_pull() {
        let mut policy = RootPolicy::default();
        policy.models.pull = PolicyMode::Deny;
        assert!(!evaluate(&policy, PolicyAction::ModelPull, Some("qwen3:8b")).allowed);
    }

    #[test]
    fn policy_missing_models_section_deserializes_allow() {
        let policy: RootPolicy = toml::from_str("version = 1\n").unwrap();
        assert_eq!(policy.models.pull, PolicyMode::Allow);
        assert!(evaluate(&policy, PolicyAction::ModelPull, Some("qwen3:8b")).allowed);
    }
}
