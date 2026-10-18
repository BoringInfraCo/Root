use std::collections::{BTreeMap, BTreeSet};

use toml::Value;

use crate::canonical::canonical_record;
use crate::digest::{require_canonical_digest, sha256_digest};
use crate::error::TaskError;
use crate::raw::{
    RawAcceptance, RawCommandStep, RawFixture, RawProhibitedPattern, RawPrompt, RawTaskRecord,
};
use crate::schema::{
    cost_usd_cents, Acceptance, AcceptanceKind, AcceptancePayload, ArtifactProperty,
    ArtifactPropertyName, ArtifactSource, Budgets, CommandSequence, CommandStep, DiffAllowlist,
    Environment, EvidenceChannel, FakeToolProfile, Fixture, GeneratedFixture, ProhibitedAbsent,
    ProhibitedPattern, Prompt, PromptKind, RepoCommitFixture, ResponseFact, StepMatcher,
    TaskRecord, TerminationSubtype, TestPass, Verifier, VerifierCommand,
};

const PROMPT_MAX_BYTES: usize = 4096;
const MAX_FIXTURE_FILES: usize = 64;
const MAX_FAKE_TOOLS: usize = 16;
const MAX_ALLOWED_TOOLS: usize = 128;
const MAX_ENV_ALLOWLIST: usize = 64;
const MAX_COMMAND_STEPS: usize = 32;
const MAX_PROHIBITED_PATTERNS: usize = 64;
const MAX_VERIFIER_COMMANDS: usize = 8;
const FIXED_FAKE_TOOL_AUDIT_PATH: &str = "fake-tool-audit.jsonl";

/// Validate the raw record and build the public typed projection.
pub(crate) fn build_record(raw: RawTaskRecord, document: &Value) -> Result<TaskRecord, TaskError> {
    validate_task_identity(&raw)?;

    let prompt = build_prompt(raw.prompt)?;
    let fixture = build_fixture(raw.fixture)?;
    validate_environment(&raw.environment)?;
    validate_budgets(&raw.budgets)?;

    let verifier = raw.verifier.map(build_verifier).transpose()?;
    let mut acceptance = Vec::with_capacity(raw.acceptance.len());
    for (index, entry) in raw.acceptance.iter().enumerate() {
        acceptance.push(build_acceptance(entry, index)?);
    }

    validate_unique_identifiers(&acceptance, verifier.as_ref())?;
    validate_acceptance_outcomes(&acceptance, verifier.as_ref())?;
    validate_termination_overrides(&raw.termination_overrides)?;

    let canonical = canonical_record(document)?;
    require_canonical_digest("integrity.content_sha256", &raw.integrity.content_sha256)?;
    if raw.integrity.content_sha256 != canonical.sha256() {
        return Err(TaskError::IntegrityMismatch {
            field: "integrity.content_sha256".to_string(),
            expected: canonical.sha256().to_string(),
            found: raw.integrity.content_sha256,
        });
    }

    Ok(TaskRecord {
        schema_version: raw.schema_version,
        task_id: raw.task_id,
        version: raw.version,
        title: raw.title,
        skill_relevance: raw.skill_relevance,
        skill_under_test: raw.skill_under_test,
        task_kind: raw.task_kind,
        prompt,
        fixture,
        environment: raw.environment,
        budgets: raw.budgets,
        acceptance,
        verifier,
        termination_overrides: raw.termination_overrides,
        integrity: raw.integrity,
        canonical,
    })
}

fn validate_task_identity(raw: &RawTaskRecord) -> Result<(), TaskError> {
    validate_identifier("task_id", &raw.task_id)?;
    if raw.version == 0 {
        return Err(TaskError::RangeInvalid {
            field: "version".to_string(),
            reason: "version must be >= 1".to_string(),
        });
    }
    non_empty("title", &raw.title)?;
    no_controls("title", &raw.title, false)?;
    non_empty("skill_under_test", &raw.skill_under_test)?;
    no_controls("skill_under_test", &raw.skill_under_test, false)?;
    Ok(())
}

fn build_prompt(raw: RawPrompt) -> Result<Prompt, TaskError> {
    match raw.kind {
        PromptKind::Inline => {
            let inline = raw.inline.ok_or_else(|| TaskError::MissingField {
                field: "prompt.inline".to_string(),
            })?;
            non_empty("prompt.inline", &inline)?;
            no_controls("prompt.inline", &inline, true)?;
            let normalized = inline.trim_end_matches('\n');
            if normalized.len() > PROMPT_MAX_BYTES {
                return Err(TaskError::PromptRule {
                    field: "prompt.inline".to_string(),
                    reason: format!(
                        "prompt is {} bytes after trailing-newline normalization (max {PROMPT_MAX_BYTES})",
                        normalized.len()
                    ),
                });
            }
            Ok(Prompt::Inline { inline })
        }
        PromptKind::File => {
            let file = raw.file.ok_or_else(|| TaskError::MissingField {
                field: "prompt.file".to_string(),
            })?;
            validate_path("prompt.file", &file)?;
            let file_sha256 = raw.file_sha256.ok_or_else(|| TaskError::MissingField {
                field: "prompt.file_sha256".to_string(),
            })?;
            require_canonical_digest("prompt.file_sha256", &file_sha256)?;
            Ok(Prompt::File { file, file_sha256 })
        }
    }
}

fn build_fixture(raw: RawFixture) -> Result<Fixture, TaskError> {
    validate_path("fixture.entry_point", &raw.entry_point)?;
    match raw.kind {
        crate::schema::FixtureKind::Generated => {
            let files = raw.files.ok_or_else(|| TaskError::MissingField {
                field: "fixture.files".to_string(),
            })?;
            if files.is_empty() || files.len() > MAX_FIXTURE_FILES {
                return Err(TaskError::FixtureRule {
                    field: "fixture.files".to_string(),
                    reason: format!("must contain 1-{MAX_FIXTURE_FILES} entries"),
                });
            }
            let mut paths = BTreeSet::new();
            for (index, file) in files.iter().enumerate() {
                let location = format!("fixture.files[{index}]");
                let path_field = format!("{location}.path");
                validate_path(&path_field, &file.path)?;
                if !paths.insert(file.path.clone()) {
                    return Err(TaskError::Duplicate {
                        field: path_field,
                        value: file.path.clone(),
                    });
                }
                no_controls(&format!("{location}.content"), &file.content, true)?;
                let digest_field = format!("{location}.content_sha256");
                require_canonical_digest(&digest_field, &file.content_sha256)?;
                let computed = sha256_digest(file.content.as_bytes());
                if computed != file.content_sha256 {
                    return Err(TaskError::IntegrityMismatch {
                        field: digest_field,
                        expected: computed,
                        found: file.content_sha256.clone(),
                    });
                }
            }
            Ok(Fixture::Generated(GeneratedFixture {
                entry_point: raw.entry_point,
                max_tree_bytes: raw.max_tree_bytes,
                files,
            }))
        }
        crate::schema::FixtureKind::RepoCommit => {
            let repo = raw.repo.ok_or_else(|| TaskError::MissingField {
                field: "fixture.repo".to_string(),
            })?;
            non_empty("fixture.repo", &repo)?;
            no_controls("fixture.repo", &repo, false)?;
            let commit = raw.commit.ok_or_else(|| TaskError::MissingField {
                field: "fixture.commit".to_string(),
            })?;
            if !is_git_commit_hash(&commit) {
                return Err(TaskError::FixtureRule {
                    field: "fixture.commit".to_string(),
                    reason: "expected exactly 40 lowercase hexadecimal characters".to_string(),
                });
            }
            let tree_sha256 = raw.tree_sha256.ok_or_else(|| TaskError::MissingField {
                field: "fixture.tree_sha256".to_string(),
            })?;
            require_canonical_digest("fixture.tree_sha256", &tree_sha256)?;
            Ok(Fixture::RepoCommit(RepoCommitFixture {
                entry_point: raw.entry_point,
                max_tree_bytes: raw.max_tree_bytes,
                repo,
                commit,
                tree_sha256,
            }))
        }
    }
}

fn validate_environment(environment: &Environment) -> Result<(), TaskError> {
    if environment.allowed_tools.is_empty() || environment.allowed_tools.len() > MAX_ALLOWED_TOOLS {
        return Err(TaskError::EnvironmentRule {
            field: "environment.allowed_tools".to_string(),
            reason: format!("must contain 1-{MAX_ALLOWED_TOOLS} entries"),
        });
    }
    for (index, tool) in environment.allowed_tools.iter().enumerate() {
        let field = format!("environment.allowed_tools[{index}]");
        non_empty(&field, tool)?;
        no_controls(&field, tool, false)?;
        if !is_capability_token(tool) {
            return Err(TaskError::EnvironmentRule {
                field,
                reason: "tool tokens are harness-neutral lowercase identifiers".to_string(),
            });
        }
    }

    if environment.env_allowlist.is_empty() || environment.env_allowlist.len() > MAX_ENV_ALLOWLIST {
        return Err(TaskError::EnvironmentRule {
            field: "environment.env_allowlist".to_string(),
            reason: format!("must contain 1-{MAX_ENV_ALLOWLIST} entries"),
        });
    }
    for (index, name) in environment.env_allowlist.iter().enumerate() {
        let field = format!("environment.env_allowlist[{index}]");
        if !is_env_allowlist_name(name) {
            return Err(TaskError::EnvironmentRule {
                field,
                reason: "expected an environment variable name or a trailing `*` glob".to_string(),
            });
        }
    }
    for required_name in ["PATH", "HOME", "TMPDIR"] {
        if !environment
            .env_allowlist
            .iter()
            .any(|name| name == required_name)
        {
            return Err(TaskError::EnvironmentRule {
                field: "environment.env_allowlist".to_string(),
                reason: format!("must contain `{required_name}`"),
            });
        }
    }

    for tool in environment.tool_permissions.keys() {
        let field = format!("environment.tool_permissions.{tool}");
        if !is_capability_token(tool) {
            return Err(TaskError::EnvironmentRule {
                field,
                reason: "permission keys are harness-neutral lowercase identifiers".to_string(),
            });
        }
    }

    if environment.fake_tools.len() > MAX_FAKE_TOOLS {
        return Err(TaskError::EnvironmentRule {
            field: "environment.fake_tools".to_string(),
            reason: format!("must contain at most {MAX_FAKE_TOOLS} entries"),
        });
    }
    let mut provides = BTreeSet::new();
    for (index, tool) in environment.fake_tools.iter().enumerate() {
        let location = format!("environment.fake_tools[{index}]");
        let provides_field = format!("{location}.provides");
        if !is_command_name(&tool.provides) {
            return Err(TaskError::EnvironmentRule {
                field: provides_field,
                reason: "expected a bare command name; path-valued provides are forbidden"
                    .to_string(),
            });
        }
        if !provides.insert(tool.provides.clone()) {
            return Err(TaskError::Duplicate {
                field: format!("{location}.provides"),
                value: tool.provides.clone(),
            });
        }
        let script_field = format!("{location}.script_sha256");
        match tool.profile {
            FakeToolProfile::Absent => {
                if tool.script_sha256.is_some() {
                    return Err(TaskError::EnvironmentRule {
                        field: script_field,
                        reason: "not permitted when profile = absent".to_string(),
                    });
                }
            }
            _ => {
                let script =
                    tool.script_sha256
                        .as_deref()
                        .ok_or_else(|| TaskError::MissingField {
                            field: script_field.clone(),
                        })?;
                require_canonical_digest(&script_field, script)?;
            }
        }
    }
    Ok(())
}

fn validate_budgets(budgets: &Budgets) -> Result<(), TaskError> {
    if let Some(cost) = &budgets.cost_usd_max {
        if cost_usd_cents(cost).is_none() {
            return Err(TaskError::BudgetRule {
                field: "budgets.cost_usd_max".to_string(),
                reason: "expected a decimal string matching ^[0-9]+\\.[0-9]{2}$".to_string(),
            });
        }
    }
    Ok(())
}

fn build_verifier(verifier: Verifier) -> Result<Verifier, TaskError> {
    if verifier.commands.len() > MAX_VERIFIER_COMMANDS {
        return Err(TaskError::VerifierRule {
            field: "verifier.commands".to_string(),
            reason: format!("must contain at most {MAX_VERIFIER_COMMANDS} entries"),
        });
    }
    for (index, command) in verifier.commands.iter().enumerate() {
        validate_verifier_command(command, index)?;
    }
    Ok(verifier)
}

fn validate_verifier_command(command: &VerifierCommand, index: usize) -> Result<(), TaskError> {
    let location = format!("verifier.commands[{index}]");
    validate_identifier(&format!("{location}.id"), &command.id)?;
    if command.argv.is_empty() {
        return Err(TaskError::VerifierRule {
            field: format!("{location}.argv"),
            reason: "must contain at least one argument".to_string(),
        });
    }
    for (arg_index, arg) in command.argv.iter().enumerate() {
        let field = format!("{location}.argv[{arg_index}]");
        non_empty(&field, arg)?;
        no_controls(&field, arg, true)?;
    }
    if let Some(cwd) = &command.cwd {
        validate_path(&format!("{location}.cwd"), cwd)?;
    }
    if let Some(digest) = &command.stdout_sha256 {
        require_canonical_digest(&format!("{location}.stdout_sha256"), digest)?;
    }
    if let Some(digest) = &command.stderr_sha256 {
        require_canonical_digest(&format!("{location}.stderr_sha256"), digest)?;
    }
    Ok(())
}

fn build_acceptance(raw: &RawAcceptance, index: usize) -> Result<Acceptance, TaskError> {
    let location = format!("acceptance[{index}]");
    validate_identifier(&format!("{location}.id"), &raw.id)?;

    if !allowed_evidence(raw.kind).contains(&raw.evidence) {
        return Err(TaskError::AcceptanceRule {
            field: format!("{location}.evidence"),
            reason: format!(
                "evidence `{}` is not allowed for kind `{}`",
                evidence_label(raw.evidence),
                kind_label(raw.kind)
            ),
        });
    }

    let payload = match raw.kind {
        AcceptanceKind::CommandSequence => {
            AcceptancePayload::CommandSequence(build_command_sequence(raw, &location)?)
        }
        AcceptanceKind::ProhibitedAbsent => {
            AcceptancePayload::ProhibitedAbsent(build_prohibited_absent(raw, &location)?)
        }
        AcceptanceKind::ArtifactProperty => {
            AcceptancePayload::ArtifactProperty(build_artifact_property(raw, &location)?)
        }
        AcceptanceKind::ResponseFact => {
            AcceptancePayload::ResponseFact(build_response_fact(raw, &location)?)
        }
        AcceptanceKind::TestPass => {
            let verifier_id = raw
                .verifier_id
                .clone()
                .ok_or_else(|| TaskError::MissingField {
                    field: format!("{location}.verifier_id"),
                })?;
            validate_identifier(&format!("{location}.verifier_id"), &verifier_id)?;
            AcceptancePayload::TestPass(TestPass { verifier_id })
        }
        AcceptanceKind::DiffAllowlist => {
            let allow_paths = raw
                .allow_paths
                .clone()
                .ok_or_else(|| TaskError::MissingField {
                    field: format!("{location}.allow_paths"),
                })?;
            for (path_index, path) in allow_paths.iter().enumerate() {
                validate_relative_glob(&format!("{location}.allow_paths[{path_index}]"), path)?;
            }
            AcceptancePayload::DiffAllowlist(DiffAllowlist { allow_paths })
        }
    };

    Ok(Acceptance {
        id: raw.id.clone(),
        kind: raw.kind,
        primary: raw.primary,
        evidence: raw.evidence,
        payload,
    })
}

fn build_command_sequence(
    raw: &RawAcceptance,
    location: &str,
) -> Result<CommandSequence, TaskError> {
    let steps = raw.steps.as_ref().ok_or_else(|| TaskError::MissingField {
        field: format!("{location}.steps"),
    })?;
    if steps.is_empty() || steps.len() > MAX_COMMAND_STEPS {
        return Err(TaskError::AcceptanceRule {
            field: format!("{location}.steps"),
            reason: format!("must contain 1-{MAX_COMMAND_STEPS} steps"),
        });
    }
    let mut built = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        built.push(build_command_step(step, location, index)?);
    }
    Ok(CommandSequence {
        steps: built,
        allow_interleaved: raw.allow_interleaved.unwrap_or(true),
        forbid_after_last_step: raw.forbid_after_last_step.unwrap_or(false),
    })
}

fn build_command_step(
    raw: &RawCommandStep,
    acceptance_location: &str,
    index: usize,
) -> Result<CommandStep, TaskError> {
    let location = format!("{acceptance_location}.steps[{index}]");
    let matcher = match raw.match_kind {
        crate::schema::CommandStepMatch::Exact => {
            if raw.regex.is_some() {
                return Err(TaskError::AcceptanceRule {
                    field: format!("{location}.regex"),
                    reason: "not permitted when match = exact".to_string(),
                });
            }
            let argv = raw.argv.clone().ok_or_else(|| TaskError::MissingField {
                field: format!("{location}.argv"),
            })?;
            if argv.is_empty() {
                return Err(TaskError::AcceptanceRule {
                    field: format!("{location}.argv"),
                    reason: "must contain at least one argument".to_string(),
                });
            }
            for (arg_index, arg) in argv.iter().enumerate() {
                let field = format!("{location}.argv[{arg_index}]");
                non_empty(&field, arg)?;
                no_controls(&field, arg, true)?;
            }
            StepMatcher::Exact { argv }
        }
        crate::schema::CommandStepMatch::Pattern => {
            if raw.argv.is_some() {
                return Err(TaskError::AcceptanceRule {
                    field: format!("{location}.argv"),
                    reason: "not permitted when match = pattern".to_string(),
                });
            }
            let regex = raw.regex.clone().ok_or_else(|| TaskError::MissingField {
                field: format!("{location}.regex"),
            })?;
            non_empty(&format!("{location}.regex"), &regex)?;
            no_controls(&format!("{location}.regex"), &regex, false)?;
            StepMatcher::Pattern { regex }
        }
    };
    Ok(CommandStep {
        matcher,
        ordinal: raw.ordinal.unwrap_or_default(),
    })
}

fn build_prohibited_absent(
    raw: &RawAcceptance,
    location: &str,
) -> Result<ProhibitedAbsent, TaskError> {
    let patterns = raw
        .patterns
        .as_ref()
        .ok_or_else(|| TaskError::MissingField {
            field: format!("{location}.patterns"),
        })?;
    if patterns.is_empty() || patterns.len() > MAX_PROHIBITED_PATTERNS {
        return Err(TaskError::AcceptanceRule {
            field: format!("{location}.patterns"),
            reason: format!("must contain 1-{MAX_PROHIBITED_PATTERNS} patterns"),
        });
    }
    let mut built = Vec::with_capacity(patterns.len());
    for (index, pattern) in patterns.iter().enumerate() {
        built.push(build_prohibited_pattern(pattern, location, index)?);
    }
    Ok(ProhibitedAbsent { patterns: built })
}

fn build_prohibited_pattern(
    raw: &RawProhibitedPattern,
    acceptance_location: &str,
    index: usize,
) -> Result<ProhibitedPattern, TaskError> {
    let location = format!("{acceptance_location}.patterns[{index}]");
    non_empty(&format!("{location}.pattern"), &raw.pattern)?;
    no_controls(&format!("{location}.pattern"), &raw.pattern, false)?;
    let tool = match &raw.tool {
        Some(tool) => {
            if !is_command_name(tool) {
                return Err(TaskError::AcceptanceRule {
                    field: format!("{location}.tool"),
                    reason: "expected a bare command name".to_string(),
                });
            }
            Some(tool.clone())
        }
        None => None,
    };
    Ok(ProhibitedPattern {
        pattern: raw.pattern.clone(),
        match_kind: raw.match_kind,
        tool,
    })
}

fn build_artifact_property(
    raw: &RawAcceptance,
    location: &str,
) -> Result<ArtifactProperty, TaskError> {
    let source = raw.source.ok_or_else(|| TaskError::MissingField {
        field: format!("{location}.source"),
    })?;
    let path = raw.path.clone().ok_or_else(|| TaskError::MissingField {
        field: format!("{location}.path"),
    })?;
    let property = raw.property.ok_or_else(|| TaskError::MissingField {
        field: format!("{location}.property"),
    })?;

    if evidence_label(raw.evidence) != source_label(source) {
        return Err(TaskError::AcceptanceRule {
            field: format!("{location}.source"),
            reason: format!(
                "source `{}` must match evidence `{}`",
                source_label(source),
                evidence_label(raw.evidence)
            ),
        });
    }
    if source == ArtifactSource::FakeToolAudit {
        if path != FIXED_FAKE_TOOL_AUDIT_PATH {
            return Err(TaskError::AcceptanceRule {
                field: format!("{location}.path"),
                reason: format!(
                    "must be `{FIXED_FAKE_TOOL_AUDIT_PATH}` when source = fake-tool-audit"
                ),
            });
        }
    } else {
        validate_path(&format!("{location}.path"), &path)?;
    }

    match property {
        ArtifactPropertyName::Sha256 => {
            let value = raw
                .value
                .as_deref()
                .ok_or_else(|| TaskError::MissingField {
                    field: format!("{location}.value"),
                })?;
            require_canonical_digest(&format!("{location}.value"), value)?;
        }
        ArtifactPropertyName::Contains | ArtifactPropertyName::MatchesRegex => {
            let value = raw
                .value
                .as_deref()
                .ok_or_else(|| TaskError::MissingField {
                    field: format!("{location}.value"),
                })?;
            non_empty(&format!("{location}.value"), value)?;
            no_controls(&format!("{location}.value"), value, true)?;
        }
        ArtifactPropertyName::Exists | ArtifactPropertyName::Absent => {}
    }

    Ok(ArtifactProperty {
        source,
        path,
        property,
        value: raw.value.clone(),
    })
}

fn build_response_fact(raw: &RawAcceptance, location: &str) -> Result<ResponseFact, TaskError> {
    validate_string_list(
        &format!("{location}.expect_substrings"),
        raw.expect_substrings.as_deref(),
    )?;
    if let Some(groups) = &raw.expect_any_of {
        for (group_index, group) in groups.iter().enumerate() {
            validate_string_list(
                &format!("{location}.expect_any_of[{group_index}]"),
                Some(group.as_slice()),
            )?;
        }
    }
    validate_string_list(
        &format!("{location}.forbid_substrings"),
        raw.forbid_substrings.as_deref(),
    )?;
    if let Some(regex) = &raw.forbid_regex {
        non_empty(&format!("{location}.forbid_regex"), regex)?;
        no_controls(&format!("{location}.forbid_regex"), regex, false)?;
    }
    Ok(ResponseFact {
        strip_code_fences: raw.strip_code_fences.unwrap_or(true),
        expect_substrings: raw.expect_substrings.clone(),
        expect_any_of: raw.expect_any_of.clone(),
        forbid_substrings: raw.forbid_substrings.clone(),
        forbid_regex: raw.forbid_regex.clone(),
    })
}

fn validate_string_list(field: &str, values: Option<&[String]>) -> Result<(), TaskError> {
    let Some(values) = values else {
        return Ok(());
    };
    for (index, value) in values.iter().enumerate() {
        let item_field = format!("{field}[{index}]");
        non_empty(&item_field, value)?;
        no_controls(&item_field, value, true)?;
    }
    Ok(())
}

fn validate_unique_identifiers(
    acceptance: &[Acceptance],
    verifier: Option<&Verifier>,
) -> Result<(), TaskError> {
    let mut seen: BTreeMap<&str, String> = BTreeMap::new();
    for (index, assertion) in acceptance.iter().enumerate() {
        let field = format!("acceptance[{index}].id");
        if seen.insert(&assertion.id, field.clone()).is_some() {
            return Err(TaskError::Duplicate {
                field,
                value: assertion.id.clone(),
            });
        }
    }
    if let Some(verifier) = verifier {
        for (index, command) in verifier.commands.iter().enumerate() {
            let field = format!("verifier.commands[{index}].id");
            if seen.insert(&command.id, field.clone()).is_some() {
                return Err(TaskError::Duplicate {
                    field,
                    value: command.id.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_acceptance_outcomes(
    acceptance: &[Acceptance],
    verifier: Option<&Verifier>,
) -> Result<(), TaskError> {
    if acceptance
        .iter()
        .filter(|assertion| assertion.primary)
        .count()
        == 0
    {
        return Err(TaskError::AcceptanceRule {
            field: "acceptance".to_string(),
            reason: "at least one primary assertion is required".to_string(),
        });
    }

    let verifier_ids: BTreeSet<&str> = verifier
        .map(|verifier| {
            verifier
                .commands
                .iter()
                .map(|command| command.id.as_str())
                .collect()
        })
        .unwrap_or_default();

    for (index, assertion) in acceptance.iter().enumerate() {
        let AcceptancePayload::TestPass(payload) = &assertion.payload else {
            continue;
        };
        if !verifier_ids.contains(payload.verifier_id.as_str()) {
            return Err(TaskError::VerifierRule {
                field: format!("acceptance[{index}].verifier_id"),
                reason: format!(
                    "references verifier command `{}`, which is not declared in [verifier]",
                    payload.verifier_id
                ),
            });
        }
    }
    Ok(())
}

fn validate_termination_overrides(
    overrides: &BTreeMap<TerminationSubtype, crate::schema::TerminationClassification>,
) -> Result<(), TaskError> {
    if overrides.contains_key(&TerminationSubtype::Success) {
        return Err(TaskError::TerminationRule {
            field: "termination_overrides.success".to_string(),
            reason: "success cannot be overridden; overrides only move between task_failure and invalid_trial".to_string(),
        });
    }
    Ok(())
}

fn allowed_evidence(kind: AcceptanceKind) -> &'static [EvidenceChannel] {
    match kind {
        AcceptanceKind::CommandSequence | AcceptanceKind::ProhibitedAbsent => &[
            EvidenceChannel::FakeToolAudit,
            EvidenceChannel::HarnessEvents,
        ],
        AcceptanceKind::ArtifactProperty => {
            &[EvidenceChannel::Tree, EvidenceChannel::FakeToolAudit]
        }
        AcceptanceKind::ResponseFact => &[EvidenceChannel::FinalMessage],
        AcceptanceKind::TestPass => &[EvidenceChannel::VerifierRun],
        AcceptanceKind::DiffAllowlist => &[EvidenceChannel::Tree],
    }
}

fn kind_label(kind: AcceptanceKind) -> &'static str {
    match kind {
        AcceptanceKind::CommandSequence => "command_sequence",
        AcceptanceKind::ProhibitedAbsent => "prohibited_absent",
        AcceptanceKind::ArtifactProperty => "artifact_property",
        AcceptanceKind::ResponseFact => "response_fact",
        AcceptanceKind::TestPass => "test_pass",
        AcceptanceKind::DiffAllowlist => "diff_allowlist",
    }
}

fn evidence_label(evidence: EvidenceChannel) -> &'static str {
    match evidence {
        EvidenceChannel::FakeToolAudit => "fake-tool-audit",
        EvidenceChannel::HarnessEvents => "harness-events",
        EvidenceChannel::FinalMessage => "final-message",
        EvidenceChannel::VerifierRun => "verifier-run",
        EvidenceChannel::Tree => "tree",
    }
}

fn source_label(source: ArtifactSource) -> &'static str {
    match source {
        ArtifactSource::Tree => "tree",
        ArtifactSource::FakeToolAudit => "fake-tool-audit",
    }
}

// ---------------------------------------------------------------------------
// Scalar grammar
// ---------------------------------------------------------------------------

/// `[a-z][a-z0-9._-]{0,63}` (§2.1).
pub fn is_task_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Record-relative, `/`-separated path that cannot escape its root (§2.1).
pub fn is_contained_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return false;
    }
    if path == "." {
        return true;
    }
    if path.ends_with('/') {
        return false;
    }
    path.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.chars().any(char::is_control)
    })
}

/// Relative fnmatch-style glob. `..` segments are never allowed.
pub fn is_relative_glob(glob: &str) -> bool {
    if glob.is_empty() || glob.starts_with('/') || glob.contains('\\') || glob.contains(':') {
        return false;
    }
    if glob == "." {
        return true;
    }
    glob.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.chars().any(char::is_control)
    })
}

fn validate_identifier(field: &str, value: &str) -> Result<(), TaskError> {
    if !is_task_identifier(value) {
        return Err(TaskError::NamingRule {
            field: field.to_string(),
            reason: "expected [a-z][a-z0-9._-]{0,63}".to_string(),
        });
    }
    Ok(())
}

fn validate_path(field: &str, path: &str) -> Result<(), TaskError> {
    if !is_contained_relative_path(path) {
        return Err(TaskError::PathInvalid {
            field: field.to_string(),
            reason: "expected a relative, `/`-separated path without `..` segments".to_string(),
        });
    }
    Ok(())
}

fn validate_relative_glob(field: &str, glob: &str) -> Result<(), TaskError> {
    if !is_relative_glob(glob) {
        return Err(TaskError::PathInvalid {
            field: field.to_string(),
            reason: "expected a relative fnmatch-style glob without `..` segments".to_string(),
        });
    }
    Ok(())
}

fn non_empty(field: &str, value: &str) -> Result<(), TaskError> {
    if value.is_empty() {
        return Err(TaskError::EmptyString {
            field: field.to_string(),
        });
    }
    Ok(())
}

fn no_controls(field: &str, value: &str, allow_layout: bool) -> Result<(), TaskError> {
    for character in value.chars() {
        let layout = allow_layout && matches!(character, '\n' | '\t');
        if character == '\r' || (character.is_control() && !layout) {
            return Err(TaskError::ControlChars {
                field: field.to_string(),
            });
        }
    }
    Ok(())
}

fn is_git_commit_hash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_capability_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

fn is_command_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

fn is_env_allowlist_name(value: &str) -> bool {
    let base = value.strip_suffix('*').unwrap_or(value);
    if base.is_empty() {
        return false;
    }
    let mut bytes = base.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
