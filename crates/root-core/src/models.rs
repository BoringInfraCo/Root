//! Read-only plan for declared Ollama models.
//!
//! Never POSTs, never writes the lock, and never creates model-pull.json.

use crate::get_or_create_rootfile;
use crate::inventory::{
    EvaluationState, Presence, REASON_ENDPOINT_UNREACHABLE, REASON_MALFORMED_OUTPUT,
    REASON_NOT_FOUND, REASON_NOT_SUPPORTED, REASON_PROTOCOL_UNSUPPORTED, REASON_TIMED_OUT,
};
use crate::ollama::{
    canonicalize_digest, digests_equal, is_remote_or_cloud, model_matches, resolve_model_tag,
    HttpOllama, InspectError, ListedModel, OllamaInspector, RuntimeProtocol,
    REASON_REMOTE_OR_CLOUD_UNSUPPORTED,
};
use anyhow::{Context, Result};
use root_lockfile::{get_root_dir, LockedModel, Rootfile};
use serde::Serialize;
use std::collections::BTreeMap;

const OLLAMA_RUNTIME: &str = "ollama";
const OLLAMA_ENDPOINT: &str = "127.0.0.1:11434";
const PLAN_COMMAND: &str = "plan models";
const ADDRESSABILITY: &str = "verification_record_only";
const REASON_NO_DECLARED_MODELS: &str = "no_declared_models";
const REASON_DIGEST_MISMATCH: &str = "cannot_reproduce_locked_digest";
const DOWNLOAD_STATE: &str = "unknown_until_manifest";
const DOWNLOAD_REASON: &str = "ollama_pull_does_not_expose_size_before_mutation";

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
    anyhow::anyhow!("Unknown model '{name}' is not declared in Rootfile.")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ollama::{MockOllama, RuntimeProbe};
    use root_lockfile::ModelDeclaration;
    use std::sync::Mutex;
    use std::time::Duration;

    const HEX64: &str = "c6eb81c2c3a4b5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e";
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
}
