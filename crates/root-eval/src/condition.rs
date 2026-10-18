//! Condition overlays and declared-delta enforcement (Milestone 1, Phase B).
//!
//! An experiment condition differs from its baseline by a *declared* file
//! overlay: the A/B criterion for Milestone 1 is that the two
//! [`EffectiveContextManifest`]s differ **only** by the declared skill files
//! and their metadata. This module makes that criterion mechanical:
//!
//! - [`ConditionOverlay`] is the declared file set: per path token, `add`,
//!   `modify`, or `remove`, with the expected post-change SHA-256 for
//!   additions and modifications (and an optional pre-change digest for
//!   removals);
//! - [`apply_overlay`] validates the declaration against the base manifest and
//!   produces a [`DeltaExpectation`];
//! - [`DeltaExpectation::verify`] checks a treatment manifest against the
//!   expectation: every difference from the base must be covered by a declared
//!   path, and every declared path must actually differ in the declared way;
//! - [`verify_declared_delta`] composes the two, returning the A/B audit
//!   verdict.
//!
//! # What counts as a difference
//!
//! Both manifests are flattened into records keyed by element identity:
//! `instruction|<token>`, `skill|<token>`, `error|<token>`,
//! `warning|<name>|<overridden>|<winner>`, plus the `flags` and
//! `builtin_skill_names` pseudo-records. A record's value is its canonical
//! projection with the pure order index `precedence` excluded (the *set* of
//! instructions is still compared, so selection ripples are caught). Every
//! difference must name at least one declared token; every declared token must
//! have at least one difference. Warning records reference the overridden and
//! winning tokens and are covered when either side is declared.
//!
//! Digests are only checkable where the manifest records them (instruction,
//! skill, and discovery-error records all carry the file SHA-256). A declared
//! add/modify whose treatment record has no digest is an outcome mismatch, not
//! a silent pass.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::context::{validate_canonical_token, EffectiveContextManifest};
use crate::digest::is_canonical_sha256_digest;

/// What an overlay entry does to the declared path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayAction {
    /// The path must be absent from the base manifest and present in the
    /// treatment with the declared digest.
    Add,
    /// The path must be present in the base manifest and change to the
    /// declared digest in the treatment.
    Modify,
    /// The path must be present in the base manifest and absent from the
    /// treatment.
    Remove,
}

impl OverlayAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Modify => "modify",
            Self::Remove => "remove",
        }
    }
}

/// One declared path change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayEntry {
    /// Canonical root-relative token (`home:…`, `project:…`, `abs:…`); the
    /// same token the manifest records for the path.
    pub token: String,
    /// Declared action.
    pub action: OverlayAction,
    /// Expected post-change file digest for `add`/`modify`
    /// (`sha256:<64 lowercase hex>`). Optional for `remove`, where it pins the
    /// base digest being removed.
    pub sha256: Option<String>,
}

/// The declared file set of a condition overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionOverlay {
    /// Declared changes; token-unique.
    pub entries: Vec<OverlayEntry>,
}

/// A flat manifest record used for base/treatment comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlatRecord {
    /// Canonical projection of the record with order-only fields removed.
    value: String,
    /// File digest when the record carries one.
    digest: Option<String>,
    /// Manifest tokens the record is keyed by (two for warning records).
    tokens: Vec<String>,
}

/// Machine-matchable classification for every [`DeltaError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaErrorKind {
    /// An overlay token is not a canonical token.
    OverlayPathInvalid,
    /// The same token is declared twice.
    DuplicateOverlayEntry,
    /// Declared `add` for a path the base manifest already references.
    DeclaredAddAlreadyPresent,
    /// Declared `modify` for a path the base manifest does not reference.
    DeclaredModifyAbsent,
    /// Declared `remove` for a path the base manifest does not reference.
    DeclaredRemoveAbsent,
    /// Declared `modify` whose digest already matches the base.
    DeclaredNoOpChange,
    /// Declared `remove` digest does not match the base record.
    DeclaredRemoveDigestMismatch,
    /// A base/treatment difference not covered by any declared token.
    UndeclaredDifference,
    /// A declared token with no corresponding base/treatment difference.
    MissingDeclaredChange,
    /// The treatment record's shape contradicts the declared action.
    DeclaredOutcomeMismatch,
    /// The treatment record digest differs from the declared digest.
    DeclaredDigestMismatch,
    /// A canonical serialization failed.
    Canonical,
}

/// Errors produced while applying or verifying a condition overlay.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeltaError {
    #[error("invalid overlay path `{path}`: {reason}")]
    OverlayPathInvalid { path: String, reason: String },

    #[error("duplicate overlay entry for `{path}`")]
    DuplicateOverlayEntry { path: String },

    #[error("declared add for `{path}` but the base manifest already references it")]
    DeclaredAddAlreadyPresent { path: String },

    #[error("declared modify for `{path}` but the base manifest does not reference it")]
    DeclaredModifyAbsent { path: String },

    #[error("declared remove for `{path}` but the base manifest does not reference it")]
    DeclaredRemoveAbsent { path: String },

    #[error("declared modify for `{path}` is a no-op: base digest already equals {declared}")]
    DeclaredNoOpChange { path: String, declared: String },

    #[error(
        "declared remove digest mismatch for `{path}`: declared {declared}, base records {found}"
    )]
    DeclaredRemoveDigestMismatch {
        path: String,
        declared: String,
        found: String,
    },

    #[error("undeclared difference at `{path}`: {detail}")]
    UndeclaredDifference { path: String, detail: String },

    #[error("declared change for `{path}` is missing in the treatment manifest")]
    MissingDeclaredChange { path: String },

    #[error("declared {expected} for `{path}` but treatment records {found}")]
    DeclaredOutcomeMismatch {
        path: String,
        expected: &'static str,
        found: String,
    },

    #[error(
        "declared digest mismatch for `{path}`: declared {declared}, treatment records {found}"
    )]
    DeclaredDigestMismatch {
        path: String,
        declared: String,
        found: String,
    },

    #[error("canonical serialization failed: {message}")]
    Canonical { message: String },
}

impl DeltaError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> DeltaErrorKind {
        match self {
            Self::OverlayPathInvalid { .. } => DeltaErrorKind::OverlayPathInvalid,
            Self::DuplicateOverlayEntry { .. } => DeltaErrorKind::DuplicateOverlayEntry,
            Self::DeclaredAddAlreadyPresent { .. } => DeltaErrorKind::DeclaredAddAlreadyPresent,
            Self::DeclaredModifyAbsent { .. } => DeltaErrorKind::DeclaredModifyAbsent,
            Self::DeclaredRemoveAbsent { .. } => DeltaErrorKind::DeclaredRemoveAbsent,
            Self::DeclaredNoOpChange { .. } => DeltaErrorKind::DeclaredNoOpChange,
            Self::DeclaredRemoveDigestMismatch { .. } => {
                DeltaErrorKind::DeclaredRemoveDigestMismatch
            }
            Self::UndeclaredDifference { .. } => DeltaErrorKind::UndeclaredDifference,
            Self::MissingDeclaredChange { .. } => DeltaErrorKind::MissingDeclaredChange,
            Self::DeclaredOutcomeMismatch { .. } => DeltaErrorKind::DeclaredOutcomeMismatch,
            Self::DeclaredDigestMismatch { .. } => DeltaErrorKind::DeclaredDigestMismatch,
            Self::Canonical { .. } => DeltaErrorKind::Canonical,
        }
    }
}

/// Validated expectation produced by [`apply_overlay`].
#[derive(Debug, Clone)]
pub struct DeltaExpectation {
    base: BTreeMap<String, FlatRecord>,
    changes: Vec<OverlayEntry>,
}

impl DeltaExpectation {
    /// The declared tokens, in declaration order.
    pub fn declared_tokens(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().map(|entry| entry.token.as_str())
    }

    /// Check `treatment` against the expectation.
    pub fn verify(&self, treatment: &EffectiveContextManifest) -> Result<(), DeltaError> {
        let treatment_flat = flatten(treatment)?;
        let declared: BTreeSet<&str> = self
            .changes
            .iter()
            .map(|entry| entry.token.as_str())
            .collect();
        let mut observed: BTreeSet<&str> = BTreeSet::new();

        let keys: BTreeSet<&String> = self.base.keys().chain(treatment_flat.keys()).collect();
        for key in keys {
            let base_record = self.base.get(key);
            let treatment_record = treatment_flat.get(key);
            if base_record == treatment_record {
                continue;
            }
            let record = treatment_record.or(base_record);
            let tokens: &[String] = record.map(|record| record.tokens.as_slice()).unwrap_or(&[]);
            let covered: Vec<&str> = tokens
                .iter()
                .map(String::as_str)
                .filter(|token| declared.contains(token))
                .collect();
            if covered.is_empty() {
                let path = if tokens.is_empty() {
                    key.clone()
                } else {
                    tokens.join(",")
                };
                return Err(DeltaError::UndeclaredDifference {
                    path,
                    detail: describe_difference(key, base_record, treatment_record),
                });
            }
            for token in covered {
                observed.insert(token);
            }
        }

        for entry in &self.changes {
            if !observed.contains(entry.token.as_str()) {
                return Err(DeltaError::MissingDeclaredChange {
                    path: entry.token.clone(),
                });
            }
            match entry.action {
                OverlayAction::Add | OverlayAction::Modify => {
                    let declared_digest = entry
                        .sha256
                        .as_deref()
                        .expect("apply_overlay requires a digest for add/modify");
                    let Some(record) = primary_record(&treatment_flat, &entry.token) else {
                        return Err(DeltaError::DeclaredOutcomeMismatch {
                            path: entry.token.clone(),
                            expected: entry.action.as_str(),
                            found: "no record in treatment".to_string(),
                        });
                    };
                    let Some(found) = record.digest.as_deref() else {
                        return Err(DeltaError::DeclaredOutcomeMismatch {
                            path: entry.token.clone(),
                            expected: entry.action.as_str(),
                            found: "a record without a digest".to_string(),
                        });
                    };
                    if found != declared_digest {
                        return Err(DeltaError::DeclaredDigestMismatch {
                            path: entry.token.clone(),
                            declared: declared_digest.to_string(),
                            found: found.to_string(),
                        });
                    }
                }
                OverlayAction::Remove => {
                    if primary_record(&treatment_flat, &entry.token).is_some() {
                        return Err(DeltaError::DeclaredOutcomeMismatch {
                            path: entry.token.clone(),
                            expected: "absence",
                            found: "a record still present".to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// Validate a declared overlay against the base manifest and build the
/// expectation a treatment manifest must satisfy.
pub fn apply_overlay(
    base: &EffectiveContextManifest,
    overlay: &ConditionOverlay,
) -> Result<DeltaExpectation, DeltaError> {
    let base_flat = flatten(base)?;
    let mut changes: Vec<OverlayEntry> = Vec::new();

    for entry in &overlay.entries {
        validate_canonical_token(&entry.token).map_err(|reason| {
            DeltaError::OverlayPathInvalid {
                path: entry.token.clone(),
                reason,
            }
        })?;
        if changes.iter().any(|existing| existing.token == entry.token) {
            return Err(DeltaError::DuplicateOverlayEntry {
                path: entry.token.clone(),
            });
        }

        match entry.action {
            OverlayAction::Add | OverlayAction::Modify => {
                let Some(declared) = entry.sha256.as_deref() else {
                    return Err(DeltaError::OverlayPathInvalid {
                        path: entry.token.clone(),
                        reason: format!("`{}` requires a sha256", entry.action.as_str()),
                    });
                };
                if !is_canonical_sha256_digest(declared) {
                    return Err(DeltaError::OverlayPathInvalid {
                        path: entry.token.clone(),
                        reason: "sha256 must be `sha256:<64 lowercase hex>`".to_string(),
                    });
                }
                match entry.action {
                    OverlayAction::Add => {
                        if primary_record(&base_flat, &entry.token).is_some() {
                            return Err(DeltaError::DeclaredAddAlreadyPresent {
                                path: entry.token.clone(),
                            });
                        }
                    }
                    OverlayAction::Modify => {
                        let Some(record) = primary_record(&base_flat, &entry.token) else {
                            return Err(DeltaError::DeclaredModifyAbsent {
                                path: entry.token.clone(),
                            });
                        };
                        if record.digest.as_deref() == Some(declared) {
                            return Err(DeltaError::DeclaredNoOpChange {
                                path: entry.token.clone(),
                                declared: declared.to_string(),
                            });
                        }
                    }
                    OverlayAction::Remove => unreachable!("outer match excludes remove"),
                }
            }
            OverlayAction::Remove => {
                if let Some(declared) = entry.sha256.as_deref() {
                    if !is_canonical_sha256_digest(declared) {
                        return Err(DeltaError::OverlayPathInvalid {
                            path: entry.token.clone(),
                            reason: "sha256 must be `sha256:<64 lowercase hex>`".to_string(),
                        });
                    }
                }
                let Some(record) = primary_record(&base_flat, &entry.token) else {
                    return Err(DeltaError::DeclaredRemoveAbsent {
                        path: entry.token.clone(),
                    });
                };
                if let Some(declared) = entry.sha256.as_deref() {
                    match record.digest.as_deref() {
                        Some(found) if found == declared => {}
                        Some(found) => {
                            return Err(DeltaError::DeclaredRemoveDigestMismatch {
                                path: entry.token.clone(),
                                declared: declared.to_string(),
                                found: found.to_string(),
                            })
                        }
                        None => {
                            return Err(DeltaError::DeclaredRemoveDigestMismatch {
                                path: entry.token.clone(),
                                declared: declared.to_string(),
                                found: "a record without a digest".to_string(),
                            })
                        }
                    }
                }
            }
        }
        changes.push(entry.clone());
    }

    Ok(DeltaExpectation {
        base: base_flat,
        changes,
    })
}

/// The Milestone 1 A/B audit check: `treatment` may differ from `base` only by
/// exactly the overlay's declared paths and digests.
pub fn verify_declared_delta(
    base: &EffectiveContextManifest,
    treatment: &EffectiveContextManifest,
    overlay: &ConditionOverlay,
) -> Result<(), DeltaError> {
    apply_overlay(base, overlay)?.verify(treatment)
}

fn primary_record<'a>(
    flat: &'a BTreeMap<String, FlatRecord>,
    token: &str,
) -> Option<&'a FlatRecord> {
    ["instruction", "skill", "error"]
        .iter()
        .find_map(|kind| flat.get(&format!("{kind}|{token}")))
}

fn describe_difference(
    key: &str,
    base: Option<&FlatRecord>,
    treatment: Option<&FlatRecord>,
) -> String {
    match (base, treatment) {
        (None, Some(_)) => format!("`{key}` is present in treatment but absent from base"),
        (Some(_), None) => format!("`{key}` is present in base but absent from treatment"),
        (Some(_), Some(_)) => format!("`{key}` differs between base and treatment"),
        (None, None) => format!("`{key}` is absent from both manifests"),
    }
}

fn flatten(
    manifest: &EffectiveContextManifest,
) -> Result<BTreeMap<String, FlatRecord>, DeltaError> {
    let projection: Value =
        serde_json::from_slice(manifest.canonical_bytes()).map_err(|error| {
            DeltaError::Canonical {
                message: format!("manifest projection is not JSON: {error}"),
            }
        })?;

    let mut flat: BTreeMap<String, FlatRecord> = BTreeMap::new();

    for record in records(&projection, "instructions") {
        let token = string_field(record, "token")?;
        let mut value = record.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("token");
            object.remove("precedence");
        }
        flat.insert(
            format!("instruction|{token}"),
            FlatRecord {
                value: canonical_value_string(&value)?,
                digest: record
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tokens: vec![token],
            },
        );
    }

    for record in records(&projection, "skills") {
        let token = string_field(record, "token")?;
        let mut value = record.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("token");
        }
        flat.insert(
            format!("skill|{token}"),
            FlatRecord {
                value: canonical_value_string(&value)?,
                digest: record
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tokens: vec![token],
            },
        );
    }

    for record in records(&projection, "discovery_errors") {
        let token = string_field(record, "token")?;
        let mut value = record.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("token");
        }
        flat.insert(
            format!("error|{token}"),
            FlatRecord {
                value: canonical_value_string(&value)?,
                digest: record
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tokens: vec![token],
            },
        );
    }

    for record in records(&projection, "discovery_warnings") {
        let name = string_field(record, "name")?;
        let overridden = string_field(record, "overridden_token")?;
        let winner = string_field(record, "winner_token")?;
        flat.insert(
            format!("warning|{name}|{overridden}|{winner}"),
            FlatRecord {
                value: String::new(),
                digest: None,
                tokens: vec![overridden, winner],
            },
        );
    }

    if let Some(flags) = projection.get("flags") {
        flat.insert(
            "flags".to_string(),
            FlatRecord {
                value: canonical_value_string(flags)?,
                digest: None,
                tokens: Vec::new(),
            },
        );
    }
    if let Some(builtins) = projection.get("builtin_skill_names") {
        flat.insert(
            "builtin_skill_names".to_string(),
            FlatRecord {
                value: canonical_value_string(builtins)?,
                digest: None,
                tokens: Vec::new(),
            },
        );
    }

    Ok(flat)
}

fn records<'a>(projection: &'a Value, key: &str) -> &'a [Value] {
    projection
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn string_field(record: &Value, key: &str) -> Result<String, DeltaError> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| DeltaError::Canonical {
            message: format!("manifest record is missing string field `{key}`"),
        })
}

fn canonical_value_string(value: &Value) -> Result<String, DeltaError> {
    let bytes = canonical_json_bytes(value).map_err(|message| DeltaError::Canonical { message })?;
    String::from_utf8(bytes).map_err(|_| DeltaError::Canonical {
        message: "canonical record bytes are not UTF-8".to_string(),
    })
}
