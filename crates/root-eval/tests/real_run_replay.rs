//! Replay of the M6 live-harness captures (`Docs/Eval/milestone-6/`).
//!
//! The M0 golden captures froze the parser against a synthetic harness. The
//! M6 integration probe (`Docs/Eval/milestone-6/INTEGRATION-PROBE.md` §B.2)
//! replayed 390 real NDJSON lines from the pinned `1.18.27` binary through
//! the parser: **zero hard incompatibilities**, three *missing reads*.
//!
//! This file replays three of those real captures (copied byte-for-byte
//! into `tests/fixtures/real-runs/`) and asserts the reads the probe
//! identified as gaps:
//!
//! | Gap | Live evidence |
//! |---|---|
//! | `part.state.metadata` never read | `skill` tool metadata carries the loaded skill's `{name, dir}` |
//! | `part.reason` never read | `step_finish` carries `"stop"` vs `"tool-calls"` |
//! | no "the turn ended" signal | a truncated run is indistinguishable from a completed one |
//!
//! The three captures cover the three turn shapes the probe observed:
//!
//! - `B1_treat` — a **complete** run (with skill): ends on
//!   `step_finish{reason:"stop"}` after a final assistant text event.
//! - `B2_base` — a **truncated** run: the model was still mid-exploration
//!   when the stream ended, with no final assistant text at all.
//! - `B3_base_retry` — a **provider stall**: the provider timed out
//!   (`ProviderHeaderTimeoutError`, 314 923 ms), the NDJSON simply stops
//!   mid-turn, and **no `error` envelope is ever emitted**.

use std::path::{Path, PathBuf};

use root_eval::{
    normalized_events_digest, parse_ndjson_line, turn_state, NormalizedEvent, ToolCall, TurnState,
    FINISH_REASON_STOP, FINISH_REASON_TOOL_CALLS, SOURCE_ERROR, SOURCE_STEP_FINISH,
    SOURCE_STEP_START, SOURCE_TEXT, SOURCE_TOOL_USE,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn real_runs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real-runs")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Docs/Eval/milestone-0/fixtures/golden")
}

fn parse_capture(path: &Path) -> Vec<NormalizedEvent> {
    let content =
        std::fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            parse_ndjson_line(line, index as u64)
                .unwrap_or_else(|error| panic!("{} line {index}: {error}", path.display()))
        })
        .collect()
}

fn real_run(name: &str) -> Vec<NormalizedEvent> {
    let path = real_runs_dir().join(name);
    parse_capture(&path)
}

fn tool_calls(events: &[NormalizedEvent]) -> Vec<&ToolCall> {
    events
        .iter()
        .filter(|event| event.source_event_type == SOURCE_TOOL_USE)
        .filter_map(|event| event.tool.as_ref())
        .collect()
}

fn finish_reasons(events: &[NormalizedEvent]) -> Vec<Option<&str>> {
    events
        .iter()
        .filter(|event| event.source_event_type == SOURCE_STEP_FINISH)
        .map(|event| event.reason.as_deref())
        .collect()
}

// ---------------------------------------------------------------------------
// (a) A complete run
// ---------------------------------------------------------------------------

#[test]
fn complete_run_exposes_the_skill_directory_and_ends_completed() {
    let events = real_run("B1_treat.ndjson");
    assert_eq!(events.len(), 17);

    // One session id for the whole capture, carried on every event.
    let session = "ses_ea9e524e7ffeXj9X3tXTMbIKnK";
    assert!(events.iter().all(|event| event.session_id == session));
    // M0 identity policy: the stream never names the model.
    assert!(events.iter().all(|event| event.model_observed.is_none()));
    assert!(events.iter().all(|event| event.model_requested.is_none()));

    // Gap 1: `part.state.metadata` is now readable, so a harness can verify
    // WHICH directory the skill was loaded from.
    let calls = tool_calls(&events);
    let skill = calls
        .iter()
        .find(|call| call.name == "skill")
        .expect("the treatment run loads the skill");
    assert_eq!(skill.status, "completed");
    assert_eq!(skill.metadata_name(), Some("root"));
    let dir = skill
        .metadata_dir()
        .expect("skill metadata carries the loaded directory");
    assert!(
        dir.ends_with("/cwd/.opencode/skill/root"),
        "skill dir must point at the seeded project skill: {dir}"
    );
    // The raw JSON is preserved verbatim (nothing is dropped or re-typed).
    let metadata = skill.metadata.as_ref().expect("raw metadata is kept");
    assert_eq!(metadata["name"], "root");
    assert_eq!(metadata["dir"], dir);
    assert_eq!(metadata["truncated"], false);

    // The other tools carry their own metadata objects, untouched.
    let bash = calls
        .iter()
        .find(|call| call.name == "bash")
        .expect("the treatment run shells out to the root shim");
    assert_eq!(bash.metadata.as_ref().expect("bash metadata")["exit"], 0);

    // Gap 2: `part.reason` is now readable on every step_finish.
    assert_eq!(
        finish_reasons(&events),
        vec![
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_STOP),
        ]
    );

    // Gap 3: a completed turn is classified as completed.
    assert_eq!(
        turn_state(&events),
        TurnState::Completed {
            reason: Some(FINISH_REASON_STOP.to_string()),
            step_finish_seen: true,
        }
    );
    assert!(turn_state(&events).is_completed());
    assert!(!turn_state(&events).is_truncated());

    // The run closed with a final assistant message.
    let last_text = events
        .iter()
        .rfind(|event| event.source_event_type == SOURCE_TEXT)
        .and_then(|event| event.text.as_deref())
        .expect("the complete run ends with assistant text");
    assert!(
        last_text.contains("Installed ripgrep via Root"),
        "{last_text}"
    );
}

// ---------------------------------------------------------------------------
// (b) A run truncated with no final text
// ---------------------------------------------------------------------------

#[test]
fn truncated_run_without_final_text_is_classified_truncated() {
    let events = real_run("B2_base.ndjson");
    assert_eq!(events.len(), 14);

    // The probe's finding: this run produced NO assistant text at all.
    assert!(events
        .iter()
        .all(|event| event.source_event_type != SOURCE_TEXT));
    assert!(events.iter().all(|event| event.text.is_none()));

    // Every step_finish carries the non-terminal reason: the model kept
    // asking for tool calls and the stream stopped there.
    assert_eq!(
        finish_reasons(&events),
        vec![
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
        ]
    );

    assert_eq!(
        turn_state(&events),
        TurnState::Truncated {
            last_event_type: Some(SOURCE_STEP_FINISH.to_string()),
        }
    );
    assert!(turn_state(&events).is_truncated());
    assert!(!turn_state(&events).is_completed());

    // The metadata gap still applies to this capture: the ask-denial call
    // carries no metadata, the completed ones do.
    let calls = tool_calls(&events);
    let denied = calls
        .iter()
        .find(|call| call.status == "error")
        .expect("the baseline run hits an auto-rejected permission ask");
    assert!(denied.metadata.is_none());
    assert!(calls
        .iter()
        .filter(|call| call.status == "completed")
        .all(|call| call.metadata.is_some()));
}

// ---------------------------------------------------------------------------
// (c) A run whose provider failed (no error envelope in the stream)
// ---------------------------------------------------------------------------

#[test]
fn provider_stall_truncates_the_stream_without_fabricating_an_error_event() {
    let events = real_run("B3_base_retry.ndjson");
    assert_eq!(events.len(), 10);

    // The provider stalled for ~300 s and the NDJSON simply stopped
    // mid-turn: there is NO error envelope in the stream, and the parser
    // must not invent one.
    assert!(events
        .iter()
        .all(|event| event.source_event_type != SOURCE_ERROR));
    assert!(events.iter().all(|event| event.error.is_none()));

    // Truncated, exactly like a kill path: the last step_finish still
    // carries the non-terminal reason.
    assert_eq!(
        finish_reasons(&events),
        vec![
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
            Some(FINISH_REASON_TOOL_CALLS),
        ]
    );
    assert_eq!(
        turn_state(&events),
        TurnState::Truncated {
            last_event_type: Some(SOURCE_STEP_FINISH.to_string()),
        }
    );

    // The stall is corroborated by the capture's own metadata: the run took
    // 314 923 ms, i.e. the provider header timeout (300 000 ms) plus
    // teardown.
    let meta = std::fs::read_to_string(real_runs_dir().join("B3_base_retry.meta.txt"))
        .expect("read capture metadata");
    assert!(
        meta.contains("wall_ms=314923"),
        "the provider-stalled run must record its wall time: {meta}"
    );
}

// ---------------------------------------------------------------------------
// Golden captures still replay identically
// ---------------------------------------------------------------------------

/// Frozen digests of the M0 golden captures.
///
/// The M6 reads (`reason`, `state.metadata`) are **additive**: they are not
/// part of the sealed normalized-event projection, so every golden capture
/// still replays to the exact digest recorded before those reads existed.
/// (The constants below were taken from the pre-M6 parser and are unchanged
/// by it.)
const GOLDEN_DIGESTS: &[(&str, &str)] = &[
    (
        "01-no-tool/stdout.ndjson",
        "sha256:f15f1b883b4cb24c1b2deaa2d37354d21060ce8d727046cdf6f842ffc5bc3d55",
    ),
    (
        "02-tool/stdout.ndjson",
        "sha256:247f470f04b200c6b85c65d7105b4dc3eacd536b080dfaf1a44a2a967e1ead18",
    ),
    (
        "03-skill/stdout.ndjson",
        "sha256:1de6968faa31dd92709f6c4ba5d5d677ef59f212ab3f72c6aa7b2e79cf391d6c",
    ),
    (
        "04-provider-error/stdout.ndjson",
        "sha256:8dd0e7d34c4c1a9d98c0b104987b7e0c9ef5315a640736061d4606d87816e038",
    ),
    (
        "05-denial/stdout.ndjson",
        "sha256:f335798132f8c0539358a00a46f52f976fbb135d425dda50b4a8cb8e8dc60843",
    ),
    (
        "05-denial/05b-bash-deny-tools-removed/stdout.ndjson",
        "sha256:45d9db2d3b10b084b4dcfacfd992994fd694f636fdf700f5e8624e1195a53c64",
    ),
    (
        "05-denial/05c-ask-reject-shape/stdout.ndjson",
        "sha256:8c8086c5c2a65ca3707af35f60f1174373a3e48e35990c5ba1ca5c9016700938",
    ),
    (
        "06-timeout/stdout.ndjson",
        "sha256:8bfe7e9492ae49af9ee77f592ece8eefedfd80a4f3080b319749c01ceb979aa8",
    ),
    (
        "06-timeout/06b-tool-abort-variant/stdout.ndjson",
        "sha256:40e3545ed1da767d21f7aaca4e2a65f1839641ca0d8dbdd384e0f9c57a60e54b",
    ),
    (
        "07-db-memory-optional/stdout.ndjson",
        "sha256:0d5bb99f90c2a89d2bda661676047f7f609bde5f20e7a12d2ced556d2c7dafae",
    ),
];

#[test]
fn golden_captures_still_replay_to_their_frozen_digests() {
    for (name, expected) in GOLDEN_DIGESTS {
        let events = parse_capture(&golden_dir().join(name));
        let digest = normalized_events_digest(&events).expect("digest");
        assert_eq!(
            &digest, expected,
            "golden capture {name} must replay byte-identically"
        );
    }
}

#[test]
fn golden_captures_classify_the_way_the_m0_contract_says() {
    // 01/02/03/05/07 close on `step_finish{reason:"stop"}`.
    for name in [
        "01-no-tool/stdout.ndjson",
        "02-tool/stdout.ndjson",
        "03-skill/stdout.ndjson",
        "05-denial/stdout.ndjson",
        "07-db-memory-optional/stdout.ndjson",
    ] {
        let events = parse_capture(&golden_dir().join(name));
        assert_eq!(
            turn_state(&events),
            TurnState::Completed {
                reason: Some(FINISH_REASON_STOP.to_string()),
                step_finish_seen: true,
            },
            "{name} completes"
        );
    }

    // 05c stops on `step_finish{reason:"tool-calls"}` with no final text.
    let ask_reject =
        parse_capture(&golden_dir().join("05-denial/05c-ask-reject-shape/stdout.ndjson"));
    assert_eq!(
        turn_state(&ask_reject),
        TurnState::Truncated {
            last_event_type: Some(SOURCE_STEP_FINISH.to_string()),
        }
    );

    // 06 is the M0 kill-path capture: a single `step_start`, no terminal
    // event at all.
    let timeout = parse_capture(&golden_dir().join("06-timeout/stdout.ndjson"));
    assert_eq!(
        turn_state(&timeout),
        TurnState::Truncated {
            last_event_type: Some(SOURCE_STEP_START.to_string()),
        }
    );

    // The empty stream is truncated as well (no last event at all).
    assert_eq!(
        turn_state(&[]),
        TurnState::Truncated {
            last_event_type: None,
        }
    );
}

#[test]
fn golden_skill_capture_exposes_its_metadata() {
    let events = parse_capture(&golden_dir().join("03-skill/stdout.ndjson"));
    let skill = tool_calls(&events)
        .into_iter()
        .find(|call| call.name == "skill")
        .expect("the golden capture loads the skill");
    assert_eq!(skill.metadata_name(), Some("root"));
    assert!(skill
        .metadata_dir()
        .expect("golden skill metadata carries the directory")
        .ends_with("/cwd/.opencode/skill/root"));
    // The seeded body is still replayed byte-for-byte (unchanged by the
    // M6 reads).
    assert!(skill.output_str.contains("<skill_content name=\"root\">"));
}
