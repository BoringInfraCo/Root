//! MCP tool definitions, validation, and dispatch.
//!
//! Tools are deliberately narrow. Root does not expose shell execution,
//! arbitrary filesystem mutation, package installation, or Git mutation.

use crate::policy::{CAP_CHECKPOINT, CAP_ENVIRONMENT_VERIFY, CAP_READ, CAP_RECORD};
use crate::session::ServerState;
use root_continuity::{EnvironmentState, ResumeReport};
use root_work::{ProvenanceContext, RepositoryView, SOURCE_AGENT};
use serde_json::{json, Value};

const MAX_STATEMENT_CHARS: usize = 10_000;

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub capability: &'static str,
    pub input_schema: Value,
}

pub enum ToolError {
    Invalid(String),
    Internal(String),
}

fn no_args() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "workspace.get",
            description: "Show the workspace identity bound to this MCP server.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "workspace.status",
            description: "Show workspace identity, active goal, and work counts.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "work.get_goal",
            description: "Show the active work goal, if one is set.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "work.list_decisions",
            description: "List recorded decisions.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "work.record_decision",
            description: "Record a durable decision attributed to this agent session.",
            capability: CAP_RECORD,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "statement": { "type": "string" },
                    "rationale": { "type": "string" }
                },
                "required": ["statement"]
            }),
        },
        ToolDef {
            name: "work.list_findings",
            description: "List recorded findings.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "work.record_finding",
            description: "Record an evidence-backed finding attributed to this agent session.",
            capability: CAP_RECORD,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "statement": { "type": "string" },
                    "evidence_ref": { "type": "string" }
                },
                "required": ["statement"]
            }),
        },
        ToolDef {
            name: "work.list_artifacts",
            description: "List recorded artifact references.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "continuity.checkpoint",
            description: "Create an immutable checkpoint of work, Git, and environment state.",
            capability: CAP_CHECKPOINT,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "continuity.resume",
            description: "Produce a continuation package from a checkpoint.",
            capability: CAP_READ,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "checkpoint_id": { "type": "string" },
                    "with": {
                        "type": "string",
                        "enum": ["codex", "opencode", "claude"],
                        "description": "Optional harness-aware target. When set, assembles a read-only continuation package for that agent."
                    }
                }
            }),
        },
        ToolDef {
            name: "continuity.handoff",
            description: "Produce a portable handoff package for another agent or human.",
            capability: CAP_READ,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "environment.status",
            description: "Show the observed Root environment state.",
            capability: CAP_READ,
            input_schema: no_args(),
        },
        ToolDef {
            name: "environment.verify",
            description: "Report environment state honestly. Root does not claim verification.",
            capability: CAP_ENVIRONMENT_VERIFY,
            input_schema: no_args(),
        },
    ]
}

pub fn find(name: &str) -> Option<ToolDef> {
    definitions().into_iter().find(|tool| tool.name == name)
}

pub fn list() -> Value {
    let tools: Vec<Value> = definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    json!({ "tools": tools })
}

pub fn dispatch(
    state: &mut ServerState,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    match name {
        "workspace.get" => workspace_get(state),
        "workspace.status" => workspace_status(state),
        "work.get_goal" => work_get_goal(state),
        "work.list_decisions" => work_list_decisions(state),
        "work.record_decision" => work_record_decision(state, arguments),
        "work.list_findings" => work_list_findings(state),
        "work.record_finding" => work_record_finding(state, arguments),
        "work.list_artifacts" => work_list_artifacts(state),
        "continuity.checkpoint" => continuity_checkpoint(state, arguments),
        "continuity.resume" => continuity_resume(state, arguments),
        "continuity.handoff" => continuity_handoff(state, arguments),
        "environment.status" => environment_status(state),
        "environment.verify" => environment_verify(state),
        other => Err(ToolError::Invalid(format!("Unknown tool '{other}'."))),
    }
}

fn workspace_get(state: &ServerState) -> Result<Value, ToolError> {
    let workspace = state.store.workspace().clone();
    let repository = RepositoryView {
        path: state.store.repository().root.display().to_string(),
        branch: state.store.repository().branch.clone(),
        head: state.store.repository().head.clone(),
        remote_origin: state.store.repository().remote_origin.clone(),
    };
    Ok(json!({
        "workspace": workspace,
        "repository": repository,
    }))
}

fn workspace_status(state: &ServerState) -> Result<Value, ToolError> {
    let status = state.store.status().map_err(internal)?;
    to_value(status)
}

fn work_get_goal(state: &ServerState) -> Result<Value, ToolError> {
    let goal = state.store.active_goal().map_err(internal)?;
    Ok(json!({ "goal": goal }))
}

fn work_list_decisions(state: &ServerState) -> Result<Value, ToolError> {
    let decisions = state.store.list_decisions().map_err(internal)?;
    Ok(json!({
        "workspace_id": state.store.workspace().id,
        "decisions": decisions,
    }))
}

fn work_record_decision(state: &mut ServerState, arguments: &Value) -> Result<Value, ToolError> {
    let statement = required_string(arguments, "statement")?;
    let rationale = optional_string(arguments, "rationale")?;
    let agent = state.agent.clone();
    let harness = state.harness.clone();
    let session = state.session_id.clone();
    let context = ProvenanceContext {
        source_type: SOURCE_AGENT,
        agent: agent.as_deref(),
        harness: harness.as_deref(),
        session_id: session.as_deref(),
        evidence_ref: None,
    };
    let decision = state
        .store
        .add_decision_with(&statement, rationale.as_deref(), context)
        .map_err(|error| ToolError::Invalid(error.to_string()))?;
    Ok(json!({ "decision": decision }))
}

fn work_list_findings(state: &ServerState) -> Result<Value, ToolError> {
    let findings = state.store.list_findings().map_err(internal)?;
    Ok(json!({
        "workspace_id": state.store.workspace().id,
        "findings": findings,
    }))
}

fn work_record_finding(state: &mut ServerState, arguments: &Value) -> Result<Value, ToolError> {
    let statement = required_string(arguments, "statement")?;
    let evidence_ref = optional_string(arguments, "evidence_ref")?;
    let agent = state.agent.clone();
    let harness = state.harness.clone();
    let session = state.session_id.clone();
    let context = ProvenanceContext {
        source_type: SOURCE_AGENT,
        agent: agent.as_deref(),
        harness: harness.as_deref(),
        session_id: session.as_deref(),
        evidence_ref: evidence_ref.as_deref(),
    };
    let finding = state
        .store
        .add_finding_with(&statement, evidence_ref.as_deref(), context)
        .map_err(|error| ToolError::Invalid(error.to_string()))?;
    Ok(json!({ "finding": finding }))
}

fn work_list_artifacts(state: &ServerState) -> Result<Value, ToolError> {
    let artifacts = state.store.list_artifacts().map_err(internal)?;
    Ok(json!({
        "workspace_id": state.store.workspace().id,
        "artifacts": artifacts,
    }))
}

fn continuity_checkpoint(state: &mut ServerState, arguments: &Value) -> Result<Value, ToolError> {
    let message = optional_string(arguments, "message")?;
    let agent = state.agent.clone();
    let harness = state.harness.clone();
    let session = state.session_id.clone();
    let context = ProvenanceContext {
        source_type: SOURCE_AGENT,
        agent: agent.as_deref(),
        harness: harness.as_deref(),
        session_id: session.as_deref(),
        evidence_ref: None,
    };
    let repository = state.repository.clone();
    let root_dir = state.root_dir.clone();
    let report = root_continuity::create_on_store(
        &mut state.store,
        &repository,
        &root_dir,
        message.as_deref(),
        context,
    )
    .map_err(|error| ToolError::Invalid(error.to_string()))?;
    to_value(report)
}

fn continuity_resume(state: &ServerState, arguments: &Value) -> Result<Value, ToolError> {
    let checkpoint_id = optional_string(arguments, "checkpoint_id")?;
    let target = optional_string(arguments, "with")?;
    if let Some(target) = target.as_deref() {
        let report =
            root_continuity::resume_with(&state.repository.root, checkpoint_id.as_deref(), target)
                .map_err(|error| ToolError::Invalid(error.to_string()))?;
        return to_value(report);
    }
    let report = root_continuity::resume(&state.repository.root, checkpoint_id.as_deref())
        .map_err(|error| ToolError::Invalid(error.to_string()))?;
    let rendered = render_resume(&report);
    Ok(json!({
        "resume": report,
        "rendered": rendered,
    }))
}

fn continuity_handoff(state: &ServerState, arguments: &Value) -> Result<Value, ToolError> {
    let to = optional_string(arguments, "to")?;
    let report = root_continuity::handoff(&state.repository.root, to.as_deref())
        .map_err(|error| ToolError::Invalid(error.to_string()))?;
    let rendered = root_continuity::render_handoff(&report);
    Ok(json!({
        "handoff": report,
        "rendered": rendered,
    }))
}

fn environment_status(state: &ServerState) -> Result<Value, ToolError> {
    let environment = EnvironmentState::capture_at(&state.root_dir).map_err(internal)?;
    Ok(json!({ "environment": environment }))
}

fn environment_verify(state: &ServerState) -> Result<Value, ToolError> {
    let environment = EnvironmentState::capture_at(&state.root_dir).map_err(internal)?;
    Ok(json!({
        "verified": false,
        "environment": environment,
        "note": "No global environment verification ran. Root reports observed environment state only and does not claim verification.",
    }))
}

fn render_resume(report: &ResumeReport) -> String {
    let mut output = String::from("Root Resume\n");
    output.push_str(&format!("\nWorkspace\n  {}\n", report.workspace.name));
    output.push_str("\nGoal\n");
    match &report.goal {
        Some(goal) => output.push_str(&format!("  {}\n", goal.statement)),
        None => output.push_str("  (none)\n"),
    }
    output.push_str(&format!("\nCheckpoint\n  {}\n", report.checkpoint.id));
    if let Some(message) = &report.checkpoint.message {
        output.push_str(&format!("  {message}\n"));
    }
    output.push_str(&format!(
        "\nCurrent state\n  {}\n",
        report.current_state.summary
    ));
    if !report.decisions.is_empty() {
        output.push_str("\nDecisions\n");
        for decision in &report.decisions {
            output.push_str(&format!("  {}\n", decision.statement));
        }
        if report.decisions_omitted > 0 {
            output.push_str(&format!(
                "  ({} older decisions omitted)\n",
                report.decisions_omitted
            ));
        }
    }
    if !report.findings.is_empty() {
        output.push_str("\nFindings\n");
        for finding in &report.findings {
            output.push_str(&format!("  {}\n", finding.statement));
        }
        if report.findings_omitted > 0 {
            output.push_str(&format!(
                "  ({} older findings omitted)\n",
                report.findings_omitted
            ));
        }
    }
    if !report.artifacts.is_empty() {
        output.push_str("\nRelevant artifacts\n");
        for artifact in &report.artifacts {
            output.push_str(&format!("  {}\n", artifact.uri));
        }
        if report.artifacts_omitted > 0 {
            output.push_str(&format!(
                "  ({} older artifacts omitted)\n",
                report.artifacts_omitted
            ));
        }
    }
    if !report.suggested_continuation.is_empty() {
        output.push_str("\nSuggested continuation\n");
        for suggestion in &report.suggested_continuation {
            output.push_str(&format!("  Suggestion (not verified): {suggestion}\n"));
        }
    }
    output
}

fn required_string(arguments: &Value, key: &str) -> Result<String, ToolError> {
    let value = arguments
        .get(key)
        .ok_or_else(|| ToolError::Invalid(format!("Missing required argument '{key}'.")))?;
    let text = value
        .as_str()
        .ok_or_else(|| ToolError::Invalid(format!("Argument '{key}' must be a string.")))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ToolError::Invalid(format!(
            "Argument '{key}' must not be empty."
        )));
    }
    if trimmed.chars().count() > MAX_STATEMENT_CHARS {
        return Err(ToolError::Invalid(format!(
            "Argument '{key}' exceeds {MAX_STATEMENT_CHARS} characters."
        )));
    }
    Ok(trimmed.to_string())
}

fn optional_string(arguments: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            if text.chars().count() > MAX_STATEMENT_CHARS {
                return Err(ToolError::Invalid(format!(
                    "Argument '{key}' exceeds {MAX_STATEMENT_CHARS} characters."
                )));
            }
            Ok(Some(text.clone()))
        }
        Some(_) => Err(ToolError::Invalid(format!(
            "Argument '{key}' must be a string."
        ))),
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(|error| ToolError::Internal(error.to_string()))
}

fn internal(error: anyhow::Error) -> ToolError {
    ToolError::Internal(error.to_string())
}
