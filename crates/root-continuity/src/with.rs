//! Harness-aware continuation assembly (`root resume --with`, Sprint 013 §3.3).
//!
//! Seven observable steps that project the existing bounded resume package onto
//! a canonical target adapter (`codex` | `opencode` | `claude`). This module is
//! strictly read-only: it never applies configuration, never runs Nix, and
//! never mutates work state. Applying the emitted plan is a separate, explicit
//! `root agent plan` / `root agent apply` action.

use crate::checkpoint::parse_snapshot;
use crate::drift::{self, DRIFT_BLOCKING};
use crate::environment::EnvironmentState;
use crate::git::GitState;
use crate::resume;
use crate::snapshot::CheckpointSnapshot;
use root_agent_bundle::canonical::{canonical_adapter_id, looks_like_secret_value, CanonicalEnv};
use root_agent_bundle::translate::plan_translation;
use root_work::model::{SOURCE_HUMAN, SOURCE_ROOT};
use root_work::{CheckpointRecord, Repository, WorkStore};
use serde::Serialize;
use std::path::Path;

/// Canonical target set for `--with` (Sprint 013).
pub const SUPPORTED_TARGETS: &[&str] = &["codex", "opencode", "claude"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Step {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResumeWithReport {
    pub target: String,
    pub source: Option<String>,
    pub checkpoint_id: Option<String>,
    pub steps: Vec<Step>,
    pub package: serde_json::Value,
    pub translation: Option<serde_json::Value>,
    /// True only when the checkpoint stored a canonical agent environment that
    /// deserialized cleanly. `false` means steps 2 and 3 honestly failed.
    pub mapping_available: bool,
    pub instructions: Vec<String>,
    pub next: Vec<String>,
    pub warnings: Vec<String>,
}

/// Agent environment as captured inside the target checkpoint (immutable).
/// Resume NEVER re-resolves the live `.root/agent.toml`.
#[allow(clippy::large_enum_variant)]
enum StoredAgentEnv {
    Available(CanonicalEnv),
    NotCaptured,
    Corrupt,
}

/// Compare a stored digest against the recomputed one. Length is checked
/// first so a shape difference fails closed before any byte comparison;
/// both sides are normalized (trim + ASCII lowercase) so hex casing cannot
/// hide a mismatch. No constant-time crate (`subtle`) is available in this
/// workspace, so this is a length-then-`==` comparison of equal-length hex
/// strings — the strongest semantics we can offer without a new dependency.
fn digest_matches(stored: &str, computed: &str) -> bool {
    let stored = stored.trim().to_ascii_lowercase();
    let computed = computed.trim().to_ascii_lowercase();
    stored.len() == computed.len() && stored == computed
}

/// Load the checkpointed canonical environment, verifying it against the
/// stored `agent_env_sha256` integrity digest. The digest was written by
/// `CheckpointSnapshot::capture_with_agent_env` using
/// [`CanonicalEnv::env_hash`] (`root-agent-bundle` canonical.rs: the same
/// `serde_json::to_vec(self)` + sha256 computation used everywhere else);
/// this recomputes it through that exact helper — never a second scheme.
///
/// Fails closed (`Err`) on any integrity violation: value/digest mismatch,
/// a value with no digest, or a digest with no value. Errors name only the
/// checkpoint id and the field — environment values are never echoed.
/// A structurally unreadable value (unparseable) stays a soft
/// [`StoredAgentEnv::Corrupt`] so mapping honestly reports "unreadable".
fn stored_agent_env(
    snapshot: &CheckpointSnapshot,
    checkpoint_id: &str,
) -> anyhow::Result<StoredAgentEnv> {
    match (&snapshot.agent_env, &snapshot.agent_env_sha256) {
        (None, None) => Ok(StoredAgentEnv::NotCaptured),
        (Some(_), None) => anyhow::bail!(
            "Checkpoint '{checkpoint_id}' stores an agent environment without its \
             agent_env_sha256 integrity digest; refusing to use it.\n\n\
             The checkpoint may be truncated, stale, or tampered with. \
             Create a new checkpoint with:  root checkpoint create"
        ),
        (None, Some(_)) => anyhow::bail!(
            "Checkpoint '{checkpoint_id}' stores an agent_env_sha256 integrity digest \
             without the agent environment it protects; refusing to continue.\n\n\
             The checkpoint may be truncated, stale, or tampered with. \
             Create a new checkpoint with:  root checkpoint create"
        ),
        (Some(value), Some(stored_digest)) => {
            let env = match serde_json::from_value::<CanonicalEnv>(value.clone()) {
                Ok(env) => env,
                Err(_) => return Ok(StoredAgentEnv::Corrupt),
            };
            let computed = match env.env_hash() {
                Ok(hash) => hash,
                Err(_) => return Ok(StoredAgentEnv::Corrupt),
            };
            if !digest_matches(stored_digest, &computed) {
                anyhow::bail!(
                    "Checkpoint '{checkpoint_id}' failed its agent environment integrity \
                     check: agent_env_sha256 does not match the stored agent environment.\n\n\
                     The checkpoint may be truncated, stale, or tampered with. \
                     Create a new checkpoint with:  root checkpoint create"
                );
            }
            Ok(StoredAgentEnv::Available(env))
        }
    }
}

/// Assemble a harness-aware continuation package for `cwd`.
///
/// Deterministic for identical inputs; an unknown `target` fails closed.
pub fn resume_with(
    cwd: &Path,
    checkpoint: Option<&str>,
    target: &str,
) -> anyhow::Result<ResumeWithReport> {
    let target = match canonical_adapter_id(target) {
        Ok(id) => id,
        Err(_) => anyhow::bail!(
            "Unsupported --with target '{}'. Supported targets: {}.",
            target.trim(),
            SUPPORTED_TARGETS.join(", ")
        ),
    };

    let repository = Repository::discover(cwd)?;
    let store = WorkStore::open(repository.clone())?;
    let checkpoint_record = match checkpoint {
        Some(id) => store.show_checkpoint(id)?,
        None => store.latest_checkpoint()?.ok_or_else(|| {
            anyhow::anyhow!(
                "No checkpoints exist for this workspace.\n\n\
                 Create one with:  root checkpoint create"
            )
        })?,
    };
    let snapshot = parse_snapshot(&checkpoint_record)?;
    let git = GitState::capture(&repository);
    let environment = EnvironmentState::capture()?;
    let current = resume::current_work(&store)?;
    let report = resume::build_report(
        store.workspace(),
        &checkpoint_record,
        &snapshot,
        &git,
        &environment,
        &repository.root,
        &current,
    );

    // Mapping comes from the checkpoint's stored canonical environment, never
    // from the live project file. The live file may have been edited, deleted,
    // or replaced after checkpointing; that must not change an immutable
    // checkpoint's translation. The stored environment is verified against
    // its stored `agent_env_sha256` digest before any mapping is built; an
    // integrity failure fails the whole projection closed.
    let (mapping, mapping_note) = match stored_agent_env(&snapshot, &checkpoint_record.id)? {
        StoredAgentEnv::Available(env) => (Some(env), None),
        StoredAgentEnv::NotCaptured => (
            None,
            Some(
                "agent environment was not captured at this checkpoint; \
                 skills/instructions/credentials cannot be mapped"
                    .to_string(),
            ),
        ),
        StoredAgentEnv::Corrupt => (
            None,
            Some(
                "stored agent environment is unreadable; \
                 skills/instructions/credentials cannot be mapped"
                    .to_string(),
            ),
        ),
    };
    let mapping_available = mapping.is_some();

    let mut warnings = Vec::new();
    if let Some(note) = &mapping_note {
        warnings.push(note.clone());
    }
    let mut steps = Vec::new();

    // 1. prepare config.
    let mcp = mcp_snippet(target)?;
    let detection = detect(target);
    warnings.extend(detection.warnings.clone());
    let presence = if detection.present {
        match detection.version.as_deref() {
            Some(version) => format!("{target} present, version {version}"),
            None => format!("{target} present, version unknown"),
        }
    } else {
        format!("{target} not detected on PATH")
    };
    steps.push(Step {
        name: "prepare config".to_string(),
        ok: true,
        detail: format!("{presence}; {} snippet ready", mcp.format),
    });

    // 2. map skills/instructions/tools.
    let mut translation_value = None;
    let map_detail = match &mapping {
        Some(env) => {
            let plan = plan_translation(&env.source_agent, env, target)?;
            let detail = format!(
                "mapped {} portable, {} requires review, {} unsupported (no apply)",
                plan.portable.len(),
                plan.requires_review.len(),
                plan.unsupported.len()
            );
            warnings.extend(plan.warnings.clone());
            translation_value = Some(serde_json::to_value(&plan)?);
            detail
        }
        None => mapping_note
            .clone()
            .unwrap_or_else(|| "agent environment unavailable; nothing mapped".to_string()),
    };
    steps.push(Step {
        name: "map skills/instructions".to_string(),
        ok: mapping_available,
        detail: map_detail,
    });

    // 3. credential refs.
    let (credential_detail, refused) = match &mapping {
        Some(env) => {
            let names = &env.environment.needs_env;
            let refused = secret_shaped_count(env);
            let detail = format!(
                "{} credential refs (names only); secret scan: {refused} refused",
                names.len()
            );
            (detail, refused)
        }
        None => (
            mapping_note
                .clone()
                .unwrap_or_else(|| "agent environment unavailable".to_string()),
            0,
        ),
    };
    if refused > 0 {
        warnings.push(format!(
            "{refused} secret-shaped value(s) found in the canonical environment; values were not read or copied"
        ));
    }
    steps.push(Step {
        name: "credential refs".to_string(),
        ok: mapping_available,
        detail: credential_detail,
    });

    // 4. restore workspace (VERIFY-ONLY).
    let environment_matches = checkpoint_record.rootfile_digest == environment.rootfile_digest
        && checkpoint_record.root_lock_digest == environment.root_lock_digest;
    if !environment_matches {
        warnings.push(
            "current Root environment digests differ from the checkpoint; run `root restore`"
                .to_string(),
        );
    }
    steps.push(Step {
        name: "restore workspace".to_string(),
        ok: environment_matches,
        detail: if environment_matches {
            format!(
                "environment {} matches checkpoint (verify only)",
                environment.status
            )
        } else {
            format!(
                "environment {} differs from checkpoint; run `root restore`",
                environment.status
            )
        },
    });

    // 5. repo drift check.
    let drift = drift::detect(
        &repository.root,
        &checkpoint_record,
        &snapshot,
        &git,
        &environment,
    );
    let drift_detail = if drift.items.is_empty() {
        "none".to_string()
    } else {
        drift
            .items
            .iter()
            .map(|item| format!("[{}] {}", item.level, item.detail))
            .collect::<Vec<_>>()
            .join("; ")
    };
    if drift.level == DRIFT_BLOCKING {
        warnings.push(format!("drift is blocking: {drift_detail}"));
    }
    steps.push(Step {
        name: "repo drift check".to_string(),
        ok: drift.level != DRIFT_BLOCKING,
        detail: format!("{} ({drift_detail})", drift.level),
    });

    // 6. assemble work state (reuse the bounded resume projection).
    let omitted = report.decisions_omitted + report.findings_omitted + report.artifacts_omitted;
    steps.push(Step {
        name: "assemble work state".to_string(),
        ok: true,
        detail: format!(
            "{} decisions, {} findings, {} artifacts; {omitted} omitted",
            report.decisions.len(),
            report.findings.len(),
            report.artifacts.len()
        ),
    });

    // 7. launch continuation.
    let mut package = serde_json::to_value(&report)?;
    // Generated suggestions stay explicitly labeled "not verified", matching
    // the v0.5 handoff projection policy.
    if let Some(array) = package
        .get_mut("suggested_continuation")
        .and_then(|value| value.as_array_mut())
    {
        for value in array.iter_mut() {
            if let Some(text) = value.as_str() {
                *value = serde_json::Value::String(format!("Suggestion (not verified): {text}"));
            }
        }
    }
    let plan_hash = translation_value
        .as_ref()
        .and_then(|value| value.get("plan_hash"))
        .and_then(|value| value.as_str());
    let next = next_commands(target, plan_hash);
    let instructions = instruction_lines(target, &mcp.instructions);
    steps.push(Step {
        name: "launch continuation".to_string(),
        ok: true,
        detail: "ready (resume never applies configuration)".to_string(),
    });

    Ok(ResumeWithReport {
        target: target.to_string(),
        source: origin(&store, &checkpoint_record),
        checkpoint_id: Some(checkpoint_record.id.clone()),
        steps,
        package,
        translation: translation_value,
        mapping_available,
        instructions,
        next,
        warnings,
    })
}

fn origin(store: &WorkStore, checkpoint: &CheckpointRecord) -> Option<String> {
    let provenance_id = checkpoint.provenance_id.as_deref()?;
    let record = store.provenance(provenance_id).ok()?;
    if record.source_type == SOURCE_HUMAN || record.source_type == SOURCE_ROOT {
        return None;
    }
    record.harness.or(record.agent)
}

struct McpSnippet {
    format: String,
    instructions: Vec<String>,
}

fn mcp_snippet(target: &str) -> anyhow::Result<McpSnippet> {
    match target {
        "opencode" => Ok(McpSnippet {
            format: "opencode.json".to_string(),
            instructions: vec![
                "Add an \"mcp\".root entry to opencode.json (type = \"local\").".to_string(),
                "Restart OpenCode so it loads the Root MCP server.".to_string(),
                "Verify the connection with: root mcp status".to_string(),
            ],
        }),
        _ => {
            let config = root_adapters::mcp_config(target)?;
            Ok(McpSnippet {
                format: config.format,
                instructions: config.instructions,
            })
        }
    }
}

fn detect(target: &str) -> root_adapters::AdapterDetection {
    root_adapters::detect_binary(target, target)
}

/// Count canonical free-text fields that look secret-shaped (names/refs only;
/// values are never read). Canonical validation already refuses most of these,
/// so this is a defense-in-depth report.
fn secret_shaped_count(env: &CanonicalEnv) -> usize {
    let mut count = 0;
    let mut check = |value: &str| {
        if looks_like_secret_value(value) {
            count += 1;
        }
    };
    check(&env.source_agent);
    check(&env.source_agent_version);
    check(&env.provenance.captured_at);
    for tool in &env.tools {
        check(&tool.name);
        check(&tool.value);
    }
    for skill in &env.skills {
        check(&skill.name);
        for file in &skill.files {
            check(file);
        }
    }
    for policy in &env.policies {
        check(&policy.key);
        check(&policy.reason);
    }
    for entry in env.mcp_servers.values() {
        check(&entry.id);
        check(&entry.transport);
        for part in entry.command.iter().chain(entry.args.iter()) {
            check(part);
        }
    }
    count
}

/// Target-specific Root continuity instruction block. Codex/Claude delegate to
/// `root-adapters`; OpenCode carries the Sprint 013 block. Shared with
/// `handoff`, which uses the same text for its `instructions` field.
pub(crate) fn target_instruction_text(target: &str) -> anyhow::Result<String> {
    match target {
        "opencode" => Ok("Root continuity instructions (OpenCode)\n\
             - Inspect Root before you start: call workspace.status and work.get_goal.\n\
             - Record durable decisions with work.record_decision when a choice must survive this session.\n\
             - Record evidence-backed findings with work.record_finding when you learn something that changes direction.\n\
             - Create a checkpoint with continuity.checkpoint before stopping or switching agents.\n\
             - Resume with continuity.resume from the latest checkpoint, or use continuity.handoff to brief another agent.\n\
             Root captures durable engineering state, not chain-of-thought or raw transcripts."
            .to_string()),
        _ => root_adapters::root_instructions(target),
    }
}

fn instruction_lines(target: &str, mcp_instructions: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = target_instruction_text(target)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    lines.extend(mcp_instructions.iter().cloned());
    lines
}

fn next_commands(target: &str, plan_hash: Option<&str>) -> Vec<String> {
    let hash = plan_hash.unwrap_or("<plan-hash>");
    vec![
        format!("root agent plan --env .root/agent.toml --to {target}"),
        format!(
            "root agent apply --env .root/agent.toml --apply --plan-hash {hash} --approve <sha>..."
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_lock;
    use root_work::{NewCheckpoint, ProvenanceContext, WorkStore};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    struct EnvGuard {
        previous_root: Option<OsString>,
        previous_home: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(root_dir: &Path, home: &Path) -> Self {
            let lock = test_lock();
            let previous_root = std::env::var_os("ROOT_DIR");
            let previous_home = std::env::var_os("HOME");
            std::env::set_var("ROOT_DIR", root_dir);
            std::env::set_var("HOME", home);
            Self {
                previous_root,
                previous_home,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous_root {
                Some(value) => std::env::set_var("ROOT_DIR", value),
                None => std::env::remove_var("ROOT_DIR"),
            }
            match &self.previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    struct Fixture {
        base: PathBuf,
        repo: PathBuf,
        root_dir: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("root_continuity_with_{tag}_{}", std::process::id()));
            let repo = base.join("campfire");
            let root_dir = base.join("root");
            let home = base.join("home");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(&root_dir).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            let repository = Repository::discover(&repo).unwrap();
            WorkStore::init_at(&root_dir, repository).unwrap();
            Self {
                base,
                repo,
                root_dir,
                home,
            }
        }

        fn repository(&self) -> Repository {
            Repository::discover(&self.repo).unwrap()
        }

        fn open(&self) -> WorkStore {
            WorkStore::open_at(&self.root_dir, self.repository()).unwrap()
        }

        fn checkpoint(&self, context: ProvenanceContext<'_>) -> String {
            let mut store = self.open();
            let repository = self.repository();
            let report = crate::create_on_store(
                &mut store,
                &repository,
                &self.root_dir,
                Some("checkpoint"),
                context,
            )
            .unwrap();
            report.checkpoint.id
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn list_files(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(cur) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&cur) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path.display().to_string());
                }
            }
        }
        out.sort();
        out
    }

    fn with_content(repo: &Path) -> String {
        let dir = repo.join(".root");
        std::fs::create_dir_all(&dir).unwrap();
        let env = root_agent_bundle::canonical::CanonicalEnv {
            schema_version: root_agent_bundle::canonical::CANONICAL_SCHEMA_VERSION,
            source_agent: "codex".to_string(),
            source_agent_version: "0.150.1".to_string(),
            instructions: root_agent_bundle::canonical::CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: vec![root_agent_bundle::canonical::CanonicalSkill {
                name: "docs-writer".to_string(),
                files: vec!["SKILL.md".to_string()],
            }],
            tools: vec![root_agent_bundle::canonical::CanonicalTool {
                name: "model".to_string(),
                value: "gpt-5".to_string(),
            }],
            mcp_servers: std::collections::BTreeMap::new(),
            policies: Vec::new(),
            environment: root_agent_bundle::canonical::CanonicalEnvironment {
                needs_env: Vec::new(),
            },
            provenance: root_agent_bundle::canonical::CanonicalProvenance {
                captured_at: "1234567890".to_string(),
                bundle_hash: String::new(),
                disclosure: root_agent_bundle::manifest::SECRET_DISCLOSURE.to_string(),
            },
        };
        env.validate().unwrap();
        let toml = root_agent_bundle::project::emit_agent_toml(&env).unwrap();
        std::fs::write(dir.join("agent.toml"), toml).unwrap();
        dir.join("agent.toml").display().to_string()
    }

    #[test]
    fn seven_steps_present_and_ordered() {
        let fixture = Fixture::new("steps");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        fixture.checkpoint(ProvenanceContext::default());

        let report = resume_with(&fixture.repo, None, "codex").unwrap();
        let names: Vec<&str> = report.steps.iter().map(|step| step.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "prepare config",
                "map skills/instructions",
                "credential refs",
                "restore workspace",
                "repo drift check",
                "assemble work state",
                "launch continuation",
            ]
        );
        assert_eq!(report.target, "codex");
        assert_eq!(report.steps.len(), 7);
        let suggestions = report.package["suggested_continuation"].as_array().unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().all(|value| value
            .as_str()
            .unwrap()
            .starts_with("Suggestion (not verified):")));
    }

    #[test]
    fn deterministic_re_run() {
        let fixture = Fixture::new("deterministic");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let first = resume_with(&fixture.repo, None, "opencode").unwrap();
        let second = resume_with(&fixture.repo, None, "opencode").unwrap();
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }

    #[test]
    fn unknown_target_fails_closed() {
        let fixture = Fixture::new("unknown");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        fixture.checkpoint(ProvenanceContext::default());

        let error = resume_with(&fixture.repo, None, "gemini")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unsupported --with target"), "{error}");
        assert!(error.contains("codex, opencode, claude"), "{error}");
    }

    #[test]
    fn caps_and_omitted_counts_are_preserved() {
        let fixture = Fixture::new("caps");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        {
            let mut store = fixture.open();
            store.set_goal("Bound the package").unwrap();
            for index in 0..12 {
                store
                    .add_decision(&format!("Decision {index:02}"), None)
                    .unwrap();
            }
            for index in 0..12 {
                store
                    .add_finding(&format!("Finding {index:02}"), None)
                    .unwrap();
            }
        }
        fixture.checkpoint(ProvenanceContext::default());

        let report = resume_with(&fixture.repo, None, "claude").unwrap();
        assert_eq!(report.package["decisions"].as_array().unwrap().len(), 10);
        assert_eq!(report.package["decisions_omitted"], 2);
        assert_eq!(report.package["findings"].as_array().unwrap().len(), 10);
        assert_eq!(report.package["findings_omitted"], 2);
    }

    #[test]
    fn agent_provenance_is_reused_not_invented() {
        let fixture = Fixture::new("from");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        let context = ProvenanceContext {
            source_type: root_work::model::SOURCE_AGENT,
            agent: Some("codex"),
            harness: None,
            session_id: None,
            evidence_ref: None,
        };
        fixture.checkpoint(context);

        let report = resume_with(&fixture.repo, None, "claude").unwrap();
        assert_eq!(report.source.as_deref(), Some("codex"));
    }

    #[test]
    fn human_provenance_yields_no_source() {
        let fixture = Fixture::new("from_human");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        fixture.checkpoint(ProvenanceContext::default());
        let report = resume_with(&fixture.repo, None, "codex").unwrap();
        assert_eq!(report.source, None);
    }

    #[test]
    fn resume_with_never_mutates_repo_or_work_state() {
        let fixture = Fixture::new("no_mutation");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let files_before = list_files(&fixture.repo);
        let (events_before, checkpoints_before) = {
            let store = fixture.open();
            (
                store.events().unwrap().len(),
                store.list_checkpoints().unwrap().len(),
            )
        };

        let _ = resume_with(&fixture.repo, None, "codex").unwrap();

        assert_eq!(list_files(&fixture.repo), files_before);
        let store = fixture.open();
        assert_eq!(store.events().unwrap().len(), events_before);
        assert_eq!(store.list_checkpoints().unwrap().len(), checkpoints_before);
    }

    /// Insert a hand-crafted snapshot as the workspace's newest checkpoint
    /// (`latest_checkpoint` orders by `created_at DESC, rowid DESC`, so the
    /// freshly inserted row wins ties within the same second).
    fn insert_snapshot(fixture: &Fixture, snapshot: &CheckpointSnapshot) -> String {
        let mut store = fixture.open();
        store
            .create_checkpoint_with(
                NewCheckpoint {
                    message: Some("raw env"),
                    work_revision: snapshot.work_revision,
                    git_head: None,
                    git_branch: None,
                    git_dirty: false,
                    git_dirty_fingerprint: None,
                    rootfile_digest: None,
                    root_lock_digest: None,
                    profile_reference: None,
                    environment_status: root_work::model::ENV_OBSERVED,
                    continuation_summary: "summary",
                    snapshot: &snapshot.to_json().unwrap(),
                    agent_env_ref: None,
                },
                ProvenanceContext::default(),
            )
            .unwrap()
            .id
    }

    /// Insert a checkpoint whose stored snapshot carries `agent_env` verbatim.
    fn checkpoint_with_raw_agent_env(
        fixture: &Fixture,
        agent_env: Option<serde_json::Value>,
        agent_env_sha256: Option<String>,
    ) -> String {
        insert_snapshot(
            fixture,
            &CheckpointSnapshot {
                goal: None,
                decisions: Vec::new(),
                findings: Vec::new(),
                artifacts: Vec::new(),
                work_revision: 0,
                agent_env,
                agent_env_sha256,
            },
        )
    }

    /// Snapshot of the newest checkpoint (real pipeline capture).
    fn latest_snapshot(fixture: &Fixture) -> CheckpointSnapshot {
        let store = fixture.open();
        let record = store
            .latest_checkpoint()
            .unwrap()
            .expect("fixture must have a checkpoint");
        crate::checkpoint::parse_snapshot(&record).unwrap()
    }

    #[test]
    fn resume_with_uses_checkpointed_agent_env() {
        let fixture = Fixture::new("checkpointed_env");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let before = resume_with(&fixture.repo, None, "opencode").unwrap();
        assert!(before.mapping_available);
        assert_eq!(before.translation.as_ref().unwrap()["from"], "codex");

        // Hostile live edit AFTER the checkpoint: the immutable checkpoint's
        // translation must not change.
        std::fs::write(
            fixture.repo.join(".root/agent.toml"),
            "{ this is not valid toml",
        )
        .unwrap();

        let after = resume_with(&fixture.repo, None, "opencode").unwrap();
        assert!(after.mapping_available);
        assert_eq!(after.translation, before.translation);
        assert_eq!(
            serde_json::to_string(&after).unwrap(),
            serde_json::to_string(&before).unwrap()
        );
    }

    #[test]
    fn resume_with_survives_deleted_live_file() {
        let fixture = Fixture::new("deleted_env");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let before = resume_with(&fixture.repo, None, "claude").unwrap();
        assert!(before.mapping_available);

        std::fs::remove_file(fixture.repo.join(".root/agent.toml")).unwrap();

        let after = resume_with(&fixture.repo, None, "claude").unwrap();
        assert!(after.mapping_available);
        assert_eq!(after.translation, before.translation);
        assert_eq!(
            serde_json::to_string(&after).unwrap(),
            serde_json::to_string(&before).unwrap()
        );
    }

    #[test]
    fn resume_with_without_captured_agent_env_fails_honestly() {
        let fixture = Fixture::new("no_env");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        fixture.checkpoint(ProvenanceContext::default());

        let report = resume_with(&fixture.repo, None, "codex").unwrap();
        assert!(!report.mapping_available);
        assert!(report.translation.is_none());
        let map = report
            .steps
            .iter()
            .find(|step| step.name == "map skills/instructions")
            .unwrap();
        assert!(!map.ok, "mapping step must fail honestly");
        assert!(map.detail.contains("not captured"), "{}", map.detail);
        let credentials = report
            .steps
            .iter()
            .find(|step| step.name == "credential refs")
            .unwrap();
        assert!(!credentials.ok, "credential step must fail honestly");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("not captured")),
            "warnings: {:?}",
            report.warnings
        );
        // Work-state steps still run and report success.
        for name in ["assemble work state", "launch continuation"] {
            let step = report.steps.iter().find(|step| step.name == name).unwrap();
            assert!(step.ok, "{name} should still run: {}", step.detail);
        }
    }

    #[test]
    fn resume_with_corrupt_stored_agent_env_fails_honestly() {
        let fixture = Fixture::new("corrupt_env");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        // Structurally unreadable value with a (dummy) digest present: the
        // value cannot be hashed, so this stays the soft "unreadable" path —
        // one-sided presence is covered by the integrity tests below.
        checkpoint_with_raw_agent_env(
            &fixture,
            Some(serde_json::json!({"bogus": true})),
            Some("0".repeat(64)),
        );

        let report = resume_with(&fixture.repo, None, "codex").unwrap();
        assert!(!report.mapping_available);
        assert!(report.translation.is_none());
        let map = report
            .steps
            .iter()
            .find(|step| step.name == "map skills/instructions")
            .unwrap();
        assert!(!map.ok, "corrupt stored env must fail honestly");
        assert!(map.detail.contains("unreadable"), "{}", map.detail);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("unreadable")),
            "warnings: {:?}",
            report.warnings
        );
    }

    /// A checkpoint created through the real pipeline always carries both
    /// integrity fields, and the stored digest is exactly what
    /// `stored_agent_env` will recompute — write path and verification agree.
    #[test]
    fn real_pipeline_checkpoint_binds_agent_env_to_matching_digest() {
        let fixture = Fixture::new("digest_bound");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let snapshot = latest_snapshot(&fixture);
        let value = snapshot.agent_env.clone().expect("agent_env captured");
        let digest = snapshot.agent_env_sha256.clone().expect("digest captured");
        assert_eq!(digest.len(), 64, "digest must be full sha256 hex: {digest}");
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "digest must be lowercase hex: {digest}"
        );
        let env: CanonicalEnv = serde_json::from_value(value).unwrap();
        assert_eq!(
            env.env_hash().unwrap(),
            digest,
            "write-path digest and verification recomputation must agree"
        );

        let report = resume_with(&fixture.repo, None, "codex").unwrap();
        assert!(report.mapping_available);
        for name in ["map skills/instructions", "credential refs"] {
            let step = report.steps.iter().find(|step| step.name == name).unwrap();
            assert!(step.ok, "{name} must succeed: {}", step.detail);
        }
    }

    /// Tamper the stored value (structurally valid modification) while the
    /// digest stays put: resume must fail closed without using the mapping
    /// and without echoing the (secret-shaped) tampered value.
    #[test]
    fn resume_with_rejects_tampered_agent_env_value() {
        let fixture = Fixture::new("tamper_value");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());
        assert!(
            resume_with(&fixture.repo, None, "codex")
                .unwrap()
                .mapping_available
        );

        let secret = "sk-tampered-value-0123456789";
        let mut snapshot = latest_snapshot(&fixture);
        {
            let value = snapshot.agent_env.as_mut().expect("captured");
            value["tools"][0]["value"] = serde_json::json!(secret);
        }
        // Digest intentionally left unchanged.
        insert_snapshot(&fixture, &snapshot);

        let error = resume_with(&fixture.repo, None, "codex")
            .unwrap_err()
            .to_string();
        assert!(error.contains("integrity check"), "{error}");
        assert!(error.contains("agent_env_sha256"), "{error}");
        assert!(!error.contains(secret), "secret value leaked: {error}");
        assert!(!error.contains("gpt-5"), "env values leaked: {error}");
    }

    /// Tamper the digest only (well-formed hex, wrong value): fail closed.
    #[test]
    fn resume_with_rejects_tampered_agent_env_digest() {
        let fixture = Fixture::new("tamper_digest");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let mut snapshot = latest_snapshot(&fixture);
        snapshot.agent_env_sha256 = Some("0".repeat(64));
        insert_snapshot(&fixture, &snapshot);

        let error = resume_with(&fixture.repo, None, "codex")
            .unwrap_err()
            .to_string();
        assert!(error.contains("integrity check"), "{error}");
        assert!(error.contains("agent_env_sha256"), "{error}");
    }

    /// Value present, digest absent: fail closed (one-sided presence).
    #[test]
    fn resume_with_rejects_agent_env_without_digest() {
        let fixture = Fixture::new("value_no_digest");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let mut snapshot = latest_snapshot(&fixture);
        snapshot.agent_env_sha256 = None;
        insert_snapshot(&fixture, &snapshot);

        let error = resume_with(&fixture.repo, None, "codex")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("without its agent_env_sha256 integrity digest"),
            "{error}"
        );
    }

    /// Digest present, value absent: fail closed (one-sided presence).
    #[test]
    fn resume_with_rejects_digest_without_agent_env() {
        let fixture = Fixture::new("digest_no_value");
        let _guard = EnvGuard::set(&fixture.root_dir, &fixture.home);
        with_content(&fixture.repo);
        fixture.checkpoint(ProvenanceContext::default());

        let mut snapshot = latest_snapshot(&fixture);
        snapshot.agent_env = None;
        insert_snapshot(&fixture, &snapshot);

        let error = resume_with(&fixture.repo, None, "codex")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("without the agent environment it protects"),
            "{error}"
        );
    }
}
