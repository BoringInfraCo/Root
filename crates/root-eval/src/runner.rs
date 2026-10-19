//! Single-trial execution wiring supervisor output into a trial record
//! (Milestone 2, Track B).
//!
//! [`run_trial`] runs one harness invocation through
//! [`crate::supervisor::run_supervised`], then folds the supervised result
//! into a [`TrialRunRecord`] without re-interpreting termination:
//!
//! - the requested model is recorded **verbatim** from the request;
//! - the observed model is always `None` with
//!   [`crate::events::MODEL_OBSERVED_LIMITATION`] (M0 identity policy — the
//!   stream never names the model);
//! - session IDs are harvested from events in first-seen order;
//! - usage totals are summed **only** from `step_finish` parts; any missing
//!   field is named in `missing_telemetry`, never zero-filled as if observed.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::events::{MODEL_OBSERVED_LIMITATION, SOURCE_STEP_FINISH};
use crate::supervisor::{run_supervised, SupervisorConfig, TerminationKind};

/// Input for one supervised trial run.
#[derive(Debug, Clone)]
pub struct TrialRunRequest {
    /// Trial identity assigned by the caller (scheduler owns ID allocation).
    pub trial_id: String,
    /// Canonical hash of the effective-context manifest materialized for this
    /// trial (links execution to configuration, not used at runtime here).
    pub manifest_hash: String,
    /// Requested model as `provider/model`, recorded verbatim in the record.
    pub requested_model: String,
    /// Harness command: `argv[0]` is the program, the rest are arguments.
    pub argv: Vec<String>,
    /// Explicit child environment (the child inherits nothing else).
    pub env: Vec<(String, String)>,
    /// Trial working directory (expected to be a fresh checkout).
    pub cwd: PathBuf,
    /// Wall budget in milliseconds (see [`SupervisorConfig`]).
    pub wall_timeout_ms: u64,
    /// Stdout cap in bytes (see [`SupervisorConfig`]).
    pub output_cap_bytes: u64,
}

/// Summed provider-reported usage over a trial's `step_finish` events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageTotals {
    /// Sum of `tokens.input` over `step_finish` events carrying tokens.
    pub tokens_in: u64,
    /// Sum of `tokens.output` over `step_finish` events carrying tokens.
    pub tokens_out: u64,
    /// Sum of `cost` over `step_finish` events carrying a cost.
    pub cost: f64,
}

/// The sealed per-trial execution record.
#[derive(Debug, Clone)]
pub struct TrialRunRecord {
    /// Trial identity from the request.
    pub trial_id: String,
    /// Requested model recorded verbatim from the request.
    pub requested_model: String,
    /// Always `None` per the M0 identity policy.
    pub observed_model: Option<String>,
    /// Why [`Self::observed_model`] is unavailable.
    pub observed_model_limitation: String,
    /// Spawn time as Unix milliseconds.
    pub started_at_ms: u64,
    /// Reap time as Unix milliseconds.
    pub finished_at_ms: u64,
    /// Terminal classification from the supervisor (never re-interpreted here).
    pub termination: TerminationKind,
    /// Process exit code (`None` when signaled or never spawned).
    pub exit_code: Option<i32>,
    /// Distinct harness session IDs in first-seen event order.
    pub session_ids: Vec<String>,
    /// Number of parsed stream events.
    pub event_count: usize,
    /// `sha256:<hex>` over the exact stdout bytes consumed.
    pub raw_sha256: String,
    /// Usage summed only from `step_finish` parts.
    pub usage_totals: UsageTotals,
    /// Telemetry that is unavailable (not observed): `observed_model` is
    /// always listed; usage fields are listed when no `step_finish` part
    /// carried them; unparseable lines are listed as
    /// `dropped_line[<sequence>]`. Missing is never zeroed.
    pub missing_telemetry: Vec<String>,
}

/// Execute one trial and fold the supervised outcome into a [`TrialRunRecord`].
pub fn run_trial(request: &TrialRunRequest) -> TrialRunRecord {
    let started_at_ms = unix_millis();
    let config = SupervisorConfig::new(request.wall_timeout_ms, request.output_cap_bytes);
    let supervised = run_supervised(&request.argv, &request.env, &request.cwd, &config);
    let finished_at_ms = unix_millis();

    let mut session_ids: Vec<String> = Vec::new();
    for event in &supervised.events {
        if !session_ids.contains(&event.session_id) {
            session_ids.push(event.session_id.clone());
        }
    }

    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut cost = 0.0f64;
    let mut saw_tokens = false;
    let mut saw_cost = false;
    let mut saw_step_finish = false;
    for event in &supervised.events {
        if event.source_event_type != SOURCE_STEP_FINISH {
            continue;
        }
        saw_step_finish = true;
        if let Some(tokens) = event.tokens {
            saw_tokens = true;
            tokens_in = tokens_in.saturating_add(tokens.input);
            tokens_out = tokens_out.saturating_add(tokens.output);
        }
        if let Some(event_cost) = event.cost {
            saw_cost = true;
            cost += event_cost;
        }
    }

    let mut missing_telemetry = vec!["observed_model".to_string()];
    if !saw_step_finish {
        missing_telemetry.push("step_finish".to_string());
    }
    if !saw_tokens {
        missing_telemetry.push("tokens.input".to_string());
        missing_telemetry.push("tokens.output".to_string());
    }
    if !saw_cost {
        missing_telemetry.push("cost".to_string());
    }
    if supervised.events.is_empty() {
        missing_telemetry.push("events".to_string());
    }
    for dropped in &supervised.dropped_lines {
        missing_telemetry.push(format!(
            "dropped_line[{}]: {}",
            dropped.sequence, dropped.reason
        ));
    }

    TrialRunRecord {
        trial_id: request.trial_id.clone(),
        requested_model: request.requested_model.clone(),
        observed_model: None,
        observed_model_limitation: MODEL_OBSERVED_LIMITATION.to_string(),
        started_at_ms,
        finished_at_ms,
        termination: supervised.termination,
        exit_code: supervised.exit_code,
        session_ids,
        event_count: supervised.events.len(),
        raw_sha256: supervised.raw_bytes_sha256,
        usage_totals: UsageTotals {
            tokens_in,
            tokens_out,
            cost,
        },
        missing_telemetry,
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn trial_dir(name: &str, script: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("root-eval-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create trial dir");
        let path = dir.join("harness.sh");
        std::fs::write(&path, script).expect("write fake harness");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod +x");
        }
        dir
    }

    fn request_for(dir: &Path, trial_id: &str) -> TrialRunRequest {
        TrialRunRequest {
            trial_id: trial_id.to_string(),
            manifest_hash:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            requested_model: "opencode/big-pickle".to_string(),
            argv: vec![dir.join("harness.sh").to_string_lossy().into_owned()],
            env: Vec::new(),
            cwd: dir.to_path_buf(),
            wall_timeout_ms: 5_000,
            output_cap_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn trial_record_harvests_sessions_and_sums_step_finish_only() {
        let dir = trial_dir(
            "harvest",
            r#"#!/bin/sh
echo '{"type":"step_start","timestamp":10,"sessionID":"ses_h1","part":{"type":"step-start"}}'
echo '{"type":"tool_use","timestamp":11,"sessionID":"ses_h1","part":{"type":"tool","tool":"read","callID":"call_a","state":{"status":"completed","input":{"filePath":"f"},"output":"data"}}}'
echo '{"type":"step_finish","timestamp":12,"sessionID":"ses_h1","part":{"type":"step-finish","reason":"tool-calls","tokens":{"total":100,"input":90,"output":5,"reasoning":5,"cache":{"write":0,"read":0}},"cost":0.5}}'
echo '{"type":"step_start","timestamp":13,"sessionID":"ses_h1","part":{"type":"step-start"}}'
echo '{"type":"text","timestamp":14,"sessionID":"ses_h1","part":{"type":"text","text":"done"}}'
echo '{"type":"step_finish","timestamp":15,"sessionID":"ses_h1","part":{"type":"step-finish","reason":"stop","tokens":{"total":50,"input":40,"output":4,"reasoning":6,"cache":{"write":1,"read":2}},"cost":0.25}}'
"#,
        );
        let record = run_trial(&request_for(&dir, "trial-1"));
        assert_eq!(record.trial_id, "trial-1");
        // Requested model verbatim; observed always unavailable with reason.
        assert_eq!(record.requested_model, "opencode/big-pickle");
        assert_eq!(record.observed_model, None);
        assert_eq!(record.observed_model_limitation, MODEL_OBSERVED_LIMITATION);
        assert_eq!(record.session_ids, vec!["ses_h1".to_string()]);
        assert_eq!(record.event_count, 6);
        // Totals come only from the two step_finish parts (90+40, 5+4).
        assert_eq!(record.usage_totals.tokens_in, 130);
        assert_eq!(record.usage_totals.tokens_out, 9);
        assert!((record.usage_totals.cost - 0.75).abs() < f64::EPSILON);
        assert!(record
            .missing_telemetry
            .contains(&"observed_model".to_string()));
        assert!(!record
            .missing_telemetry
            .iter()
            .any(|m| m.starts_with("tokens.")));
        assert!(!record.missing_telemetry.iter().any(|m| m == "cost"));
        assert_eq!(record.termination, TerminationKind::Completed);
        assert_eq!(record.exit_code, Some(0));
        assert!(record.started_at_ms <= record.finished_at_ms);
        assert!(record.raw_sha256.starts_with("sha256:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trial_record_lists_missing_telemetry_instead_of_zeroing() {
        // Error-shaped stream: no step_finish, no tokens, no cost.
        let dir = trial_dir(
            "missing",
            r#"#!/bin/sh
echo '{"type":"error","timestamp":1,"sessionID":"ses_h2","error":{"name":"UnknownError","data":{"message":"m","ref":"r"}}}'
exit 1
"#,
        );
        let record = run_trial(&request_for(&dir, "trial-2"));
        assert_eq!(record.termination, TerminationKind::ProviderError);
        assert_eq!(record.usage_totals.tokens_in, 0);
        assert_eq!(record.usage_totals.tokens_out, 0);
        for expected in [
            "observed_model",
            "step_finish",
            "tokens.input",
            "tokens.output",
            "cost",
        ] {
            assert!(
                record.missing_telemetry.contains(&expected.to_string()),
                "missing_telemetry must name `{expected}`: {:?}",
                record.missing_telemetry
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trial_record_replays_golden_01_totals() {
        // Golden 01 replayed through the real supervisor via cat: totals must
        // equal the frozen step_finish buckets (input 6544, output 8, cost 0).
        let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../Docs/Eval/milestone-0/fixtures/golden/01-no-tool/stdout.ndjson");
        let dir = trial_dir("golden01", "#!/bin/sh\nexit 0\n");
        let golden_arg = golden.to_string_lossy().into_owned();
        let mut request = request_for(&dir, "trial-golden-01");
        request.argv = vec!["cat".to_string(), golden_arg];
        let record = run_trial(&request);
        assert_eq!(record.termination, TerminationKind::Completed);
        assert_eq!(
            record.session_ids,
            vec!["ses_eaf975e00ffeULvvWmeMCr2iBf".to_string()]
        );
        assert_eq!(record.event_count, 3);
        assert_eq!(record.usage_totals.tokens_in, 6544);
        assert_eq!(record.usage_totals.tokens_out, 8);
        assert_eq!(record.usage_totals.cost, 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
