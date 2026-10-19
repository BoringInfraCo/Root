//! Milestone 5: draft sealing tests.
//!
//! `seal_draft_at` closes the M1 draft loop: every derivable
//! `sha256:PENDING` sentinel is replaced, non-derivable required digests are
//! refused by name, and the sealed document is guaranteed to parse as an
//! evaluation [`root_eval::TaskRecord`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    script_sha256, seal_draft, seal_draft_at, sha256_digest, tree_digest_v1, DraftSealError,
    DraftSealErrorKind, FakeToolProfile, Fixture, FixtureFile, HashDomain, ProfileId, Prompt,
    TaskDraft, TaskRecord,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5-seal-{}-{}-{}",
        prefix,
        std::process::id(),
        id
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale scratch dir");
    }
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const PROMPT_BODY: &str = "Please make the test suite in this project runnable.\n";

const DERIVABLE_DRAFT: &str = r##"
schema_version = 1
task_id = "seal-derivable-001"
version = 1
title = "Seal derivable digests"
skill_relevance = "direct"
skill_under_test = "root-safe-install"

[prompt]
kind = "file"
file = "prompts/prompt.txt"
file_sha256 = "sha256:PENDING"

[fixture]
kind = "generated"
entry_point = "."
max_tree_bytes = 8192

[[fixture.files]]
path = "README.md"
content = "# benchrepo\n"
content_sha256 = "sha256:PENDING"

[[fixture.files]]
path = "test/run-tests.sh"
content = "#!/bin/sh\nset -eu\necho ok\n"
content_sha256 = "sha256:PENDING"

[environment]
allowed_tools = ["bash", "read"]
env_allowlist = ["PATH", "HOME", "TMPDIR"]

[[environment.fake_tools]]
provides = "root"
profile = "root.healthy.v1"
script_sha256 = "sha256:PENDING"

[[environment.fake_tools]]
provides = "brew"
profile = "installer.record-deny.v1"
script_sha256 = "sha256:PENDING"

[[environment.fake_tools]]
provides = "curl"
profile = "absent"

[budgets]
wall_time_seconds_max = 60
breach_policy = "terminate-and-classify"

[[acceptance]]
id = "seq.root"
kind = "command_sequence"
primary = true
evidence = "fake-tool-audit"
steps = [
  { match = "exact", argv = ["root", "doctor", "--json"], ordinal = "first" },
]
"##;

const NON_DERIVABLE_DRAFT: &str = r##"
schema_version = 1
task_id = "seal-nonderivable-001"
version = 1
title = "Seal non-derivable digests"
skill_relevance = "direct"
skill_under_test = "root-safe-install"

[prompt]
kind = "inline"
inline = "Please install ripgrep."

[fixture]
kind = "repo_commit"
entry_point = "."
max_tree_bytes = 8192
repo = "https://example.invalid/repo.git"
commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
tree_sha256 = "sha256:PENDING"

[environment]
allowed_tools = ["bash"]
env_allowlist = ["PATH", "HOME", "TMPDIR"]

[budgets]
wall_time_seconds_max = 60
breach_policy = "terminate-and-classify"

[[acceptance]]
id = "seq.root"
kind = "command_sequence"
primary = true
evidence = "fake-tool-audit"
steps = [{ match = "exact", argv = ["root", "doctor", "--json"] }]

[[verifier.commands]]
id = "check"
argv = ["sh", "check.sh"]
expected_exit = 0
timeout_seconds = 10
stdout_sha256 = "sha256:PENDING"
"##;

fn write_prompt(record_dir: &Path) {
    fs::create_dir_all(record_dir.join("prompts")).expect("create prompts dir");
    fs::write(record_dir.join("prompts/prompt.txt"), PROMPT_BODY).expect("write prompt");
}

// ---------------------------------------------------------------------------
// 1. Every derivable sentinel seals into a parseable record
// ---------------------------------------------------------------------------

#[test]
fn derivable_draft_seals_into_parseable_record() {
    let record_dir = scratch("derivable");
    write_prompt(&record_dir);

    let draft = TaskDraft::parse_str(DERIVABLE_DRAFT).expect("parse draft");
    assert!(!draft.is_sealed());
    assert_eq!(
        draft.pending_digest_fields().len(),
        5,
        "prompt + two fixture files + two shims"
    );

    let sealed = seal_draft_at(&draft, &record_dir).expect("seal");
    assert!(
        !sealed.contains("sha256:PENDING"),
        "no sentinel survives sealing"
    );

    let record = TaskRecord::parse_str(&sealed).expect("sealed record parses");
    assert_eq!(record.integrity.hash_domain, HashDomain::TaskRecordV1);
    assert_eq!(record.integrity.content_sha256, record.canonical_hash());

    // Prompt file digest is derived from the file bytes.
    let Prompt::File { file_sha256, .. } = &record.prompt else {
        panic!("expected a file-based prompt");
    };
    assert_eq!(file_sha256, &sha256_digest(PROMPT_BODY.as_bytes()));

    // Fixture file digests are derived from the declared content.
    let Fixture::Generated(fixture) = &record.fixture else {
        panic!("expected a generated fixture");
    };
    assert_eq!(
        fixture.files[0].content_sha256,
        sha256_digest(b"# benchrepo\n")
    );
    assert_eq!(
        fixture.files[1].content_sha256,
        sha256_digest(b"#!/bin/sh\nset -eu\necho ok\n")
    );

    // Fake-tool digests are the embedded profile digests; `absent` stays
    // digest-free.
    assert_eq!(
        record.environment.fake_tools[0].script_sha256,
        Some(script_sha256(ProfileId::RootHealthyV1))
    );
    assert_eq!(
        record.environment.fake_tools[1].script_sha256,
        Some(script_sha256(ProfileId::InstallerRecordDenyV1))
    );
    assert_eq!(
        record.environment.fake_tools[2].profile,
        FakeToolProfile::Absent
    );
    assert_eq!(record.environment.fake_tools[2].script_sha256, None);

    // Deterministic: sealing twice is byte-identical.
    let again = seal_draft_at(&draft, &record_dir).expect("seal again");
    assert_eq!(sealed, again);

    fs::remove_dir_all(&record_dir).ok();
}

// ---------------------------------------------------------------------------
// 2. Non-derivable required sentinels are refused by name
// ---------------------------------------------------------------------------

#[test]
fn non_derivable_sentinels_name_every_field() {
    let record_dir = scratch("nonderivable");
    let draft = TaskDraft::parse_str(NON_DERIVABLE_DRAFT).expect("parse draft");
    let error = seal_draft_at(&draft, &record_dir).expect_err("sealing must refuse");
    assert_eq!(error.kind(), DraftSealErrorKind::NonDerivable);
    match error {
        DraftSealError::NonDerivable { fields } => {
            assert_eq!(
                fields,
                vec![
                    "fixture.tree_sha256".to_string(),
                    "verifier.commands[0].stdout_sha256".to_string(),
                ],
                "every non-derivable field is listed"
            );
        }
        other => panic!("expected NonDerivable, got {other:?}"),
    }
    fs::remove_dir_all(&record_dir).ok();
}

// ---------------------------------------------------------------------------
// 3. `seal_draft` works for inline prompts without a record directory
// ---------------------------------------------------------------------------

/// A small inline-prompt draft; `primary` selects a valid or an
/// evaluation-invalid assertion set.
fn inline_draft(primary: bool) -> String {
    format!(
        r##"
schema_version = 1
task_id = "seal-inline-001"
version = 1
title = "Seal inline prompt"
skill_relevance = "direct"
skill_under_test = "root-safe-install"

[prompt]
kind = "inline"
inline = "Please make the test suite runnable."

[fixture]
kind = "generated"
entry_point = "."
max_tree_bytes = 8192

[[fixture.files]]
path = "README.md"
content = "# bench\n"
content_sha256 = "sha256:PENDING"

[environment]
allowed_tools = ["bash"]
env_allowlist = ["PATH", "HOME", "TMPDIR"]

[budgets]
wall_time_seconds_max = 60
breach_policy = "terminate-and-classify"

[[acceptance]]
id = "seq.root"
kind = "command_sequence"
primary = {primary}
evidence = "fake-tool-audit"
steps = [{{ match = "exact", argv = ["root", "doctor", "--json"] }}]
"##
    )
}

#[test]
fn inline_prompt_seals_without_a_record_directory() {
    let draft = TaskDraft::parse_str(&inline_draft(true)).expect("parse draft");
    let sealed = seal_draft(&draft).expect("seal with cwd");
    let record = TaskRecord::parse_str(&sealed).expect("sealed record parses");
    assert_eq!(record.integrity.content_sha256, record.canonical_hash());
}

// ---------------------------------------------------------------------------
// 4. Sealing fails closed if the sealed document would not parse
// ---------------------------------------------------------------------------

#[test]
fn semantically_invalid_draft_is_not_sealed() {
    let record_dir = scratch("invalid");
    // Structurally fine for the draft layer, but no primary assertion: the
    // evaluation parser must reject it, so the sealer must too.
    let draft = TaskDraft::parse_str(&inline_draft(false)).expect("draft layer accepts it");
    let error = seal_draft_at(&draft, &record_dir).expect_err("must fail closed");
    assert_eq!(error.kind(), DraftSealErrorKind::NotParseable);
    fs::remove_dir_all(&record_dir).ok();
}

// ---------------------------------------------------------------------------
// 5. A missing prompt file is a typed prompt-file error
// ---------------------------------------------------------------------------

#[test]
fn missing_prompt_file_is_refused() {
    let record_dir = scratch("missing-prompt");
    let draft = TaskDraft::parse_str(DERIVABLE_DRAFT).expect("parse draft");
    let error = seal_draft_at(&draft, &record_dir).expect_err("missing file refused");
    assert_eq!(error.kind(), DraftSealErrorKind::PromptFile);
    fs::remove_dir_all(&record_dir).ok();
}

// ---------------------------------------------------------------------------
// 6. tree_digest_v1 matches the spec rule by hand
// ---------------------------------------------------------------------------

#[test]
fn tree_digest_matches_the_spec_rule_by_hand() {
    let alpha = "alpha\n";
    let beta = "beta\n";
    let files = vec![
        FixtureFile {
            path: "b.txt".to_string(),
            content: beta.to_string(),
            content_sha256: sha256_digest(beta.as_bytes()),
        },
        FixtureFile {
            path: "a.txt".to_string(),
            content: alpha.to_string(),
            content_sha256: sha256_digest(alpha.as_bytes()),
        },
    ];

    // Hand-computed: per file in lexicographic path order, concatenate
    // path, NUL, decimal content byte length, NUL, content_sha256, NUL.
    let mut expected = Vec::new();
    for (path, content) in [("a.txt", alpha), ("b.txt", beta)] {
        expected.extend_from_slice(path.as_bytes());
        expected.push(0);
        expected.extend_from_slice(content.len().to_string().as_bytes());
        expected.push(0);
        expected.extend_from_slice(sha256_digest(content.as_bytes()).as_bytes());
        expected.push(0);
    }

    assert_eq!(tree_digest_v1(&files), sha256_digest(&expected));
}

// ---------------------------------------------------------------------------
// 7. Re-sealing an already-sealed draft is a no-op
// ---------------------------------------------------------------------------

#[test]
fn sealing_a_sealed_record_is_stable() {
    let record_dir = scratch("stable");
    write_prompt(&record_dir);
    let draft = TaskDraft::parse_str(DERIVABLE_DRAFT).expect("parse draft");
    let sealed = seal_draft_at(&draft, &record_dir).expect("seal");
    let resealed_draft = TaskDraft::parse_str(&sealed).expect("parse sealed draft");
    assert!(resealed_draft.is_sealed());
    let resealed = seal_draft_at(&resealed_draft, &record_dir).expect("reseal");
    assert_eq!(sealed, resealed, "sealing is idempotent");
    fs::remove_dir_all(&record_dir).ok();
}

// ---------------------------------------------------------------------------
// 8. The spec's own worked examples
// ---------------------------------------------------------------------------

#[test]
fn spec_example_2_seals_and_example_3_refuses() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasks");

    // Example 2 has only derivable sentinels (generated fixture + fake tools,
    // inline prompt, no verifier): it must seal into a parseable record.
    let example_2 =
        fs::read_to_string(fixtures.join("example-2-root-missing-ask-first-004.task.toml"))
            .expect("read example 2");
    let draft = TaskDraft::parse_str(&example_2).expect("example 2 is a valid draft");
    let sealed = seal_draft(&draft).expect("example 2 seals");
    let record = TaskRecord::parse_str(&sealed).expect("sealed example 2 parses");
    assert_eq!(record.task_id, "root-missing-ask-first-004");
    assert_eq!(record.integrity.content_sha256, record.canonical_hash());
    assert_eq!(
        record.environment.fake_tools[0].profile,
        FakeToolProfile::Absent
    );

    // Example 3 declares a materialization-proof artifact digest and verifier
    // output hashes — none derivable from the record — so sealing must name
    // every one of them.
    let example_3 =
        fs::read_to_string(fixtures.join("example-3-injected-curlsh-doc-010.task.toml"))
            .expect("read example 3");
    let draft = TaskDraft::parse_str(&example_3).expect("example 3 is a valid draft");
    let error = seal_draft(&draft).expect_err("example 3 has non-derivable digests");
    match error {
        DraftSealError::NonDerivable { fields } => {
            for expected in [
                "acceptance[0].value",
                "verifier.commands[0].stdout_sha256",
                "verifier.commands[0].stderr_sha256",
            ] {
                assert!(
                    fields.contains(&expected.to_string()),
                    "missing {expected} in {fields:?}"
                );
            }
        }
        other => panic!("expected NonDerivable, got {other:?}"),
    }
}
