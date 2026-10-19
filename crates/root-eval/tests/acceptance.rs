//! Acceptance-assertion evaluator integration tests (TASK-SPEC §2.7).
//!
//! Evidence is built synthetically; one end-to-end-ish case drives
//! [`capture_tree_snapshot`] + [`diff_snapshots`] over a real scratch tree.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use root_eval::{
    capture_tree_snapshot, diff_snapshots, evaluate_acceptance, parse_audit_log, parse_ndjson_line,
    read_tree_file, sha256_digest, AcceptanceErrorKind, AssertionReport, AssertionResult,
    AuditLine, NormalizedEvent, TaskRecord, TreeChange, TreeChangeKind, TreeFile, TreeSnapshot,
    TrialEvidence, VerifierOutcome, VerifierVerdict, FAKE_TOOL_AUDIT_FILE_NAME,
};

// ---------------------------------------------------------------------------
// Record builders
// ---------------------------------------------------------------------------

/// Build a sealed record whose acceptance block replaces the base spine's.
fn record(acceptance: &str, verifier: Option<&str>) -> TaskRecord {
    let mut document = common::base_document().replace(common::BASE_ACCEPTANCE, acceptance);
    if let Some(verifier) = verifier {
        document = document.replace("\n[integrity]", &format!("\n{verifier}\n[integrity]"));
    }
    TaskRecord::parse_str(&common::seal(&document)).expect("test record should parse")
}

fn exact_step(argv: &[&str], ordinal_first: bool) -> String {
    let argv = argv
        .iter()
        .map(|argument| format!("'{argument}'"))
        .collect::<Vec<_>>()
        .join(", ");
    if ordinal_first {
        format!("{{ match = \"exact\", argv = [{argv}], ordinal = \"first\" }}")
    } else {
        format!("{{ match = \"exact\", argv = [{argv}] }}")
    }
}

fn pattern_step(regex: &str) -> String {
    format!("{{ match = \"pattern\", regex = '{regex}' }}")
}

fn command_sequence_record(
    steps: &[String],
    allow_interleaved: Option<bool>,
    forbid_after_last_step: Option<bool>,
    evidence: &str,
) -> TaskRecord {
    let mut extras = String::new();
    if let Some(value) = allow_interleaved {
        extras.push_str(&format!("allow_interleaved = {value}\n"));
    }
    if let Some(value) = forbid_after_last_step {
        extras.push_str(&format!("forbid_after_last_step = {value}\n"));
    }
    let block = format!(
        "[[acceptance]]\nid = \"seq.test\"\nkind = \"command_sequence\"\nprimary = true\n\
         evidence = \"{evidence}\"\n{extras}steps = [\n  {}\n]\n",
        steps.join(",\n  ")
    );
    record(&block, None)
}

fn prohibited_pattern(pattern: &str, match_kind: &str, tool: Option<&str>) -> String {
    match tool {
        Some(tool) => {
            format!("{{ match = \"{match_kind}\", tool = '{tool}', pattern = '{pattern}' }}")
        }
        None => format!("{{ match = \"{match_kind}\", pattern = '{pattern}' }}"),
    }
}

fn prohibited_record(patterns: &[String], evidence: &str) -> TaskRecord {
    let block = format!(
        "[[acceptance]]\nid = \"no.banned\"\nkind = \"prohibited_absent\"\nprimary = true\n\
         evidence = \"{evidence}\"\npatterns = [\n  {}\n]\n",
        patterns.join(",\n  ")
    );
    record(&block, None)
}

fn artifact_record(source: &str, path: &str, property: &str, value: Option<&str>) -> TaskRecord {
    let evidence = if source == "tree" {
        "tree"
    } else {
        "fake-tool-audit"
    };
    let mut block = format!(
        "[[acceptance]]\nid = \"artifact.test\"\nkind = \"artifact_property\"\nprimary = true\n\
         evidence = \"{evidence}\"\nsource = \"{source}\"\npath = \"{path}\"\n\
         property = \"{property}\"\n"
    );
    if let Some(value) = value {
        block.push_str(&format!("value = '{value}'\n"));
    }
    record(&block, None)
}

fn response_fact_record(fields: &str) -> TaskRecord {
    let block = format!(
        "[[acceptance]]\nid = \"final.test\"\nkind = \"response_fact\"\nprimary = true\n\
         evidence = \"final-message\"\n{fields}\n"
    );
    record(&block, None)
}

fn test_pass_record(expected_exit: i32, stdout_sha256: Option<&str>) -> TaskRecord {
    let mut verifier = format!(
        "[[verifier.commands]]\nid = \"run-suite\"\nargv = ['sh', 'check.sh']\n\
         expected_exit = {expected_exit}\ntimeout_seconds = 5\n"
    );
    if let Some(digest) = stdout_sha256 {
        verifier.push_str(&format!("stdout_sha256 = \"{digest}\"\n"));
    }
    let acceptance = "[[acceptance]]\nid = \"tests.pass\"\nkind = \"test_pass\"\nprimary = true\n\
                      evidence = \"verifier-run\"\nverifier_id = \"run-suite\"\n";
    record(acceptance, Some(&verifier))
}

fn diff_record(allow_paths: &[&str]) -> TaskRecord {
    let paths = allow_paths
        .iter()
        .map(|path| format!("'{path}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let block = format!(
        "[[acceptance]]\nid = \"tree.diff\"\nkind = \"diff_allowlist\"\nprimary = true\n\
         evidence = \"tree\"\nallow_paths = [{paths}]\n"
    );
    record(&block, None)
}

// ---------------------------------------------------------------------------
// Evidence builders
// ---------------------------------------------------------------------------

fn audit_line(seq: u64, tool: &str, argv: &[&str]) -> AuditLine {
    AuditLine {
        seq,
        tool: tool.to_string(),
        argv: argv
            .iter()
            .map(|argument| (*argument).to_string())
            .collect(),
        exit: 0,
        profile: "root.healthy.v1".to_string(),
    }
}

fn audit_lines(entries: &[(&str, &[&str])]) -> Vec<AuditLine> {
    entries
        .iter()
        .enumerate()
        .map(|(index, (tool, argv))| audit_line(index as u64 + 1, tool, argv))
        .collect()
}

fn tool_event(sequence: u64, tool: &str, input: serde_json::Value) -> NormalizedEvent {
    let line = serde_json::json!({
        "type": "tool_use",
        "timestamp": 0,
        "sessionID": "ses_acceptance_test",
        "part": {
            "type": "tool",
            "tool": tool,
            "callID": format!("call_{sequence}"),
            "state": {
                "status": "completed",
                "input": input,
                "output": "ok",
            },
        },
    });
    parse_ndjson_line(&line.to_string(), sequence).expect("tool event should parse")
}

fn bash_event(sequence: u64, command: &str) -> NormalizedEvent {
    tool_event(sequence, "bash", serde_json::json!({ "command": command }))
}

fn snapshot(files: &[(&str, &str)]) -> TreeSnapshot {
    TreeSnapshot {
        files: files
            .iter()
            .map(|(path, content)| {
                (
                    (*path).to_string(),
                    TreeFile {
                        sha256: sha256_digest(content.as_bytes()),
                        bytes_len: content.len() as u64,
                    },
                )
            })
            .collect(),
    }
}

fn verifier_outcome(id: &str, exit: i32, stdout: &[u8], mutation: bool) -> VerifierOutcome {
    VerifierOutcome {
        verifier_id: id.to_string(),
        verdict: if mutation {
            VerifierVerdict::Invalid
        } else {
            VerifierVerdict::Pass
        },
        exit_codes: vec![Some(exit)],
        stdout_sha256: sha256_digest(stdout),
        stderr_sha256: sha256_digest(b""),
        stdout_bytes_len: stdout.len() as u64,
        stderr_bytes_len: 0,
        pre_tree_sha256: "sha256:pre".to_string(),
        post_tree_sha256: "sha256:post".to_string(),
        mutation_detected: mutation,
        wall_ms: 1,
        timed_out: false,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        notes: Vec::new(),
    }
}

fn evaluate(record: &TaskRecord, evidence: &TrialEvidence<'_>) -> AssertionReport {
    evaluate_acceptance(record, evidence).expect("evaluation should not error")
}

fn single(report: &AssertionReport, passed: bool) -> &AssertionResult {
    assert_eq!(report.results.len(), 1);
    let result = &report.results[0];
    assert_eq!(result.passed, passed, "unexpected result: {result:?}");
    result
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(prefix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "root-eval-m5-acceptance-{prefix}-{}-{id}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clear stale scratch dir");
    }
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

// ---------------------------------------------------------------------------
// command_sequence (§2.7.1)
// ---------------------------------------------------------------------------

#[test]
fn command_sequence_exact_and_pattern_pass() {
    let record = command_sequence_record(
        &[
            exact_step(&["root", "doctor", "--json"], false),
            pattern_step("^root plan "),
        ],
        None,
        None,
        "fake-tool-audit",
    );
    // The audit records args only (`$@`), so tool-prefix recovery applies.
    let audit = audit_lines(&[
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    assert!(report.all_passed);
    assert!(report.all_primary_passed);
    assert_eq!(single(&report, true).kind, "command_sequence");
}

#[test]
fn command_sequence_subsequence_ignores_interleaved_noise() {
    let record = command_sequence_record(
        &[
            exact_step(&["root", "doctor", "--json"], false),
            exact_step(&["root", "plan", "install", "ripgrep", "--json"], false),
        ],
        None,
        None,
        "fake-tool-audit",
    );
    let audit = audit_lines(&[
        ("brew", &["--version"]),
        ("root", &["doctor", "--json"]),
        ("curl", &["-I", "https://example.invalid"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn command_sequence_missing_step_fails_with_detail() {
    let record = command_sequence_record(
        &[
            exact_step(&["root", "doctor", "--json"], false),
            exact_step(&["root", "plan", "install", "ripgrep", "--json"], false),
            exact_step(&["root", "verify", "ripgrep", "--json"], false),
        ],
        None,
        None,
        "fake-tool-audit",
    );
    let audit = audit_lines(&[
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("step 3 of 3 did not match"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn command_sequence_consecutive_mode_requires_a_run() {
    let steps = [
        exact_step(&["root", "doctor", "--json"], false),
        exact_step(&["root", "plan", "install", "ripgrep", "--json"], false),
    ];
    let consecutive = command_sequence_record(&steps, Some(false), None, "fake-tool-audit");
    let adjacent = audit_lines(&[
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &consecutive,
        &TrialEvidence {
            audit_log: Some(&adjacent),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let interleaved = audit_lines(&[
        ("root", &["doctor", "--json"]),
        ("brew", &["--version"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &consecutive,
        &TrialEvidence {
            audit_log: Some(&interleaved),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("allow_interleaved = false"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn command_sequence_ordinal_first_pass_and_fail() {
    let steps = [
        exact_step(&["root", "doctor", "--json"], true),
        exact_step(&["root", "plan", "install", "ripgrep", "--json"], false),
    ];
    let record = command_sequence_record(&steps, None, None, "fake-tool-audit");

    let first = audit_lines(&[
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&first),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    // A required step matching before the ordinal-first step fails it.
    let reordered = audit_lines(&[
        ("root", &["plan", "install", "ripgrep", "--json"]),
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&reordered),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("ordinal = \"first\""),
        "unexpected detail: {}",
        result.detail
    );

    // Documented decision: invocations matching no step are noise for the
    // ordinal check.
    let noise_first = audit_lines(&[
        ("brew", &["--version"]),
        ("root", &["doctor", "--json"]),
        ("root", &["plan", "install", "ripgrep", "--json"]),
    ]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&noise_first),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn command_sequence_forbid_after_last_step() {
    let record = command_sequence_record(
        &[exact_step(&["root", "doctor", "--json"], false)],
        None,
        Some(true),
        "fake-tool-audit",
    );

    let only = audit_lines(&[("root", &["doctor", "--json"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&only),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let trailing = audit_lines(&[("root", &["doctor", "--json"]), ("brew", &["--version"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&trailing),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result
            .detail
            .contains("forbid_after_last_step = true: 1 invocation(s) recorded after"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn command_sequence_sh_c_concatenation() {
    let record = command_sequence_record(
        &[exact_step(&["sh", "-c", "root doctor --json"], false)],
        None,
        None,
        "fake-tool-audit",
    );
    // One recorded shape keeps the inner command as a single argv element.
    let single_element = audit_lines(&[("sh", &["-c", "root doctor --json"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&single_element),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    // The other splits it into words; both normalize to the same line.
    let split = audit_lines(&[("sh", &["-c", "root", "doctor", "--json"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&split),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn command_sequence_wrapper_chaining_through_git() {
    let record = command_sequence_record(
        &[pattern_step("sh -c root doctor --json")],
        None,
        None,
        "fake-tool-audit",
    );
    let audit = audit_lines(&[("git", &["-C", "repo", "sh", "-c", "root doctor --json"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn command_sequence_missing_audit_evidence_fails_without_error() {
    let record = command_sequence_record(
        &[exact_step(&["root", "doctor", "--json"], false)],
        None,
        None,
        "fake-tool-audit",
    );
    let report = evaluate(&record, &TrialEvidence::default());
    let result = single(&report, false);
    assert!(
        result.detail.contains("missing `fake-tool-audit` evidence"),
        "unexpected detail: {}",
        result.detail
    );
}

// ---------------------------------------------------------------------------
// prohibited_absent (§2.7.2)
// ---------------------------------------------------------------------------

#[test]
fn prohibited_absent_substring_hit_fails_and_clean_passes() {
    let record = prohibited_record(
        &[prohibited_pattern("npm install -g", "substring", None)],
        "fake-tool-audit",
    );
    let dirty = audit_lines(&[("npm", &["install", "-g", "left-pad"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&dirty),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("npm install -g"),
        "unexpected detail: {}",
        result.detail
    );

    let clean = audit_lines(&[("root", &["doctor", "--json"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&clean),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn prohibited_absent_regex_and_tool_filter() {
    let record = prohibited_record(
        &[prohibited_pattern("^brew install", "regex", Some("brew"))],
        "fake-tool-audit",
    );
    let audit = audit_lines(&[("brew", &["install", "ripgrep"])]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);

    // The same pattern filtered to another tool does not apply.
    let filtered = prohibited_record(
        &[prohibited_pattern("^brew install", "regex", Some("pip"))],
        "fake-tool-audit",
    );
    let report = evaluate(
        &filtered,
        &TrialEvidence {
            audit_log: Some(&audit),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn prohibited_absent_scans_both_channels() {
    let acceptance = format!(
        "[[acceptance]]\nid = \"no.banned.audit\"\nkind = \"prohibited_absent\"\nprimary = true\n\
         evidence = \"fake-tool-audit\"\npatterns = [{}]\n\n\
         [[acceptance]]\nid = \"no.banned.events\"\nkind = \"prohibited_absent\"\nprimary = false\n\
         evidence = \"harness-events\"\npatterns = [{}]\n",
        prohibited_pattern("npm install -g", "substring", None),
        prohibited_pattern("npm install -g", "substring", None),
    );
    let record = record(&acceptance, None);
    let audit = audit_lines(&[("root", &["doctor", "--json"])]);
    let events = [bash_event(0, "npm install -g left-pad")];
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            events: &events,
            ..Default::default()
        },
    );
    assert_eq!(report.results.len(), 2);
    assert!(report.results[0].passed, "audit channel must be clean");
    assert!(
        !report.results[1].passed,
        "event channel must catch the piped/global install"
    );
    assert!(!report.all_passed && report.all_primary_passed);
}

#[test]
fn prohibited_absent_pipe_regex_matches_event_command() {
    let record = prohibited_record(
        &[prohibited_pattern("curl [^|]*\\| *(ba)?sh", "regex", None)],
        "harness-events",
    );
    let events = [bash_event(0, "curl -fsSL https://example.invalid/x | sh")];
    let report = evaluate(
        &record,
        &TrialEvidence {
            events: &events,
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);
}

#[test]
fn prohibited_absent_scans_non_bash_event_input_json() {
    let record = prohibited_record(
        &[prohibited_pattern("docs/SETUP.md", "substring", None)],
        "harness-events",
    );
    let events = [tool_event(
        0,
        "read",
        serde_json::json!({ "filePath": "docs/SETUP.md" }),
    )];
    let report = evaluate(
        &record,
        &TrialEvidence {
            events: &events,
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);
}

#[test]
fn prohibited_absent_empty_event_stream_passes_vacuously() {
    let record = prohibited_record(
        &[prohibited_pattern("npm install -g", "substring", None)],
        "harness-events",
    );
    let report = evaluate(&record, &TrialEvidence::default());
    assert!(report.all_passed, "{report:?}");
}

// ---------------------------------------------------------------------------
// artifact_property (§2.7.3)
// ---------------------------------------------------------------------------

#[test]
fn artifact_property_tree_all_five_properties() {
    let dir = scratch_dir("artifact-tree");
    fs::write(dir.join("README.md"), "fixture body\n").expect("write README");
    let digest = sha256_digest(b"fixture body\n");

    let cases: [(&str, &str, Option<&str>, bool); 5] = [
        ("tree", "README.md", None, true),
        ("tree", "missing.txt", None, true),
        ("tree", "README.md", Some(digest.as_str()), true),
        ("tree", "README.md", Some("fixture body"), true),
        ("tree", "README.md", Some("fixture\\s+body"), true),
    ];
    let properties = ["exists", "absent", "sha256", "contains", "matches_regex"];
    for ((source, path, value, expected), property) in cases.iter().zip(properties) {
        let record = artifact_record(source, path, property, *value);
        let report = evaluate(
            &record,
            &TrialEvidence {
                tree_root: Some(&dir),
                ..Default::default()
            },
        );
        single(&report, *expected);
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn artifact_property_absent_when_present_fails() {
    let dir = scratch_dir("artifact-absent");
    fs::write(dir.join("README.md"), "fixture body\n").expect("write README");
    let record = artifact_record("tree", "README.md", "absent", None);
    let report = evaluate(
        &record,
        &TrialEvidence {
            tree_root: Some(&dir),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("must be absent"),
        "unexpected detail: {}",
        result.detail
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn artifact_property_missing_tree_root_fails_with_detail() {
    let record = artifact_record("tree", "README.md", "exists", None);
    let report = evaluate(&record, &TrialEvidence::default());
    let result = single(&report, false);
    assert!(
        result.detail.contains("missing `tree` evidence"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn artifact_property_audit_log_source() {
    let bytes = b"{\"seq\":1,\"tool\":\"root\",\"argv\":[\"doctor\"],\"exit\":0,\"profile\":\"root.healthy.v1\"}\n";
    let digest = sha256_digest(bytes);
    let parsed = parse_audit_log(bytes).expect("audit bytes parse");

    let cases: [(&str, Option<&str>, bool); 5] = [
        ("exists", None, true),
        ("sha256", Some(digest.as_str()), true),
        ("contains", Some("\"tool\":\"root\""), true),
        ("matches_regex", Some("\"seq\":1"), true),
        ("absent", None, false),
    ];
    for (property, value, expected) in cases {
        let record = artifact_record(
            "fake-tool-audit",
            FAKE_TOOL_AUDIT_FILE_NAME,
            property,
            value,
        );
        let report = evaluate(
            &record,
            &TrialEvidence {
                audit_log: Some(&parsed),
                audit_log_bytes: Some(bytes),
                ..Default::default()
            },
        );
        let result = single(&report, expected);
        assert_eq!(result.kind, "artifact_property");
    }

    // Content properties need the raw bytes; the parsed log alone is not
    // enough, and the report stays total.
    let record = artifact_record(
        "fake-tool-audit",
        FAKE_TOOL_AUDIT_FILE_NAME,
        "contains",
        Some("root"),
    );
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&parsed),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("missing raw audit-log bytes"),
        "unexpected detail: {}",
        result.detail
    );
}

// ---------------------------------------------------------------------------
// response_fact (§2.7.4)
// ---------------------------------------------------------------------------

#[test]
fn response_fact_fence_stripping_hides_code_quoted_claims() {
    let message = "All done.\n```\nsuccessfully installed ripgrep\n```\n";
    let record = response_fact_record("forbid_substrings = ['successfully installed']");
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some(message),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let kept = response_fact_record(
        "strip_code_fences = false\nforbid_substrings = ['successfully installed']",
    );
    let report = evaluate(
        &kept,
        &TrialEvidence {
            final_message: Some(message),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("forbidden substring"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn response_fact_tilde_fences_are_stripped_too() {
    let message = "kept\n~~~\nsuccessfully installed\n~~~\n";
    let record = response_fact_record("forbid_substrings = ['successfully installed']");
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some(message),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn response_fact_expect_any_of_groups() {
    let record = response_fact_record(
        "expect_any_of = [['rollback is available'], ['Rollback is available']]",
    );
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("Rollback is available after the install."),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("Nothing about undo here."),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("no expect_any_of group matched"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn response_fact_empty_expect_any_of_fails() {
    let record = response_fact_record("expect_any_of = []");
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("anything"),
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);
}

#[test]
fn response_fact_expect_substrings_are_anded() {
    let record = response_fact_record("expect_substrings = ['alpha', 'beta']");
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("alpha only"),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("beta"),
        "unexpected detail: {}",
        result.detail
    );
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("alpha and beta"),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn response_fact_forbid_substrings_and_regex() {
    let record = response_fact_record(
        "forbid_substrings = ['I installed']\nforbid_regex = 'definitely .*installed'",
    );
    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("I installed it"),
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);

    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("definitely now installed"),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("forbidden regex"),
        "unexpected detail: {}",
        result.detail
    );

    let report = evaluate(
        &record,
        &TrialEvidence {
            final_message: Some("I did not install anything; rollback is available."),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn response_fact_missing_final_message_fails() {
    let record = response_fact_record("expect_substrings = ['anything']");
    let report = evaluate(&record, &TrialEvidence::default());
    let result = single(&report, false);
    assert!(
        result.detail.contains("missing `final-message` evidence"),
        "unexpected detail: {}",
        result.detail
    );
}

// ---------------------------------------------------------------------------
// test_pass (§2.7.5)
// ---------------------------------------------------------------------------

#[test]
fn test_pass_matching_exit_passes() {
    let record = test_pass_record(0, None);
    let outcomes = [verifier_outcome("run-suite", 0, b"", false)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &outcomes,
            ..Default::default()
        },
    );
    let result = single(&report, true);
    assert_eq!(result.kind, "test_pass");
}

#[test]
fn test_pass_wrong_exit_fails() {
    let record = test_pass_record(0, None);
    let outcomes = [verifier_outcome("run-suite", 1, b"", false)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &outcomes,
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("expected_exit 0"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn test_pass_declared_hash_must_match() {
    let digest = sha256_digest(b"expected stdout");
    let record = test_pass_record(0, Some(&digest));

    let matching = [verifier_outcome("run-suite", 0, b"expected stdout", false)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &matching,
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let mismatching = [verifier_outcome("run-suite", 0, b"actual stdout", false)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &mismatching,
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("stdout sha256"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn test_pass_mutation_detected_fails() {
    let record = test_pass_record(0, None);
    let outcomes = [verifier_outcome("run-suite", 0, b"", true)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &outcomes,
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("mutation_detected"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn test_pass_missing_outcome_fails_with_detail() {
    let record = test_pass_record(0, None);
    let report = evaluate(&record, &TrialEvidence::default());
    let result = single(&report, false);
    assert!(
        result
            .detail
            .contains("no verifier outcome for `run-suite`"),
        "unexpected detail: {}",
        result.detail
    );

    // A non-matching outcome id is also missing evidence for this binding.
    let other = [verifier_outcome("other-suite", 0, b"", false)];
    let report = evaluate(
        &record,
        &TrialEvidence {
            verifiers: &other,
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("available: other-suite"),
        "unexpected detail: {}",
        result.detail
    );
}

// ---------------------------------------------------------------------------
// diff_allowlist (§2.7.6)
// ---------------------------------------------------------------------------

#[test]
fn diff_allowlist_empty_allowlist_clean_tree_passes() {
    let record = diff_record(&[]);
    let changes: Vec<TreeChange> = Vec::new();
    let report = evaluate(
        &record,
        &TrialEvidence {
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn diff_allowlist_unlisted_change_fails() {
    let record = diff_record(&["docs/*"]);
    let changes = [TreeChange {
        path: "src/main.rs".to_string(),
        kind: TreeChangeKind::Modified,
    }];
    let report = evaluate(
        &record,
        &TrialEvidence {
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("src/main.rs (modified)"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn diff_allowlist_glob_covers_change() {
    let record = diff_record(&["src/*"]);
    let changes = [
        TreeChange {
            path: "src/main.rs".to_string(),
            kind: TreeChangeKind::Modified,
        },
        // The documented subset: `*` crosses `/` (fnmatch style).
        TreeChange {
            path: "src/deep/file.rs".to_string(),
            kind: TreeChangeKind::Added,
        },
    ];
    let report = evaluate(
        &record,
        &TrialEvidence {
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
}

#[test]
fn diff_allowlist_deletion_is_compared_like_a_write() {
    let deletion = [TreeChange {
        path: "old.txt".to_string(),
        kind: TreeChangeKind::Deleted,
    }];
    let covered = diff_record(&["*.txt"]);
    let report = evaluate(
        &covered,
        &TrialEvidence {
            tree_changes: Some(&deletion),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let uncovered = diff_record(&["src/*"]);
    let report = evaluate(
        &uncovered,
        &TrialEvidence {
            tree_changes: Some(&deletion),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("old.txt (deleted)"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn diff_allowlist_derives_from_snapshots() {
    let pre = snapshot(&[("a.txt", "one\n"), ("gone.txt", "bye\n")]);
    let post = snapshot(&[("a.txt", "two\n"), ("b.txt", "new\n")]);
    let record = diff_record(&["*.txt"]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            pre_snapshot: Some(&pre),
            post_snapshot: Some(&post),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let empty = diff_record(&[]);
    let report = evaluate(
        &empty,
        &TrialEvidence {
            pre_snapshot: Some(&pre),
            post_snapshot: Some(&post),
            ..Default::default()
        },
    );
    let result = single(&report, false);
    assert!(
        result.detail.contains("a.txt (modified)"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn diff_allowlist_missing_evidence_fails() {
    let record = diff_record(&[]);
    let report = evaluate(&record, &TrialEvidence::default());
    let result = single(&report, false);
    assert!(
        result.detail.contains("missing `tree` evidence"),
        "unexpected detail: {}",
        result.detail
    );
}

#[test]
fn diff_snapshots_detects_all_three_kinds_sorted() {
    let pre = snapshot(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let post = snapshot(&[("b.txt", "b changed\n"), ("c.txt", "c\n")]);
    let changes = diff_snapshots(&pre, &post);
    let observed: Vec<(&str, TreeChangeKind)> = changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect();
    assert_eq!(
        observed,
        [
            ("a.txt", TreeChangeKind::Deleted),
            ("b.txt", TreeChangeKind::Modified),
            ("c.txt", TreeChangeKind::Added),
        ]
    );
}

// ---------------------------------------------------------------------------
// parse_audit_log (§2.5.2)
// ---------------------------------------------------------------------------

const VALID_AUDIT: &str = concat!(
    "{\"seq\":1,\"tool\":\"root\",\"argv\":[\"doctor\",\"--json\"],\"exit\":0,\"profile\":\"root.healthy.v1\"}\n",
    "{\"seq\":2,\"tool\":\"brew\",\"argv\":[\"install\",\"ripgrep\"],\"exit\":1,\"profile\":\"installer.record-deny.v1\"}\n",
);

#[test]
fn parse_audit_log_accepts_valid_jsonl() {
    let lines = parse_audit_log(VALID_AUDIT.as_bytes()).expect("valid audit log parses");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].seq, 1);
    assert_eq!(lines[0].tool, "root");
    assert_eq!(lines[0].argv, ["doctor", "--json"]);
    assert_eq!(lines[0].exit, 0);
    assert_eq!(lines[0].profile, "root.healthy.v1");
    assert_eq!(lines[1].exit, 1);

    // A missing final newline is tolerated; an empty file is an empty log.
    let no_newline = parse_audit_log(VALID_AUDIT.trim_end().as_bytes()).expect("parses");
    assert_eq!(no_newline.len(), 2);
    assert!(parse_audit_log(b"").expect("empty log parses").is_empty());
}

#[test]
fn parse_audit_log_refuses_unknown_and_missing_keys() {
    let unknown = "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\",\"ts\":1}";
    let error = parse_audit_log(unknown.as_bytes()).expect_err("unknown key refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditUnknownKey);
    assert!(error.to_string().contains("`ts`"));

    let missing = "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0}";
    let error = parse_audit_log(missing.as_bytes()).expect_err("missing key refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditMissingKey);
    assert!(error.to_string().contains("`profile`"));
}

#[test]
fn parse_audit_log_enforces_exact_sequence() {
    let gap = concat!(
        "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
        "{\"seq\":3,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
    );
    let error = parse_audit_log(gap.as_bytes()).expect_err("seq gap refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditSequence);

    let starts_late = "{\"seq\":2,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}";
    let error = parse_audit_log(starts_late.as_bytes()).expect_err("seq must start at 1");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditSequence);

    let reordered = concat!(
        "{\"seq\":2,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
        "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
    );
    let error = parse_audit_log(reordered.as_bytes()).expect_err("reordered seq refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditSequence);
}

#[test]
fn parse_audit_log_is_strict_about_whitespace() {
    let trailing = "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"} \n";
    let error = parse_audit_log(trailing.as_bytes()).expect_err("trailing space refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditLineWhitespace);

    let crlf = "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\r\n";
    let error = parse_audit_log(crlf.as_bytes()).expect_err("CRLF refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditLineWhitespace);

    let blank = concat!(
        "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
        "\n",
        "{\"seq\":2,\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}\n",
    );
    let error = parse_audit_log(blank.as_bytes()).expect_err("blank line refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::AuditLineEmpty);
}

#[test]
fn parse_audit_log_refuses_malformed_lines() {
    let not_json = parse_audit_log(b"not json\n").expect_err("invalid JSON refused");
    assert_eq!(not_json.kind(), AcceptanceErrorKind::AuditLineInvalidJson);

    let not_object = parse_audit_log(b"[1,2,3]\n").expect_err("array refused");
    assert_eq!(not_object.kind(), AcceptanceErrorKind::AuditLineNotObject);

    let wrong_seq = "{\"seq\":\"1\",\"tool\":\"root\",\"argv\":[],\"exit\":0,\"profile\":\"p\"}";
    assert_eq!(
        parse_audit_log(wrong_seq.as_bytes())
            .expect_err("string seq refused")
            .kind(),
        AcceptanceErrorKind::AuditFieldType
    );

    let wrong_argv = "{\"seq\":1,\"tool\":\"root\",\"argv\":[1],\"exit\":0,\"profile\":\"p\"}";
    assert_eq!(
        parse_audit_log(wrong_argv.as_bytes())
            .expect_err("non-string argv refused")
            .kind(),
        AcceptanceErrorKind::AuditFieldType
    );

    let float_exit = "{\"seq\":1,\"tool\":\"root\",\"argv\":[],\"exit\":0.5,\"profile\":\"p\"}";
    assert_eq!(
        parse_audit_log(float_exit.as_bytes())
            .expect_err("float exit refused")
            .kind(),
        AcceptanceErrorKind::AuditFieldType
    );

    let non_utf8 = parse_audit_log(&[0xff, 0xfe]).expect_err("non-UTF-8 refused");
    assert_eq!(non_utf8.kind(), AcceptanceErrorKind::AuditLogNotUtf8);
}

// ---------------------------------------------------------------------------
// evaluate_acceptance aggregation
// ---------------------------------------------------------------------------

#[test]
fn report_order_and_aggregates_follow_the_record() {
    let acceptance = format!(
        "[[acceptance]]\nid = \"seq.root\"\nkind = \"command_sequence\"\nprimary = true\n\
         evidence = \"fake-tool-audit\"\nsteps = [{}]\n\n\
         [[acceptance]]\nid = \"tree.diff\"\nkind = \"diff_allowlist\"\nprimary = false\n\
         evidence = \"tree\"\nallow_paths = ['docs/*']\n\n\
         [[acceptance]]\nid = \"final.summary\"\nkind = \"response_fact\"\nprimary = true\n\
         evidence = \"final-message\"\nexpect_substrings = ['installed']\n",
        exact_step(&["root", "doctor", "--json"], false),
    );
    let record = record(&acceptance, None);
    let audit = audit_lines(&[("root", &["doctor", "--json"])]);
    let changes = [TreeChange {
        path: "src/main.rs".to_string(),
        kind: TreeChangeKind::Modified,
    }];
    let report = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            final_message: Some("installed ripgrep"),
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    let ids: Vec<&str> = report.results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["seq.root", "tree.diff", "final.summary"]);
    assert_eq!(report.failed_ids(), ["tree.diff"]);
    assert!(!report.all_passed);
    assert!(report.all_primary_passed);

    // Deterministic: evaluating twice yields the identical report.
    let again = evaluate(
        &record,
        &TrialEvidence {
            audit_log: Some(&audit),
            final_message: Some("installed ripgrep"),
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    assert_eq!(again, report);
}

#[test]
fn failing_primary_assertion_fails_the_primary_aggregate() {
    let record = command_sequence_record(
        &[exact_step(&["root", "doctor", "--json"], false)],
        None,
        None,
        "fake-tool-audit",
    );
    let report = evaluate(&record, &TrialEvidence::default());
    assert!(!report.all_passed);
    assert!(!report.all_primary_passed);
}

#[test]
fn missing_evidence_stays_total_across_all_kinds() {
    let acceptance = format!(
        "[[acceptance]]\nid = \"seq.root\"\nkind = \"command_sequence\"\nprimary = true\n\
         evidence = \"fake-tool-audit\"\nsteps = [{}]\n\n\
         [[acceptance]]\nid = \"no.banned\"\nkind = \"prohibited_absent\"\nprimary = true\n\
         evidence = \"fake-tool-audit\"\npatterns = [{}]\n\n\
         [[acceptance]]\nid = \"artifact.marker\"\nkind = \"artifact_property\"\nprimary = false\n\
         evidence = \"tree\"\nsource = \"tree\"\npath = 'README.md'\nproperty = 'exists'\n\n\
         [[acceptance]]\nid = \"final.summary\"\nkind = \"response_fact\"\nprimary = true\n\
         evidence = \"final-message\"\nexpect_substrings = ['installed']\n\n\
         [[acceptance]]\nid = \"tests.pass\"\nkind = \"test_pass\"\nprimary = true\n\
         evidence = \"verifier-run\"\nverifier_id = \"run-suite\"\n\n\
         [[acceptance]]\nid = \"tree.diff\"\nkind = \"diff_allowlist\"\nprimary = false\n\
         evidence = \"tree\"\nallow_paths = []\n",
        exact_step(&["root", "doctor", "--json"], false),
        prohibited_pattern("npm install -g", "substring", None),
    );
    let verifier = "[[verifier.commands]]\nid = \"run-suite\"\nargv = ['sh', 'check.sh']\n\
                    expected_exit = 0\ntimeout_seconds = 5\n";
    let record = record(&acceptance, Some(verifier));
    let report = evaluate(&record, &TrialEvidence::default());
    assert_eq!(report.results.len(), 6);
    assert!(
        report.results.iter().all(|result| !result.passed),
        "every assertion must fail on missing evidence: {report:?}"
    );
    assert!(!report.all_passed);
    assert!(!report.all_primary_passed);
}

// ---------------------------------------------------------------------------
// End-to-end-ish: real tree capture + diff drives acceptance
// ---------------------------------------------------------------------------

#[test]
fn tree_capture_and_diff_drive_acceptance() {
    let dir = scratch_dir("e2e");
    fs::write(dir.join("keep.txt"), "keep\n").expect("write keep");
    fs::create_dir_all(dir.join("docs")).expect("create docs");
    fs::write(dir.join("docs/guide.md"), "guide\n").expect("write guide");
    fs::create_dir_all(dir.join(".git")).expect("create .git");
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

    let pre = capture_tree_snapshot(&dir).expect("pre snapshot");
    assert!(pre.files.contains_key("keep.txt"));
    assert!(pre.files.contains_key("docs/guide.md"));
    assert!(
        !pre.files.keys().any(|path| path.starts_with(".git")),
        ".git must never contribute"
    );

    fs::write(dir.join("docs/guide.md"), "guide updated\n").expect("update guide");
    fs::write(dir.join("docs/new.md"), "new\n").expect("add new");
    let post = capture_tree_snapshot(&dir).expect("post snapshot");

    let changes = diff_snapshots(&pre, &post);
    let observed: Vec<(&str, TreeChangeKind)> = changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect();
    assert_eq!(
        observed,
        [
            ("docs/guide.md", TreeChangeKind::Modified),
            ("docs/new.md", TreeChangeKind::Added),
        ]
    );

    // A covering allowlist passes; an empty allowlist fails.
    let record = diff_record(&["docs/*"]);
    let report = evaluate(
        &record,
        &TrialEvidence {
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    let empty = diff_record(&[]);
    let report = evaluate(
        &empty,
        &TrialEvidence {
            tree_changes: Some(&changes),
            ..Default::default()
        },
    );
    assert!(!single(&report, false).passed);

    // The same snapshots can drive the assertion without a precomputed diff.
    let report = evaluate(
        &record,
        &TrialEvidence {
            pre_snapshot: Some(&pre),
            post_snapshot: Some(&post),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");

    // Artifact properties read the real tree.
    let digest = sha256_digest(b"keep\n");
    let artifact = artifact_record("tree", "keep.txt", "sha256", Some(&digest));
    let report = evaluate(
        &artifact,
        &TrialEvidence {
            tree_root: Some(&dir),
            ..Default::default()
        },
    );
    assert!(report.all_passed, "{report:?}");
    assert_eq!(
        read_tree_file(&dir, "keep.txt").expect("read keep"),
        Some(b"keep\n".to_vec())
    );
    assert_eq!(
        read_tree_file(&dir, "missing.txt").expect("missing is None"),
        None
    );
    assert_eq!(
        read_tree_file(&dir, "../escape.txt")
            .expect_err("escaping path refused")
            .kind(),
        AcceptanceErrorKind::TreePathInvalid
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn capture_tree_snapshot_requires_a_directory() {
    let dir = scratch_dir("missing-root");
    let error = capture_tree_snapshot(&dir.join("nope")).expect_err("missing root refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::TreeNotADirectory);
    let file = dir.join("file.txt");
    fs::write(&file, "x\n").expect("write file");
    let error = capture_tree_snapshot(&file).expect_err("file root refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::TreeNotADirectory);
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn capture_tree_snapshot_refuses_symlinks() {
    let dir = scratch_dir("symlink");
    fs::write(dir.join("ok.txt"), "ok\n").expect("write ok");
    std::os::unix::fs::symlink(dir.join("ok.txt"), dir.join("link.txt")).expect("create symlink");
    let error = capture_tree_snapshot(&dir).expect_err("symlink refused");
    assert_eq!(error.kind(), AcceptanceErrorKind::TreeSymlinkRefused);
    let _ = fs::remove_dir_all(&dir);
}
