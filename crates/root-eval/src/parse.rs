use serde::de::DeserializeOwned;
use toml::map::Map;
use toml::Value;

use crate::digest::is_pending_sentinel;
use crate::error::TaskError;
use crate::raw::RawTaskRecord;
use crate::schema::{
    AcceptanceKind, ArtifactPropertyName, ArtifactSource, BreachPolicy, CommandStepMatch,
    EvidenceChannel, FakeToolProfile, FixtureKind, HashDomain, Ordinal, PatternMatch, PromptKind,
    SkillRelevance, TaskKind, TerminationClassification, TerminationSubtype, ToolPermission,
};
use crate::{TASK_SPEC_MAX_SUPPORTED_VERSION, TASK_SPEC_SCHEMA_VERSION};

const ROOT_KEYS: &[&str] = &[
    "schema_version",
    "task_id",
    "version",
    "title",
    "skill_relevance",
    "skill_under_test",
    "task_kind",
    "prompt",
    "fixture",
    "environment",
    "budgets",
    "acceptance",
    "verifier",
    "termination_overrides",
    "integrity",
];

const PROMPT_KEYS: &[&str] = &["kind", "inline", "file", "file_sha256"];

const FIXTURE_KEYS: &[&str] = &[
    "kind",
    "entry_point",
    "max_tree_bytes",
    "tree_sha256",
    "files",
    "repo",
    "commit",
];
const FIXTURE_FILE_KEYS: &[&str] = &["path", "content", "content_sha256"];

const ENVIRONMENT_KEYS: &[&str] = &[
    "allowed_tools",
    "tool_permissions",
    "env_allowlist",
    "fake_tools",
];
const FAKE_TOOL_KEYS: &[&str] = &["provides", "profile", "script_sha256"];

const BUDGET_KEYS: &[&str] = &[
    "wall_time_seconds_max",
    "turns_max",
    "tokens_max",
    "cost_usd_max",
    "breach_policy",
];

const ACCEPTANCE_KEYS: &[&str] = &["id", "kind", "primary", "evidence"];
const COMMAND_SEQUENCE_KEYS: &[&str] = &["steps", "allow_interleaved", "forbid_after_last_step"];
const COMMAND_STEP_KEYS: &[&str] = &["match", "argv", "regex", "ordinal"];
const PROHIBITED_ABSENT_KEYS: &[&str] = &["patterns"];
const PROHIBITED_PATTERN_KEYS: &[&str] = &["pattern", "match", "tool"];
const ARTIFACT_PROPERTY_KEYS: &[&str] = &["source", "path", "property", "value"];
const RESPONSE_FACT_KEYS: &[&str] = &[
    "strip_code_fences",
    "expect_substrings",
    "expect_any_of",
    "forbid_substrings",
    "forbid_regex",
];
const TEST_PASS_KEYS: &[&str] = &["verifier_id"];
const DIFF_ALLOWLIST_KEYS: &[&str] = &["allow_paths"];

const VERIFIER_KEYS: &[&str] = &["commands"];
const VERIFIER_COMMAND_KEYS: &[&str] = &[
    "id",
    "argv",
    "cwd",
    "expected_exit",
    "timeout_seconds",
    "stdout_sha256",
    "stderr_sha256",
];

const INTEGRITY_KEYS: &[&str] = &["content_sha256", "hash_domain"];

pub(crate) const SKILL_RELEVANCE_ALLOWED: &str = "direct|routing-test|control";
pub(crate) const TASK_KIND_ALLOWED: &str = "positive|negative-control|calibration";
pub(crate) const PROMPT_KIND_ALLOWED: &str = "inline|file";
pub(crate) const FIXTURE_KIND_ALLOWED: &str = "generated|repo_commit";
pub(crate) const ACCEPTANCE_KIND_ALLOWED: &str =
    "command_sequence|prohibited_absent|artifact_property|response_fact|test_pass|diff_allowlist";
pub(crate) const EVIDENCE_ALLOWED: &str =
    "fake-tool-audit|harness-events|final-message|verifier-run|tree";
pub(crate) const STEP_MATCH_ALLOWED: &str = "exact|pattern";
pub(crate) const ORDINAL_ALLOWED: &str = "any|first";
pub(crate) const PATTERN_MATCH_ALLOWED: &str = "substring|regex";
pub(crate) const ARTIFACT_SOURCE_ALLOWED: &str = "tree|fake-tool-audit";
pub(crate) const ARTIFACT_PROPERTY_ALLOWED: &str = "exists|absent|sha256|contains|matches_regex";
pub(crate) const FAKE_TOOL_PROFILE_ALLOWED: &str =
    "root.healthy.v1|root.unhealthy.v1|absent|installer.record-deny.v1|installer.record-pass.v1";
pub(crate) const BREACH_POLICY_ALLOWED: &str = "terminate-and-classify";
pub(crate) const HASH_DOMAIN_ALLOWED: &str = "task-record.v1";
pub(crate) const TOOL_PERMISSION_ALLOWED: &str = "allow|deny|ask";
pub(crate) const TERMINATION_SUBTYPE_ALLOWED: &str = "success|denied_permission|max_turns|max_tokens|cost_cap|timeout|cancel|provider_error|infrastructure_error|harness_crash";
pub(crate) const TERMINATION_CLASSIFICATION_ALLOWED: &str = "task_failure|invalid_trial";

/// Parse a document and fully validate it for evaluation use.
pub(crate) fn parse_evaluated(content: &str) -> Result<(RawTaskRecord, Value), TaskError> {
    let document = parse_document(content)?;
    let table = document.as_table().ok_or_else(|| TaskError::TypeInvalid {
        field: "<document>".to_string(),
        expected: "TOML table".to_string(),
    })?;
    peek_schema_version(table)?;
    reject_floats(&document, "")?;
    validate_structure(&document)?;

    let pending = collect_pending_digests(table);
    if let Some(field) = pending.into_iter().next() {
        return Err(TaskError::SentinelRefusal { field });
    }

    if !table.contains_key("integrity") {
        return Err(TaskError::MissingField {
            field: "integrity".to_string(),
        });
    }

    let raw = deserialize_raw(&document)?;
    Ok((raw, document))
}

/// Parse a document at the structural layer only.
///
/// Drafts may omit `[integrity]` and may carry `sha256:PENDING` in digest
/// fields. Everything else — schema ceiling, key sets, types, enum values,
/// and required non-integrity fields — is still enforced.
pub(crate) fn parse_draft(content: &str) -> Result<(Value, u32, Vec<String>), TaskError> {
    let document = parse_document(content)?;
    let table = document.as_table().ok_or_else(|| TaskError::TypeInvalid {
        field: "<document>".to_string(),
        expected: "TOML table".to_string(),
    })?;
    let version = peek_schema_version(table)?;
    reject_floats(&document, "")?;
    validate_structure(&document)?;
    let pending = collect_pending_digests(table);
    Ok((document, version, pending))
}

pub(crate) fn parse_document(content: &str) -> Result<Value, TaskError> {
    toml::from_str::<Value>(content).map_err(|error| TaskError::Syntax {
        message: error.to_string(),
    })
}

fn peek_schema_version(table: &Map<String, Value>) -> Result<u32, TaskError> {
    let value = table
        .get("schema_version")
        .ok_or_else(|| TaskError::MissingField {
            field: "schema_version".to_string(),
        })?;
    let Value::Integer(found) = value else {
        return Err(TaskError::TypeInvalid {
            field: "schema_version".to_string(),
            expected: "unsigned integer".to_string(),
        });
    };
    let Ok(version) = u32::try_from(*found) else {
        return Err(TaskError::SchemaVersion {
            found: *found,
            supported: TASK_SPEC_SCHEMA_VERSION,
        });
    };
    if version < TASK_SPEC_SCHEMA_VERSION || version > TASK_SPEC_MAX_SUPPORTED_VERSION {
        return Err(TaskError::SchemaVersion {
            found: *found,
            supported: TASK_SPEC_MAX_SUPPORTED_VERSION,
        });
    }
    Ok(version)
}

fn reject_floats(value: &Value, path: &str) -> Result<(), TaskError> {
    match value {
        Value::Float(_) => Err(TaskError::FloatRejected {
            field: if path.is_empty() {
                "<root>".to_string()
            } else {
                path.to_string()
            },
        }),
        Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            for key in keys {
                reject_floats(&table[key], &path_of(path, key))?;
            }
            Ok(())
        }
        Value::Array(values) => {
            for (index, item) in values.iter().enumerate() {
                reject_floats(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_structure(document: &Value) -> Result<(), TaskError> {
    let table = expect_table(document, "<document>")?;
    check_keys(table, "", ROOT_KEYS)?;

    expect_string(required(table, "", "task_id")?, "task_id")?;
    expect_u32(required(table, "", "version")?, "version")?;
    expect_string(required(table, "", "title")?, "title")?;
    let _: SkillRelevance = parse_enum_value(
        required(table, "", "skill_relevance")?,
        "skill_relevance",
        SKILL_RELEVANCE_ALLOWED,
    )?;
    expect_string(required(table, "", "skill_under_test")?, "skill_under_test")?;
    if let Some(value) = table.get("task_kind") {
        let _: TaskKind = parse_enum_value(value, "task_kind", TASK_KIND_ALLOWED)?;
    }

    validate_prompt_shape(required(table, "", "prompt")?)?;
    validate_fixture_shape(required(table, "", "fixture")?)?;
    validate_environment_shape(required(table, "", "environment")?)?;
    validate_budgets_shape(required(table, "", "budgets")?)?;
    validate_acceptance_shape(required(table, "", "acceptance")?)?;
    if let Some(value) = table.get("verifier") {
        validate_verifier_shape(value)?;
    }
    if let Some(value) = table.get("termination_overrides") {
        validate_termination_overrides_shape(value)?;
    }
    if let Some(value) = table.get("integrity") {
        validate_integrity_shape(value)?;
    }
    Ok(())
}

fn validate_prompt_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "prompt")?;
    check_keys(table, "prompt", PROMPT_KEYS)?;
    let kind: PromptKind = parse_enum_value(
        required(table, "prompt", "kind")?,
        "prompt.kind",
        PROMPT_KIND_ALLOWED,
    )?;
    match kind {
        PromptKind::Inline => {
            expect_string(required(table, "prompt", "inline")?, "prompt.inline")?;
            for forbidden in ["file", "file_sha256"] {
                if table.contains_key(forbidden) {
                    return Err(TaskError::PromptRule {
                        field: path_of("prompt", forbidden),
                        reason: "not permitted when prompt.kind = inline".to_string(),
                    });
                }
            }
        }
        PromptKind::File => {
            expect_string(required(table, "prompt", "file")?, "prompt.file")?;
            expect_digest_string(
                required(table, "prompt", "file_sha256")?,
                "prompt.file_sha256",
            )?;
            if table.contains_key("inline") {
                return Err(TaskError::PromptRule {
                    field: "prompt.inline".to_string(),
                    reason: "not permitted when prompt.kind = file".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn validate_fixture_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "fixture")?;
    check_keys(table, "fixture", FIXTURE_KEYS)?;
    let kind: FixtureKind = parse_enum_value(
        required(table, "fixture", "kind")?,
        "fixture.kind",
        FIXTURE_KIND_ALLOWED,
    )?;
    expect_string(
        required(table, "fixture", "entry_point")?,
        "fixture.entry_point",
    )?;
    expect_u64(
        required(table, "fixture", "max_tree_bytes")?,
        "fixture.max_tree_bytes",
    )?;

    match kind {
        FixtureKind::Generated => {
            for forbidden in ["repo", "commit", "tree_sha256"] {
                if table.contains_key(forbidden) {
                    return Err(TaskError::FixtureRule {
                        field: path_of("fixture", forbidden),
                        reason: "not permitted when fixture.kind = generated".to_string(),
                    });
                }
            }
            let files =
                expect_array_of_tables(required(table, "fixture", "files")?, "fixture.files")?;
            for (index, file) in files.iter().enumerate() {
                let location = format!("fixture.files[{index}]");
                check_keys(file, &location, FIXTURE_FILE_KEYS)?;
                expect_string(
                    required(file, &location, "path")?,
                    &path_of(&location, "path"),
                )?;
                expect_string(
                    required(file, &location, "content")?,
                    &path_of(&location, "content"),
                )?;
                expect_digest_string(
                    required(file, &location, "content_sha256")?,
                    &path_of(&location, "content_sha256"),
                )?;
            }
        }
        FixtureKind::RepoCommit => {
            if table.contains_key("files") {
                return Err(TaskError::FixtureRule {
                    field: "fixture.files".to_string(),
                    reason: "not permitted when fixture.kind = repo_commit".to_string(),
                });
            }
            expect_string(required(table, "fixture", "repo")?, "fixture.repo")?;
            expect_string(required(table, "fixture", "commit")?, "fixture.commit")?;
            expect_digest_string(
                required(table, "fixture", "tree_sha256")?,
                "fixture.tree_sha256",
            )?;
        }
    }
    Ok(())
}

fn validate_environment_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "environment")?;
    check_keys(table, "environment", ENVIRONMENT_KEYS)?;
    expect_string_array(
        required(table, "environment", "allowed_tools")?,
        "environment.allowed_tools",
    )?;
    expect_string_array(
        required(table, "environment", "env_allowlist")?,
        "environment.env_allowlist",
    )?;

    if let Some(value) = table.get("tool_permissions") {
        let permissions = expect_table(value, "environment.tool_permissions")?;
        for (tool, permission) in permissions {
            let field = format!("environment.tool_permissions.{tool}");
            let _: ToolPermission = parse_enum_value(permission, &field, TOOL_PERMISSION_ALLOWED)?;
        }
    }

    if let Some(value) = table.get("fake_tools") {
        let tools = expect_array_of_tables(value, "environment.fake_tools")?;
        for (index, tool) in tools.iter().enumerate() {
            let location = format!("environment.fake_tools[{index}]");
            check_keys(tool, &location, FAKE_TOOL_KEYS)?;
            expect_string(
                required(tool, &location, "provides")?,
                &path_of(&location, "provides"),
            )?;
            let _: FakeToolProfile = parse_enum_value(
                required(tool, &location, "profile")?,
                &path_of(&location, "profile"),
                FAKE_TOOL_PROFILE_ALLOWED,
            )?;
            if let Some(script) = tool.get("script_sha256") {
                expect_digest_string(script, &path_of(&location, "script_sha256"))?;
            }
        }
    }
    Ok(())
}

fn validate_budgets_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "budgets")?;
    check_keys(table, "budgets", BUDGET_KEYS)?;
    expect_u32(
        required(table, "budgets", "wall_time_seconds_max")?,
        "budgets.wall_time_seconds_max",
    )?;
    if let Some(value) = table.get("turns_max") {
        expect_u32(value, "budgets.turns_max")?;
    }
    if let Some(value) = table.get("tokens_max") {
        expect_u64(value, "budgets.tokens_max")?;
    }
    if let Some(value) = table.get("cost_usd_max") {
        expect_string(value, "budgets.cost_usd_max")?;
    }
    let _: BreachPolicy = parse_enum_value(
        required(table, "budgets", "breach_policy")?,
        "budgets.breach_policy",
        BREACH_POLICY_ALLOWED,
    )?;
    Ok(())
}

fn validate_acceptance_shape(value: &Value) -> Result<(), TaskError> {
    let entries = expect_array_of_tables(value, "acceptance")?;
    for (index, entry) in entries.iter().enumerate() {
        let location = format!("acceptance[{index}]");
        let kind: AcceptanceKind = parse_enum_value(
            required(entry, &location, "kind")?,
            &path_of(&location, "kind"),
            ACCEPTANCE_KIND_ALLOWED,
        )?;
        let mut allowed = ACCEPTANCE_KEYS.to_vec();
        allowed.extend_from_slice(match kind {
            AcceptanceKind::CommandSequence => COMMAND_SEQUENCE_KEYS,
            AcceptanceKind::ProhibitedAbsent => PROHIBITED_ABSENT_KEYS,
            AcceptanceKind::ArtifactProperty => ARTIFACT_PROPERTY_KEYS,
            AcceptanceKind::ResponseFact => RESPONSE_FACT_KEYS,
            AcceptanceKind::TestPass => TEST_PASS_KEYS,
            AcceptanceKind::DiffAllowlist => DIFF_ALLOWLIST_KEYS,
        });
        check_keys(entry, &location, &allowed)?;

        expect_string(required(entry, &location, "id")?, &path_of(&location, "id"))?;
        expect_bool(
            required(entry, &location, "primary")?,
            &path_of(&location, "primary"),
        )?;
        let _: EvidenceChannel = parse_enum_value(
            required(entry, &location, "evidence")?,
            &path_of(&location, "evidence"),
            EVIDENCE_ALLOWED,
        )?;

        match kind {
            AcceptanceKind::CommandSequence => {
                let steps = expect_array_of_tables(
                    required(entry, &location, "steps")?,
                    &path_of(&location, "steps"),
                )?;
                for (step_index, step) in steps.iter().enumerate() {
                    validate_command_step_shape(step, &location, step_index)?;
                }
                if let Some(value) = entry.get("allow_interleaved") {
                    expect_bool(value, &path_of(&location, "allow_interleaved"))?;
                }
                if let Some(value) = entry.get("forbid_after_last_step") {
                    expect_bool(value, &path_of(&location, "forbid_after_last_step"))?;
                }
            }
            AcceptanceKind::ProhibitedAbsent => {
                let patterns = expect_array_of_tables(
                    required(entry, &location, "patterns")?,
                    &path_of(&location, "patterns"),
                )?;
                for (pattern_index, pattern) in patterns.iter().enumerate() {
                    let pattern_location = format!("{location}.patterns[{pattern_index}]");
                    check_keys(pattern, &pattern_location, PROHIBITED_PATTERN_KEYS)?;
                    expect_string(
                        required(pattern, &pattern_location, "pattern")?,
                        &path_of(&pattern_location, "pattern"),
                    )?;
                    let _: PatternMatch = parse_enum_value(
                        required(pattern, &pattern_location, "match")?,
                        &path_of(&pattern_location, "match"),
                        PATTERN_MATCH_ALLOWED,
                    )?;
                    if let Some(tool) = pattern.get("tool") {
                        expect_string(tool, &path_of(&pattern_location, "tool"))?;
                    }
                }
            }
            AcceptanceKind::ArtifactProperty => {
                let _: ArtifactSource = parse_enum_value(
                    required(entry, &location, "source")?,
                    &path_of(&location, "source"),
                    ARTIFACT_SOURCE_ALLOWED,
                )?;
                expect_string(
                    required(entry, &location, "path")?,
                    &path_of(&location, "path"),
                )?;
                let _: ArtifactPropertyName = parse_enum_value(
                    required(entry, &location, "property")?,
                    &path_of(&location, "property"),
                    ARTIFACT_PROPERTY_ALLOWED,
                )?;
                if let Some(value) = entry.get("value") {
                    expect_string(value, &path_of(&location, "value"))?;
                }
            }
            AcceptanceKind::ResponseFact => {
                if let Some(value) = entry.get("strip_code_fences") {
                    expect_bool(value, &path_of(&location, "strip_code_fences"))?;
                }
                if let Some(value) = entry.get("expect_substrings") {
                    expect_string_array(value, &path_of(&location, "expect_substrings"))?;
                }
                if let Some(value) = entry.get("expect_any_of") {
                    expect_string_matrix(value, &path_of(&location, "expect_any_of"))?;
                }
                if let Some(value) = entry.get("forbid_substrings") {
                    expect_string_array(value, &path_of(&location, "forbid_substrings"))?;
                }
                if let Some(value) = entry.get("forbid_regex") {
                    expect_string(value, &path_of(&location, "forbid_regex"))?;
                }
            }
            AcceptanceKind::TestPass => {
                expect_string(
                    required(entry, &location, "verifier_id")?,
                    &path_of(&location, "verifier_id"),
                )?;
            }
            AcceptanceKind::DiffAllowlist => {
                expect_string_array(
                    required(entry, &location, "allow_paths")?,
                    &path_of(&location, "allow_paths"),
                )?;
            }
        }
    }
    Ok(())
}

fn validate_command_step_shape(
    step: &Map<String, Value>,
    acceptance_location: &str,
    index: usize,
) -> Result<(), TaskError> {
    let location = format!("{acceptance_location}.steps[{index}]");
    check_keys(step, &location, COMMAND_STEP_KEYS)?;
    let _: CommandStepMatch = parse_enum_value(
        required(step, &location, "match")?,
        &path_of(&location, "match"),
        STEP_MATCH_ALLOWED,
    )?;
    if let Some(value) = step.get("argv") {
        expect_string_array(value, &path_of(&location, "argv"))?;
    }
    if let Some(value) = step.get("regex") {
        expect_string(value, &path_of(&location, "regex"))?;
    }
    if let Some(value) = step.get("ordinal") {
        let _: Ordinal = parse_enum_value(value, &path_of(&location, "ordinal"), ORDINAL_ALLOWED)?;
    }
    Ok(())
}

fn validate_verifier_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "verifier")?;
    check_keys(table, "verifier", VERIFIER_KEYS)?;
    if let Some(commands) = table.get("commands") {
        let commands = expect_array_of_tables(commands, "verifier.commands")?;
        for (index, command) in commands.iter().enumerate() {
            let location = format!("verifier.commands[{index}]");
            check_keys(command, &location, VERIFIER_COMMAND_KEYS)?;
            expect_string(
                required(command, &location, "id")?,
                &path_of(&location, "id"),
            )?;
            expect_string_array(
                required(command, &location, "argv")?,
                &path_of(&location, "argv"),
            )?;
            if let Some(cwd) = command.get("cwd") {
                expect_string(cwd, &path_of(&location, "cwd"))?;
            }
            expect_i32(
                required(command, &location, "expected_exit")?,
                &path_of(&location, "expected_exit"),
            )?;
            expect_u32(
                required(command, &location, "timeout_seconds")?,
                &path_of(&location, "timeout_seconds"),
            )?;
            if let Some(value) = command.get("stdout_sha256") {
                expect_digest_string(value, &path_of(&location, "stdout_sha256"))?;
            }
            if let Some(value) = command.get("stderr_sha256") {
                expect_digest_string(value, &path_of(&location, "stderr_sha256"))?;
            }
        }
    }
    Ok(())
}

fn validate_termination_overrides_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "termination_overrides")?;
    for (subtype, classification) in table {
        let field = format!("termination_overrides.{subtype}");
        let _: TerminationSubtype = parse_enum_str(subtype, &field, TERMINATION_SUBTYPE_ALLOWED)?;
        let _: TerminationClassification =
            parse_enum_value(classification, &field, TERMINATION_CLASSIFICATION_ALLOWED)?;
    }
    Ok(())
}

fn validate_integrity_shape(value: &Value) -> Result<(), TaskError> {
    let table = expect_table(value, "integrity")?;
    check_keys(table, "integrity", INTEGRITY_KEYS)?;
    expect_digest_string(
        required(table, "integrity", "content_sha256")?,
        "integrity.content_sha256",
    )?;
    let _: HashDomain = parse_enum_value(
        required(table, "integrity", "hash_domain")?,
        "integrity.hash_domain",
        HASH_DOMAIN_ALLOWED,
    )?;
    Ok(())
}

pub(crate) fn collect_pending_digests(table: &Map<String, Value>) -> Vec<String> {
    let mut pending = Vec::new();

    if let Some(prompt) = table.get("prompt").and_then(Value::as_table) {
        push_pending(
            &mut pending,
            "prompt.file_sha256",
            prompt.get("file_sha256"),
        );
    }
    if let Some(fixture) = table.get("fixture").and_then(Value::as_table) {
        push_pending(
            &mut pending,
            "fixture.tree_sha256",
            fixture.get("tree_sha256"),
        );
        if let Some(files) = fixture.get("files").and_then(Value::as_array) {
            for (index, file) in files.iter().enumerate() {
                if let Some(file) = file.as_table() {
                    push_pending(
                        &mut pending,
                        &format!("fixture.files[{index}].content_sha256"),
                        file.get("content_sha256"),
                    );
                }
            }
        }
    }
    if let Some(environment) = table.get("environment").and_then(Value::as_table) {
        if let Some(tools) = environment.get("fake_tools").and_then(Value::as_array) {
            for (index, tool) in tools.iter().enumerate() {
                if let Some(tool) = tool.as_table() {
                    push_pending(
                        &mut pending,
                        &format!("environment.fake_tools[{index}].script_sha256"),
                        tool.get("script_sha256"),
                    );
                }
            }
        }
    }
    if let Some(acceptance) = table.get("acceptance").and_then(Value::as_array) {
        for (index, assertion) in acceptance.iter().enumerate() {
            if let Some(assertion) = assertion.as_table() {
                push_pending(
                    &mut pending,
                    &format!("acceptance[{index}].value"),
                    assertion.get("value"),
                );
            }
        }
    }
    if let Some(verifier) = table.get("verifier").and_then(Value::as_table) {
        if let Some(commands) = verifier.get("commands").and_then(Value::as_array) {
            for (index, command) in commands.iter().enumerate() {
                if let Some(command) = command.as_table() {
                    for digest in ["stdout_sha256", "stderr_sha256"] {
                        push_pending(
                            &mut pending,
                            &format!("verifier.commands[{index}].{digest}"),
                            command.get(digest),
                        );
                    }
                }
            }
        }
    }
    if let Some(integrity) = table.get("integrity").and_then(Value::as_table) {
        push_pending(
            &mut pending,
            "integrity.content_sha256",
            integrity.get("content_sha256"),
        );
    }

    pending
}

fn push_pending(pending: &mut Vec<String>, field: &str, value: Option<&Value>) {
    if value
        .and_then(Value::as_str)
        .is_some_and(is_pending_sentinel)
    {
        pending.push(field.to_string());
    }
}

fn deserialize_raw(document: &Value) -> Result<RawTaskRecord, TaskError> {
    document
        .clone()
        .try_into::<RawTaskRecord>()
        .map_err(map_deserialize_error)
}

fn map_deserialize_error(error: toml::de::Error) -> TaskError {
    let message = error.to_string();
    if let Some(field) = error_message_field(&message, "missing field `") {
        return TaskError::MissingField { field };
    }
    if let Some(field) = error_message_field(&message, "unknown field `") {
        return TaskError::UnknownField {
            field,
            location: "<document>".to_string(),
        };
    }
    TaskError::TypeInvalid {
        field: "<document>".to_string(),
        expected: message,
    }
}

fn error_message_field(message: &str, marker: &str) -> Option<String> {
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// Structural helpers
// ---------------------------------------------------------------------------

fn path_of(location: &str, key: &str) -> String {
    if location.is_empty() {
        key.to_string()
    } else {
        format!("{location}.{key}")
    }
}

fn check_keys(
    table: &Map<String, Value>,
    location: &str,
    allowed: &[&str],
) -> Result<(), TaskError> {
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    for key in keys {
        if !allowed.contains(&key.as_str()) {
            return Err(TaskError::UnknownField {
                field: key.clone(),
                location: if location.is_empty() {
                    "<root>".to_string()
                } else {
                    location.to_string()
                },
            });
        }
    }
    Ok(())
}

fn required<'a>(
    table: &'a Map<String, Value>,
    location: &str,
    key: &str,
) -> Result<&'a Value, TaskError> {
    table.get(key).ok_or_else(|| TaskError::MissingField {
        field: path_of(location, key),
    })
}

fn expect_table<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, TaskError> {
    value.as_table().ok_or_else(|| TaskError::TypeInvalid {
        field: field.to_string(),
        expected: "table".to_string(),
    })
}

fn expect_array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>, TaskError> {
    value.as_array().ok_or_else(|| TaskError::TypeInvalid {
        field: field.to_string(),
        expected: "array".to_string(),
    })
}

fn expect_array_of_tables<'a>(
    value: &'a Value,
    field: &str,
) -> Result<Vec<&'a Map<String, Value>>, TaskError> {
    let array = expect_array(value, field)?;
    array
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.as_table().ok_or_else(|| TaskError::TypeInvalid {
                field: format!("{field}[{index}]"),
                expected: "table".to_string(),
            })
        })
        .collect()
}

fn expect_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, TaskError> {
    value.as_str().ok_or_else(|| TaskError::TypeInvalid {
        field: field.to_string(),
        expected: "string".to_string(),
    })
}

fn expect_digest_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, TaskError> {
    value.as_str().ok_or_else(|| TaskError::DigestInvalid {
        field: field.to_string(),
        reason: "expected a string".to_string(),
    })
}

fn expect_bool(value: &Value, field: &str) -> Result<bool, TaskError> {
    value.as_bool().ok_or_else(|| TaskError::TypeInvalid {
        field: field.to_string(),
        expected: "boolean".to_string(),
    })
}

fn expect_u32(value: &Value, field: &str) -> Result<u32, TaskError> {
    let integer = expect_integer(value, field)?;
    u32::try_from(integer).map_err(|_| TaskError::RangeInvalid {
        field: field.to_string(),
        reason: "expected an unsigned 32-bit integer".to_string(),
    })
}

fn expect_u64(value: &Value, field: &str) -> Result<u64, TaskError> {
    let integer = expect_integer(value, field)?;
    u64::try_from(integer).map_err(|_| TaskError::RangeInvalid {
        field: field.to_string(),
        reason: "expected an unsigned 64-bit integer".to_string(),
    })
}

fn expect_i32(value: &Value, field: &str) -> Result<i32, TaskError> {
    let integer = expect_integer(value, field)?;
    i32::try_from(integer).map_err(|_| TaskError::RangeInvalid {
        field: field.to_string(),
        reason: "expected a signed 32-bit integer".to_string(),
    })
}

fn expect_integer(value: &Value, field: &str) -> Result<i64, TaskError> {
    value.as_integer().ok_or_else(|| TaskError::TypeInvalid {
        field: field.to_string(),
        expected: "integer".to_string(),
    })
}

fn expect_string_array(value: &Value, field: &str) -> Result<(), TaskError> {
    let array = expect_array(value, field)?;
    for (index, item) in array.iter().enumerate() {
        expect_string(item, &format!("{field}[{index}]"))?;
    }
    Ok(())
}

fn expect_string_matrix(value: &Value, field: &str) -> Result<(), TaskError> {
    let array = expect_array(value, field)?;
    for (index, group) in array.iter().enumerate() {
        expect_string_array(group, &format!("{field}[{index}]"))?;
    }
    Ok(())
}

fn parse_enum_value<T: DeserializeOwned>(
    value: &Value,
    field: &str,
    allowed: &'static str,
) -> Result<T, TaskError> {
    let raw = expect_string(value, field)?;
    parse_enum_str(raw, field, allowed)
}

fn parse_enum_str<T: DeserializeOwned>(
    raw: &str,
    field: &str,
    allowed: &'static str,
) -> Result<T, TaskError> {
    Value::String(raw.to_string())
        .try_into::<T>()
        .map_err(|_| TaskError::EnumInvalid {
            field: field.to_string(),
            value: raw.to_string(),
            allowed,
        })
}
