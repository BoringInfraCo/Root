//! Canonical-hash determinism rules (TASK-SPEC §2.10).
//!
//! Presentation changes (comments, whitespace, key order, fixture-file
//! declaration order for `generated` fixtures) MUST NOT change the hash.
//! Semantic changes MUST.

mod common;

use root_eval::{sha256_digest, TaskRecord};

fn doc_with_second_file() -> (String, String) {
    let file_2_content = "echo hi\n";
    let file_2_hash = sha256_digest(file_2_content.as_bytes());
    let first_file = format!(
        "[[fixture.files]]\npath = \"README.md\"\ncontent = \"fixture body\\n\"\ncontent_sha256 = \"{}\"\n",
        common::CONTENT_SHA
    );
    let second_file = format!(
        "[[fixture.files]]\npath = \"test/run.sh\"\ncontent = \"echo hi\\n\"\ncontent_sha256 = \"{file_2_hash}\"\n"
    );
    let base = common::base_document();
    let files_first = base.replace(&first_file, &format!("{first_file}\n{second_file}"));
    let files_reversed = base.replace(&first_file, &format!("{second_file}\n{first_file}"));
    (files_first, files_reversed)
}

#[test]
fn comments_whitespace_and_key_order_do_not_change_hash() {
    let plain = common::sealed_base();

    let decorated = common::base_document()
        // Root keys reordered (version before schema_version).
        .replace(
            "schema_version = 1\ntask_id = \"sample-task-001\"\nversion = 1",
            "version = 1\nschema_version = 1\ntask_id = \"sample-task-001\"",
        )
        // Budget keys reordered, extra whitespace and a trailing comment.
        .replace(
            "wall_time_seconds_max = 900\nturns_max = 24",
            "turns_max = 24        # reordered\nwall_time_seconds_max   =   900",
        )
        // Comments and blank lines are presentation only.
        .replace(
            "[budgets]",
            "# budgets follow; values are illustrative\n\n[budgets]",
        );
    let decorated = common::seal(&decorated);

    let plain_record = TaskRecord::parse_str(&plain).expect("plain base parses");
    let decorated_record = TaskRecord::parse_str(&decorated).expect("decorated base parses");
    assert_eq!(
        plain_record.canonical_hash(),
        decorated_record.canonical_hash()
    );
    assert_eq!(
        plain_record.canonical_bytes(),
        decorated_record.canonical_bytes()
    );
}

#[test]
fn task_id_change_changes_hash() {
    let changed = common::base_document().replace("sample-task-001", "sample-task-002");
    let changed = TaskRecord::parse_str(&common::seal(&changed)).expect("changed task id parses");
    assert_ne!(
        changed.canonical_hash(),
        common::parse_base().canonical_hash()
    );
}

#[test]
fn version_change_changes_hash() {
    let changed = common::base_document().replacen("\nversion = 1\n", "\nversion = 2\n", 1);
    let changed = TaskRecord::parse_str(&common::seal(&changed)).expect("version 2 parses");
    let base = common::parse_base();
    assert_ne!(changed.canonical_hash(), base.canonical_hash());
    assert_eq!(changed.identity().version, 2);
    assert_eq!(base.identity().version, 1);
}

#[test]
fn title_change_changes_hash() {
    let changed = common::base_document().replace(
        "title = \"Install a supported CLI\"",
        "title = \"Install a supported command-line tool\"",
    );
    let changed = TaskRecord::parse_str(&common::seal(&changed)).expect("changed title parses");
    assert_ne!(
        changed.canonical_hash(),
        common::parse_base().canonical_hash()
    );
}

#[test]
fn fixture_file_declaration_order_does_not_change_hash() {
    let (first, reversed) = doc_with_second_file();
    let first = TaskRecord::parse_str(&common::seal(&first)).expect("first order parses");
    let reversed = TaskRecord::parse_str(&common::seal(&reversed)).expect("reversed order parses");
    assert_eq!(first.canonical_hash(), reversed.canonical_hash());
}

#[test]
fn map_key_order_does_not_change_hash() {
    let forward = common::base_document().replace(
        "[environment]",
        "[environment]\ntool_permissions = { bash = \"allow\", webfetch = \"deny\" }",
    );
    let reverse = common::base_document().replace(
        "[environment]",
        "[environment]\ntool_permissions = { webfetch = \"deny\", bash = \"allow\" }",
    );
    let forward = TaskRecord::parse_str(&common::seal(&forward)).expect("forward map parses");
    let reverse = TaskRecord::parse_str(&common::seal(&reverse)).expect("reverse map parses");
    assert_eq!(forward.canonical_hash(), reverse.canonical_hash());
}

#[test]
fn acceptance_order_is_semantically_meaningful() {
    let second = r#"[[acceptance]]
id = "tree.diff"
kind = "diff_allowlist"
primary = false
evidence = "tree"
allow_paths = []
"#;
    let acceptance_block = common::BASE_ACCEPTANCE;
    let ab =
        common::base_document().replace(acceptance_block, &format!("{acceptance_block}{second}"));
    let ba =
        common::base_document().replace(acceptance_block, &format!("{second}{acceptance_block}"));
    let ab = TaskRecord::parse_str(&common::seal(&ab)).expect("A,B parses");
    let ba = TaskRecord::parse_str(&common::seal(&ba)).expect("B,A parses");
    assert_ne!(ab.canonical_hash(), ba.canonical_hash());
}

#[test]
fn unicode_and_escaped_content_round_trips() {
    let content = "say \"hi\" — ünïcode\ttab\n";
    let content_hash = sha256_digest(content.as_bytes());
    let escaped = content
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    let document = common::base_document()
        .replace(common::CONTENT_SHA, &content_hash)
        .replace(
            "content = \"fixture body\\n\"",
            &format!("content = \"{escaped}\""),
        );
    let record = TaskRecord::parse_str(&common::seal(&document)).expect("unicode content parses");

    let redocument = format!(
        "{}\n[integrity]\ncontent_sha256 = \"{}\"\nhash_domain = \"task-record.v1\"\n",
        std::str::from_utf8(record.canonical_bytes()).expect("utf-8"),
        record.canonical_hash()
    );
    let reparsed = TaskRecord::parse_str(&redocument).expect("canonical bytes re-parse");
    assert_eq!(reparsed.canonical_hash(), record.canonical_hash());
}
