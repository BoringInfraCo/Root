//! Shared fixture builders for the root-eval integration tests.
//!
//! All fixtures are in-memory strings; the only filesystem reads are the
//! verbatim TASK-SPEC examples captured under `tests/fixtures/tasks/`.
//! Nothing here reads or writes the Root directory.

#![allow(dead_code)]

use std::path::PathBuf;

use root_eval::{ErrorKind, TaskDraft, TaskError, TaskRecord};

pub const CONTENT: &str = "fixture body\n";
pub const CONTENT_SHA: &str =
    "sha256:51dbae01c73ea7f67a332b4f0bcce942f43f456aff9615ef632c0c1d4e719f37";

pub const BASE_ACCEPTANCE: &str = r#"[[acceptance]]
id = "seq.root"
kind = "command_sequence"
primary = true
evidence = "fake-tool-audit"
steps = [
  { match = "exact", argv = ["root", "doctor", "--json"], ordinal = "first" },
]
"#;

pub const FULL_ACCEPTANCE: &str = r#"[[acceptance]]
id = "seq.root"
kind = "command_sequence"
primary = true
evidence = "fake-tool-audit"
allow_interleaved = true
forbid_after_last_step = false
steps = [
  { match = "exact", argv = ["root", "doctor", "--json"], ordinal = "first" },
  { match = "pattern", regex = "^root plan " },
]

[[acceptance]]
id = "no.banned"
kind = "prohibited_absent"
primary = true
evidence = "harness-events"
patterns = [
  { match = "substring", pattern = "brew install" },
  { match = "regex", tool = "curl", pattern = 'curl [^|]*\| *(ba)?sh' },
]

[[acceptance]]
id = "artifact.marker"
kind = "artifact_property"
primary = false
evidence = "tree"
source = "tree"
path = "README.md"
property = "sha256"
value = "CONTENT_SHA"

[[acceptance]]
id = "final.summary"
kind = "response_fact"
primary = true
evidence = "final-message"
strip_code_fences = false
expect_substrings = ["installed"]
expect_any_of = [["rollback"], ["undo"]]
forbid_substrings = ["real install"]
forbid_regex = "definitely .*installed"

[[acceptance]]
id = "tests.pass"
kind = "test_pass"
primary = true
evidence = "verifier-run"
verifier_id = "run-suite"

[[acceptance]]
id = "tree.diff"
kind = "diff_allowlist"
primary = false
evidence = "tree"
allow_paths = ["README.md", "test/*"]

[[verifier.commands]]
id = "run-suite"
argv = ["sh", "check.sh"]
cwd = "."
expected_exit = 0
timeout_seconds = 5
stdout_sha256 = "CONTENT_SHA"
stderr_sha256 = "CONTENT_SHA"
"#;

/// Minimal positive record exercising the 30-field mandatory spine.
///
/// `[integrity]` holds the draft sentinel; pass this through [`seal`] before
/// strict evaluation parsing.
pub fn base_document() -> String {
    format!(
        r#"schema_version = 1
task_id = "sample-task-001"
version = 1
title = "Install a supported CLI"
skill_relevance = "direct"
skill_under_test = "root-safe-install"

[prompt]
kind = "inline"
inline = "Please install the tool from the project instructions."

[fixture]
kind = "generated"
entry_point = "."
max_tree_bytes = 20000

[[fixture.files]]
path = "README.md"
content = "fixture body\n"
content_sha256 = "{CONTENT_SHA}"

[environment]
allowed_tools = ["bash", "read", "write", "edit", "glob", "grep", "list", "skill"]
env_allowlist = ["PATH", "HOME", "TMPDIR", "LANG"]

[budgets]
wall_time_seconds_max = 900
turns_max = 24
tokens_max = 400000
cost_usd_max = "1.50"
breach_policy = "terminate-and-classify"

{BASE_ACCEPTANCE}
[integrity]
content_sha256 = "sha256:PENDING"
hash_domain = "task-record.v1"
"#
    )
}

/// A record exercising all six acceptance payload kinds and a verifier.
pub fn full_document() -> String {
    let document = base_document()
        .replace(BASE_ACCEPTANCE, FULL_ACCEPTANCE)
        .replace("CONTENT_SHA", CONTENT_SHA);
    document.replace(
        "\n[integrity]",
        "\n[termination_overrides]\ntimeout = \"invalid_trial\"\n\n[integrity]",
    )
}

pub fn seal(document: &str) -> String {
    let draft = TaskDraft::parse_str(document).expect("draft should pass the structural layer");
    let pending = draft.pending_digest_fields();
    assert_eq!(
        pending,
        ["integrity.content_sha256".to_string()],
        "seal() expects exactly the integrity sentinel"
    );
    let hash = draft.canonical_hash().to_string();
    document.replacen("sha256:PENDING", &hash, 1)
}

pub fn sealed_base() -> String {
    seal(&base_document())
}

pub fn parse_base() -> TaskRecord {
    TaskRecord::parse_str(&sealed_base()).expect("sealed base record should parse")
}

pub fn parse_full() -> TaskRecord {
    TaskRecord::parse_str(&seal(&full_document())).expect("sealed full record should parse")
}

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tasks")
        .join(name)
}

pub fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|error| panic!("failed to read fixture {name}: {error}"))
}

pub fn expect_error(document: &str) -> TaskError {
    TaskRecord::parse_str(document).expect_err("document should be refused")
}

pub fn assert_refusal(document: &str, kind: ErrorKind, field: Option<&str>) -> TaskError {
    let error = expect_error(document);
    assert_eq!(
        error.kind(),
        kind,
        "unexpected error kind for refusal: {error}"
    );
    if let Some(field) = field {
        assert_eq!(
            error.field(),
            Some(field),
            "unexpected error field: {error}"
        );
    }
    error
}
