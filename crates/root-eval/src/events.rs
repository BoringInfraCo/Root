//! Normalized harness event parsing (Milestone 2, Track B).
//!
//! Parses the frozen OpenCode `1.18.27` NDJSON stream contract from
//! `Docs/Eval/milestone-0/` into [`NormalizedEvent`] records. The envelope is
//! `{type, timestamp, sessionID, part|error}` where `type` is one of
//! `step_start`, `text`, `step_finish`, `tool_use`, or `error` (underscore
//! form; the nested `part.type` uses the hyphenated `step-start` /
//! `step-finish` / `text` / `tool` form and is not authoritative).
//!
//! # M0 contract rules encoded here
//!
//! - **No terminal event.** Streams settle at EOF + process exit; a truncated
//!   stream (kill paths, `06-timeout`) ends with no terminal event. The parser
//!   therefore never requires a terminal event.
//! - **Deny-vs-ask (live correction).** A `deny` rule removes the tool from
//!   the model toolset entirely, so no event is emitted for it. The denial
//!   error shape exists only for `ask`-mode auto-reject: a `tool_use` with
//!   `state.status == "error"` and `state.error == "The user rejected
//!   permission to use this specific tool call."` ([`ASK_REJECT_MESSAGE`]).
//! - **Identity policy.** The stream never names the model that produced a
//!   message, so [`NormalizedEvent::model_observed`] is always `None`; see
//!   [`MODEL_OBSERVED_LIMITATION`].
//! - **Unknown types are typed errors, never silent skips.**
//!   [`parse_ndjson_line`] returns [`EventParseError::UnknownEventType`] for
//!   any envelope `type` outside the five frozen shapes, and
//!   [`EventParseError::InvalidShape`] for envelopes that fail structural
//!   validation. Callers surface these (e.g. in trial `missing_telemetry`)
//!   rather than dropping lines.
//!
//! # Live-harness reads (M6)
//!
//! The M0 contract froze the *envelope* shapes; the M6 integration probe
//! (`Docs/Eval/milestone-6/INTEGRATION-PROBE.md` §B.2) replayed 390 real
//! NDJSON lines through this parser and found **zero hard incompatibilities**
//! and three *missing reads*. They are now read:
//!
//! - [`ToolCall::metadata`] carries `part.state.metadata` verbatim, so a
//!   harness can assert **which** directory a skill loaded from
//!   ([`ToolCall::metadata_dir`]) and which skill ([`ToolCall::metadata_name`]).
//! - [`NormalizedEvent::reason`] carries `part.reason` (`"stop"` vs
//!   `"tool-calls"`), the only finish signal the frozen contract has.
//! - [`turn_state`] classifies how a turn ended ([`TurnState`]), so a
//!   truncated turn is distinguishable from a completed one.

use serde_json::Value;
use thiserror::Error;

use crate::digest::sha256_digest;

/// Envelope `type` of a stream-open lifecycle event.
pub const SOURCE_STEP_START: &str = "step_start";
/// Envelope `type` of a stream lifecycle-finish event carrying tokens + cost.
pub const SOURCE_STEP_FINISH: &str = "step_finish";
/// Envelope `type` of an assistant text event.
pub const SOURCE_TEXT: &str = "text";
/// Envelope `type` of a tool-call event (terminal states `completed`/`error`).
pub const SOURCE_TOOL_USE: &str = "tool_use";
/// Envelope `type` of a provider-error event (`{name, data}` union).
pub const SOURCE_ERROR: &str = "error";

/// Exact `ask`-mode auto-reject denial message frozen live at `1.18.27`
/// (golden `05c`).
pub const ASK_REJECT_MESSAGE: &str = "The user rejected permission to use this specific tool call.";

/// The only `step_finish.part.reason` value that **terminates** a turn: the
/// model declared the turn finished.
///
/// Every other reason (notably [`FINISH_REASON_TOOL_CALLS`]) means the turn
/// was still open when the stream ended.
pub const FINISH_REASON_STOP: &str = "stop";

/// `step_finish.part.reason` value meaning "the model asked for another tool
/// call": the turn continues, so a stream that ends right after it is
/// **truncated**, not completed.
pub const FINISH_REASON_TOOL_CALLS: &str = "tool-calls";

/// M0 identity policy: the harness NDJSON stream carries no provider/model
/// identity anywhere, so the observed model is always unavailable. The trial
/// layer records the requested `--model provider/model` verbatim instead.
pub const MODEL_OBSERVED_LIMITATION: &str =
    "M0 identity policy: the 1.18.27 NDJSON stream carries no provider/model \
     identity, so the observed model is always unavailable; the requested \
     model is recorded verbatim at the trial level";

/// Machine-matchable classification for every [`EventParseError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventParseErrorKind {
    /// The line is not valid JSON.
    InvalidJson,
    /// A required envelope field is absent or mistyped.
    MissingField,
    /// The envelope `type` is outside the five frozen shapes.
    UnknownEventType,
    /// The envelope is structurally wrong for its declared type.
    InvalidShape,
}

/// Errors produced while parsing one NDJSON stream line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventParseError {
    #[error("line {sequence}: invalid JSON: {message}")]
    InvalidJson { sequence: u64, message: String },

    #[error("line {sequence}: missing or mistyped field `{field}`")]
    MissingField { sequence: u64, field: String },

    #[error("line {sequence}: unknown event type `{event_type}`")]
    UnknownEventType { sequence: u64, event_type: String },

    #[error("line {sequence}: invalid {event_type} shape: {reason}")]
    InvalidShape {
        sequence: u64,
        event_type: String,
        reason: String,
    },
}

impl EventParseError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> EventParseErrorKind {
        match self {
            Self::InvalidJson { .. } => EventParseErrorKind::InvalidJson,
            Self::MissingField { .. } => EventParseErrorKind::MissingField,
            Self::UnknownEventType { .. } => EventParseErrorKind::UnknownEventType,
            Self::InvalidShape { .. } => EventParseErrorKind::InvalidShape,
        }
    }
}

/// Tool-call detail carried by a `tool_use` event.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Harness tool name (`part.tool`, e.g. `read`, `skill`, `bash`).
    pub name: String,
    /// Harness call identifier (`part.callID`).
    pub call_id: String,
    /// Canonical JSON of `state.input` (`"null"` when the event has no input).
    pub input_json: String,
    /// `state.output` as text (serialized JSON when the output is not a string).
    pub output_str: String,
    /// `state.status` (`completed` | `error`; `"unknown"` when absent).
    pub status: String,
    /// `state.error` message, present for error-status calls (including the
    /// [`ASK_REJECT_MESSAGE`] ask-mode denial family).
    pub error: Option<String>,
    /// `state.metadata` verbatim.
    ///
    /// The harness attaches an arbitrary JSON object per tool: the `skill`
    /// tool carries `{name, dir, truncated}` (the directory the skill was
    /// loaded from), `bash` carries `{output, exit, truncated}`, `glob`
    /// carries `{count, truncated}`, and an error-status call usually carries
    /// none. The raw JSON is preserved exactly — nothing is dropped, renamed,
    /// or re-typed — so a harness can assert on tool-specific metadata this
    /// parser knows nothing about.
    ///
    /// `None` when the event carries no `state.metadata` (or carries it as
    /// JSON `null`).
    pub metadata: Option<Value>,
}

impl ToolCall {
    /// A string field of [`Self::metadata`], when the metadata object carries
    /// that key as a string.
    ///
    /// Returns `None` for a missing key, a non-string value, or a call with
    /// no metadata at all.
    pub fn metadata_str(&self, key: &str) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
    }

    /// `metadata.dir` — the directory the tool loaded from.
    ///
    /// For the `skill` tool this is the skill directory the harness resolved
    /// (e.g. `.../.opencode/skill/root`), which is how a trial proves *which*
    /// copy of a skill was loaded.
    pub fn metadata_dir(&self) -> Option<&str> {
        self.metadata_str("dir")
    }

    /// `metadata.name` — the name the tool loaded.
    ///
    /// For the `skill` tool this is the skill name (e.g. `root`).
    pub fn metadata_name(&self) -> Option<&str> {
        self.metadata_str("name")
    }
}

/// Token/cost buckets carried by a `step_finish` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Provider-error detail carried by an `error` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    /// `error.name` (e.g. `UnknownError` in golden `04`).
    pub name: String,
    /// Canonical JSON of `error.data` (e.g. `{message, ref}`).
    pub data_json: String,
}

/// One normalized harness stream event.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedEvent {
    /// Zero-based position of the line in stream order.
    pub sequence: u64,
    /// Envelope `type`: `step_start` | `step_finish` | `text` | `tool_use` |
    /// `error`.
    pub source_event_type: String,
    /// Envelope `sessionID`, shared by every line of one harness session.
    pub session_id: String,
    /// Envelope `timestamp` (milliseconds).
    pub timestamp_ms: i64,
    /// Always `None` at parse time: the stream carries no requested-model
    /// field. The runner fills the trial-level requested model verbatim.
    pub model_requested: Option<String>,
    /// Always `None` per [`MODEL_OBSERVED_LIMITATION`].
    pub model_observed: Option<String>,
    /// Set for `tool_use` events.
    pub tool: Option<ToolCall>,
    /// Set for `text` events (`part.text`).
    pub text: Option<String>,
    /// Set for `step_finish` events carrying `part.tokens`.
    pub tokens: Option<StepTokens>,
    /// Set for `step_finish` events carrying `part.cost`.
    pub cost: Option<f64>,
    /// `part.reason` — why the step ended.
    ///
    /// Set for `step_finish` events: `"stop"` when the model finished the
    /// turn, `"tool-calls"` when it asked for another tool call (the turn
    /// continues). This is the **only** finish signal the frozen contract
    /// has — there is no terminal event — so it is what distinguishes a
    /// completed turn from a truncated one (see [`turn_state`]).
    pub reason: Option<String>,
    /// Set for `error` events.
    pub error: Option<StreamError>,
    /// `sha256:<hex>` of the raw NDJSON line bytes.
    pub raw_line_hash: String,
}

impl NormalizedEvent {
    /// The M0 identity-policy limitation note for [`Self::model_observed`].
    pub fn model_observed_limitation(&self) -> &'static str {
        MODEL_OBSERVED_LIMITATION
    }

    /// True when this is an `ask`-mode auto-reject denial (`tool_use` with
    /// error status carrying the frozen [`ASK_REJECT_MESSAGE`] family).
    pub fn is_ask_denial(&self) -> bool {
        self.tool.as_ref().is_some_and(|tool| {
            tool.status == "error" && tool.error.as_deref() == Some(ASK_REJECT_MESSAGE)
        })
    }
}

/// How a harness turn ended, as classified by [`turn_state`].
///
/// The frozen `1.18.27` contract has **no terminal event**: a stream settles
/// at EOF + process exit and the only finish signal is
/// `step_finish.part.reason`. [`turn_state`] turns that signal into exactly
/// one of these two states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnState {
    /// The turn reached a terminal finish signal.
    ///
    /// See [`turn_state`] for the two ways this happens.
    Completed {
        /// `part.reason` of the **last** `step_finish` event in the stream
        /// (`None` when the stream carried no `step_finish` at all).
        reason: Option<String>,
        /// Whether the stream carried at least one `step_finish` event.
        ///
        /// `false` only for a stream that completed on a trailing `text`
        /// event without ever closing its step.
        step_finish_seen: bool,
    },
    /// The stream ended while the turn was still open.
    ///
    /// See [`turn_state`] for the exact enumeration. The stream is
    /// truncated — nothing is inferred about why.
    Truncated {
        /// `source_event_type` of the **last** event in the stream; `None`
        /// when the stream was empty.
        last_event_type: Option<String>,
    },
}

impl TurnState {
    /// True when the turn completed ([`TurnState::Completed`]).
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    /// True when the stream ended without the turn completing
    /// ([`TurnState::Truncated`]).
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}

/// Classify how a harness turn ended.
///
/// # Semantics
///
/// The frozen contract has **no terminal event** — a stream settles at EOF +
/// process exit, and the only finish signal is `step_finish.part.reason`
/// (M0 `UPSTREAM-CONTRACT.md`: "finish signal ✓ (path) —
/// `step_finish.part.reason` + stream EOF + exit code; no dedicated terminal
/// event"). This helper classifies a parsed stream against that signal:
///
/// - **[`TurnState::Completed`]** — the stream carries a *terminal* finish
///   signal, which is one of:
///   1. the last `step_finish` event carries `part.reason == "stop"`
///      ([`FINISH_REASON_STOP`]) — the model declared the turn finished; or
///   2. the stream's **last event is a `text` event** — the assistant's
///      closing message arrived and the closing `step_finish` was never
///      flushed.
/// - **[`TurnState::Truncated`]** — everything else. The stream ended while
///   the turn was still open. Concretely, the stream:
///   1. ended right after a `step_finish` whose reason is
///      `"tool-calls"` ([`FINISH_REASON_TOOL_CALLS`]) — the model asked for
///      another tool call and the stream stopped there (the M6 probe's
///      `B2_base`/`C_run2` baseline runs are real examples);
///   2. ended mid-step (`step_start` with no closing `step_finish`, or a
///      `tool_use` with no closing `step_finish`) — the M0 `06-timeout`
///      capture is a real example;
///   3. carried only a provider `error` envelope (the stream stopped at the
///      error instead of completing the turn);
///   4. was empty.
///
/// "Truncated" is a statement about the **stream**, never about the run's
/// success or failure: a provider failure that never reached the NDJSON (the
/// M6 probe's `ProviderHeaderTimeoutError`) truncates the stream exactly like
/// a kill path does, and **no error event is invented for it**. Use
/// [`NormalizedEvent::error`] to tell a provider error envelope apart from a
/// cut-off stream.
///
/// A finish reason other than [`FINISH_REASON_STOP`] is **not** treated as
/// terminal: completion is only ever claimed when positively observed, so an
/// unknown reason degrades to [`TurnState::Truncated`] instead of a false
/// "completed".
pub fn turn_state(events: &[NormalizedEvent]) -> TurnState {
    let last_step_finish = events
        .iter()
        .rev()
        .find(|event| event.source_event_type == SOURCE_STEP_FINISH);
    let reason = last_step_finish.and_then(|event| event.reason.clone());
    let step_finish_seen = last_step_finish.is_some();
    let terminal_finish = reason
        .as_deref()
        .is_some_and(|finish| finish == FINISH_REASON_STOP);
    let ends_with_text = events
        .last()
        .is_some_and(|event| event.source_event_type == SOURCE_TEXT);

    if terminal_finish || ends_with_text {
        TurnState::Completed {
            reason,
            step_finish_seen,
        }
    } else {
        TurnState::Truncated {
            last_event_type: events.last().map(|event| event.source_event_type.clone()),
        }
    }
}

/// Parse one NDJSON stream line into a [`NormalizedEvent`].
///
/// `sequence` is the zero-based line position in stream order. Unknown
/// envelope types and structurally invalid envelopes are typed errors, never
/// silent skips.
pub fn parse_ndjson_line(line: &str, sequence: u64) -> Result<NormalizedEvent, EventParseError> {
    let value: Value =
        serde_json::from_str(line).map_err(|error| EventParseError::InvalidJson {
            sequence,
            message: error.to_string(),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| EventParseError::InvalidShape {
            sequence,
            event_type: String::new(),
            reason: "top level must be a JSON object".to_string(),
        })?;

    let event_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| EventParseError::MissingField {
            sequence,
            field: "type".to_string(),
        })?
        .to_string();
    let session_id = object
        .get("sessionID")
        .and_then(Value::as_str)
        .ok_or_else(|| EventParseError::MissingField {
            sequence,
            field: "sessionID".to_string(),
        })?
        .to_string();
    let timestamp_ms = object
        .get("timestamp")
        .and_then(Value::as_i64)
        .ok_or_else(|| EventParseError::MissingField {
            sequence,
            field: "timestamp".to_string(),
        })?;

    let mut event = NormalizedEvent {
        sequence,
        source_event_type: event_type.clone(),
        session_id,
        timestamp_ms,
        model_requested: None,
        model_observed: None,
        tool: None,
        text: None,
        tokens: None,
        cost: None,
        reason: None,
        error: None,
        raw_line_hash: sha256_digest(line.as_bytes()),
    };

    match event_type.as_str() {
        SOURCE_STEP_START => {
            require_part(object, sequence, &event_type)?;
        }
        SOURCE_TEXT => {
            let part = require_part(object, sequence, &event_type)?;
            let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                EventParseError::InvalidShape {
                    sequence,
                    event_type: event_type.clone(),
                    reason: "`part.text` must be a string".to_string(),
                }
            })?;
            event.text = Some(text.to_string());
        }
        SOURCE_STEP_FINISH => {
            let part = require_part(object, sequence, &event_type)?;
            if let Some(tokens) = part.get("tokens") {
                event.tokens = Some(parse_tokens(tokens, sequence)?);
            }
            if let Some(cost) = part.get("cost") {
                event.cost = Some(parse_cost(cost, sequence, &event_type)?);
            }
            // `part.reason` is the contract's only finish signal ("stop" vs
            // "tool-calls"); a non-string value is treated as absent rather
            // than as a parse failure.
            event.reason = part
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        SOURCE_TOOL_USE => {
            let part = require_part(object, sequence, &event_type)?;
            event.tool = Some(parse_tool(part, sequence, &event_type)?);
        }
        SOURCE_ERROR => {
            let error = object
                .get("error")
                .and_then(Value::as_object)
                .ok_or_else(|| EventParseError::InvalidShape {
                    sequence,
                    event_type: event_type.clone(),
                    reason: "`error` must be an object with `name` and `data`".to_string(),
                })?;
            let name = error
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| EventParseError::InvalidShape {
                    sequence,
                    event_type: event_type.clone(),
                    reason: "`error.name` must be a string".to_string(),
                })?
                .to_string();
            let data = error.get("data").cloned().unwrap_or(Value::Null);
            event.error = Some(StreamError {
                name,
                data_json: canonical_json_string(&data),
            });
        }
        other => {
            return Err(EventParseError::UnknownEventType {
                sequence,
                event_type: other.to_string(),
            })
        }
    }

    Ok(event)
}

fn require_part<'a>(
    object: &'a serde_json::Map<String, Value>,
    sequence: u64,
    event_type: &str,
) -> Result<&'a serde_json::Map<String, Value>, EventParseError> {
    object
        .get("part")
        .and_then(Value::as_object)
        .ok_or_else(|| EventParseError::InvalidShape {
            sequence,
            event_type: event_type.to_string(),
            reason: "`part` must be an object".to_string(),
        })
}

fn parse_tokens(tokens: &Value, sequence: u64) -> Result<StepTokens, EventParseError> {
    let shape = |reason: &str| EventParseError::InvalidShape {
        sequence,
        event_type: SOURCE_STEP_FINISH.to_string(),
        reason: reason.to_string(),
    };
    let object = tokens
        .as_object()
        .ok_or_else(|| shape("`part.tokens` must be an object"))?;
    let field = |name: &str| {
        object.get(name).and_then(Value::as_u64).ok_or_else(|| {
            shape(&format!(
                "`part.tokens.{name}` must be a non-negative integer"
            ))
        })
    };
    let cache = object
        .get("cache")
        .and_then(Value::as_object)
        .ok_or_else(|| shape("`part.tokens.cache` must be an object"))?;
    let cache_field = |name: &str| {
        cache.get(name).and_then(Value::as_u64).ok_or_else(|| {
            shape(&format!(
                "`part.tokens.cache.{name}` must be a non-negative integer"
            ))
        })
    };
    Ok(StepTokens {
        input: field("input")?,
        output: field("output")?,
        reasoning: field("reasoning")?,
        cache_read: cache_field("read")?,
        cache_write: cache_field("write")?,
    })
}

fn parse_cost(cost: &Value, sequence: u64, event_type: &str) -> Result<f64, EventParseError> {
    cost.as_f64().ok_or_else(|| EventParseError::InvalidShape {
        sequence,
        event_type: event_type.to_string(),
        reason: "`part.cost` must be a number".to_string(),
    })
}

fn parse_tool(
    part: &serde_json::Map<String, Value>,
    sequence: u64,
    event_type: &str,
) -> Result<ToolCall, EventParseError> {
    let shape = |reason: &str| EventParseError::InvalidShape {
        sequence,
        event_type: event_type.to_string(),
        reason: reason.to_string(),
    };
    let name = part
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| shape("`part.tool` must be a string"))?
        .to_string();
    let call_id = part
        .get("callID")
        .and_then(Value::as_str)
        .ok_or_else(|| shape("`part.callID` must be a string"))?
        .to_string();
    let state = part
        .get("state")
        .and_then(Value::as_object)
        .ok_or_else(|| shape("`part.state` must be an object"))?;
    let input_json = canonical_json_string(state.get("input").unwrap_or(&Value::Null));
    let output_str = match state.get("output") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => canonical_json_string(other),
    };
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let error = state
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    // `state.metadata` is an arbitrary per-tool JSON object (the `skill`
    // tool carries `{name, dir, truncated}`); it is preserved verbatim so a
    // harness can assert on fields this parser knows nothing about.
    let metadata = match state.get("metadata") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.clone()),
    };
    Ok(ToolCall {
        name,
        call_id,
        input_json,
        output_str,
        status,
        error,
        metadata,
    })
}

fn canonical_json_string(value: &Value) -> String {
    match crate::canonical_json::canonical_json_bytes(value) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| value.to_string()),
        Err(_) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn golden_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../Docs/Eval/milestone-0/fixtures/golden")
    }

    fn read_lines(name: &str) -> Vec<String> {
        let path = golden_dir().join(name);
        let content =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
        content.lines().map(str::to_string).collect()
    }

    fn parse_all(name: &str) -> Vec<NormalizedEvent> {
        read_lines(name)
            .iter()
            .enumerate()
            .map(|(index, line)| {
                parse_ndjson_line(line, index as u64)
                    .unwrap_or_else(|error| panic!("{name} line {index}: {error}"))
            })
            .collect()
    }

    #[test]
    fn golden_01_no_tool_replays() {
        let events = parse_all("01-no-tool/stdout.ndjson");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].source_event_type, SOURCE_STEP_START);
        assert_eq!(events[1].source_event_type, SOURCE_TEXT);
        assert_eq!(events[1].text.as_deref(), Some("ok"));
        assert_eq!(events[2].source_event_type, SOURCE_STEP_FINISH);
        let tokens = events[2].tokens.expect("step_finish carries tokens");
        assert_eq!(tokens.input, 6544);
        assert_eq!(tokens.output, 8);
        assert_eq!(tokens.reasoning, 175);
        assert_eq!(tokens.cache_read, 98);
        assert_eq!(tokens.cache_write, 0);
        assert_eq!(events[2].cost, Some(0.0));
        let session = "ses_eaf975e00ffeULvvWmeMCr2iBf";
        assert!(events.iter().all(|event| event.session_id == session));
        assert!(events.iter().all(|event| event.model_observed.is_none()));
        assert!(events
            .iter()
            .enumerate()
            .all(|(index, event)| event.sequence == index as u64));
    }

    #[test]
    fn golden_02_tool_replays() {
        let events = parse_all("02-tool/stdout.ndjson");
        assert_eq!(events.len(), 6);
        let tool = events[1].tool.as_ref().expect("tool_use parses");
        assert_eq!(tool.name, "read");
        assert_eq!(tool.call_id, "call_310a541fe03c41b2a5440b4e");
        assert_eq!(tool.status, "completed");
        assert!(tool.error.is_none());
        let input: Value = serde_json::from_str(&tool.input_json).expect("input is JSON");
        assert_eq!(
            input["filePath"].as_str().unwrap().rsplit('/').next(),
            Some("note.txt")
        );
        assert_eq!(input["limit"], 1);
        assert!(tool.output_str.contains("alpha-read-line-ok"));
        // Second step finishes with cache-heavy tokens.
        let mut finish = events
            .iter()
            .filter(|event| event.source_event_type == SOURCE_STEP_FINISH);
        assert_eq!(finish.clone().count(), 2);
        let last = finish.next_back().expect("two step_finish events");
        assert_eq!(last.tokens.expect("tokens").cache_read, 7744);
    }

    #[test]
    fn golden_03_skill_replays_with_byte_equal_body() {
        let events = parse_all("03-skill/stdout.ndjson");
        assert_eq!(events.len(), 6);
        let tool = events[1].tool.as_ref().expect("skill tool_use parses");
        assert_eq!(tool.name, "skill");
        assert_eq!(tool.status, "completed");
        let input: Value = serde_json::from_str(&tool.input_json).expect("input is JSON");
        assert_eq!(input["name"], "root");

        // Byte-equality: the seeded SKILL.md body (post-frontmatter) must
        // appear verbatim inside the harness-returned skill content, and its
        // hash must equal the M0-recorded constant. M0 normalized the body by
        // trimming trailing newlines before hashing.
        let seeded = std::fs::read_to_string(golden_dir().join("03-skill/seeded-SKILL.md"))
            .expect("read seeded SKILL.md");
        let body = seeded
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("---\n"))
            .map(|(_, body)| body.trim_end_matches('\n'))
            .expect("seeded file carries YAML frontmatter");
        assert!(
            tool.output_str.contains(body),
            "harness output must embed the seeded body byte-for-byte"
        );
        let body_hash = sha256_digest(body.as_bytes());
        assert_eq!(
            body_hash, "sha256:36e66cf54132a641a10386db8273d94e7bafb35ef64e123361c6217c83dab910",
            "M0-recorded byte-equal body hash"
        );
        assert!(tool.output_str.contains("<skill_content name=\"root\">"));
    }

    #[test]
    fn golden_04_provider_error_replays() {
        let events = parse_all("04-provider-error/stdout.ndjson");
        assert_eq!(events.len(), 1);
        let error = events[0].error.as_ref().expect("error event parses");
        assert_eq!(error.name, "UnknownError");
        let data: Value = serde_json::from_str(&error.data_json).expect("data is JSON");
        assert!(data["message"]
            .as_str()
            .unwrap()
            .contains("Unexpected server error"));
        assert_eq!(data["ref"], "err_354fccf3");
        assert_eq!(events[0].source_event_type, SOURCE_ERROR);
    }

    #[test]
    fn golden_05c_ask_reject_maps_to_error_status() {
        let events = parse_all("05-denial/05c-ask-reject-shape/stdout.ndjson");
        assert_eq!(events.len(), 3);
        let tool = events[1].tool.as_ref().expect("tool_use parses");
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.status, "error");
        assert_eq!(tool.error.as_deref(), Some(ASK_REJECT_MESSAGE));
        assert!(events[1].is_ask_denial());
        assert!(!events[0].is_ask_denial());
        assert!(!events[2].is_ask_denial());
    }

    #[test]
    fn golden_06_timeout_is_truncated_without_terminal_event() {
        let events = parse_all("06-timeout/stdout.ndjson");
        // M0 pending-event race tolerance: EOF follows the last part with no
        // terminal event, and the parser accepts that as a complete stream.
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source_event_type, SOURCE_STEP_START);
        assert_eq!(events[0].session_id, "ses_eaf8d11fbffem3iZGxG695z03X");
        assert!(events[0].tokens.is_none());
    }

    // -----------------------------------------------------------------------
    // M6 live-harness reads
    // -----------------------------------------------------------------------

    #[test]
    fn state_metadata_is_read_verbatim_with_dir_and_name_accessors() {
        let events = parse_all("03-skill/stdout.ndjson");
        let tool = events[1].tool.as_ref().expect("skill tool_use parses");
        // The golden capture already carries `state.metadata` — the M6 probe
        // showed the parser simply never read it.
        let metadata = tool.metadata.as_ref().expect("metadata is read");
        assert_eq!(metadata["name"], "root");
        assert_eq!(metadata["truncated"], false);
        assert_eq!(tool.metadata_name(), Some("root"));
        let dir = tool.metadata_dir().expect("metadata carries the directory");
        assert!(
            dir.ends_with("/cwd/.opencode/skill/root"),
            "the skill directory must be observable: {dir}"
        );
        assert_eq!(metadata["dir"], dir);
        // Unknown keys survive untouched.
        assert!(metadata.get("truncated").is_some());
    }

    #[test]
    fn a_tool_without_metadata_reads_as_none() {
        let events = parse_all("05-denial/05c-ask-reject-shape/stdout.ndjson");
        let tool = events[1].tool.as_ref().expect("tool_use parses");
        assert!(tool.metadata.is_none());
        assert!(tool.metadata_dir().is_none());
        assert!(tool.metadata_name().is_none());
        assert!(tool.metadata_str("anything").is_none());
    }

    #[test]
    fn step_finish_reason_is_read() {
        let events = parse_all("01-no-tool/stdout.ndjson");
        let finish = events
            .iter()
            .find(|event| event.source_event_type == SOURCE_STEP_FINISH)
            .expect("step_finish");
        assert_eq!(finish.reason.as_deref(), Some(FINISH_REASON_STOP));
        // Non-step_finish events carry no reason.
        assert!(events[0].reason.is_none());
        assert!(events[1].reason.is_none());
    }

    #[test]
    fn turn_state_classifies_the_m0_golden_captures() {
        // Completed: the last step_finish says the model stopped.
        for name in [
            "01-no-tool/stdout.ndjson",
            "02-tool/stdout.ndjson",
            "03-skill/stdout.ndjson",
            "05-denial/stdout.ndjson",
            "07-db-memory-optional/stdout.ndjson",
        ] {
            let events = parse_all(name);
            assert_eq!(
                turn_state(&events),
                TurnState::Completed {
                    reason: Some(FINISH_REASON_STOP.to_string()),
                    step_finish_seen: true,
                },
                "{name} completes"
            );
        }

        // Truncated: the stream stopped on a "tool-calls" finish reason.
        let ask_reject = parse_all("05-denial/05c-ask-reject-shape/stdout.ndjson");
        assert_eq!(
            turn_state(&ask_reject),
            TurnState::Truncated {
                last_event_type: Some(SOURCE_STEP_FINISH.to_string()),
            }
        );

        // Truncated: the M0 kill-path capture (a lone `step_start`).
        let timeout = parse_all("06-timeout/stdout.ndjson");
        assert_eq!(
            turn_state(&timeout),
            TurnState::Truncated {
                last_event_type: Some(SOURCE_STEP_START.to_string()),
            }
        );

        // Truncated: a provider error envelope is not a completed turn.
        let provider_error = parse_all("04-provider-error/stdout.ndjson");
        assert_eq!(
            turn_state(&provider_error),
            TurnState::Truncated {
                last_event_type: Some(SOURCE_ERROR.to_string()),
            }
        );

        // Truncated: an empty stream has no last event at all.
        assert_eq!(
            turn_state(&[]),
            TurnState::Truncated {
                last_event_type: None,
            }
        );
    }

    #[test]
    fn turn_state_treats_a_trailing_text_event_as_completed() {
        // A stream that ends on the assistant's closing message, with no
        // closing step_finish, still completed.
        let events = vec![
            parse_ndjson_line(
                r#"{"type":"step_start","timestamp":1,"sessionID":"s","part":{"type":"step-start"}}"#,
                0,
            )
            .expect("step_start"),
            parse_ndjson_line(
                r#"{"type":"text","timestamp":2,"sessionID":"s","part":{"type":"text","text":"done"}}"#,
                1,
            )
            .expect("text"),
        ];
        assert_eq!(
            turn_state(&events),
            TurnState::Completed {
                reason: None,
                step_finish_seen: false,
            }
        );
    }

    #[test]
    fn unknown_event_type_is_a_typed_error_not_a_skip() {
        let line = r#"{"type":"message_aborted","timestamp":1,"sessionID":"ses_x","part":{}}"#;
        let error = parse_ndjson_line(line, 7).expect_err("unknown type must fail");
        assert_eq!(error.kind(), EventParseErrorKind::UnknownEventType);
        assert_eq!(
            error,
            EventParseError::UnknownEventType {
                sequence: 7,
                event_type: "message_aborted".to_string(),
            }
        );
    }

    #[test]
    fn malformed_lines_are_typed_errors() {
        let not_json = parse_ndjson_line("not json", 0).expect_err("invalid JSON must fail");
        assert_eq!(not_json.kind(), EventParseErrorKind::InvalidJson);

        let missing =
            parse_ndjson_line(r#"{"type":"text"}"#, 1).expect_err("missing field must fail");
        assert_eq!(missing.kind(), EventParseErrorKind::MissingField);

        let bad_shape = parse_ndjson_line(
            r#"{"type":"text","timestamp":1,"sessionID":"s","part":{"type":"text"}}"#,
            2,
        )
        .expect_err("missing part.text must fail");
        assert_eq!(bad_shape.kind(), EventParseErrorKind::InvalidShape);
    }
}
