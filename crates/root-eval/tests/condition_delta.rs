//! Condition-overlay enforcement tests against the A/B fixture trees.
//!
//! The audit's Milestone 1 criterion: the two condition manifests may differ
//! only by the declared skill files/metadata. These tests exercise both the
//! happy path (declared add/modify/remove) and every fail-closed branch.

mod context_support;

use context_support::{case_dir, load_overlay};
use root_eval::{
    apply_overlay, sha256_digest, verify_declared_delta, ConditionOverlay, ContextRoots,
    DeltaErrorKind, DiscoveryFlags, EffectiveContextManifest, OverlayAction, OverlayEntry,
};

const CASE: &str = "12-ab-treatment";

fn resolve_tree(tree: &str) -> Result<EffectiveContextManifest, root_eval::ContextError> {
    let dir = case_dir(CASE).join(tree);
    let home = dir.join("home");
    let xdg = dir.join("home/config");
    let project = dir.join("project");
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &project,
    };
    root_eval::resolve_manifest(&roots, &DiscoveryFlags::default())
}

fn base() -> EffectiveContextManifest {
    resolve_tree("base").expect("base tree resolves")
}

fn treatment(tree: &str) -> EffectiveContextManifest {
    resolve_tree(tree).expect("treatment tree resolves")
}

fn overlay_add() -> ConditionOverlay {
    load_overlay(CASE, "overlay-add.json")
}

fn overlay_modify() -> ConditionOverlay {
    load_overlay(CASE, "overlay-modify.json")
}

fn overlay_remove() -> ConditionOverlay {
    load_overlay(CASE, "overlay-remove.json")
}

fn entry(action: OverlayAction, token: &str, sha256: Option<String>) -> OverlayEntry {
    OverlayEntry {
        token: token.to_string(),
        action,
        sha256,
    }
}

#[test]
fn declared_add_is_the_only_delta() {
    let base = base();
    let treatment = treatment("treatment-added");
    assert_ne!(
        base.sha256(),
        treatment.sha256(),
        "the treatment must change the context"
    );
    assert_eq!(
        verify_declared_delta(&base, &treatment, &overlay_add()),
        Ok(())
    );

    let expectation = apply_overlay(&base, &overlay_add()).expect("overlay applies");
    let declared: Vec<&str> = expectation.declared_tokens().collect();
    assert_eq!(declared, ["project:.opencode/skill/root/SKILL.md"]);
}

#[test]
fn undeclared_extra_difference_names_the_offending_path() {
    let base = base();
    let treatment = treatment("treatment-extra");
    let error = verify_declared_delta(&base, &treatment, &overlay_add())
        .expect_err("the extra skill is not declared");
    assert_eq!(error.kind(), DeltaErrorKind::UndeclaredDifference);
    assert!(
        error.to_string().contains(".opencode/skill/other/SKILL.md"),
        "error must name the undeclared path: {error}"
    );
}

#[test]
fn declared_but_absent_is_an_error() {
    let base = base();
    let error = verify_declared_delta(&base, &base, &overlay_add())
        .expect_err("the declared add is missing from the treatment");
    assert_eq!(error.kind(), DeltaErrorKind::MissingDeclaredChange);
    assert!(
        error.to_string().contains(".opencode/skill/root/SKILL.md"),
        "error must name the declared path: {error}"
    );
}

#[test]
fn modify_in_place_is_ok_only_when_declared() {
    let base = base();
    let modified = treatment("treatment-modified");
    assert_eq!(
        verify_declared_delta(&base, &modified, &overlay_modify()),
        Ok(())
    );

    // The add overlay does not declare the in-place modification.
    let error = verify_declared_delta(&base, &modified, &overlay_add())
        .expect_err("the modification is not declared");
    assert_eq!(error.kind(), DeltaErrorKind::UndeclaredDifference);
    assert!(error.to_string().contains(".opencode/skill/foo/SKILL.md"));

    // A declared modify with a stale/foreign digest is a digest mismatch.
    let wrong = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Modify,
            "project:.opencode/skill/foo/SKILL.md",
            Some(sha256_digest(b"not the treatment bytes")),
        )],
    };
    let error =
        verify_declared_delta(&base, &modified, &wrong).expect_err("wrong digest must fail");
    assert_eq!(error.kind(), DeltaErrorKind::DeclaredDigestMismatch);

    // Declaring the *base* digest as the modification target is a no-op.
    let no_op = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Modify,
            "project:.opencode/skill/foo/SKILL.md",
            Some(base.skills()[0].sha256.clone()),
        )],
    };
    let error = apply_overlay(&base, &no_op).expect_err("no-op refused");
    assert_eq!(error.kind(), DeltaErrorKind::DeclaredNoOpChange);
}

#[test]
fn declared_remove_is_ok_only_when_declared() {
    let base = base();
    let removed = treatment("treatment-removed");
    assert_eq!(
        verify_declared_delta(&base, &removed, &overlay_remove()),
        Ok(())
    );

    // Declaring the removal against an unchanged treatment is a missing
    // declared change, not a pass.
    let error = verify_declared_delta(&base, &base, &overlay_remove())
        .expect_err("the declared removal did not happen");
    assert_eq!(error.kind(), DeltaErrorKind::MissingDeclaredChange);
    assert!(error.to_string().contains(".opencode/skill/foo/SKILL.md"));
}

#[test]
fn declared_add_must_match_the_treatment_digest() {
    let base = base();
    let added = treatment("treatment-added");
    let wrong = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Add,
            "project:.opencode/skill/root/SKILL.md",
            Some(sha256_digest(b"different bytes")),
        )],
    };
    let error = verify_declared_delta(&base, &added, &wrong).expect_err("digest mismatch");
    assert_eq!(error.kind(), DeltaErrorKind::DeclaredDigestMismatch);
    assert!(error.to_string().contains("root/SKILL.md"));
}

#[test]
fn overlay_validation_is_fail_closed() {
    let base = base();

    // `add` for a path the base already references.
    let already_present = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Add,
            "project:.opencode/skill/foo/SKILL.md",
            Some(sha256_digest(b"anything")),
        )],
    };
    assert_eq!(
        apply_overlay(&base, &already_present)
            .expect_err("add-existing refused")
            .kind(),
        DeltaErrorKind::DeclaredAddAlreadyPresent
    );

    // `modify` for a path the base does not reference.
    let absent = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Modify,
            "project:.opencode/skill/missing/SKILL.md",
            Some(sha256_digest(b"anything")),
        )],
    };
    assert_eq!(
        apply_overlay(&base, &absent)
            .expect_err("modify-absent refused")
            .kind(),
        DeltaErrorKind::DeclaredModifyAbsent
    );

    // `remove` for a path the base does not reference.
    let absent = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Remove,
            "project:.opencode/skill/missing/SKILL.md",
            None,
        )],
    };
    assert_eq!(
        apply_overlay(&base, &absent)
            .expect_err("remove-absent refused")
            .kind(),
        DeltaErrorKind::DeclaredRemoveAbsent
    );

    // `remove` with a stale digest.
    let stale = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Remove,
            "project:.opencode/skill/foo/SKILL.md",
            Some(sha256_digest(b"stale")),
        )],
    };
    assert_eq!(
        apply_overlay(&base, &stale)
            .expect_err("stale remove digest refused")
            .kind(),
        DeltaErrorKind::DeclaredRemoveDigestMismatch
    );

    // Duplicate tokens.
    let duplicate = ConditionOverlay {
        entries: vec![
            entry(
                OverlayAction::Add,
                "project:.opencode/skill/root/SKILL.md",
                Some(sha256_digest(b"a")),
            ),
            entry(
                OverlayAction::Remove,
                "project:.opencode/skill/root/SKILL.md",
                None,
            ),
        ],
    };
    assert_eq!(
        apply_overlay(&base, &duplicate)
            .expect_err("duplicate refused")
            .kind(),
        DeltaErrorKind::DuplicateOverlayEntry
    );

    // Non-canonical tokens.
    let traversal = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Add,
            "project:../../escape/SKILL.md",
            Some(sha256_digest(b"a")),
        )],
    };
    assert_eq!(
        apply_overlay(&base, &traversal)
            .expect_err("traversal refused")
            .kind(),
        DeltaErrorKind::OverlayPathInvalid
    );

    // Missing/!canonical digest for add.
    let missing_digest = ConditionOverlay {
        entries: vec![entry(
            OverlayAction::Add,
            "project:.opencode/skill/root/SKILL.md",
            None,
        )],
    };
    assert_eq!(
        apply_overlay(&base, &missing_digest)
            .expect_err("digest required")
            .kind(),
        DeltaErrorKind::OverlayPathInvalid
    );
}
