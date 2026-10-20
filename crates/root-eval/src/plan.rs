//! Experiment spec and canonical plan hash (Milestone 1, Phase B).
//!
//! [`ExperimentSpec`] is the minimal immutable description of a planned
//! experiment: benchmark task refs (identity, version, and sealed content
//! digest), conditions (id, the expected effective-context manifest hash, and
//! whether an overlay is present), the harness lock, the repository lock, and
//! a caller-supplied `created_at` string.
//!
//! The plan hash is SHA-256 over the canonical JSON projection of the spec
//! (see [`crate::canonical_json`]):
//!
//! - task refs are sorted by `(task_id, version)`, and conditions by `id`, so
//!   declaration order never changes the hash;
//! - the projection carries the constant hash domain
//!   ([`EXPERIMENT_HASH_DOMAIN`]);
//! - nothing is read from the environment, the clock, or the filesystem —
//!   `created_at` is an explicit field and the only time-like input.
//!
//! Validation is fail-closed: unsupported schema versions, empty identity
//! fields, malformed digests, unsupported harness locks, and duplicate task
//! or condition identities are refused before any hashing happens.
//!
//! # Documented M5 relaxations (feasibility-only)
//!
//! Two additive relaxations support the Milestone 5 deterministic fake-agent
//! feasibility matrix (`crate::feasibility`) without changing the default
//! behavior:
//!
//! 1. the harness name [`FEASIBILITY_HARNESS_NAME`] (`m5-fake-agent`) is
//!    accepted alongside [`PINNED_HARNESS_NAME`] (`opencode`); any other name
//!    is still refused;
//! 2. `repository.commit` may be a generated-fixture marker of the form
//!    `generated:sha256:<64 lowercase hex>` (the marker
//!    [`GENERATED_COMMIT_PREFIX`] introduces) instead of a 40-hex Git commit.
//!    Every other value must be exactly 40 lowercase hex characters.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::canonical_json::canonical_json_bytes;
use crate::digest::is_canonical_sha256_digest;
use crate::eval_error::EvalError;

/// Experiment-spec schema version emitted and accepted by this crate.
pub const EXPERIMENT_SPEC_SCHEMA_VERSION: u32 = 1;

/// Highest experiment-spec schema version this crate can read.
pub const EXPERIMENT_SPEC_MAX_SUPPORTED_VERSION: u32 = 1;

/// Constant hash domain mixed into the plan projection.
pub const EXPERIMENT_HASH_DOMAIN: &str = "experiment-spec.v1";

/// The only harness this milestone pins.
pub const PINNED_HARNESS_NAME: &str = "opencode";

/// Feasibility-only harness name for the deterministic fake-agent matrix.
///
/// Accepted alongside [`PINNED_HARNESS_NAME`]; documented in the module docs.
pub const FEASIBILITY_HARNESS_NAME: &str = "m5-fake-agent";

/// Prefix of a `RepositoryLock::commit` marker for generated-fixture
/// experiments: `generated:sha256:<64 lowercase hex>`.
///
/// Generated fixtures have no Git commit to pin; the marker binds the
/// combined tree digest instead. Every commit value without this prefix must
/// be exactly 40 lowercase hex characters (a Git object id).
pub const GENERATED_COMMIT_PREFIX: &str = "generated:";

/// A benchmark task pinned into the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkTaskRef {
    /// Task identifier (`[a-z0-9-]` grammar enforced by the task record itself).
    pub task_id: String,
    /// Task record version.
    pub version: u64,
    /// Sealed task-record content digest (`sha256:<64 lowercase hex>`).
    pub content_sha256: String,
}

/// An experiment condition pinned by its resolved context hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionSpec {
    /// Condition identifier (for example `baseline` / `treatment`).
    pub id: String,
    /// Expected `sha256:<64 lowercase hex>` of the condition's
    /// [`crate::EffectiveContextManifest`].
    pub expected_manifest_sha256: String,
    /// Whether this condition declares a file overlay over the baseline.
    pub overlay_present: bool,
}

/// Harness identity and artifact lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessLock {
    /// Harness name; must be [`PINNED_HARNESS_NAME`].
    pub name: String,
    /// Exact harness version string.
    pub version: String,
    /// SHA-256 of the harness binary artifact.
    pub binary_sha256: String,
}

/// Repository identity lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryLock {
    /// Commit the trials will be checked out at.
    pub commit: String,
    /// SHA-256 of the checked-out tree.
    pub tree_sha256: String,
}

/// The minimal Milestone 1 experiment spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperimentSpec {
    /// Spec schema version; must equal [`EXPERIMENT_SPEC_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Stable experiment identifier.
    pub experiment_id: String,
    /// Benchmark tasks in scope (hashed in canonical order).
    pub tasks: Vec<BenchmarkTaskRef>,
    /// Conditions in scope (hashed in canonical order).
    pub conditions: Vec<ConditionSpec>,
    /// Harness lock.
    pub harness: HarnessLock,
    /// Repository lock.
    pub repository: RepositoryLock,
    /// Explicit creation timestamp string; never injected by this crate.
    pub created_at: String,
    /// Driver-level wall-time cap for one trial, in seconds.
    ///
    /// **Optional and additive.** `None` means "no driver cap": every trial
    /// used its task record's own `budgets.wall_time_seconds_max`. A value
    /// caps the trial wall time from above — the **effective** cap is
    /// `min(record budget, driver cap)`.
    ///
    /// The field is part of the plan because it changes trial semantics: a
    /// run with a driver cap is not comparable with a run without one, so
    /// the cap is hashed into the plan hash. An absent cap is omitted from
    /// the hash projection, so the default plan hash is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_timeout_seconds: Option<u64>,
}

/// Compute the canonical `sha256:<hex>` plan hash of `spec`.
///
/// Deterministic: the same spec value always hashes identically, declaration
/// order of tasks/conditions is not part of the hash, and no environment,
/// clock, or filesystem input exists.
pub fn compute_plan_hash(spec: &ExperimentSpec) -> Result<String, EvalError> {
    validate_spec(spec)?;
    let projection = plan_projection(spec);
    let bytes = canonical_json_bytes(&projection)
        .map_err(|message| EvalError::Canonicalization { message })?;
    Ok(format!("sha256:{}", root_lockfile::compute_sha256(&bytes)))
}

/// Verify that `spec` hashes to `expected_hash`.
pub fn verify_plan(spec: &ExperimentSpec, expected_hash: &str) -> Result<(), EvalError> {
    if !is_canonical_sha256_digest(expected_hash) {
        return Err(EvalError::DigestInvalid {
            field: "expected_plan_hash".to_string(),
            value: expected_hash.to_string(),
        });
    }
    let found = compute_plan_hash(spec)?;
    if found != expected_hash {
        return Err(EvalError::PlanHashMismatch {
            expected: expected_hash.to_string(),
            found,
        });
    }
    Ok(())
}

fn validate_spec(spec: &ExperimentSpec) -> Result<(), EvalError> {
    if spec.schema_version != EXPERIMENT_SPEC_SCHEMA_VERSION {
        return Err(EvalError::SchemaVersion {
            found: spec.schema_version,
            supported: EXPERIMENT_SPEC_SCHEMA_VERSION,
        });
    }
    require_non_empty(&spec.experiment_id, "experiment_id")?;
    require_non_empty(&spec.created_at, "created_at")?;
    if spec.tasks.is_empty() {
        return Err(EvalError::EmptyField {
            field: "tasks".to_string(),
        });
    }
    if spec.conditions.is_empty() {
        return Err(EvalError::EmptyField {
            field: "conditions".to_string(),
        });
    }

    if spec.harness.name != PINNED_HARNESS_NAME && spec.harness.name != FEASIBILITY_HARNESS_NAME {
        return Err(EvalError::UnsupportedHarness {
            name: spec.harness.name.clone(),
        });
    }
    require_non_empty(&spec.harness.version, "harness.version")?;
    require_digest(&spec.harness.binary_sha256, "harness.binary_sha256")?;
    require_non_empty(&spec.repository.commit, "repository.commit")?;
    require_repository_commit(&spec.repository.commit)?;
    require_digest(&spec.repository.tree_sha256, "repository.tree_sha256")?;

    let mut task_keys: Vec<(&str, u64)> = Vec::new();
    for task in &spec.tasks {
        require_non_empty(&task.task_id, "tasks[].task_id")?;
        require_digest(&task.content_sha256, "tasks[].content_sha256")?;
        let key = (task.task_id.as_str(), task.version);
        if task_keys.contains(&key) {
            return Err(EvalError::DuplicateTaskRef {
                task_id: task.task_id.clone(),
                version: task.version,
            });
        }
        task_keys.push(key);
    }

    let mut condition_ids: Vec<&str> = Vec::new();
    for condition in &spec.conditions {
        require_non_empty(&condition.id, "conditions[].id")?;
        require_digest(
            &condition.expected_manifest_sha256,
            "conditions[].expected_manifest_sha256",
        )?;
        if condition_ids.contains(&condition.id.as_str()) {
            return Err(EvalError::DuplicateConditionId {
                id: condition.id.clone(),
            });
        }
        condition_ids.push(&condition.id);
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), EvalError> {
    if value.is_empty() {
        Err(EvalError::EmptyField {
            field: field.to_string(),
        })
    } else {
        Ok(())
    }
}

fn require_digest(value: &str, field: &str) -> Result<(), EvalError> {
    if is_canonical_sha256_digest(value) {
        Ok(())
    } else {
        Err(EvalError::DigestInvalid {
            field: field.to_string(),
            value: value.to_string(),
        })
    }
}

/// Accept a 40-hex Git commit, or the documented generated-fixture marker
/// `generated:sha256:<64 lowercase hex>` (see the module docs).
fn require_repository_commit(value: &str) -> Result<(), EvalError> {
    if let Some(digest) = value.strip_prefix(GENERATED_COMMIT_PREFIX) {
        return require_digest(digest, "repository.commit");
    }
    let is_git_commit = value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if is_git_commit {
        Ok(())
    } else {
        Err(EvalError::DigestInvalid {
            field: "repository.commit".to_string(),
            value: value.to_string(),
        })
    }
}

fn plan_projection(spec: &ExperimentSpec) -> Value {
    let mut tasks: Vec<&BenchmarkTaskRef> = spec.tasks.iter().collect();
    tasks.sort_by(|left, right| {
        (left.task_id.as_str(), left.version).cmp(&(right.task_id.as_str(), right.version))
    });
    let tasks: Vec<Value> = tasks
        .iter()
        .map(|task| {
            json!({
                "task_id": task.task_id,
                "version": task.version,
                "content_sha256": task.content_sha256,
            })
        })
        .collect();

    let mut conditions: Vec<&ConditionSpec> = spec.conditions.iter().collect();
    conditions.sort_by(|left, right| left.id.cmp(&right.id));
    let conditions: Vec<Value> = conditions
        .iter()
        .map(|condition| {
            json!({
                "id": condition.id,
                "expected_manifest_sha256": condition.expected_manifest_sha256,
                "overlay_present": condition.overlay_present,
            })
        })
        .collect();

    let mut projection = json!({
        "hash_domain": EXPERIMENT_HASH_DOMAIN,
        "schema_version": spec.schema_version,
        "experiment_id": spec.experiment_id,
        "created_at": spec.created_at,
        "harness": {
            "name": spec.harness.name,
            "version": spec.harness.version,
            "binary_sha256": spec.harness.binary_sha256,
        },
        "repository": {
            "commit": spec.repository.commit,
            "tree_sha256": spec.repository.tree_sha256,
        },
        "tasks": tasks,
        "conditions": conditions,
    });
    // An absent driver cap is **omitted** from the projection (not hashed as
    // `null`), so the default plan hash is unchanged by this additive field.
    if let Some(cap) = spec.trial_timeout_seconds {
        projection
            .as_object_mut()
            .expect("the projection is an object")
            .insert("trial_timeout_seconds".to_string(), json!(cap));
    }
    projection
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: &str) -> String {
        format!("sha256:{}", root_lockfile::compute_sha256(seed.as_bytes()))
    }

    fn base_spec() -> ExperimentSpec {
        ExperimentSpec {
            schema_version: EXPERIMENT_SPEC_SCHEMA_VERSION,
            experiment_id: "exp-root-skill-install".to_string(),
            tasks: vec![
                BenchmarkTaskRef {
                    task_id: "install-ripgrep-001".to_string(),
                    version: 1,
                    content_sha256: digest("task-a"),
                },
                BenchmarkTaskRef {
                    task_id: "install-poppler-002".to_string(),
                    version: 1,
                    content_sha256: digest("task-b"),
                },
            ],
            conditions: vec![
                ConditionSpec {
                    id: "baseline".to_string(),
                    expected_manifest_sha256: digest("manifest-base"),
                    overlay_present: false,
                },
                ConditionSpec {
                    id: "treatment".to_string(),
                    expected_manifest_sha256: digest("manifest-treatment"),
                    overlay_present: true,
                },
            ],
            harness: HarnessLock {
                name: PINNED_HARNESS_NAME.to_string(),
                version: "1.18.27".to_string(),
                binary_sha256: digest("opencode-binary"),
            },
            repository: RepositoryLock {
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                tree_sha256: digest("tree"),
            },
            created_at: "2026-10-18T00:00:00Z".to_string(),
            trial_timeout_seconds: None,
        }
    }

    #[test]
    fn plan_hash_is_stable_and_canonical() {
        let first = compute_plan_hash(&base_spec()).expect("hashes");
        let second = compute_plan_hash(&base_spec()).expect("hashes");
        assert_eq!(first, second);
        assert!(is_canonical_sha256_digest(&first));

        let mut permuted = base_spec();
        permuted.tasks.reverse();
        permuted.conditions.reverse();
        assert_eq!(
            compute_plan_hash(&permuted).expect("hashes"),
            first,
            "declaration order must not change the hash"
        );
    }

    #[test]
    fn any_pinned_input_changes_the_hash() {
        let base = compute_plan_hash(&base_spec()).expect("hashes");

        let mut changed_manifest = base_spec();
        changed_manifest.conditions[1].expected_manifest_sha256 = digest("other-manifest");
        assert_ne!(compute_plan_hash(&changed_manifest).expect("hashes"), base);

        let mut changed_task_version = base_spec();
        changed_task_version.tasks[0].version = 2;
        assert_ne!(
            compute_plan_hash(&changed_task_version).expect("hashes"),
            base
        );

        let mut changed_harness = base_spec();
        changed_harness.harness.version = "1.18.28".to_string();
        assert_ne!(compute_plan_hash(&changed_harness).expect("hashes"), base);

        let mut changed_overlay = base_spec();
        changed_overlay.conditions[1].overlay_present = false;
        assert_ne!(compute_plan_hash(&changed_overlay).expect("hashes"), base);

        let mut changed_created_at = base_spec();
        changed_created_at.created_at = "2026-10-19T00:00:00Z".to_string();
        assert_ne!(
            compute_plan_hash(&changed_created_at).expect("hashes"),
            base
        );
    }

    #[test]
    fn validation_is_fail_closed() {
        let mut spec = base_spec();
        spec.schema_version = 2;
        assert_eq!(
            compute_plan_hash(&spec)
                .expect_err("version refused")
                .kind(),
            crate::EvalErrorKind::SchemaVersion
        );

        let mut spec = base_spec();
        spec.experiment_id = String::new();
        assert_eq!(
            compute_plan_hash(&spec).expect_err("empty id").kind(),
            crate::EvalErrorKind::EmptyField
        );

        let mut spec = base_spec();
        spec.harness.name = "codex".to_string();
        assert_eq!(
            compute_plan_hash(&spec)
                .expect_err("harness refused")
                .kind(),
            crate::EvalErrorKind::UnsupportedHarness
        );

        let mut spec = base_spec();
        spec.harness.binary_sha256 = "sha256:PENDING".to_string();
        assert_eq!(
            compute_plan_hash(&spec).expect_err("digest refused").kind(),
            crate::EvalErrorKind::DigestInvalid
        );

        let mut spec = base_spec();
        spec.tasks.push(BenchmarkTaskRef {
            task_id: "install-ripgrep-001".to_string(),
            version: 1,
            content_sha256: digest("dup"),
        });
        assert_eq!(
            compute_plan_hash(&spec).expect_err("duplicate task").kind(),
            crate::EvalErrorKind::DuplicateTaskRef
        );

        let mut spec = base_spec();
        spec.conditions.push(ConditionSpec {
            id: "baseline".to_string(),
            expected_manifest_sha256: digest("dup"),
            overlay_present: false,
        });
        assert_eq!(
            compute_plan_hash(&spec)
                .expect_err("duplicate condition")
                .kind(),
            crate::EvalErrorKind::DuplicateConditionId
        );
    }

    #[test]
    fn verify_plan_checks_the_exact_hash() {
        let spec = base_spec();
        let hash = compute_plan_hash(&spec).expect("hashes");
        assert_eq!(verify_plan(&spec, &hash), Ok(()));
        assert_eq!(
            verify_plan(&spec, "not-a-digest")
                .expect_err("bad expected hash")
                .kind(),
            crate::EvalErrorKind::DigestInvalid
        );
        assert_eq!(
            verify_plan(&spec, &digest("other"))
                .expect_err("mismatch")
                .kind(),
            crate::EvalErrorKind::PlanHashMismatch
        );
    }

    #[test]
    fn feasibility_harness_name_is_accepted_alongside_the_pin() {
        let base = compute_plan_hash(&base_spec()).expect("hashes");

        let mut feasibility = base_spec();
        feasibility.harness.name = FEASIBILITY_HARNESS_NAME.to_string();
        let feasibility_hash =
            compute_plan_hash(&feasibility).expect("m5-fake-agent must be accepted");
        assert_ne!(feasibility_hash, base, "the harness name is hashed");

        let mut other = base_spec();
        other.harness.name = "codex".to_string();
        assert_eq!(
            compute_plan_hash(&other).expect_err("refused").kind(),
            crate::EvalErrorKind::UnsupportedHarness
        );
    }

    #[test]
    fn generated_commit_marker_is_accepted_and_other_commits_must_be_40_hex() {
        let mut generated = base_spec();
        generated.repository.commit = format!("{GENERATED_COMMIT_PREFIX}{}", digest("tree"));
        compute_plan_hash(&generated).expect("generated:<sha256> marker must be accepted");

        for bad in [
            format!("{GENERATED_COMMIT_PREFIX}nope"),
            format!("{GENERATED_COMMIT_PREFIX}sha256:PENDING"),
            // 39 hex characters: one short of a Git object id.
            "0123456789abcdef0123456789abcdef0123456".to_string(),
            // Uppercase hex is not a canonical Git object id here.
            "0123456789ABCDEF0123456789ABCDEF01234567".to_string(),
            "m5a-fixture".to_string(),
        ] {
            let mut spec = base_spec();
            spec.repository.commit = bad.clone();
            assert_eq!(
                compute_plan_hash(&spec).expect_err("refused").kind(),
                crate::EvalErrorKind::DigestInvalid,
                "commit `{bad}` must be refused"
            );
        }
    }
}
