//! One focused refusal test per TASK-SPEC rule.
//!
//! Every test asserts the machine-matchable error kind (and usually the field
//! path), so the error taxonomy — not a message — is the contract.

mod common;

use root_eval::{ErrorKind, TaskRecord};

fn base() -> String {
    common::sealed_base()
}

fn mutated(from: &str, to: &str) -> String {
    base().replacen(from, to, 1)
}

fn sealed_full() -> String {
    common::seal(&common::full_document())
}

fn full_mutated(from: &str, to: &str) -> String {
    sealed_full().replacen(from, to, 1)
}

// ---------------------------------------------------------------------------
// Unknown fields (§1.2 rule 1)
// ---------------------------------------------------------------------------

#[test]
fn unknown_root_field_is_refused() {
    let document = base().replace("title =", "mystery = 1\ntitle =");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_acceptance_field_is_refused() {
    let document = base().replace("primary = true", "primary = true\nmystery = 1");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn wrong_kind_acceptance_field_is_refused() {
    // `allow_paths` belongs to diff_allowlist, not command_sequence.
    let document = base().replace("primary = true", "primary = true\nallow_paths = []");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("allow_paths"));
}

#[test]
fn unknown_budgets_field_is_refused() {
    let document = mutated("[budgets]", "[budgets]\nmystery = 1");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_fixture_field_is_refused() {
    let document = mutated(
        "max_tree_bytes = 20000",
        "max_tree_bytes = 20000\nmystery = \"x\"",
    );
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_environment_field_is_refused() {
    let document = mutated("[environment]", "[environment]\nmystery = true");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_step_field_is_refused() {
    let document = mutated("ordinal = \"first\"", "ordinal = \"first\", mystery = 1");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_verifier_field_is_refused() {
    let document = base().replace("[integrity]", "[verifier]\nmystery = 1\n\n[integrity]");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_verifier_command_field_is_refused() {
    let document = full_mutated("cwd = \".\"", "cwd = \".\"\nmystery = 1");
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_prohibited_pattern_field_is_refused() {
    let document = full_mutated(
        "{ match = \"substring\", pattern = \"brew install\" }",
        "{ match = \"substring\", pattern = \"brew install\", mystery = 1 }",
    );
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

#[test]
fn unknown_fixture_file_field_is_refused() {
    let document = mutated(
        "content_sha256 = \"",
        "size_bytes = 14\ncontent_sha256 = \"",
    );
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("size_bytes"));
}

#[test]
fn unknown_fake_tool_field_is_refused() {
    let document = mutated(
        "[budgets]",
        "[[environment.fake_tools]]\nprovides = \"root\"\nprofile = \"absent\"\nmystery = 1\n\n[budgets]",
    );
    common::assert_refusal(&document, ErrorKind::UnknownField, Some("mystery"));
}

// ---------------------------------------------------------------------------
// Schema-version gate (§2.1, §1.2 rule 2)
// ---------------------------------------------------------------------------

#[test]
fn schema_version_above_max_is_refused() {
    let document = common::base_document().replace("schema_version = 1", "schema_version = 2");
    common::assert_refusal(&document, ErrorKind::SchemaVersion, Some("schema_version"));
}

#[test]
fn schema_version_zero_is_refused() {
    let document = common::base_document().replace("schema_version = 1", "schema_version = 0");
    common::assert_refusal(&document, ErrorKind::SchemaVersion, Some("schema_version"));
}

#[test]
fn schema_version_negative_is_refused() {
    let document = common::base_document().replace("schema_version = 1", "schema_version = -1");
    common::assert_refusal(&document, ErrorKind::SchemaVersion, Some("schema_version"));
}

#[test]
fn missing_schema_version_is_refused() {
    let document = common::base_document().replace("schema_version = 1\n", "");
    common::assert_refusal(&document, ErrorKind::MissingField, Some("schema_version"));
}

#[test]
fn non_integer_schema_version_is_refused() {
    let document = common::base_document().replace("schema_version = 1", "schema_version = \"1\"");
    common::assert_refusal(&document, ErrorKind::TypeInvalid, Some("schema_version"));
}

// ---------------------------------------------------------------------------
// Sentinel refusal (§2.10.3) — distinct from DigestInvalid
// ---------------------------------------------------------------------------

#[test]
fn pending_fixture_digest_is_a_sentinel_refusal() {
    let document = mutated(common::CONTENT_SHA, "sha256:PENDING");
    let error = common::assert_refusal(
        &document,
        ErrorKind::SentinelRefusal,
        Some("fixture.files[0].content_sha256"),
    );
    assert!(error.is_draft_refusal());
}

#[test]
fn pending_integrity_digest_is_a_sentinel_refusal() {
    let digest = common::parse_base().canonical_hash().to_string();
    let document = base().replace(&digest, "sha256:PENDING");
    common::assert_refusal(
        &document,
        ErrorKind::SentinelRefusal,
        Some("integrity.content_sha256"),
    );
}

// ---------------------------------------------------------------------------
// Digest grammar (§2.1, §1.2 rule 3)
// ---------------------------------------------------------------------------

fn content_digest_field() -> &'static str {
    "fixture.files[0].content_sha256"
}

#[test]
fn uppercase_digest_is_refused() {
    let document = mutated(common::CONTENT_SHA, &common::CONTENT_SHA.to_uppercase());
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

#[test]
fn short_digest_is_refused() {
    let short = &common::CONTENT_SHA[..common::CONTENT_SHA.len() - 1];
    let document = mutated(common::CONTENT_SHA, short);
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

#[test]
fn long_digest_is_refused() {
    let long = format!("{}a", common::CONTENT_SHA);
    let document = mutated(common::CONTENT_SHA, &long);
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

#[test]
fn unprefixed_digest_is_refused() {
    let bare = common::CONTENT_SHA.trim_start_matches("sha256:");
    let document = mutated(common::CONTENT_SHA, bare);
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

#[test]
fn empty_digest_is_refused() {
    let document = mutated(
        &format!("content_sha256 = \"{}\"", common::CONTENT_SHA),
        "content_sha256 = \"\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

#[test]
fn non_string_digest_is_refused() {
    let document = mutated(
        &format!("content_sha256 = \"{}\"", common::CONTENT_SHA),
        "content_sha256 = 5",
    );
    common::assert_refusal(
        &document,
        ErrorKind::DigestInvalid,
        Some(content_digest_field()),
    );
}

// ---------------------------------------------------------------------------
// Control characters and empty strings (§1.2 rule 3)
// ---------------------------------------------------------------------------

#[test]
fn control_character_in_title_is_refused() {
    let document = mutated(
        "title = \"Install a supported CLI\"",
        "title = \"bad\\u0001title\"",
    );
    common::assert_refusal(&document, ErrorKind::ControlChars, Some("title"));
}

#[test]
fn carriage_return_in_fixture_content_is_refused() {
    let document = mutated(
        "content = \"fixture body\\n\"",
        "content = \"fixture body\\r\\n\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::ControlChars,
        Some("fixture.files[0].content"),
    );
}

#[test]
fn nul_in_fixture_content_is_refused() {
    let document = mutated(
        "content = \"fixture body\\n\"",
        "content = \"bad\\u0000body\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::ControlChars,
        Some("fixture.files[0].content"),
    );
}

#[test]
fn empty_title_is_refused() {
    let document = mutated("title = \"Install a supported CLI\"", "title = \"\"");
    common::assert_refusal(&document, ErrorKind::EmptyString, Some("title"));
}

#[test]
fn empty_prompt_inline_is_refused() {
    let document = mutated(
        "inline = \"Please install the tool from the project instructions.\"",
        "inline = \"\"",
    );
    common::assert_refusal(&document, ErrorKind::EmptyString, Some("prompt.inline"));
}

// ---------------------------------------------------------------------------
// Enums (§2 — closed sets)
// ---------------------------------------------------------------------------

#[test]
fn invalid_skill_relevance_is_refused() {
    let document = mutated(
        "skill_relevance = \"direct\"",
        "skill_relevance = \"sometimes\"",
    );
    common::assert_refusal(&document, ErrorKind::EnumInvalid, Some("skill_relevance"));
}

#[test]
fn invalid_task_kind_is_refused() {
    let document = mutated(
        "skill_under_test = \"root-safe-install\"",
        "skill_under_test = \"root-safe-install\"\ntask_kind = \"control-x\"",
    );
    common::assert_refusal(&document, ErrorKind::EnumInvalid, Some("task_kind"));
}

#[test]
fn invalid_acceptance_kind_is_refused() {
    let document = mutated("kind = \"command_sequence\"", "kind = \"vibes\"");
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("acceptance[0].kind"),
    );
}

#[test]
fn invalid_fixture_kind_is_refused() {
    let document = mutated("kind = \"generated\"", "kind = \"magic\"");
    common::assert_refusal(&document, ErrorKind::EnumInvalid, Some("fixture.kind"));
}

#[test]
fn invalid_step_match_is_refused() {
    let document = mutated("match = \"exact\"", "match = \"glob\"");
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("acceptance[0].steps[0].match"),
    );
}

#[test]
fn invalid_step_ordinal_is_refused() {
    let document = mutated("ordinal = \"first\"", "ordinal = \"second\"");
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("acceptance[0].steps[0].ordinal"),
    );
}

#[test]
fn invalid_evidence_value_is_refused() {
    let document = mutated(
        "evidence = \"fake-tool-audit\"",
        "evidence = \"not-a-channel\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("acceptance[0].evidence"),
    );
}

#[test]
fn invalid_breach_policy_is_refused() {
    let document = mutated(
        "breach_policy = \"terminate-and-classify\"",
        "breach_policy = \"retry-forever\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("budgets.breach_policy"),
    );
}

#[test]
fn invalid_hash_domain_is_refused() {
    let document = mutated(
        "hash_domain = \"task-record.v1\"",
        "hash_domain = \"task-record.v2\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("integrity.hash_domain"),
    );
}

#[test]
fn invalid_fake_tool_profile_is_refused() {
    let document = mutated(
        "[budgets]",
        "[[environment.fake_tools]]\nprovides = \"root\"\nprofile = \"root.healthy.v2\"\nscript_sha256 = \"\
         sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n\n[budgets]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("environment.fake_tools[0].profile"),
    );
}

#[test]
fn invalid_tool_permission_value_is_refused() {
    let document = mutated(
        "[environment]",
        "[environment]\ntool_permissions = { webfetch = \"deny-all\" }",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("environment.tool_permissions.webfetch"),
    );
}

#[test]
fn invalid_artifact_property_is_refused() {
    let document = full_mutated("property = \"sha256\"", "property = \"hash\"");
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("acceptance[2].property"),
    );
}

// ---------------------------------------------------------------------------
// Evidence dispatch (§2.7.7)
// ---------------------------------------------------------------------------

#[test]
fn evidence_must_be_allowed_for_kind() {
    let document = mutated("evidence = \"fake-tool-audit\"", "evidence = \"tree\"");
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].evidence"),
    );
}

#[test]
fn response_fact_rejects_a_non_message_channel() {
    let document = full_mutated(
        "id = \"final.summary\"\nkind = \"response_fact\"\nprimary = true\nevidence = \"final-message\"",
        "id = \"final.summary\"\nkind = \"response_fact\"\nprimary = true\nevidence = \"harness-events\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[3].evidence"),
    );
}

// ---------------------------------------------------------------------------
// Budgets (§2.6)
// ---------------------------------------------------------------------------

#[test]
fn missing_wall_budget_is_refused() {
    let document = mutated("wall_time_seconds_max = 900\n", "");
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("budgets.wall_time_seconds_max"),
    );
}

#[test]
fn cost_with_one_decimal_is_refused() {
    let document = mutated("cost_usd_max = \"1.50\"", "cost_usd_max = \"1.5\"");
    common::assert_refusal(
        &document,
        ErrorKind::BudgetRule,
        Some("budgets.cost_usd_max"),
    );
}

#[test]
fn cost_with_three_decimals_is_refused() {
    let document = mutated("cost_usd_max = \"1.50\"", "cost_usd_max = \"1.500\"");
    common::assert_refusal(
        &document,
        ErrorKind::BudgetRule,
        Some("budgets.cost_usd_max"),
    );
}

#[test]
fn negative_token_budget_is_refused() {
    let document = mutated("tokens_max = 400000", "tokens_max = -1");
    common::assert_refusal(
        &document,
        ErrorKind::RangeInvalid,
        Some("budgets.tokens_max"),
    );
}

#[test]
fn out_of_range_turn_budget_is_refused() {
    let document = mutated("turns_max = 24", "turns_max = 4294967296");
    common::assert_refusal(
        &document,
        ErrorKind::RangeInvalid,
        Some("budgets.turns_max"),
    );
}

// ---------------------------------------------------------------------------
// Identity (§2.2, §2.10.2)
// ---------------------------------------------------------------------------

#[test]
fn uppercase_task_id_is_refused() {
    let document = mutated("sample-task-001", "Sample-Task-001");
    common::assert_refusal(&document, ErrorKind::NamingRule, Some("task_id"));
}

#[test]
fn leading_digit_task_id_is_refused() {
    let document = mutated("sample-task-001", "1sample-task");
    common::assert_refusal(&document, ErrorKind::NamingRule, Some("task_id"));
}

#[test]
fn overlong_task_id_is_refused() {
    let long = "a".repeat(65);
    let document = mutated("sample-task-001", &long);
    common::assert_refusal(&document, ErrorKind::NamingRule, Some("task_id"));
}

#[test]
fn empty_task_id_is_refused() {
    let document = mutated("task_id = \"sample-task-001\"", "task_id = \"\"");
    common::assert_refusal(&document, ErrorKind::NamingRule, Some("task_id"));
}

#[test]
fn version_zero_is_refused() {
    let document = common::sealed_base().replacen("\nversion = 1\n", "\nversion = 0\n", 1);
    common::assert_refusal(&document, ErrorKind::RangeInvalid, Some("version"));
}

// ---------------------------------------------------------------------------
// Prompt (§2.3)
// ---------------------------------------------------------------------------

#[test]
fn prompt_inline_with_file_field_is_refused() {
    let document = mutated(
        "kind = \"inline\"",
        "kind = \"inline\"\nfile = \"prompt.txt\"",
    );
    common::assert_refusal(&document, ErrorKind::PromptRule, Some("prompt.file"));
}

#[test]
fn prompt_file_without_digest_is_refused() {
    let document = mutated(
        "kind = \"inline\"",
        "kind = \"file\"\nfile = \"prompt.txt\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("prompt.file_sha256"),
    );
}

#[test]
fn overlong_prompt_is_refused() {
    let long = "a".repeat(5000);
    let document = mutated(
        "inline = \"Please install the tool from the project instructions.\"",
        &format!("inline = \"{long}\""),
    );
    common::assert_refusal(&document, ErrorKind::PromptRule, Some("prompt.inline"));
}

// ---------------------------------------------------------------------------
// Fixture (§2.4)
// ---------------------------------------------------------------------------

const BASE_FILE_BLOCK: &str = "[[fixture.files]]\npath = \"README.md\"\ncontent = \"fixture body\\n\"\ncontent_sha256 = \"sha256:51dbae01c73ea7f67a332b4f0bcce942f43f456aff9615ef632c0c1d4e719f37\"\n";

#[test]
fn missing_fixture_files_is_refused() {
    let document = base().replace(BASE_FILE_BLOCK, "");
    common::assert_refusal(&document, ErrorKind::MissingField, Some("fixture.files"));
}

#[test]
fn empty_fixture_file_list_is_refused() {
    let document = base().replace(BASE_FILE_BLOCK, "").replace(
        "max_tree_bytes = 20000",
        "max_tree_bytes = 20000\nfiles = []",
    );
    common::assert_refusal(&document, ErrorKind::FixtureRule, Some("fixture.files"));
}

#[test]
fn generated_fixture_with_repo_field_is_refused() {
    let document = mutated(
        "kind = \"generated\"",
        "kind = \"generated\"\nrepo = \"https://example.invalid/repo.git\"",
    );
    common::assert_refusal(&document, ErrorKind::FixtureRule, Some("fixture.repo"));
}

#[test]
fn repo_commit_fixture_with_files_is_refused() {
    let document = mutated(
        "kind = \"generated\"",
        "kind = \"repo_commit\"\nrepo = \"https://example.invalid/repo.git\"\ncommit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\ntree_sha256 = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
    );
    common::assert_refusal(&document, ErrorKind::FixtureRule, Some("fixture.files"));
}

#[test]
fn repo_commit_with_bad_commit_hash_is_refused() {
    let document = base()
        .replace("kind = \"generated\"", "kind = \"repo_commit\"")
        .replace(
            BASE_FILE_BLOCK,
            "repo = \"https://example.invalid/repo.git\"\ncommit = \"not-a-commit\"\ntree_sha256 = \"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        );
    common::assert_refusal(&document, ErrorKind::FixtureRule, Some("fixture.commit"));
}

#[test]
fn absolute_entry_point_is_refused() {
    let document = mutated("entry_point = \".\"", "entry_point = \"/tmp\"");
    common::assert_refusal(
        &document,
        ErrorKind::PathInvalid,
        Some("fixture.entry_point"),
    );
}

#[test]
fn parent_traversal_file_path_is_refused() {
    let document = mutated("path = \"README.md\"", "path = \"../escape.txt\"");
    common::assert_refusal(
        &document,
        ErrorKind::PathInvalid,
        Some("fixture.files[0].path"),
    );
}

#[test]
fn duplicate_fixture_paths_are_refused() {
    let document = base().replace(
        BASE_FILE_BLOCK,
        &format!("{BASE_FILE_BLOCK}{BASE_FILE_BLOCK}"),
    );
    common::assert_refusal(
        &document,
        ErrorKind::Duplicate,
        Some("fixture.files[1].path"),
    );
}

#[test]
fn fixture_content_digest_must_recompute() {
    let document = mutated("content = \"fixture body\\n\"", "content = \"tampered\\n\"");
    common::assert_refusal(
        &document,
        ErrorKind::IntegrityMismatch,
        Some("fixture.files[0].content_sha256"),
    );
}

// ---------------------------------------------------------------------------
// Environment (§2.5)
// ---------------------------------------------------------------------------

#[test]
fn env_allowlist_missing_path_is_refused() {
    let document = mutated(
        "env_allowlist = [\"PATH\", \"HOME\", \"TMPDIR\", \"LANG\"]",
        "env_allowlist = [\"HOME\", \"TMPDIR\", \"LANG\"]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.env_allowlist"),
    );
}

#[test]
fn empty_allowed_tools_is_refused() {
    let document = mutated(
        "allowed_tools = [\"bash\", \"read\", \"write\", \"edit\", \"glob\", \"grep\", \"list\", \"skill\"]",
        "allowed_tools = []",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.allowed_tools"),
    );
}

#[test]
fn invalid_env_allowlist_name_is_refused() {
    let document = mutated("\"LANG\"]", "\"1PATH\"]");
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.env_allowlist[3]"),
    );
}

#[test]
fn env_glob_name_is_allowed() {
    let document = common::base_document().replace("\"LANG\"]", "\"LANG\", \"XDG_*\"]");
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("XDG_* is a valid glob");
    assert!(record
        .environment
        .env_allowlist
        .contains(&"XDG_*".to_string()));
}

#[test]
fn duplicate_fake_tool_provides_is_refused() {
    let tool = "[[environment.fake_tools]]\nprovides = \"root\"\nprofile = \"absent\"\n";
    let document = mutated("[budgets]", &format!("{tool}\n{tool}\n[budgets]"));
    common::assert_refusal(
        &document,
        ErrorKind::Duplicate,
        Some("environment.fake_tools[1].provides"),
    );
}

#[test]
fn absent_profile_with_script_is_refused() {
    let document = mutated(
        "[budgets]",
        "[[environment.fake_tools]]\nprovides = \"root\"\nprofile = \"absent\"\nscript_sha256 = \"\
         sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n\n[budgets]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.fake_tools[0].script_sha256"),
    );
}

#[test]
fn shim_profile_without_script_is_refused() {
    let document = mutated(
        "[budgets]",
        "[[environment.fake_tools]]\nprovides = \"root\"\nprofile = \"root.healthy.v1\"\n\n[budgets]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("environment.fake_tools[0].script_sha256"),
    );
}

#[test]
fn path_valued_fake_tool_provides_is_refused() {
    let document = mutated(
        "[budgets]",
        "[[environment.fake_tools]]\nprovides = \"/usr/bin/root\"\nprofile = \"absent\"\n\n[budgets]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.fake_tools[0].provides"),
    );
}

#[test]
fn invalid_tool_permission_key_is_refused() {
    let document = mutated(
        "[environment]",
        "[environment]\ntool_permissions = { WebFetch = \"deny\" }",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnvironmentRule,
        Some("environment.tool_permissions.WebFetch"),
    );
}

// ---------------------------------------------------------------------------
// Acceptance semantics (§2.7)
// ---------------------------------------------------------------------------

#[test]
fn acceptance_requires_a_primary_assertion() {
    let document = mutated("primary = true", "primary = false");
    common::assert_refusal(&document, ErrorKind::AcceptanceRule, Some("acceptance"));
}

#[test]
fn duplicate_acceptance_ids_are_refused() {
    let second = "[[acceptance]]\nid = \"seq.root\"\nkind = \"diff_allowlist\"\nprimary = false\nevidence = \"tree\"\nallow_paths = []\n";
    let document = base().replace(
        common::BASE_ACCEPTANCE,
        &format!("{}{second}", common::BASE_ACCEPTANCE),
    );
    common::assert_refusal(&document, ErrorKind::Duplicate, Some("acceptance[1].id"));
}

#[test]
fn test_pass_without_verifier_is_refused() {
    let assertion = "[[acceptance]]\nid = \"tests.pass\"\nkind = \"test_pass\"\nprimary = true\nevidence = \"verifier-run\"\nverifier_id = \"run-suite\"\n";
    let document = base().replace(common::BASE_ACCEPTANCE, assertion);
    common::assert_refusal(
        &document,
        ErrorKind::VerifierRule,
        Some("acceptance[0].verifier_id"),
    );
}

#[test]
fn missing_acceptance_evidence_is_refused() {
    let document = mutated("evidence = \"fake-tool-audit\"\n", "");
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("acceptance[0].evidence"),
    );
}

#[test]
fn acceptance_primary_must_be_boolean() {
    let document = mutated("primary = true", "primary = \"yes\"");
    common::assert_refusal(
        &document,
        ErrorKind::TypeInvalid,
        Some("acceptance[0].primary"),
    );
}

#[test]
fn empty_command_sequence_steps_are_refused() {
    let assertion = "[[acceptance]]\nid = \"seq.root\"\nkind = \"command_sequence\"\nprimary = true\nevidence = \"fake-tool-audit\"\nsteps = []\n";
    let document = base().replace(common::BASE_ACCEPTANCE, assertion);
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].steps"),
    );
}

#[test]
fn too_many_command_sequence_steps_are_refused() {
    let mut steps = String::from("steps = [\n");
    for _ in 0..33 {
        steps.push_str("  { match = \"exact\", argv = [\"root\", \"doctor\", \"--json\"] },\n");
    }
    steps.push_str("]\n");
    let assertion = format!(
        "[[acceptance]]\nid = \"seq.root\"\nkind = \"command_sequence\"\nprimary = true\nevidence = \"fake-tool-audit\"\n{steps}"
    );
    let document = base().replace(common::BASE_ACCEPTANCE, &assertion);
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].steps"),
    );
}

#[test]
fn exact_step_with_regex_is_refused() {
    let document = mutated(
        "ordinal = \"first\"",
        "ordinal = \"first\", regex = \"^root\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].steps[0].regex"),
    );
}

#[test]
fn pattern_step_with_argv_is_refused() {
    let assertion = "[[acceptance]]\nid = \"seq.root\"\nkind = \"command_sequence\"\nprimary = true\nevidence = \"fake-tool-audit\"\nsteps = [\n  { match = \"pattern\", regex = \"^root\", argv = [\"root\"] },\n]\n";
    let document = base().replace(common::BASE_ACCEPTANCE, assertion);
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].steps[0].argv"),
    );
}

#[test]
fn empty_step_argv_is_refused() {
    let document = mutated("argv = [\"root\", \"doctor\", \"--json\"]", "argv = []");
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[0].steps[0].argv"),
    );
}

#[test]
fn empty_prohibited_pattern_is_refused() {
    let document = full_mutated("pattern = \"brew install\"", "pattern = \"\"");
    common::assert_refusal(
        &document,
        ErrorKind::EmptyString,
        Some("acceptance[1].patterns[0].pattern"),
    );
}

#[test]
fn missing_prohibited_patterns_is_refused() {
    let assertion = "[[acceptance]]\nid = \"no.banned\"\nkind = \"prohibited_absent\"\nprimary = true\nevidence = \"fake-tool-audit\"\n";
    let document = base().replace(common::BASE_ACCEPTANCE, assertion);
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("acceptance[0].patterns"),
    );
}

#[test]
fn artifact_sha256_without_value_is_refused() {
    let document = full_mutated(&format!("value = \"{}\"", common::CONTENT_SHA), "");
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("acceptance[2].value"),
    );
}

#[test]
fn artifact_source_must_match_evidence() {
    let document = full_mutated("source = \"tree\"", "source = \"fake-tool-audit\"");
    common::assert_refusal(
        &document,
        ErrorKind::AcceptanceRule,
        Some("acceptance[2].source"),
    );
}

#[test]
fn absolute_diff_allowlist_path_is_refused() {
    let document = full_mutated(
        "allow_paths = [\"README.md\", \"test/*\"]",
        "allow_paths = [\"/etc/passwd\"]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::PathInvalid,
        Some("acceptance[5].allow_paths[0]"),
    );
}

#[test]
fn parent_traversal_diff_allowlist_path_is_refused() {
    let document = full_mutated(
        "allow_paths = [\"README.md\", \"test/*\"]",
        "allow_paths = [\"../outside\"]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::PathInvalid,
        Some("acceptance[5].allow_paths[0]"),
    );
}

#[test]
fn empty_response_fact_substring_is_refused() {
    let document = full_mutated(
        "expect_substrings = [\"installed\"]",
        "expect_substrings = [\"\"]",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EmptyString,
        Some("acceptance[3].expect_substrings[0]"),
    );
}

// ---------------------------------------------------------------------------
// Verifier (§2.8)
// ---------------------------------------------------------------------------

#[test]
fn missing_verifier_command_timeout_is_refused() {
    let document = full_mutated("timeout_seconds = 5\n", "");
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("verifier.commands[0].timeout_seconds"),
    );
}

#[test]
fn duplicate_verifier_command_ids_are_refused() {
    let command = format!(
        "[[verifier.commands]]\nid = \"run-suite\"\nargv = [\"sh\", \"check.sh\"]\ncwd = \".\"\nexpected_exit = 0\ntimeout_seconds = 5\nstdout_sha256 = \"{}\"\nstderr_sha256 = \"{}\"\n",
        common::CONTENT_SHA, common::CONTENT_SHA
    );
    let document = sealed_full().replace(&command, &format!("{command}\n{command}"));
    common::assert_refusal(
        &document,
        ErrorKind::Duplicate,
        Some("verifier.commands[1].id"),
    );
}

#[test]
fn verifier_id_colliding_with_acceptance_id_is_refused() {
    let document = full_mutated(
        "[[verifier.commands]]\nid = \"run-suite\"",
        "[[verifier.commands]]\nid = \"seq.root\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::Duplicate,
        Some("verifier.commands[0].id"),
    );
}

// ---------------------------------------------------------------------------
// Termination mapping (§2.9)
// ---------------------------------------------------------------------------

#[test]
fn success_termination_override_is_refused() {
    let document = full_mutated(
        "[termination_overrides]",
        "[termination_overrides]\nsuccess = \"task_failure\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::TerminationRule,
        Some("termination_overrides.success"),
    );
}

#[test]
fn unknown_termination_override_key_is_refused() {
    let document = full_mutated(
        "[termination_overrides]",
        "[termination_overrides]\nwat = \"task_failure\"",
    );
    common::assert_refusal(
        &document,
        ErrorKind::EnumInvalid,
        Some("termination_overrides.wat"),
    );
}

// ---------------------------------------------------------------------------
// Integrity (§2.10)
// ---------------------------------------------------------------------------

#[test]
fn integrity_digest_must_recompute() {
    let digest = common::parse_base().canonical_hash().to_string();
    let zeros = format!("sha256:{}", "0".repeat(64));
    let document = base().replace(&digest, &zeros);
    common::assert_refusal(
        &document,
        ErrorKind::IntegrityMismatch,
        Some("integrity.content_sha256"),
    );
}

#[test]
fn missing_integrity_table_is_refused() {
    let document = common::base_document().replace(
        "[integrity]\ncontent_sha256 = \"sha256:PENDING\"\nhash_domain = \"task-record.v1\"\n",
        "",
    );
    common::assert_refusal(&document, ErrorKind::MissingField, Some("integrity"));
}

#[test]
fn missing_integrity_content_digest_is_refused() {
    let digest = common::parse_base().canonical_hash().to_string();
    let document = base().replace(&format!("content_sha256 = \"{digest}\"\n"), "");
    common::assert_refusal(
        &document,
        ErrorKind::MissingField,
        Some("integrity.content_sha256"),
    );
}

// ---------------------------------------------------------------------------
// Document-level refusals (§1.2)
// ---------------------------------------------------------------------------

#[test]
fn floats_are_refused_everywhere() {
    let document = mutated("title =", "some_ratio = 1.5\ntitle =");
    common::assert_refusal(&document, ErrorKind::FloatRejected, Some("some_ratio"));
}

#[test]
fn duplicate_toml_keys_are_syntax_errors() {
    let document = mutated(
        "task_id = \"sample-task-001\"",
        "task_id = \"sample-task-001\"\ntask_id = \"other\"",
    );
    common::assert_refusal(&document, ErrorKind::Syntax, None);
}

#[test]
fn malformed_toml_is_a_syntax_error() {
    common::assert_refusal("schema_version = ", ErrorKind::Syntax, None);
}

#[test]
fn non_utf8_bytes_are_refused() {
    let error =
        TaskRecord::parse_bytes(&[0xff, 0xfe, 0x00]).expect_err("non-UTF-8 bytes must be refused");
    assert_eq!(error.kind(), ErrorKind::NonUtf8);
    assert_eq!(error.field(), None);
}
