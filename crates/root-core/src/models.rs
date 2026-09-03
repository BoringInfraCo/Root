//! Plan and pull-and-verify for declared Ollama models.
//!
//! Plan never POSTs, never writes the lock, and never creates model-pull.json.
//! Pull acquires the exclusive marker only after Rootfile/policy gates.

use crate::events::{self, RootEventStatus, RootEventType};
use crate::get_or_create_rootfile;
use crate::inventory::{
    EvaluationState, InventoryItem, InventoryReport, Presence, REASON_ENDPOINT_UNREACHABLE,
    REASON_MALFORMED_OUTPUT, REASON_NOT_FOUND, REASON_NOT_SUPPORTED, REASON_PROTOCOL_UNSUPPORTED,
    REASON_TIMED_OUT,
};
use crate::ollama::{
    canonicalize_digest, digests_equal, is_remote_or_cloud, model_matches, resolve_model_tag,
    HttpOllama, InspectError, ListedModel, OllamaInspector, OllamaRealizer, PullProgress,
    RealizeError, RuntimeProtocol, REASON_REMOTE_OR_CLOUD_UNSUPPORTED,
};
use crate::policy::PolicyAction;
use crate::{get_or_create_lock_v2, save_lock_v2, MutationGuard};
use anyhow::{Context, Result};
use root_lockfile::{get_root_dir, LockedModel, RootLockV2, Rootfile};
use root_snapshot::Snapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const OLLAMA_RUNTIME: &str = "ollama";
const OLLAMA_ENDPOINT: &str = "127.0.0.1:11434";
const PLAN_COMMAND: &str = "plan models";
const PULL_COMMAND: &str = "models pull";
const ADDRESSABILITY: &str = "verification_record_only";
const REASON_NO_DECLARED_MODELS: &str = "no_declared_models";
const REASON_DIGEST_MISMATCH: &str = "cannot_reproduce_locked_digest";
const REASON_STOPPED: &str = "stopped_after_prior_failure";
const DOWNLOAD_STATE: &str = "unknown_until_manifest";
const DOWNLOAD_REASON: &str = "ollama_pull_does_not_expose_size_before_mutation";
const VERIFY_METHOD_PULL: &str = "pull_tag_then_compare_tags_digest";
const VERIFY_METHOD_INSPECT: &str = "inspect_tags_digest";
const OLLAMA_ENDPOINT_URL: &str = "http://127.0.0.1:11434";
const MARKER_FILE: &str = "model-pull.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedAction {
    AlreadyVerified,
    VerifyOnly,
    PullTagThenVerify,
    CannotReproduceLockedDigest,
    UnsupportedRuntime,
    RuntimeUnavailable,
    ProtocolUnsupported,
    RemoteOrCloudUnsupported,
}

impl PlannedAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyVerified => "already_verified",
            Self::VerifyOnly => "verify_only",
            Self::PullTagThenVerify => "pull_tag_then_verify",
            Self::CannotReproduceLockedDigest => "cannot_reproduce_locked_digest",
            Self::UnsupportedRuntime => "unsupported_runtime",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::ProtocolUnsupported => "protocol_unsupported",
            Self::RemoteOrCloudUnsupported => "remote_or_cloud_unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRuntimeProtocol {
    #[serde(rename = "supported")]
    Supported,
    Unreachable,
    TimedOut,
    Malformed,
    ProtocolUnsupported,
    NotProbed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnsupportedOperation {
    pub operation: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectedDownload {
    pub state: &'static str,
    pub bytes: Option<u64>,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanRuntime {
    pub name: String,
    pub endpoint: String,
    pub reachable: Option<bool>,
    pub version: Option<String>,
    pub protocol: PlanRuntimeProtocol,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanModel {
    pub name: String,
    pub runtime: String,
    pub observation: Presence,
    pub evaluation: EvaluationState,
    pub desired_tag: String,
    pub resolved_name: String,
    pub current_tag: Option<String>,
    pub current_digest: Option<String>,
    pub locked_digest: Option<String>,
    pub digest_match: Option<bool>,
    pub expected_download: ExpectedDownload,
    pub planned_action: PlannedAction,
    pub would_mutate: bool,
    pub would_write_lock: bool,
    pub addressability: &'static str,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanModelsReport {
    pub success: bool,
    pub command: &'static str,
    pub would_mutate: bool,
    pub runtime: PlanRuntime,
    pub unsupported_operations: Vec<UnsupportedOperation>,
    pub models: Vec<PlanModel>,
}

struct RuntimeSnapshot {
    reachable: Option<bool>,
    version: Option<String>,
    protocol: PlanRuntimeProtocol,
    reason: Option<String>,
    listed: Option<Vec<ListedModel>>,
}

struct LockLookup {
    has_entry: bool,
    digest: Option<String>,
}

fn unsupported_operations() -> Vec<UnsupportedOperation> {
    vec![
        UnsupportedOperation {
            operation: "digest_addressable_restore",
            reason: "ollama_api_pull_is_tag_only",
        },
        UnsupportedOperation {
            operation: "pull_by_digest",
            reason: "ollama_api_pull_is_tag_only",
        },
        UnsupportedOperation {
            operation: "delete_weights",
            reason: "not_in_v0.3_surface",
        },
        UnsupportedOperation {
            operation: "deterministic_restore",
            reason: "lock_is_verification_record_only",
        },
    ]
}

fn expected_download() -> ExpectedDownload {
    ExpectedDownload {
        state: DOWNLOAD_STATE,
        bytes: None,
        reason: DOWNLOAD_REASON,
    }
}

fn unknown_model_error(name: &str) -> anyhow::Error {
    ModelError::UnknownName(name.to_string()).into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    Unreachable(String),
    Io(String),
    PullInProgress { pid: u32 },
    LockChanged,
    UnknownName(String),
    PolicyDenied(String),
}

impl ModelError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Unreachable(_)
            | Self::Io(_)
            | Self::PullInProgress { .. }
            | Self::LockChanged => 1,
            Self::UnknownName(_) => 2,
            Self::PolicyDenied(_) => 9,
        }
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(
                f,
                "Ollama is unreachable at {OLLAMA_ENDPOINT}: {detail}. Start the Ollama daemon and retry."
            ),
            Self::Io(detail) => write!(f, "{detail}"),
            Self::PullInProgress { pid } => {
                write!(f, "A model pull is already in progress (PID {pid}).")
            }
            Self::LockChanged => write!(
                f,
                "root.lock changed during pull; weights retained; re-run plan"
            ),
            Self::UnknownName(name) => {
                write!(f, "Unknown model '{name}' is not declared in Rootfile.")
            }
            Self::PolicyDenied(reason) => write!(f, "Policy denied: {reason}"),
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullVerb {
    PulledAndVerified,
    VerifiedAndLocked,
    AlreadyVerified,
    VerificationFailed,
    PullFailed,
    SkippedUnsupported,
    NotAttempted,
}

impl PullVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PulledAndVerified => "pulled_and_verified",
            Self::VerifiedAndLocked => "verified_and_locked",
            Self::AlreadyVerified => "already_verified",
            Self::VerificationFailed => "verification_failed",
            Self::PullFailed => "pull_failed",
            Self::SkippedUnsupported => "skipped_unsupported",
            Self::NotAttempted => "not_attempted",
        }
    }

    fn is_success(self) -> bool {
        matches!(
            self,
            Self::PulledAndVerified | Self::VerifiedAndLocked | Self::AlreadyVerified
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelsPullResult {
    pub name: String,
    pub verb: PullVerb,
    pub requested_tag: String,
    pub observed_digest: Option<String>,
    pub locked_digest: Option<String>,
    pub digest_match: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_completed: Option<u64>,
    pub lock_written: bool,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelsPullReport {
    pub success: bool,
    pub command: &'static str,
    pub models_restored: bool,
    pub model_weights_deleted: bool,
    pub lock_schema_version: u32,
    pub results: Vec<ModelsPullResult>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelPullMarker {
    name: String,
    started_at: String,
    pid: u32,
}

struct ModelPullGuard {
    path: PathBuf,
}

fn load_lock_models() -> Result<BTreeMap<String, BTreeMap<String, LockedModel>>> {
    let path = get_root_dir()?.join("root.lock");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read lockfile at {}", path.display()))?;
    let version = root_lockfile::peek_lock_schema_version(&content)?;
    root_lockfile::validate_supported_lock_version(version)?;
    let lock = root_lockfile::read_compatible_lock_v2_from_str(&content)?;
    root_lockfile::validate_locked_models(&lock).with_context(|| {
        format!(
            "Existing lockfile at {} failed models validation",
            path.display()
        )
    })?;
    Ok(lock.models)
}

fn snapshot_from_error(err: InspectError, version: Option<String>) -> RuntimeSnapshot {
    let (protocol, reason) = match err {
        InspectError::EndpointUnreachable => (
            PlanRuntimeProtocol::Unreachable,
            REASON_ENDPOINT_UNREACHABLE,
        ),
        InspectError::TimedOut => (PlanRuntimeProtocol::TimedOut, REASON_TIMED_OUT),
        InspectError::Malformed => (PlanRuntimeProtocol::Malformed, REASON_MALFORMED_OUTPUT),
        InspectError::ProtocolUnsupported => (
            PlanRuntimeProtocol::ProtocolUnsupported,
            REASON_PROTOCOL_UNSUPPORTED,
        ),
    };
    RuntimeSnapshot {
        reachable: Some(false),
        version,
        protocol,
        reason: Some(reason.to_string()),
        listed: None,
    }
}

fn snapshot_runtime(inspector: &impl OllamaInspector) -> RuntimeSnapshot {
    let probe = inspector.inspect_runtime();
    match probe.protocol {
        RuntimeProtocol::Ready => match inspector.list_models() {
            Ok(listed) => RuntimeSnapshot {
                reachable: Some(true),
                version: probe.version,
                protocol: PlanRuntimeProtocol::Supported,
                reason: None,
                listed: Some(listed),
            },
            Err(err) => snapshot_from_error(err, probe.version),
        },
        RuntimeProtocol::Unreachable => {
            snapshot_from_error(InspectError::EndpointUnreachable, probe.version)
        }
        RuntimeProtocol::TimedOut => snapshot_from_error(InspectError::TimedOut, probe.version),
        RuntimeProtocol::Malformed => snapshot_from_error(InspectError::Malformed, probe.version),
        RuntimeProtocol::ProtocolUnsupported => {
            snapshot_from_error(InspectError::ProtocolUnsupported, probe.version)
        }
    }
}

fn lock_lookup(
    lock_models: &BTreeMap<String, BTreeMap<String, LockedModel>>,
    runtime: &str,
    name: &str,
) -> LockLookup {
    match lock_models
        .get(&runtime.to_ascii_lowercase())
        .and_then(|models| models.get(name))
    {
        Some(model) => LockLookup {
            has_entry: true,
            digest: canonicalize_digest(&model.observed_digest),
        },
        None => LockLookup {
            has_entry: false,
            digest: None,
        },
    }
}

fn needs_runtime_probe(name: &str, runtime: &str) -> bool {
    runtime.to_ascii_lowercase() == OLLAMA_RUNTIME && !is_remote_or_cloud(name, None)
}

fn plan_one(
    name: &str,
    runtime: &str,
    snapshot: &RuntimeSnapshot,
    lock_models: &BTreeMap<String, BTreeMap<String, LockedModel>>,
) -> PlanModel {
    let resolved_name = resolve_model_tag(name);
    let lock = lock_lookup(lock_models, runtime, name);
    let locked_digest = lock.digest.clone();

    if runtime.to_ascii_lowercase() != OLLAMA_RUNTIME {
        return PlanModel {
            name: name.to_string(),
            runtime: runtime.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unsupported,
            desired_tag: name.to_string(),
            resolved_name,
            current_tag: None,
            current_digest: None,
            locked_digest,
            digest_match: None,
            expected_download: expected_download(),
            planned_action: PlannedAction::UnsupportedRuntime,
            would_mutate: false,
            would_write_lock: false,
            addressability: ADDRESSABILITY,
            reason: Some(REASON_NOT_SUPPORTED.to_string()),
        };
    }

    // Declared cloud/remote names are classified without listing that name.
    if is_remote_or_cloud(name, None) {
        return PlanModel {
            name: name.to_string(),
            runtime: runtime.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unsupported,
            desired_tag: name.to_string(),
            resolved_name,
            current_tag: None,
            current_digest: None,
            locked_digest,
            digest_match: None,
            expected_download: expected_download(),
            planned_action: PlannedAction::RemoteOrCloudUnsupported,
            would_mutate: false,
            would_write_lock: false,
            addressability: ADDRESSABILITY,
            reason: Some(REASON_REMOTE_OR_CLOUD_UNSUPPORTED.to_string()),
        };
    }

    match snapshot.protocol {
        PlanRuntimeProtocol::NotProbed
        | PlanRuntimeProtocol::Unreachable
        | PlanRuntimeProtocol::TimedOut
        | PlanRuntimeProtocol::Malformed => PlanModel {
            name: name.to_string(),
            runtime: runtime.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unknown,
            desired_tag: name.to_string(),
            resolved_name,
            current_tag: None,
            current_digest: None,
            locked_digest,
            digest_match: None,
            expected_download: expected_download(),
            planned_action: PlannedAction::RuntimeUnavailable,
            would_mutate: false,
            would_write_lock: false,
            addressability: ADDRESSABILITY,
            reason: snapshot.reason.clone(),
        },
        PlanRuntimeProtocol::ProtocolUnsupported => PlanModel {
            name: name.to_string(),
            runtime: runtime.to_string(),
            observation: Presence::Unknown,
            evaluation: EvaluationState::Unsupported,
            desired_tag: name.to_string(),
            resolved_name,
            current_tag: None,
            current_digest: None,
            locked_digest,
            digest_match: None,
            expected_download: expected_download(),
            planned_action: PlannedAction::ProtocolUnsupported,
            would_mutate: false,
            would_write_lock: false,
            addressability: ADDRESSABILITY,
            reason: snapshot.reason.clone(),
        },
        PlanRuntimeProtocol::Supported => {
            let found = snapshot.listed.as_ref().and_then(|models| {
                models
                    .iter()
                    .find(|model| model_matches(name, &model.name, model.model.as_deref()))
            });
            match found {
                Some(found) if is_remote_or_cloud(name, Some(found)) => PlanModel {
                    name: name.to_string(),
                    runtime: runtime.to_string(),
                    observation: Presence::Present,
                    evaluation: EvaluationState::Unsupported,
                    desired_tag: name.to_string(),
                    resolved_name,
                    current_tag: Some(found.name.clone()),
                    current_digest: found.digest.clone(),
                    locked_digest,
                    digest_match: None,
                    expected_download: expected_download(),
                    planned_action: PlannedAction::RemoteOrCloudUnsupported,
                    would_mutate: false,
                    would_write_lock: false,
                    addressability: ADDRESSABILITY,
                    reason: Some(REASON_REMOTE_OR_CLOUD_UNSUPPORTED.to_string()),
                },
                Some(found) => {
                    let current_digest = found.digest.clone();
                    let digest_match = match (current_digest.as_deref(), locked_digest.as_deref()) {
                        (Some(current), Some(locked)) => Some(digests_equal(current, locked)),
                        _ => None,
                    };
                    let (planned_action, evaluation, would_write_lock, reason) =
                        match (lock.has_entry, digest_match) {
                            (true, Some(true)) => (
                                PlannedAction::AlreadyVerified,
                                EvaluationState::Satisfied,
                                false,
                                None,
                            ),
                            (true, _) => (
                                PlannedAction::CannotReproduceLockedDigest,
                                EvaluationState::Drifted,
                                false,
                                Some(REASON_DIGEST_MISMATCH.to_string()),
                            ),
                            (false, _) => (
                                PlannedAction::VerifyOnly,
                                EvaluationState::Satisfied,
                                true,
                                None,
                            ),
                        };
                    PlanModel {
                        name: name.to_string(),
                        runtime: runtime.to_string(),
                        observation: Presence::Present,
                        evaluation,
                        desired_tag: name.to_string(),
                        resolved_name,
                        current_tag: Some(found.name.clone()),
                        current_digest,
                        locked_digest,
                        digest_match,
                        expected_download: expected_download(),
                        planned_action,
                        would_mutate: false,
                        would_write_lock,
                        addressability: ADDRESSABILITY,
                        reason,
                    }
                }
                None => {
                    let reason = if lock.has_entry {
                        Some("ollama_api_pull_is_tag_only".to_string())
                    } else {
                        Some(REASON_NOT_FOUND.to_string())
                    };
                    PlanModel {
                        name: name.to_string(),
                        runtime: runtime.to_string(),
                        observation: Presence::Absent,
                        evaluation: EvaluationState::Missing,
                        desired_tag: name.to_string(),
                        resolved_name,
                        current_tag: None,
                        current_digest: None,
                        locked_digest,
                        digest_match: None,
                        expected_download: expected_download(),
                        planned_action: PlannedAction::PullTagThenVerify,
                        would_mutate: true,
                        would_write_lock: true,
                        addressability: ADDRESSABILITY,
                        reason,
                    }
                }
            }
        }
    }
}

fn plan_models_from(
    name: Option<&str>,
    rootfile: &Rootfile,
    lock_models: &BTreeMap<String, BTreeMap<String, LockedModel>>,
    inspector: &impl OllamaInspector,
) -> Result<PlanModelsReport> {
    if let Some(requested) = name {
        if !rootfile.models.contains_key(requested) {
            return Err(unknown_model_error(requested));
        }
    }

    let selected: Vec<(&String, &root_lockfile::ModelDeclaration)> = match name {
        Some(requested) => rootfile
            .models
            .iter()
            .filter(|(key, _)| key.as_str() == requested)
            .collect(),
        None => rootfile.models.iter().collect(),
    };

    let probe = !selected.is_empty()
        && selected
            .iter()
            .any(|(model_name, declaration)| needs_runtime_probe(model_name, &declaration.runtime));

    let (snapshot, empty_reason) = if selected.is_empty() {
        (
            RuntimeSnapshot {
                reachable: None,
                version: None,
                protocol: PlanRuntimeProtocol::NotProbed,
                reason: Some(REASON_NO_DECLARED_MODELS.to_string()),
                listed: None,
            },
            true,
        )
    } else if probe {
        (snapshot_runtime(inspector), false)
    } else {
        (
            RuntimeSnapshot {
                reachable: None,
                version: None,
                protocol: PlanRuntimeProtocol::NotProbed,
                reason: None,
                listed: None,
            },
            false,
        )
    };

    let models: Vec<PlanModel> = selected
        .iter()
        .map(|(model_name, declaration)| {
            plan_one(model_name, &declaration.runtime, &snapshot, lock_models)
        })
        .collect();
    let would_mutate = models.iter().any(|model| model.would_mutate);

    Ok(PlanModelsReport {
        success: true,
        command: PLAN_COMMAND,
        would_mutate,
        runtime: PlanRuntime {
            name: OLLAMA_RUNTIME.to_string(),
            endpoint: OLLAMA_ENDPOINT.to_string(),
            reachable: snapshot.reachable,
            version: snapshot.version,
            protocol: snapshot.protocol,
            reason: if empty_reason {
                Some(REASON_NO_DECLARED_MODELS.to_string())
            } else {
                snapshot.reason
            },
        },
        unsupported_operations: unsupported_operations(),
        models,
    })
}

/// Read-only plan using the loopback Ollama inspector.
pub fn plan_models(name: Option<&str>) -> Result<PlanModelsReport> {
    plan_models_with_inspector(name, &HttpOllama::default())
}

pub fn plan_models_with_inspector(
    name: Option<&str>,
    inspector: &impl OllamaInspector,
) -> Result<PlanModelsReport> {
    let rootfile = get_or_create_rootfile()?;
    let lock_models = load_lock_models()?;
    plan_models_from(name, &rootfile, &lock_models, inspector)
}

fn presence_label(presence: Presence) -> &'static str {
    match presence {
        Presence::Present => "present",
        Presence::Absent => "absent",
        Presence::Unknown => "unknown",
    }
}

fn evaluation_label(evaluation: EvaluationState) -> &'static str {
    match evaluation {
        EvaluationState::Satisfied => "satisfied",
        EvaluationState::Missing => "missing",
        EvaluationState::Drifted => "drifted",
        EvaluationState::Unknown => "unknown",
        EvaluationState::Unsupported => "unsupported",
    }
}

pub fn format_plan_models_human(report: &PlanModelsReport) -> String {
    let mut out = String::from("Unsupported operations:\n");
    for operation in &report.unsupported_operations {
        out.push_str(&format!(
            "  - {} ({})\n",
            operation.operation, operation.reason
        ));
    }

    if report.models.is_empty() {
        out.push_str("\nNo declared Ollama models.\n");
    } else {
        out.push_str("\nPlan for declared Ollama models\n");
        for model in &report.models {
            out.push_str(&format!(
                "\n{}\n  runtime: {}\n  observation: {}\n  evaluation: {}\n  planned action: {}\n",
                model.name,
                model.runtime,
                presence_label(model.observation),
                evaluation_label(model.evaluation),
                model.planned_action.as_str()
            ));
            if model.planned_action == PlannedAction::PullTagThenVerify {
                out.push_str(&format!("  would pull {}\n", model.resolved_name));
            }
            if let Some(reason) = &model.reason {
                out.push_str(&format!("  reason: {reason}\n"));
            }
        }
    }

    out.push_str("\nThis is a preview. No changes have been made.");
    out
}

/// Canonical lock digest plus whether the observed daemon digest matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestCompare {
    pub locked_digest: String,
    pub digest_match: bool,
}

pub fn compare_locked_digest(observed: Option<&str>, locked: &str) -> Option<DigestCompare> {
    let locked_digest = canonicalize_digest(locked)?;
    let digest_match = observed
        .map(|value| digests_equal(value, &locked_digest))
        .unwrap_or(false);
    Some(DigestCompare {
        locked_digest,
        digest_match,
    })
}

pub fn overlay_locked_digests(report: &mut InventoryReport, lock: &RootLockV2) {
    for item in &mut report.models {
        overlay_one(item, lock);
    }
}

fn overlay_one(item: &mut InventoryItem, lock: &RootLockV2) {
    let Some(entry) = locked_model_for(lock, &item.name, &item.desired) else {
        return;
    };
    let Some(compare) =
        compare_locked_digest(item.observed_digest.as_deref(), &entry.observed_digest)
    else {
        return;
    };
    item.locked_digest = Some(compare.locked_digest);
    item.digest_match = Some(compare.digest_match);
    if item.observation == Presence::Present && !compare.digest_match {
        item.evaluation = EvaluationState::Drifted;
    }
}

fn locked_model_for<'a>(
    lock: &'a RootLockV2,
    declared_name: &str,
    runtime: &str,
) -> Option<&'a LockedModel> {
    lock.models
        .get(&runtime.to_ascii_lowercase())
        .and_then(|models| models.get(declared_name))
}

fn io_err(err: impl ToString) -> ModelError {
    ModelError::Io(err.to_string())
}

fn process_is_alive(pid: u32) -> Result<bool, ModelError> {
    let status = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map_err(|err| io_err(format!("Cannot check process liveness: {err}")))?;
    Ok(status.status.success())
}

fn marker_path() -> Result<PathBuf, ModelError> {
    let dir = root_lockfile::init_root_dir().map_err(io_err)?;
    Ok(dir.join(MARKER_FILE))
}

fn try_create_marker(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn read_marker_pid(path: &Path) -> Result<u32, ModelError> {
    let content = fs::read_to_string(path).map_err(|_| {
        io_err(format!(
            "{} exists and could not be read.\nDelete {} and try again.",
            MARKER_FILE,
            path.display()
        ))
    })?;
    let marker: ModelPullMarker = serde_json::from_str(&content).map_err(|_| {
        io_err(format!(
            "{} exists and could not be read.\nDelete {} and try again.",
            MARKER_FILE,
            path.display()
        ))
    })?;
    Ok(marker.pid)
}

impl ModelPullGuard {
    fn acquire(name: &str) -> Result<Self, ModelError> {
        let path = marker_path()?;
        let marker = ModelPullMarker {
            name: name.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            pid: std::process::id(),
        };
        let content = serde_json::to_vec_pretty(&marker).map_err(io_err)?;
        match try_create_marker(&path, &content) {
            Ok(()) => Ok(Self { path }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let pid = read_marker_pid(&path)?;
                if process_is_alive(pid)? {
                    return Err(ModelError::PullInProgress { pid });
                }
                let _ = fs::remove_file(&path);
                try_create_marker(&path, &content).map_err(|retry| {
                    io_err(format!(
                        "Failed to acquire model pull marker after recovering stale marker: {retry}"
                    ))
                })?;
                Ok(Self { path })
            }
            Err(err) => Err(io_err(format!(
                "Failed to acquire model pull marker: {err}"
            ))),
        }
    }
}

impl Drop for ModelPullGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_file_path() -> Result<PathBuf, ModelError> {
    Ok(get_root_dir().map_err(io_err)?.join("root.lock"))
}

fn read_lock_bytes(path: &Path) -> Result<Vec<u8>, ModelError> {
    if path.exists() {
        fs::read(path).map_err(io_err)
    } else {
        Ok(Vec::new())
    }
}

fn peek_lock_schema_version_or_default() -> u32 {
    let Ok(path) = lock_file_path() else {
        return root_lockfile::ROOT_LOCK_SCHEMA_VERSION;
    };
    if !path.exists() {
        return root_lockfile::ROOT_LOCK_SCHEMA_VERSION;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| root_lockfile::peek_lock_schema_version(&content).ok())
        .unwrap_or(root_lockfile::ROOT_LOCK_SCHEMA_VERSION)
}

fn empty_pull_report() -> ModelsPullReport {
    ModelsPullReport {
        success: true,
        command: PULL_COMMAND,
        models_restored: false,
        model_weights_deleted: false,
        lock_schema_version: peek_lock_schema_version_or_default(),
        results: Vec::new(),
    }
}

fn policy_denied_inner_reason(message: &str) -> Option<String> {
    message
        .strip_prefix("Policy denied")
        .and_then(|rest| rest.rsplit_once(": "))
        .map(|(_, reason)| reason.to_string())
}

fn enforce_model_pull_policy(subject: &str) -> Result<()> {
    crate::enforce_policy(PolicyAction::ModelPull, Some(subject)).map_err(|err| {
        let message = err.to_string();
        if let Some(reason) = policy_denied_inner_reason(&message) {
            anyhow::Error::new(ModelError::PolicyDenied(reason))
        } else {
            err
        }
    })
}

fn skip_reason(model: &PlanModel) -> String {
    model
        .reason
        .clone()
        .unwrap_or_else(|| model.planned_action.as_str().to_string())
}

fn skipped_result(model: &PlanModel) -> ModelsPullResult {
    ModelsPullResult {
        name: model.name.clone(),
        verb: PullVerb::SkippedUnsupported,
        requested_tag: model.resolved_name.clone(),
        observed_digest: model.current_digest.clone(),
        locked_digest: model.locked_digest.clone(),
        digest_match: model.digest_match,
        bytes_completed: None,
        lock_written: false,
        exit_code: 2,
        reason: Some(skip_reason(model)),
    }
}

fn not_attempted_result(model: &PlanModel) -> ModelsPullResult {
    ModelsPullResult {
        name: model.name.clone(),
        verb: PullVerb::NotAttempted,
        requested_tag: model.resolved_name.clone(),
        observed_digest: None,
        locked_digest: model.locked_digest.clone(),
        digest_match: None,
        bytes_completed: None,
        lock_written: false,
        exit_code: 0,
        reason: Some(REASON_STOPPED.to_string()),
    }
}

fn already_verified_result(model: &PlanModel) -> ModelsPullResult {
    ModelsPullResult {
        name: model.name.clone(),
        verb: PullVerb::AlreadyVerified,
        requested_tag: model.resolved_name.clone(),
        observed_digest: model
            .current_digest
            .as_deref()
            .and_then(canonicalize_digest)
            .or_else(|| model.current_digest.clone()),
        locked_digest: model.locked_digest.clone(),
        digest_match: Some(true),
        bytes_completed: None,
        lock_written: false,
        exit_code: 0,
        reason: None,
    }
}

fn record_model_event(
    event_type: RootEventType,
    status: RootEventStatus,
    model: &str,
    snapshot_id: Option<String>,
    message: Option<String>,
    duration_ms: Option<u64>,
) {
    let mut event = events::create_event(
        event_type,
        status,
        "root models pull",
        None,
        snapshot_id,
        None,
        message,
    );
    event.model = Some(model.to_string());
    event.duration_ms = duration_ms;
    let _ = events::append_event(&event);
}

fn write_verified_lock(
    name: &str,
    resolved_name: &str,
    observed_digest: &str,
    size_bytes: Option<u64>,
    backend_version: Option<String>,
    verification_method: &str,
    expected_bytes: &mut Vec<u8>,
) -> Result<String, ModelError> {
    let _guard = MutationGuard::acquire().map_err(io_err)?;
    let path = lock_file_path()?;
    let current_bytes = read_lock_bytes(&path)?;
    if current_bytes != *expected_bytes {
        return Err(ModelError::LockChanged);
    }

    let mut lock = get_or_create_lock_v2().map_err(io_err)?;
    let snapshot =
        Snapshot::create_from_v2(&format!("before model verification record {name}"), &lock)
            .map_err(io_err)?;
    let now = chrono::Utc::now().to_rfc3339();
    let existing = lock
        .models
        .get(OLLAMA_RUNTIME)
        .and_then(|models| models.get(name))
        .cloned();
    let locked = LockedModel {
        runtime: OLLAMA_RUNTIME.to_string(),
        requested_name: resolved_name.to_string(),
        observed_digest: observed_digest.to_string(),
        size_bytes,
        endpoint: Some(OLLAMA_ENDPOINT_URL.to_string()),
        backend_version,
        locked_at: existing
            .as_ref()
            .map(|model| model.locked_at.clone())
            .unwrap_or_else(|| now.clone()),
        verified_at: now.clone(),
        verification_method: verification_method.to_string(),
        addressability: ADDRESSABILITY.to_string(),
    };
    lock.models
        .entry(OLLAMA_RUNTIME.to_string())
        .or_default()
        .insert(name.to_string(), locked);
    lock.version = root_lockfile::emit_lock_version(&lock.models);
    lock.updated_at = Some(now.clone());
    if lock.created_at.is_none() {
        lock.created_at = Some(now);
    }
    if lock.root_version.is_none() {
        lock.root_version = Some(env!("CARGO_PKG_VERSION").to_string());
    }
    save_lock_v2(&lock).map_err(io_err)?;
    *expected_bytes = read_lock_bytes(&path)?;
    Ok(snapshot.id)
}

fn find_listed<'a>(name: &str, listed: &'a [ListedModel]) -> Option<&'a ListedModel> {
    listed
        .iter()
        .find(|model| model_matches(name, &model.name, model.model.as_deref()))
}

fn verify_only_row(
    model: &PlanModel,
    backend_version: Option<String>,
    expected_bytes: &mut Vec<u8>,
) -> Result<ModelsPullResult, ModelError> {
    let Some(canonical) = model
        .current_digest
        .as_deref()
        .and_then(canonicalize_digest)
    else {
        return Ok(ModelsPullResult {
            name: model.name.clone(),
            verb: PullVerb::PullFailed,
            requested_tag: model.resolved_name.clone(),
            observed_digest: model.current_digest.clone(),
            locked_digest: model.locked_digest.clone(),
            digest_match: None,
            bytes_completed: None,
            lock_written: false,
            exit_code: 1,
            reason: Some(REASON_MALFORMED_OUTPUT.to_string()),
        });
    };
    let snapshot_id = write_verified_lock(
        &model.name,
        &model.resolved_name,
        &canonical,
        None,
        backend_version,
        VERIFY_METHOD_INSPECT,
        expected_bytes,
    )?;
    record_model_event(
        RootEventType::ModelVerified,
        RootEventStatus::Verified,
        &model.name,
        Some(snapshot_id),
        Some("verified_and_locked; weights_retained=true".to_string()),
        None,
    );
    Ok(ModelsPullResult {
        name: model.name.clone(),
        verb: PullVerb::VerifiedAndLocked,
        requested_tag: model.resolved_name.clone(),
        observed_digest: Some(canonical.clone()),
        locked_digest: Some(canonical),
        digest_match: Some(true),
        bytes_completed: None,
        lock_written: true,
        exit_code: 0,
        reason: None,
    })
}

fn pull_failed_row(model: &PlanModel, err: RealizeError) -> ModelsPullResult {
    let exit_code = match err {
        RealizeError::NotFound => 3,
        _ => 1,
    };
    record_model_event(
        RootEventType::ModelPull,
        RootEventStatus::Failed,
        &model.name,
        None,
        Some(format!(
            "pull_failed; reason={}; weights_retained=true",
            err.reason()
        )),
        None,
    );
    ModelsPullResult {
        name: model.name.clone(),
        verb: PullVerb::PullFailed,
        requested_tag: model.resolved_name.clone(),
        observed_digest: None,
        locked_digest: model.locked_digest.clone(),
        digest_match: None,
        bytes_completed: None,
        lock_written: false,
        exit_code,
        reason: Some(err.reason().to_string()),
    }
}

fn pull_tag_then_verify_row(
    model: &PlanModel,
    inspector: &impl OllamaInspector,
    realizer: &impl OllamaRealizer,
    backend_version: Option<String>,
    expected_bytes: &mut Vec<u8>,
    progress: &mut dyn FnMut(&str, PullProgress),
) -> Result<(ModelsPullResult, bool), ModelError> {
    let started = Instant::now();
    let outcome = match realizer.pull_tag(&model.resolved_name, &mut |update| {
        progress(&model.name, update);
    }) {
        Ok(outcome) => outcome,
        Err(err) => return Ok((pull_failed_row(model, err), true)),
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    let listed = match inspector.list_models() {
        Ok(listed) => listed,
        Err(err) => {
            return Ok((
                pull_failed_row(
                    model,
                    match err {
                        InspectError::EndpointUnreachable => RealizeError::EndpointUnreachable,
                        InspectError::TimedOut => RealizeError::TimedOut,
                        InspectError::Malformed => RealizeError::Malformed,
                        InspectError::ProtocolUnsupported => RealizeError::ProtocolUnsupported,
                    },
                ),
                true,
            ));
        }
    };
    let Some(found) = find_listed(&model.name, &listed) else {
        return Ok((pull_failed_row(model, RealizeError::Failed), true));
    };
    if is_remote_or_cloud(&model.name, Some(found)) {
        return Ok((
            ModelsPullResult {
                name: model.name.clone(),
                verb: PullVerb::SkippedUnsupported,
                requested_tag: model.resolved_name.clone(),
                observed_digest: found.digest.clone(),
                locked_digest: model.locked_digest.clone(),
                digest_match: None,
                bytes_completed: outcome.bytes_completed,
                lock_written: false,
                exit_code: 2,
                reason: Some(REASON_REMOTE_OR_CLOUD_UNSUPPORTED.to_string()),
            },
            false,
        ));
    }
    let Some(canonical) = found.digest.as_deref().and_then(canonicalize_digest) else {
        return Ok((
            ModelsPullResult {
                name: model.name.clone(),
                verb: PullVerb::PullFailed,
                requested_tag: model.resolved_name.clone(),
                observed_digest: found.digest.clone(),
                locked_digest: model.locked_digest.clone(),
                digest_match: None,
                bytes_completed: outcome.bytes_completed,
                lock_written: false,
                exit_code: 1,
                reason: Some(REASON_MALFORMED_OUTPUT.to_string()),
            },
            true,
        ));
    };
    if let Some(locked) = model.locked_digest.as_deref() {
        if !digests_equal(&canonical, locked) {
            record_model_event(
                RootEventType::ModelVerificationFailed,
                RootEventStatus::Failed,
                &model.name,
                None,
                Some("verification_failed; weights_retained=true".to_string()),
                Some(duration_ms),
            );
            return Ok((
                ModelsPullResult {
                    name: model.name.clone(),
                    verb: PullVerb::VerificationFailed,
                    requested_tag: model.resolved_name.clone(),
                    observed_digest: Some(canonical),
                    locked_digest: model.locked_digest.clone(),
                    digest_match: Some(false),
                    bytes_completed: outcome.bytes_completed,
                    lock_written: false,
                    exit_code: 4,
                    reason: Some(REASON_DIGEST_MISMATCH.to_string()),
                },
                true,
            ));
        }
    }
    let snapshot_id = write_verified_lock(
        &model.name,
        &model.resolved_name,
        &canonical,
        found.size,
        backend_version,
        VERIFY_METHOD_PULL,
        expected_bytes,
    )?;
    record_model_event(
        RootEventType::ModelPull,
        RootEventStatus::Completed,
        &model.name,
        Some(snapshot_id.clone()),
        Some("pulled_and_verified; weights_retained=true".to_string()),
        Some(duration_ms),
    );
    record_model_event(
        RootEventType::ModelVerified,
        RootEventStatus::Verified,
        &model.name,
        Some(snapshot_id),
        Some("pulled_and_verified; weights_retained=true".to_string()),
        Some(duration_ms),
    );
    Ok((
        ModelsPullResult {
            name: model.name.clone(),
            verb: PullVerb::PulledAndVerified,
            requested_tag: model.resolved_name.clone(),
            observed_digest: Some(canonical.clone()),
            locked_digest: Some(canonical),
            digest_match: Some(true),
            bytes_completed: outcome.bytes_completed,
            lock_written: true,
            exit_code: 0,
            reason: None,
        },
        false,
    ))
}

fn finish_report(results: Vec<ModelsPullResult>) -> ModelsPullReport {
    let success = results.iter().all(|row| row.verb.is_success());
    ModelsPullReport {
        success,
        command: PULL_COMMAND,
        models_restored: false,
        model_weights_deleted: false,
        lock_schema_version: peek_lock_schema_version_or_default(),
        results,
    }
}

fn write_progress(name: &str, progress: &PullProgress) {
    match (progress.completed, progress.total) {
        (Some(completed), Some(total)) => {
            eprintln!("{}: {} ({completed}/{total})", name, progress.status);
        }
        _ => eprintln!("{}: {}", name, progress.status),
    }
}

/// Pull-and-verify declared Ollama models using the loopback daemon.
pub fn pull_models(name: Option<&str>) -> Result<ModelsPullReport> {
    let ollama = HttpOllama::default();
    pull_models_with_backend(name, &ollama, &ollama, &mut |model, progress| {
        write_progress(model, &progress);
    })
}

pub fn pull_models_with_backend(
    name: Option<&str>,
    inspector: &impl OllamaInspector,
    realizer: &impl OllamaRealizer,
    progress: &mut dyn FnMut(&str, PullProgress),
) -> Result<ModelsPullReport> {
    let rootfile = get_or_create_rootfile()?;
    if let Some(requested) = name {
        if !rootfile.models.contains_key(requested) {
            return Err(ModelError::UnknownName(requested.to_string()).into());
        }
    }
    if name.is_none() && rootfile.models.is_empty() {
        return Ok(empty_pull_report());
    }

    let policy_subject = name.unwrap_or("*");
    enforce_model_pull_policy(policy_subject)?;

    let marker_name = name.unwrap_or("*");
    let _marker = ModelPullGuard::acquire(marker_name)?;

    let lock_models = load_lock_models()?;
    let plan = plan_models_from(name, &rootfile, &lock_models, inspector)?;
    let _ = events::record_event(
        RootEventType::ModelPlan,
        RootEventStatus::Planned,
        "root models pull",
        None,
        None,
        None,
        Some(format!("planned {} model(s)", plan.models.len())),
    );

    if plan
        .models
        .iter()
        .any(|model| model.planned_action == PlannedAction::RuntimeUnavailable)
    {
        return Err(ModelError::Unreachable(
            plan.runtime
                .reason
                .clone()
                .unwrap_or_else(|| REASON_ENDPOINT_UNREACHABLE.to_string()),
        )
        .into());
    }

    let lock_path = lock_file_path()?;
    let mut expected_bytes = read_lock_bytes(&lock_path)?;
    let mut results = Vec::with_capacity(plan.models.len());
    let mut hard_stop = false;
    for model in &plan.models {
        if hard_stop {
            results.push(not_attempted_result(model));
            continue;
        }
        match model.planned_action {
            PlannedAction::AlreadyVerified => {
                results.push(already_verified_result(model));
            }
            PlannedAction::VerifyOnly => {
                let row =
                    verify_only_row(model, plan.runtime.version.clone(), &mut expected_bytes)?;
                if row.verb == PullVerb::PullFailed {
                    hard_stop = true;
                }
                results.push(row);
            }
            PlannedAction::PullTagThenVerify => {
                let (row, stop) = pull_tag_then_verify_row(
                    model,
                    inspector,
                    realizer,
                    plan.runtime.version.clone(),
                    &mut expected_bytes,
                    progress,
                )?;
                hard_stop = stop;
                results.push(row);
            }
            PlannedAction::CannotReproduceLockedDigest
            | PlannedAction::UnsupportedRuntime
            | PlannedAction::RemoteOrCloudUnsupported
            | PlannedAction::ProtocolUnsupported => {
                results.push(skipped_result(model));
            }
            PlannedAction::RuntimeUnavailable => {
                return Err(ModelError::Unreachable(
                    model
                        .reason
                        .clone()
                        .unwrap_or_else(|| REASON_ENDPOINT_UNREACHABLE.to_string()),
                )
                .into());
            }
        }
    }

    Ok(finish_report(results))
}

pub fn models_pull_exit_code(report: &ModelsPullReport) -> i32 {
    report
        .results
        .iter()
        .map(|row| row.exit_code)
        .max()
        .unwrap_or(0)
}

pub fn format_pull_models_human(report: &ModelsPullReport) -> String {
    if report.results.is_empty() {
        return "No declared Ollama models.".to_string();
    }
    let mut out = String::new();
    for row in &report.results {
        let line = match row.verb {
            PullVerb::PulledAndVerified => {
                format!(
                    "{}: pulled and verified {}.",
                    row.name,
                    row.observed_digest.as_deref().unwrap_or("digest")
                )
            }
            PullVerb::VerifiedAndLocked => {
                format!(
                    "{}: verified local digest and wrote lock; not pulled.",
                    row.name
                )
            }
            PullVerb::AlreadyVerified => {
                format!("{}: already verified; lock unchanged.", row.name)
            }
            PullVerb::VerificationFailed => format!(
                "{}: verification failed; locked digest not reproduced; weights retained.",
                row.name
            ),
            PullVerb::PullFailed => format!(
                "{}: pull failed; no lock entry; partial weights retained.",
                row.name
            ),
            PullVerb::SkippedUnsupported => format!(
                "{}: skipped ({}).",
                row.name,
                row.reason.as_deref().unwrap_or("unsupported")
            ),
            PullVerb::NotAttempted => format!(
                "{}: not attempted ({}).",
                row.name,
                row.reason.as_deref().unwrap_or(REASON_STOPPED)
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{EvidenceSource, ResourceKind};
    use crate::ollama::{MockOllama, OllamaRealizer, PullProgress, RuntimeProbe};
    use root_lockfile::{ModelDeclaration, RootLockV2};
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    const HEX64: &str = "c6eb81c2c3a4b5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e";
    const HEX64_UPPER: &str = "C6EB81C2C3A4B5D6E7F8091A2B3C4D5E6F708192A3B4C5D6E7F8091A2B3C4D5E";
    const HEX64_OTHER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn sha(hex: &str) -> String {
        format!("sha256:{hex}")
    }

    fn rootfile_with(entries: &[(&str, &str)]) -> Rootfile {
        let mut rootfile = Rootfile::default();
        for (name, runtime) in entries {
            rootfile.models.insert(
                (*name).to_string(),
                ModelDeclaration {
                    runtime: (*runtime).to_string(),
                },
            );
        }
        rootfile
    }

    fn listed(name: &str, digest: Option<&str>) -> ListedModel {
        ListedModel {
            name: name.to_string(),
            model: None,
            digest: digest.map(str::to_string),
            size: None,
            remote_host: None,
            remote_model: None,
        }
    }

    fn locked(name: &str, digest: &str) -> BTreeMap<String, BTreeMap<String, LockedModel>> {
        let mut inner = BTreeMap::new();
        inner.insert(
            name.to_string(),
            LockedModel {
                runtime: OLLAMA_RUNTIME.to_string(),
                requested_name: name.to_string(),
                observed_digest: digest.to_string(),
                size_bytes: Some(42),
                endpoint: Some(format!("http://{OLLAMA_ENDPOINT}")),
                backend_version: Some("0.11.0".into()),
                locked_at: "2026-09-01T00:00:00Z".into(),
                verified_at: "2026-09-01T00:00:01Z".into(),
                verification_method: "inspect_tags_digest".into(),
                addressability: ADDRESSABILITY.to_string(),
            },
        );
        let mut models = BTreeMap::new();
        models.insert(OLLAMA_RUNTIME.to_string(), inner);
        models
    }

    struct CountingInspector<T> {
        inner: T,
        inspect_calls: Mutex<u32>,
        list_calls: Mutex<u32>,
    }

    impl<T: OllamaInspector> CountingInspector<T> {
        fn new(inner: T) -> Self {
            Self {
                inner,
                inspect_calls: Mutex::new(0),
                list_calls: Mutex::new(0),
            }
        }

        fn inspect_count(&self) -> u32 {
            *self.inspect_calls.lock().unwrap()
        }

        fn list_count(&self) -> u32 {
            *self.list_calls.lock().unwrap()
        }
    }

    impl<T: OllamaInspector> OllamaInspector for CountingInspector<T> {
        fn inspect_runtime(&self) -> crate::ollama::RuntimeProbe {
            *self.inspect_calls.lock().unwrap() += 1;
            self.inner.inspect_runtime()
        }

        fn list_models(&self) -> Result<Vec<ListedModel>, InspectError> {
            *self.list_calls.lock().unwrap() += 1;
            self.inner.list_models()
        }
    }

    struct CountingPull {
        inner: MockOllama,
        inspect_calls: Mutex<u32>,
        pull_calls: Mutex<u32>,
    }

    impl CountingPull {
        fn new(inner: MockOllama) -> Self {
            Self {
                inner,
                inspect_calls: Mutex::new(0),
                pull_calls: Mutex::new(0),
            }
        }

        fn inspect_count(&self) -> u32 {
            *self.inspect_calls.lock().unwrap()
        }

        fn pull_count(&self) -> u32 {
            *self.pull_calls.lock().unwrap()
        }
    }

    impl OllamaInspector for CountingPull {
        fn inspect_runtime(&self) -> crate::ollama::RuntimeProbe {
            *self.inspect_calls.lock().unwrap() += 1;
            self.inner.inspect_runtime()
        }

        fn list_models(&self) -> Result<Vec<ListedModel>, InspectError> {
            self.inner.list_models()
        }
    }

    impl OllamaRealizer for CountingPull {
        fn pull_tag(
            &self,
            name: &str,
            progress: &mut dyn FnMut(PullProgress),
        ) -> Result<crate::ollama::PullOutcome, crate::ollama::RealizeError> {
            *self.pull_calls.lock().unwrap() += 1;
            self.inner.pull_tag(name, progress)
        }
    }

    fn plan(
        entries: &[(&str, &str)],
        lock: BTreeMap<String, BTreeMap<String, LockedModel>>,
        inspector: &impl OllamaInspector,
    ) -> PlanModelsReport {
        plan_models_from(None, &rootfile_with(entries), &lock, inspector).unwrap()
    }

    fn assert_unsupported(report: &PlanModelsReport) {
        let ops: Vec<_> = report
            .unsupported_operations
            .iter()
            .map(|op| (op.operation, op.reason))
            .collect();
        assert_eq!(
            ops,
            vec![
                ("digest_addressable_restore", "ollama_api_pull_is_tag_only"),
                ("pull_by_digest", "ollama_api_pull_is_tag_only"),
                ("delete_weights", "not_in_v0.3_surface"),
                ("deterministic_restore", "lock_is_verification_record_only"),
            ]
        );
    }

    #[test]
    fn golden_json_empty_models() {
        let inspector = CountingInspector::new(MockOllama::default());
        let report =
            plan_models_from(None, &Rootfile::default(), &BTreeMap::new(), &inspector).unwrap();
        assert_eq!(inspector.inspect_count(), 0);
        assert_eq!(inspector.list_count(), 0);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({
                "success": true,
                "command": "plan models",
                "would_mutate": false,
                "runtime": {
                    "name": "ollama",
                    "endpoint": "127.0.0.1:11434",
                    "reachable": null,
                    "version": null,
                    "protocol": "not_probed",
                    "reason": "no_declared_models"
                },
                "unsupported_operations": [
                    {"operation": "digest_addressable_restore", "reason": "ollama_api_pull_is_tag_only"},
                    {"operation": "pull_by_digest", "reason": "ollama_api_pull_is_tag_only"},
                    {"operation": "delete_weights", "reason": "not_in_v0.3_surface"},
                    {"operation": "deterministic_restore", "reason": "lock_is_verification_record_only"}
                ],
                "models": []
            })
        );
        let human = format_plan_models_human(&report);
        assert!(human.starts_with("Unsupported operations:"));
        let unsupported_at = human.find("Unsupported operations:").unwrap();
        let empty_at = human.find("No declared Ollama models.").unwrap();
        let preview_at = human
            .find("This is a preview. No changes have been made.")
            .unwrap();
        assert!(unsupported_at < empty_at);
        assert!(empty_at < preview_at);
    }

    #[test]
    fn golden_json_absent_no_lock_pulls_tag() {
        let inspector = MockOllama::default();
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert_eq!(inspector.captured_pull_body(), None);
        let model = serde_json::to_value(&report.models[0]).unwrap();
        assert_eq!(
            model,
            serde_json::json!({
                "name": "qwen3:8b",
                "runtime": "ollama",
                "observation": "absent",
                "evaluation": "missing",
                "desired_tag": "qwen3:8b",
                "resolved_name": "qwen3:8b",
                "current_tag": null,
                "current_digest": null,
                "locked_digest": null,
                "digest_match": null,
                "expected_download": {
                    "state": "unknown_until_manifest",
                    "bytes": null,
                    "reason": "ollama_pull_does_not_expose_size_before_mutation"
                },
                "planned_action": "pull_tag_then_verify",
                "would_mutate": true,
                "would_write_lock": true,
                "addressability": "verification_record_only",
                "reason": "not_found"
            })
        );
        assert!(report.would_mutate);
        assert_eq!(report.command, "plan models");
        assert_eq!(report.runtime.protocol, PlanRuntimeProtocol::Supported);
        assert_eq!(report.runtime.reachable, Some(true));
        assert_eq!(
            serde_json::to_value(report.runtime.protocol).unwrap(),
            serde_json::json!("supported")
        );
        assert_unsupported(&report);
    }

    #[test]
    fn absent_locked_digest_warns_addressability() {
        let digest = sha(HEX64);
        let report = plan(
            &[("qwen3:8b", "ollama")],
            locked("qwen3:8b", &digest),
            &MockOllama::default(),
        );
        let model = &report.models[0];
        assert_eq!(model.planned_action, PlannedAction::PullTagThenVerify);
        assert!(model.would_mutate);
        assert!(model.would_write_lock);
        assert_eq!(model.locked_digest.as_deref(), Some(digest.as_str()));
        assert_eq!(model.reason.as_deref(), Some("ollama_api_pull_is_tag_only"));
        assert_eq!(model.addressability, ADDRESSABILITY);
    }

    #[test]
    fn present_matching_lock_is_already_verified() {
        let digest = sha(HEX64);
        let inspector = MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]);
        let report = plan(
            &[("qwen3:8b", "ollama")],
            locked("qwen3:8b", &digest),
            &inspector,
        );
        let model = &report.models[0];
        assert_eq!(model.planned_action, PlannedAction::AlreadyVerified);
        assert!(!model.would_mutate);
        assert!(!model.would_write_lock);
        assert_eq!(model.digest_match, Some(true));
        assert!(!report.would_mutate);
        assert_eq!(inspector.captured_pull_body(), None);
    }

    #[test]
    fn present_differing_lock_cannot_reproduce() {
        let inspector =
            MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&sha(HEX64)))]);
        let report = plan(
            &[("qwen3:8b", "ollama")],
            locked("qwen3:8b", &sha(HEX64_OTHER)),
            &inspector,
        );
        let model = &report.models[0];
        assert_eq!(
            model.planned_action,
            PlannedAction::CannotReproduceLockedDigest
        );
        assert_eq!(model.evaluation, EvaluationState::Drifted);
        assert!(!model.would_mutate);
        assert!(!model.would_write_lock);
        assert_eq!(model.digest_match, Some(false));
        assert_eq!(model.current_digest.as_deref(), Some(sha(HEX64).as_str()));
    }

    #[test]
    fn present_without_lock_is_verify_only() {
        let inspector =
            MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&sha(HEX64)))]);
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        let model = &report.models[0];
        assert_eq!(model.planned_action, PlannedAction::VerifyOnly);
        assert_eq!(model.observation, Presence::Present);
        assert!(!model.would_mutate);
        assert!(model.would_write_lock);
        assert!(!report.would_mutate);
    }

    #[test]
    fn unknown_observation_never_plans_pull() {
        let inspector = MockOllama::default().with_runtime(RuntimeProbe {
            version: None,
            protocol: RuntimeProtocol::Unreachable,
        });
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert!(report.success);
        assert!(!report.would_mutate);
        assert_eq!(report.runtime.reachable, Some(false));
        assert_eq!(report.runtime.protocol, PlanRuntimeProtocol::Unreachable);
        let model = &report.models[0];
        assert_eq!(model.observation, Presence::Unknown);
        assert_ne!(model.observation, Presence::Absent);
        assert_eq!(model.planned_action, PlannedAction::RuntimeUnavailable);
        assert!(!model.would_mutate);
        assert_eq!(inspector.captured_pull_body(), None);
    }

    #[test]
    fn unknown_name_is_error_without_probe() {
        let inspector = CountingInspector::new(MockOllama::default());
        let err = plan_models_from(
            Some("missing"),
            &rootfile_with(&[("qwen3:8b", "ollama")]),
            &BTreeMap::new(),
            &inspector,
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not declared in Rootfile"));
        assert_eq!(inspector.inspect_count(), 0);
        assert_eq!(inspector.list_count(), 0);
    }

    #[test]
    fn declared_cloud_suffix_no_http() {
        let inspector = CountingInspector::new(MockOllama::default());
        let report = plan_models_from(
            None,
            &rootfile_with(&[("gpt-oss:120b-cloud", "ollama"), ("foo:cloud", "ollama")]),
            &BTreeMap::new(),
            &inspector,
        )
        .unwrap();
        assert_eq!(inspector.inspect_count(), 0);
        assert_eq!(inspector.list_count(), 0);
        assert!(!report.would_mutate);
        assert_eq!(report.runtime.protocol, PlanRuntimeProtocol::NotProbed);
        assert_eq!(report.runtime.reachable, None);
        assert_eq!(report.models.len(), 2);
        for model in &report.models {
            assert_eq!(
                model.planned_action,
                PlannedAction::RemoteOrCloudUnsupported
            );
            assert_eq!(model.observation, Presence::Unknown);
            assert!(!model.would_mutate);
        }
    }

    #[test]
    fn listed_remote_host_is_unsupported() {
        let remote = ListedModel {
            name: "qwen3:8b".into(),
            model: None,
            digest: Some(sha(HEX64)),
            size: None,
            remote_host: Some("https://ollama.example".into()),
            remote_model: None,
        };
        let inspector = MockOllama::default().with_models(vec![remote]);
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::RemoteOrCloudUnsupported
        );
        assert_eq!(report.models[0].observation, Presence::Present);
        assert!(!report.would_mutate);
    }

    #[test]
    fn unsupported_runtime_skips_http() {
        let inspector = CountingInspector::new(MockOllama::default());
        let report = plan_models_from(
            None,
            &rootfile_with(&[("qwen3:8b", "lmstudio")]),
            &BTreeMap::new(),
            &inspector,
        )
        .unwrap();
        assert_eq!(inspector.inspect_count(), 0);
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::UnsupportedRuntime
        );
        assert_eq!(report.runtime.reachable, None);
        assert!(!report.would_mutate);
    }

    #[test]
    fn protocol_unsupported_does_not_plan_pull() {
        let inspector = MockOllama::default().with_runtime(RuntimeProbe {
            version: None,
            protocol: RuntimeProtocol::ProtocolUnsupported,
        });
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::ProtocolUnsupported
        );
        assert_eq!(report.models[0].observation, Presence::Unknown);
        assert_eq!(report.runtime.reachable, Some(false));
        assert!(!report.would_mutate);
    }

    #[test]
    fn malformed_runtime_is_unavailable_not_protocol() {
        let inspector = MockOllama::default().with_runtime(RuntimeProbe {
            version: None,
            protocol: RuntimeProtocol::Malformed,
        });
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert_eq!(report.runtime.protocol, PlanRuntimeProtocol::Malformed);
        assert_eq!(report.runtime.reachable, Some(false));
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::RuntimeUnavailable
        );
        assert_eq!(report.models[0].observation, Presence::Unknown);
        assert_ne!(
            report.models[0].planned_action,
            PlannedAction::ProtocolUnsupported
        );
        assert!(!report.would_mutate);
        assert_eq!(inspector.captured_pull_body(), None);
    }

    #[test]
    fn untagged_name_resolves_latest_for_pull() {
        let report = plan(
            &[("qwen3", "ollama")],
            BTreeMap::new(),
            &MockOllama::default(),
        );
        assert_eq!(report.models[0].desired_tag, "qwen3");
        assert_eq!(report.models[0].resolved_name, "qwen3:latest");
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::PullTagThenVerify
        );
        let human = format_plan_models_human(&report);
        let unsupported_at = human.find("Unsupported operations:").unwrap();
        let pull_at = human.find("would pull qwen3:latest").unwrap();
        assert!(unsupported_at < pull_at);
        assert!(human.contains("This is a preview. No changes have been made."));
    }

    #[test]
    fn lock_inner_key_stays_rootfile_key() {
        let digest = sha(HEX64);
        let inspector =
            MockOllama::default().with_models(vec![listed("qwen3:latest", Some(&digest))]);
        let report = plan_models_from(
            None,
            &rootfile_with(&[("qwen3", "ollama")]),
            &locked("qwen3", &digest),
            &inspector,
        )
        .unwrap();
        assert_eq!(report.models[0].name, "qwen3");
        assert_eq!(report.models[0].resolved_name, "qwen3:latest");
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::AlreadyVerified
        );
        assert_eq!(
            report.models[0].locked_digest.as_deref(),
            Some(digest.as_str())
        );
    }

    #[test]
    fn name_filter_plans_only_requested_model() {
        let inspector = MockOllama::default();
        let report = plan_models_from(
            Some("qwen3:8b"),
            &rootfile_with(&[("qwen3:8b", "ollama"), ("llama3:8b", "ollama")]),
            &BTreeMap::new(),
            &inspector,
        )
        .unwrap();
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.models[0].name, "qwen3:8b");
    }

    #[test]
    fn http_port_zero_is_unreachable_preview() {
        let inspector = crate::ollama::HttpOllama::for_tests(
            "127.0.0.1",
            0,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );
        let report = plan(&[("qwen3:8b", "ollama")], BTreeMap::new(), &inspector);
        assert!(report.success);
        assert!(!report.would_mutate);
        assert_eq!(report.models[0].observation, Presence::Unknown);
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::RuntimeUnavailable
        );
    }

    #[test]
    fn plan_models_does_not_write_lock_or_pull_artifact() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!(
            "root_plan_models_nowrite_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ROOT_DIR", &tmp);
        rootfile_with(&[("qwen3:8b", "ollama")])
            .write_to_file(&tmp.join("Rootfile"))
            .unwrap();
        let inspector = MockOllama::default();
        let report = plan_models_with_inspector(None, &inspector).unwrap();
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::PullTagThenVerify
        );
        assert!(!tmp.join("root.lock").exists());
        assert!(!tmp.join("model-pull.json").exists());
        assert_eq!(inspector.captured_pull_body(), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn raw_current_digest_is_not_canonicalized() {
        let raw = format!("sha256-{HEX64}");
        let inspector = MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&raw))]);
        let report = plan(
            &[("qwen3:8b", "ollama")],
            locked("qwen3:8b", &sha(HEX64)),
            &inspector,
        );
        assert_eq!(
            report.models[0].current_digest.as_deref(),
            Some(raw.as_str())
        );
        assert_eq!(
            report.models[0].locked_digest.as_deref(),
            Some(sha(HEX64).as_str())
        );
        assert_eq!(report.models[0].digest_match, Some(true));
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::AlreadyVerified
        );
    }

    #[test]
    fn present_noncanonical_lock_is_not_unlocked() {
        let inspector =
            MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&sha(HEX64)))]);
        let report = plan(
            &[("qwen3:8b", "ollama")],
            locked("qwen3:8b", "not-a-canonical-digest"),
            &inspector,
        );
        assert_eq!(
            report.models[0].planned_action,
            PlannedAction::CannotReproduceLockedDigest
        );
        assert!(!report.models[0].would_write_lock);
        assert_eq!(report.models[0].locked_digest, None);
    }

    fn isolated_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "root_plan_models_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn future_lock_is_refused_without_rewrite() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        let tmp = isolated_root("future_lock");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ROOT_DIR", &tmp);
        rootfile_with(&[("qwen3:8b", "ollama")])
            .write_to_file(&tmp.join("Rootfile"))
            .unwrap();
        let original = r#"{
  "version": 4,
  "platform": "aarch64-darwin",
  "packages": [],
  "models": [{"name": "must-not-be-copied"}]
}"#;
        std::fs::write(tmp.join("root.lock"), original).unwrap();
        let err = plan_models_with_inspector(None, &MockOllama::default()).unwrap_err();
        assert!(
            err.to_string().contains("newer than this Root supports"),
            "unexpected error: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.join("root.lock")).unwrap(),
            original
        );
        assert!(!tmp.join("model-pull.json").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn invalid_locked_digest_is_refused_without_rewrite() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        let tmp = isolated_root("invalid_digest");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ROOT_DIR", &tmp);
        rootfile_with(&[("qwen3:8b", "ollama")])
            .write_to_file(&tmp.join("Rootfile"))
            .unwrap();
        let original = r#"{
  "version": 3,
  "platform": "aarch64-darwin",
  "packages": [],
  "models": {
    "ollama": {
      "qwen3:8b": {
        "runtime": "ollama",
        "requested_name": "qwen3:8b",
        "observed_digest": "not-canonical",
        "locked_at": "2026-09-01T00:00:00Z",
        "verified_at": "2026-09-01T00:00:01Z",
        "verification_method": "inspect_tags_digest",
        "addressability": "verification_record_only"
      }
    }
  }
}"#;
        std::fs::write(tmp.join("root.lock"), original).unwrap();
        let err = plan_models_with_inspector(None, &MockOllama::default()).unwrap_err();
        assert!(
            err.to_string().contains("failed models validation")
                || err.to_string().contains("observed_digest"),
            "unexpected error: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.join("root.lock")).unwrap(),
            original
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn canonical(hex: &str) -> String {
        format!("sha256:{hex}")
    }

    fn present_model(name: &str, observed: Option<&str>) -> InventoryItem {
        InventoryItem {
            name: name.to_string(),
            kind: ResourceKind::Model,
            desired: "ollama".into(),
            observation: Presence::Present,
            evaluation: EvaluationState::Satisfied,
            observed_version: None,
            observed_digest: observed.map(str::to_string),
            evidence_source: EvidenceSource::OllamaApiTags,
            reason: None,
            locked_digest: None,
            digest_match: None,
        }
    }

    fn missing_model(name: &str) -> InventoryItem {
        InventoryItem {
            name: name.to_string(),
            kind: ResourceKind::Model,
            desired: "ollama".into(),
            observation: Presence::Absent,
            evaluation: EvaluationState::Missing,
            observed_version: None,
            observed_digest: None,
            evidence_source: EvidenceSource::OllamaApiTags,
            reason: Some(crate::inventory::REASON_NOT_FOUND.to_string()),
            locked_digest: None,
            digest_match: None,
        }
    }

    fn lock_with(name: &str, digest: &str) -> RootLockV2 {
        let mut inner = BTreeMap::new();
        inner.insert(
            name.to_string(),
            LockedModel {
                runtime: "ollama".into(),
                requested_name: name.to_string(),
                observed_digest: digest.to_string(),
                locked_at: "2026-09-01T00:00:00Z".into(),
                verified_at: "2026-09-01T00:00:01Z".into(),
                verification_method: "inspect_tags_digest".into(),
                addressability: "verification_record_only".into(),
                ..Default::default()
            },
        );
        let mut models = BTreeMap::new();
        models.insert("ollama".into(), inner);
        RootLockV2 {
            version: 3,
            models,
            ..Default::default()
        }
    }

    #[test]
    fn compare_matches_official_tags_hex_without_prefix() {
        let compare = compare_locked_digest(Some(HEX64_UPPER), &canonical(HEX64)).unwrap();
        assert_eq!(compare.locked_digest, canonical(HEX64));
        assert!(compare.digest_match);
    }

    #[test]
    fn compare_does_not_use_naive_string_equality() {
        assert_ne!(HEX64_UPPER, canonical(HEX64));
        let compare = compare_locked_digest(Some(HEX64_UPPER), &canonical(HEX64)).unwrap();
        assert!(compare.digest_match);
    }

    #[test]
    fn compare_mismatch_and_missing_observed() {
        let mismatch = compare_locked_digest(Some(HEX64_OTHER), &canonical(HEX64)).unwrap();
        assert!(!mismatch.digest_match);
        assert_eq!(mismatch.locked_digest, canonical(HEX64));

        let missing = compare_locked_digest(None, &canonical(HEX64)).unwrap();
        assert!(!missing.digest_match);

        assert!(compare_locked_digest(Some(HEX64), "sha256-not-a-digest").is_none());
    }

    #[test]
    fn overlay_omits_fields_without_lock_entry() {
        let mut report = InventoryReport {
            models: vec![present_model("qwen3:8b", Some(HEX64_UPPER))],
            ..InventoryReport::default()
        };
        overlay_locked_digests(&mut report, &RootLockV2::default());
        assert!(report.models[0].locked_digest.is_none());
        assert!(report.models[0].digest_match.is_none());
        assert_eq!(report.models[0].evaluation, EvaluationState::Satisfied);
        assert_eq!(
            report.models[0].observed_digest.as_deref(),
            Some(HEX64_UPPER)
        );
    }

    fn with_isolated<R>(name: &str, entries: &[(&str, &str)], f: impl FnOnce(&Path) -> R) -> R {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        let tmp = isolated_root(name);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ROOT_DIR", &tmp);
        root_lockfile::init_root_dir().unwrap();
        if !entries.is_empty() {
            rootfile_with(entries)
                .write_to_file(&tmp.join("Rootfile"))
                .unwrap();
        }
        let result = f(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }

    fn pull_now(backend: &impl OllamaBackend) -> ModelsPullReport {
        pull_models_with_backend(None, backend, backend, &mut |_, _| {}).unwrap()
    }

    fn pull_named(name: &str, backend: &impl OllamaBackend) -> Result<ModelsPullReport> {
        pull_models_with_backend(Some(name), backend, backend, &mut |_, _| {})
    }

    trait OllamaBackend: OllamaInspector + OllamaRealizer {}
    impl<T: OllamaInspector + OllamaRealizer> OllamaBackend for T {}

    fn write_v3_lock(dir: &Path, models: BTreeMap<String, BTreeMap<String, LockedModel>>) {
        let lock = RootLockV2 {
            version: root_lockfile::emit_lock_version(&models),
            platform: "aarch64-darwin".into(),
            models,
            ..Default::default()
        };
        lock.write_to_file(&dir.join("root.lock")).unwrap();
    }

    fn read_lock(dir: &Path) -> RootLockV2 {
        RootLockV2::read_from_file(&dir.join("root.lock")).unwrap()
    }

    fn snapshot_reasons(dir: &Path) -> Vec<String> {
        let snaps = dir.join("snapshots");
        if !snaps.exists() {
            return Vec::new();
        }
        let mut reasons = Vec::new();
        for entry in std::fs::read_dir(snaps).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            if let Some(reason) = value.get("reason").and_then(|v| v.as_str()) {
                reasons.push(reason.to_string());
            }
        }
        reasons.sort();
        reasons
    }

    fn write_live_marker(dir: &Path, pid: u32) {
        std::fs::write(
            dir.join("model-pull.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "name": "*",
                "started_at": "2026-09-01T00:00:00Z",
                "pid": pid
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_policy(dir: &Path, body: &str) {
        std::fs::write(dir.join("policy.toml"), body).unwrap();
    }

    #[test]
    fn empty_models_pull_is_success_without_marker_or_http() {
        with_isolated("empty_pull", &[], |tmp| {
            let backend = CountingPull::new(MockOllama::default());
            let report = pull_now(&backend);
            assert_eq!(backend.inspect_count(), 0);
            assert_eq!(backend.pull_count(), 0);
            assert!(report.success);
            assert_eq!(report.command, "models pull");
            assert!(!report.models_restored);
            assert!(!report.model_weights_deleted);
            assert!(report.results.is_empty());
            assert_eq!(models_pull_exit_code(&report), 0);
            assert!(!tmp.join("model-pull.json").exists());
            let encoded = serde_json::to_value(&report).unwrap();
            assert_eq!(encoded["models_restored"], false);
            assert_eq!(encoded["model_weights_deleted"], false);
        });
    }

    #[test]
    fn unknown_name_pull_exits_2_without_marker_or_http() {
        with_isolated("unknown_pull", &[("qwen3:8b", "ollama")], |tmp| {
            let backend = CountingPull::new(MockOllama::default());
            let err = pull_named("nope", &backend).unwrap_err();
            let model_err = err.downcast_ref::<ModelError>().unwrap();
            assert_eq!(model_err.exit_code(), 2);
            assert_eq!(backend.inspect_count(), 0);
            assert_eq!(backend.pull_count(), 0);
            assert!(!tmp.join("model-pull.json").exists());
            assert!(!tmp.join("root.lock").exists());
        });
    }

    #[test]
    fn tags_present_digest_verify_only_writes_v3_without_post() {
        with_isolated("verify_only", &[("qwen3:8b", "ollama")], |tmp| {
            let digest = sha(HEX64);
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]),
            );
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 0);
            assert_eq!(report.results.len(), 1);
            assert_eq!(report.results[0].verb, PullVerb::VerifiedAndLocked);
            assert!(report.results[0].lock_written);
            assert_eq!(report.results[0].exit_code, 0);
            assert!(report.success);
            assert_eq!(models_pull_exit_code(&report), 0);
            let lock = read_lock(tmp);
            assert_eq!(lock.version, 3);
            let model = &lock.models["ollama"]["qwen3:8b"];
            assert_eq!(model.observed_digest, digest);
            assert_eq!(model.verification_method, VERIFY_METHOD_INSPECT);
            assert_eq!(model.addressability, ADDRESSABILITY);
            assert_eq!(
                snapshot_reasons(tmp),
                vec!["before model verification record qwen3:8b".to_string()]
            );
            let human = format_pull_models_human(&report);
            assert!(human.contains("verified local digest and wrote lock; not pulled."));
            assert!(!human.contains("restored"));
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn already_verified_does_not_write_lock_or_snapshot() {
        with_isolated("already_verified", &[("qwen3:8b", "ollama")], |tmp| {
            let digest = sha(HEX64);
            write_v3_lock(tmp, locked("qwen3:8b", &digest));
            let before = std::fs::read(tmp.join("root.lock")).unwrap();
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]),
            );
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 0);
            assert_eq!(report.results[0].verb, PullVerb::AlreadyVerified);
            assert!(!report.results[0].lock_written);
            assert!(report.success);
            assert_eq!(models_pull_exit_code(&report), 0);
            assert_eq!(std::fs::read(tmp.join("root.lock")).unwrap(), before);
            assert!(snapshot_reasons(tmp).is_empty());
            let human = format_pull_models_human(&report);
            assert!(human.contains("already verified; lock unchanged."));
        });
    }

    #[test]
    fn cannot_reproduce_skips_without_post() {
        with_isolated("cannot_repro", &[("qwen3:8b", "ollama")], |tmp| {
            write_v3_lock(tmp, locked("qwen3:8b", &sha(HEX64_OTHER)));
            let before = std::fs::read(tmp.join("root.lock")).unwrap();
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&sha(HEX64)))]),
            );
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 0);
            assert!(!report.success);
            assert_eq!(report.results[0].verb, PullVerb::SkippedUnsupported);
            assert_eq!(report.results[0].exit_code, 2);
            assert_eq!(
                report.results[0].reason.as_deref(),
                Some(REASON_DIGEST_MISMATCH)
            );
            assert_eq!(models_pull_exit_code(&report), 2);
            assert_eq!(std::fs::read(tmp.join("root.lock")).unwrap(), before);
        });
    }

    #[test]
    fn pull_absent_writes_v3_after_verify() {
        with_isolated("pull_absent", &[("qwen3:8b", "ollama")], |tmp| {
            let digest = sha(HEX64);
            let backend = CountingPull::new(MockOllama::default().with_models_after_pull(vec![
                ListedModel {
                    name: "qwen3:8b".into(),
                    model: None,
                    digest: Some(digest.clone()),
                    size: Some(42),
                    remote_host: None,
                    remote_model: None,
                },
            ]));
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 1);
            assert!(report.success);
            assert_eq!(report.results[0].verb, PullVerb::PulledAndVerified);
            assert!(report.results[0].lock_written);
            assert_eq!(
                report.results[0].observed_digest.as_deref(),
                Some(digest.as_str())
            );
            assert_eq!(report.lock_schema_version, 3);
            let lock = read_lock(tmp);
            assert_eq!(lock.version, 3);
            let model = &lock.models["ollama"]["qwen3:8b"];
            assert_eq!(model.verification_method, VERIFY_METHOD_PULL);
            assert_eq!(model.size_bytes, Some(42));
            assert_eq!(model.requested_name, "qwen3:8b");
            assert_eq!(
                snapshot_reasons(tmp),
                vec!["before model verification record qwen3:8b".to_string()]
            );
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn pull_404_exits_3_without_lock_write() {
        with_isolated("pull_404", &[("qwen3:8b", "ollama")], |tmp| {
            let backend = CountingPull::new(MockOllama::default().with_pull_status(404));
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 1);
            assert!(!report.success);
            assert_eq!(report.results[0].verb, PullVerb::PullFailed);
            assert_eq!(report.results[0].exit_code, 3);
            assert_eq!(models_pull_exit_code(&report), 3);
            assert!(!tmp.join("root.lock").exists());
            assert!(!report.model_weights_deleted);
        });
    }

    #[test]
    fn pull_then_mismatch_exits_4_lock_unchanged() {
        with_isolated("pull_mismatch", &[("qwen3:8b", "ollama")], |tmp| {
            write_v3_lock(tmp, locked("qwen3:8b", &sha(HEX64)));
            let before = std::fs::read(tmp.join("root.lock")).unwrap();
            let backend = CountingPull::new(
                MockOllama::default()
                    .with_models_after_pull(vec![listed("qwen3:8b", Some(&sha(HEX64_OTHER)))]),
            );
            let report = pull_now(&backend);
            assert_eq!(backend.pull_count(), 1);
            assert_eq!(report.results[0].verb, PullVerb::VerificationFailed);
            assert_eq!(report.results[0].exit_code, 4);
            assert_eq!(models_pull_exit_code(&report), 4);
            assert!(!report.results[0].lock_written);
            assert!(!report.model_weights_deleted);
            assert_eq!(std::fs::read(tmp.join("root.lock")).unwrap(), before);
            assert!(snapshot_reasons(tmp).is_empty());
        });
    }

    #[test]
    fn mixed_skip_and_pull_commits_second() {
        with_isolated(
            "mixed_skip_pull",
            &[("alpha:8b", "ollama"), ("beta:8b", "ollama")],
            |tmp| {
                write_v3_lock(tmp, locked("alpha:8b", &sha(HEX64_OTHER)));
                let digest = sha(HEX64);
                let backend = CountingPull::new(
                    MockOllama::default()
                        .with_models(vec![listed("alpha:8b", Some(&sha(HEX64)))])
                        .with_models_after_pull(vec![
                            listed("alpha:8b", Some(&sha(HEX64))),
                            listed("beta:8b", Some(&digest)),
                        ]),
                );
                let report = pull_now(&backend);
                assert_eq!(backend.pull_count(), 1);
                assert_eq!(report.results.len(), 2);
                assert_eq!(report.results[0].name, "alpha:8b");
                assert_eq!(report.results[0].verb, PullVerb::SkippedUnsupported);
                assert_eq!(report.results[0].exit_code, 2);
                assert_eq!(report.results[1].name, "beta:8b");
                assert_eq!(report.results[1].verb, PullVerb::PulledAndVerified);
                assert!(!report.success);
                assert_eq!(models_pull_exit_code(&report), 2);
                let lock = read_lock(tmp);
                assert!(lock.models["ollama"].contains_key("beta:8b"));
                assert_eq!(
                    lock.models["ollama"]["alpha:8b"].observed_digest,
                    sha(HEX64_OTHER)
                );
            },
        );
    }

    #[test]
    fn overlay_keeps_raw_observed_digest_on_canonical_match() {
        let mut report = InventoryReport {
            models: vec![present_model("qwen3:8b", Some(HEX64_UPPER))],
            ..InventoryReport::default()
        };
        overlay_locked_digests(&mut report, &lock_with("qwen3:8b", &canonical(HEX64)));
        assert_eq!(
            report.models[0].observed_digest.as_deref(),
            Some(HEX64_UPPER)
        );
        assert_eq!(
            report.models[0].locked_digest.as_deref(),
            Some(canonical(HEX64).as_str())
        );
        assert_eq!(report.models[0].digest_match, Some(true));
        assert_eq!(report.models[0].evaluation, EvaluationState::Satisfied);
    }

    #[test]
    fn overlay_marks_present_mismatch_as_drifted() {
        let mut report = InventoryReport {
            models: vec![present_model("qwen3:8b", Some(HEX64_OTHER))],
            ..InventoryReport::default()
        };
        overlay_locked_digests(&mut report, &lock_with("qwen3:8b", &canonical(HEX64)));
        assert_eq!(
            report.models[0].observed_digest.as_deref(),
            Some(HEX64_OTHER)
        );
        assert_eq!(report.models[0].digest_match, Some(false));
        assert_eq!(report.models[0].evaluation, EvaluationState::Drifted);
        assert_eq!(report.models[0].observation, Presence::Present);
    }

    #[test]
    fn overlay_does_not_turn_missing_into_drifted() {
        let mut report = InventoryReport {
            models: vec![missing_model("qwen3:8b")],
            ..InventoryReport::default()
        };
        overlay_locked_digests(&mut report, &lock_with("qwen3:8b", &canonical(HEX64)));
        assert_eq!(
            report.models[0].locked_digest.as_deref(),
            Some(canonical(HEX64).as_str())
        );
        assert_eq!(report.models[0].digest_match, Some(false));
        assert_eq!(report.models[0].evaluation, EvaluationState::Missing);
    }

    #[test]
    fn overlay_ignores_other_runtime_namespace() {
        let mut report = InventoryReport {
            models: vec![present_model("qwen3:8b", Some(HEX64_UPPER))],
            ..InventoryReport::default()
        };
        report.models[0].desired = "lmstudio".into();
        overlay_locked_digests(&mut report, &lock_with("qwen3:8b", &canonical(HEX64)));
        assert!(report.models[0].locked_digest.is_none());
        assert!(report.models[0].digest_match.is_none());
        assert_eq!(report.models[0].evaluation, EvaluationState::Satisfied);
    }

    #[test]
    fn mixed_404_stops_remaining() {
        with_isolated(
            "mixed_404",
            &[("alpha:8b", "ollama"), ("beta:8b", "ollama")],
            |tmp| {
                let backend = CountingPull::new(MockOllama::default().with_pull_status(404));
                let report = pull_now(&backend);
                assert_eq!(backend.pull_count(), 1);
                assert_eq!(report.results[0].verb, PullVerb::PullFailed);
                assert_eq!(report.results[0].exit_code, 3);
                assert_eq!(report.results[1].verb, PullVerb::NotAttempted);
                assert_eq!(report.results[1].exit_code, 0);
                assert_eq!(report.results[1].reason.as_deref(), Some(REASON_STOPPED));
                assert_eq!(models_pull_exit_code(&report), 3);
                assert!(!tmp.join("root.lock").exists());
            },
        );
    }

    #[test]
    fn declared_cloud_suffix_pull_skips_http() {
        with_isolated(
            "cloud_pull",
            &[("gpt-oss:120b-cloud", "ollama"), ("foo:cloud", "ollama")],
            |tmp| {
                let backend = CountingPull::new(MockOllama::default());
                let report = pull_now(&backend);
                assert_eq!(backend.inspect_count(), 0);
                assert_eq!(backend.pull_count(), 0);
                assert_eq!(report.results.len(), 2);
                for row in &report.results {
                    assert_eq!(row.verb, PullVerb::SkippedUnsupported);
                    assert_eq!(row.exit_code, 2);
                }
                assert_eq!(models_pull_exit_code(&report), 2);
                assert!(!tmp.join("root.lock").exists());
            },
        );
    }

    #[test]
    fn unsupported_runtime_skips_without_http() {
        with_isolated("lmstudio", &[("qwen3:8b", "lmstudio")], |tmp| {
            let backend = CountingPull::new(MockOllama::default());
            let report = pull_now(&backend);
            assert_eq!(backend.inspect_count(), 0);
            assert_eq!(backend.pull_count(), 0);
            assert_eq!(report.results[0].verb, PullVerb::SkippedUnsupported);
            assert_eq!(models_pull_exit_code(&report), 2);
            assert!(!tmp.join("root.lock").exists());
        });
    }

    #[test]
    fn unreachable_pull_exits_1_not_7() {
        with_isolated("unreachable", &[("qwen3:8b", "ollama")], |tmp| {
            let backend = CountingPull::new(MockOllama::default().with_runtime(RuntimeProbe {
                version: None,
                protocol: RuntimeProtocol::Unreachable,
            }));
            let err =
                pull_models_with_backend(None, &backend, &backend, &mut |_, _| {}).unwrap_err();
            let model_err = err.downcast_ref::<ModelError>().unwrap();
            assert_eq!(model_err.exit_code(), 1);
            assert!(!err.to_string().contains("Nix"));
            assert_eq!(backend.pull_count(), 0);
            assert!(!tmp.join("root.lock").exists());
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn http_port_zero_pull_exits_1_not_7() {
        with_isolated("port_zero", &[("qwen3:8b", "ollama")], |_tmp| {
            let inspector = crate::ollama::HttpOllama::for_tests(
                "127.0.0.1",
                0,
                Duration::from_millis(50),
                Duration::from_millis(50),
            );
            let err =
                pull_models_with_backend(None, &inspector, &inspector, &mut |_, _| {}).unwrap_err();
            let model_err = err.downcast_ref::<ModelError>().unwrap();
            assert_eq!(model_err.exit_code(), 1);
        });
    }

    #[test]
    fn policy_models_pull_deny_exits_9_without_marker() {
        with_isolated("policy_deny", &[("qwen3:8b", "ollama")], |tmp| {
            write_policy(tmp, "version = 1\n[models]\npull = \"deny\"\n");
            let backend = CountingPull::new(MockOllama::default());
            let err =
                pull_models_with_backend(None, &backend, &backend, &mut |_, _| {}).unwrap_err();
            let model_err = err.downcast_ref::<ModelError>().unwrap();
            assert_eq!(model_err.exit_code(), 9);
            assert_eq!(
                model_err.to_string(),
                "Policy denied: model-pull actions are denied by policy"
            );
            assert_eq!(model_err.to_string().matches("Policy denied").count(), 1);
            assert_eq!(backend.inspect_count(), 0);
            assert_eq!(backend.pull_count(), 0);
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn policy_network_deny_blocks_pull_without_marker() {
        with_isolated("policy_net", &[("qwen3:8b", "ollama")], |tmp| {
            write_policy(tmp, "version = 1\n[resources]\nnetwork = \"deny\"\n");
            let backend = CountingPull::new(MockOllama::default());
            let err =
                pull_models_with_backend(None, &backend, &backend, &mut |_, _| {}).unwrap_err();
            assert_eq!(err.downcast_ref::<ModelError>().unwrap().exit_code(), 9);
            assert_eq!(backend.pull_count(), 0);
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn policy_packages_deny_does_not_block_pull() {
        with_isolated("policy_pkg_deny", &[("qwen3:8b", "ollama")], |tmp| {
            write_policy(
                tmp,
                "version = 1\n[packages]\ndeny = [\"qwen3:8b\"]\nallow = [\"ripgrep\"]\n",
            );
            let digest = sha(HEX64);
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]),
            );
            let report = pull_now(&backend);
            assert_eq!(report.results[0].verb, PullVerb::VerifiedAndLocked);
            assert_eq!(models_pull_exit_code(&report), 0);
        });
    }

    #[test]
    fn policy_missing_models_section_allows_pull() {
        with_isolated("policy_old", &[("qwen3:8b", "ollama")], |tmp| {
            write_policy(tmp, "version = 1\n[packages]\ninstall = \"allow\"\n");
            let digest = sha(HEX64);
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]),
            );
            let report = pull_now(&backend);
            assert_eq!(report.results[0].verb, PullVerb::VerifiedAndLocked);
        });
    }

    #[test]
    fn marker_live_pid_exits_1_and_does_not_replace() {
        with_isolated("marker_live", &[("qwen3:8b", "ollama")], |tmp| {
            let original = serde_json::json!({
                "name": "held",
                "started_at": "2026-09-01T00:00:00Z",
                "pid": std::process::id()
            });
            std::fs::write(
                tmp.join("model-pull.json"),
                serde_json::to_vec_pretty(&original).unwrap(),
            )
            .unwrap();
            let backend = CountingPull::new(MockOllama::default());
            let err =
                pull_models_with_backend(None, &backend, &backend, &mut |_, _| {}).unwrap_err();
            let model_err = err.downcast_ref::<ModelError>().unwrap();
            assert_eq!(model_err.exit_code(), 1);
            assert!(matches!(model_err, ModelError::PullInProgress { .. }));
            assert_eq!(backend.inspect_count(), 0);
            assert_eq!(backend.pull_count(), 0);
            let kept: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(tmp.join("model-pull.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(kept["name"], "held");
        });
    }

    #[test]
    fn marker_stale_pid_recovers_and_clears() {
        with_isolated("marker_stale", &[("qwen3:8b", "ollama")], |tmp| {
            write_live_marker(tmp, u32::MAX);
            let digest = sha(HEX64);
            let backend = CountingPull::new(
                MockOllama::default().with_models(vec![listed("qwen3:8b", Some(&digest))]),
            );
            let report = pull_now(&backend);
            assert_eq!(report.results[0].verb, PullVerb::VerifiedAndLocked);
            assert!(!tmp.join("model-pull.json").exists());
        });
    }

    #[test]
    fn untagged_name_posts_latest() {
        with_isolated("untagged", &[("qwen3", "ollama")], |_tmp| {
            let digest = sha(HEX64);
            let backend = MockOllama::default()
                .with_models_after_pull(vec![listed("qwen3:latest", Some(&digest))]);
            let report = pull_now(&backend);
            assert_eq!(report.results[0].verb, PullVerb::PulledAndVerified);
            let body = backend.captured_pull_body().unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(parsed["model"], "qwen3:latest");
            assert!(parsed.get("insecure").is_none());
        });
    }

    #[test]
    fn quote_in_name_pull_escapes_json_body() {
        with_isolated("quote_name", &[("qwen\"3", "ollama")], |_tmp| {
            let digest = sha(HEX64);
            let backend = MockOllama::default()
                .with_models_after_pull(vec![listed("qwen\"3:latest", Some(&digest))]);
            let report = pull_now(&backend);
            assert_eq!(report.results[0].verb, PullVerb::PulledAndVerified);
            let body = backend.captured_pull_body().unwrap();
            assert!(!body.contains("insecure"));
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(parsed["model"], "qwen\"3:latest");
        });
    }

    #[test]
    fn honesty_flags_always_serialized_false() {
        with_isolated("honesty", &[("qwen3:8b", "ollama")], |_tmp| {
            let backend = MockOllama::default().with_pull_status(404);
            let report = pull_now(&backend);
            let encoded = serde_json::to_string(&report).unwrap();
            assert!(
                encoded.contains("\"models_restored\":false")
                    || encoded.contains("\"models_restored\": false")
            );
            assert!(
                encoded.contains("\"model_weights_deleted\":false")
                    || encoded.contains("\"model_weights_deleted\": false")
            );
            assert!(!encoded.contains("\"verb\":\"restored\""));
        });
    }

    #[test]
    fn product_code_has_no_delete_api() {
        let needle = format!("/api/{}", "delete");
        let src = include_str!("models.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(!prod.contains(&needle));
        let ollama = include_str!("ollama.rs");
        let ollama_prod = ollama.split("#[cfg(test)]").next().unwrap();
        assert!(!ollama_prod.contains(&needle));
        assert!(!ollama_prod.contains("fn delete"));
    }
}
