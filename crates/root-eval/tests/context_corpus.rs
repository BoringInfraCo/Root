//! Fixture-corpus replay for the OpenCode effective-context resolver.
//!
//! Every case under `tests/fixtures/opencode-discovery/` is resolved exactly as
//! its `expected.json` declares: instruction order/role/source/precedence and
//! absolute path/digest/size, skill set with source/token/digest, recorded
//! discovery errors and warnings, built-in names, and the canonical manifest
//! hash. The corpus is the behavioral contract test for contract §4.

mod context_support;

use std::fs;

use context_support::{
    case_dir, file_list, load_expected, read_case_file, repo_skill_path, resolve_named,
    resolve_scenario, run_case,
};
use root_eval::{resolve_manifest, ContextErrorKind, ContextRoots, DiscoveryFlags};

/// Every corpus case, in numeric order.
const CASES: &[&str] = &[
    "01-global-agents-only",
    "02-global-claude-fallback",
    "03-project-walkup-nearest-agents",
    "04-level-priority",
    "05-worktree-boundary",
    "06-project-and-global",
    "07-opencode-singular-skill",
    "08-opencode-plural-skills",
    "09-cross-agent-roots",
    "10-invalid-frontmatter",
    "11-duplicate-name",
    "12-ab-treatment",
    "13-empty-nil",
    "14-skills-paths",
    "15-unsafe-explicit-path",
    "16-flags-project-config",
    "17-opencode-walkup-global",
    "18-flag-gates-external-skills",
    "19-flag-project-config-instructions",
    "20-project-instruction-multilevel",
];

macro_rules! case_test {
    ($name:ident, $case:expr) => {
        #[test]
        fn $name() {
            run_case($case);
        }
    };
}

case_test!(case_01_global_agents_only, "01-global-agents-only");
case_test!(case_02_global_claude_fallback, "02-global-claude-fallback");
case_test!(
    case_03_project_walkup_nearest_agents,
    "03-project-walkup-nearest-agents"
);
case_test!(case_04_level_priority, "04-level-priority");
case_test!(case_05_worktree_boundary, "05-worktree-boundary");
case_test!(case_06_project_and_global, "06-project-and-global");
case_test!(
    case_07_opencode_singular_skill,
    "07-opencode-singular-skill"
);
case_test!(case_08_opencode_plural_skills, "08-opencode-plural-skills");
case_test!(case_09_cross_agent_roots, "09-cross-agent-roots");
case_test!(case_10_invalid_frontmatter, "10-invalid-frontmatter");
case_test!(case_11_duplicate_name, "11-duplicate-name");
case_test!(case_12_ab_treatment, "12-ab-treatment");
case_test!(case_13_empty_nil, "13-empty-nil");
case_test!(case_14_skills_paths, "14-skills-paths");
case_test!(case_15_unsafe_explicit_path, "15-unsafe-explicit-path");
case_test!(case_16_flags_project_config, "16-flags-project-config");
case_test!(case_17_opencode_walkup_global, "17-opencode-walkup-global");
case_test!(
    case_18_flag_gates_external_skills,
    "18-flag-gates-external-skills"
);
case_test!(
    case_19_flag_project_config_instructions,
    "19-flag-project-config-instructions"
);
case_test!(
    case_20_project_instruction_multilevel,
    "20-project-instruction-multilevel"
);

#[test]
fn every_corpus_case_is_replayed() {
    let root = context_support::fixtures_root();
    let mut cases: Vec<String> = fs::read_dir(&root)
        .expect("fixture corpus is readable")
        .map(|entry| {
            entry
                .expect("fixture entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| root.join(name).join("expected.json").is_file())
        .collect();
    cases.sort();
    assert_eq!(
        cases,
        CASES
            .iter()
            .map(|case| (*case).to_string())
            .collect::<Vec<_>>(),
        "a case directory exists that is not replayed (or vice versa)"
    );
}

#[test]
fn every_manifest_hash_is_pinned() {
    for case in CASES {
        let expected = load_expected(case);
        for scenario in &expected.scenarios {
            if let Some(expect) = &scenario.expect {
                assert_ne!(
                    expect.manifest_sha256, "TBD",
                    "{case}/{}: manifest hash is still a placeholder",
                    scenario.name
                );
            }
        }
    }
}

#[test]
fn walkup_boundary_positive_resolves_the_worktree_root_file() {
    let manifest = resolve_named("05-worktree-boundary", "inside");
    assert_eq!(manifest.instructions().len(), 1);
    assert_eq!(manifest.instructions()[0].token, "worktree:AGENTS.md");
    assert_eq!(
        manifest.instructions()[0].path,
        case_dir("05-worktree-boundary").join("cases/with-inside/worktree/AGENTS.md")
    );
}

#[test]
fn walkup_boundary_negative_does_not_scan_above_the_worktree_root() {
    let manifest = resolve_named("05-worktree-boundary", "outside-not-scanned");
    assert!(
        manifest.instructions().is_empty(),
        "no instruction may resolve from above the worktree root"
    );
    assert!(manifest.skills().is_empty());
    assert!(manifest.discovery_errors().is_empty());

    let outside = case_dir("05-worktree-boundary").join("outside/AGENTS.md");
    assert!(
        outside.is_file(),
        "fixture sanity: the above-boundary instruction exists"
    );
    assert!(manifest
        .instructions()
        .iter()
        .all(|instruction| instruction.path != outside));
    let canonical = std::str::from_utf8(manifest.canonical_bytes()).expect("canonical is UTF-8");
    assert!(!canonical.contains("outside/AGENTS.md"));
}

#[test]
fn manifest_replay_is_byte_deterministic() {
    for (case, scenario_name) in [
        ("02-global-claude-fallback", "default"),
        ("06-project-and-global", "default"),
        ("09-cross-agent-roots", "default"),
        ("11-duplicate-name", "default"),
        ("14-skills-paths", "default"),
        ("14-skills-paths", "config-dir-override"),
    ] {
        let first = resolve_named(case, scenario_name);
        let second = resolve_named(case, scenario_name);
        assert_eq!(first.sha256(), second.sha256(), "{case}/{scenario_name}");
        assert_eq!(
            first.canonical_bytes(),
            second.canonical_bytes(),
            "{case}/{scenario_name}"
        );
    }
}

#[test]
fn claude_prompt_flag_removes_the_global_fallback_and_the_project_claude_file() {
    // Upstream `1.18.27`: `disableClaudeCodePrompt` drops the global
    // `~/.claude/CLAUDE.md` fallback *and* the project-level `CLAUDE.md`
    // candidate from the project priority list (live-confirmed 2026-10-19,
    // `Docs/Eval/milestone-6/CONFORMANCE-DISCOVERY.md` §3.2).
    let case = "02-global-claude-fallback";
    let expected = load_expected(case);
    let enabled = resolve_scenario(case, context_support::scenario(&expected, "default"))
        .expect("default resolves");
    let disabled = resolve_scenario(
        case,
        context_support::scenario(&expected, "disable-claude-prompt"),
    )
    .expect("flag scenario resolves");
    assert_eq!(enabled.instructions().len(), 1);
    assert!(disabled.instructions().is_empty());
    assert_ne!(
        enabled.sha256(),
        disabled.sha256(),
        "the gate is part of the hashed context"
    );

    let case = "19-flag-project-config-instructions";
    let expected = load_expected(case);
    let enabled = resolve_scenario(case, context_support::scenario(&expected, "claude-only"))
        .expect("scenario resolves");
    let disabled = resolve_scenario(
        case,
        context_support::scenario(&expected, "claude-only-disable-claude-prompt"),
    )
    .expect("flag scenario resolves");
    assert_eq!(
        enabled
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["xdg:opencode/AGENTS.md", "project:CLAUDE.md"]
    );
    assert_eq!(
        disabled
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["xdg:opencode/AGENTS.md"],
        "the project CLAUDE.md is not a candidate under the prompt gate"
    );
}

#[test]
fn claude_skills_flag_removes_only_the_claude_roots() {
    // Upstream `1.18.27`: `disableClaudeCodeSkills` removes the `.claude`
    // roots only; the `.agents` roots belong to `disableExternalSkills`
    // (live-confirmed 2026-10-19, `CONFORMANCE-DISCOVERY.md` §3.1).
    let case = "18-flag-gates-external-skills";
    let expected = load_expected(case);
    let names = |scenario: &str| {
        let manifest = resolve_scenario(case, context_support::scenario(&expected, scenario))
            .expect("resolves");
        manifest
            .skills()
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<Vec<String>>()
    };
    assert_eq!(
        names("default"),
        [
            "agents-global",
            "claude-global",
            "project-agents",
            "project-claude",
            "project-oc"
        ]
    );
    assert_eq!(
        names("disable-claude-skills"),
        ["agents-global", "project-agents", "project-oc"],
        "only the .claude roots are gated by the Claude-skills flag"
    );
    assert_eq!(
        names("disable-external-skills"),
        ["project-oc"],
        "the external-skills flag removes the .claude and the .agents roots"
    );
    assert_eq!(
        names("disable-claude-code"),
        ["agents-global", "project-agents", "project-oc"],
        "the broad Claude-Code gate implies prompt and skills, never external skills"
    );
    assert_eq!(names("disable-claude-skills-and-external"), ["project-oc"]);
}

#[test]
fn broad_claude_code_gate_implies_prompt_and_skills_but_not_external_skills() {
    let case = "18-flag-gates-external-skills";
    let expected = load_expected(case);
    let manifest = resolve_scenario(
        case,
        context_support::scenario(&expected, "disable-claude-code"),
    )
    .expect("scenario resolves");
    let specific = resolve_scenario(
        case,
        context_support::scenario(&expected, "disable-claude-skills"),
    )
    .expect("scenario resolves");
    assert_ne!(
        manifest.sha256(),
        specific.sha256(),
        "the flag projection distinguishes the broad gate from the direct one"
    );

    let flags = DiscoveryFlags {
        disable_claude_code: true,
        ..DiscoveryFlags::default()
    };
    assert!(flags.claude_code_skills_disabled());
    assert!(flags.claude_code_prompt_disabled());
    assert!(!flags.external_skills_disabled());
    let direct = DiscoveryFlags {
        disable_claude_code_skills: true,
        disable_claude_code_prompt: true,
        ..DiscoveryFlags::default()
    };
    assert!(direct.claude_code_skills_disabled());
    assert!(direct.claude_code_prompt_disabled());
    assert!(!direct.external_skills_disabled());
}

#[test]
fn project_config_flag_removes_the_project_instruction_walk_up() {
    // Upstream `1.18.27`: `OPENCODE_DISABLE_PROJECT_CONFIG` skips the project
    // instruction lookup entirely; only the global instruction survives
    // (live-confirmed 2026-10-19, `CONFORMANCE-DISCOVERY.md` §3.2).
    let case = "19-flag-project-config-instructions";
    let expected = load_expected(case);
    let enabled = resolve_scenario(case, context_support::scenario(&expected, "agents-only"))
        .expect("scenario resolves");
    let disabled = resolve_scenario(
        case,
        context_support::scenario(&expected, "agents-only-disable-project-config"),
    )
    .expect("flag scenario resolves");
    assert_eq!(
        enabled
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["xdg:opencode/AGENTS.md", "project:AGENTS.md"]
    );
    assert_eq!(
        disabled
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["xdg:opencode/AGENTS.md"],
        "the project walk-up is skipped entirely under the project-config gate"
    );
}

#[test]
fn every_level_of_the_winning_project_instruction_name_attaches() {
    // Upstream `1.18.27`: `findUp` returns every level of the first-priority
    // name, and the system prompt carries one block per match (innermost
    // first) — live-confirmed 2026-10-19, `CONFORMANCE-DISCOVERY.md` §3.2.
    let manifest = resolve_named("20-project-instruction-multilevel", "default");
    assert_eq!(
        manifest
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["worktree:app/AGENTS.md", "worktree:AGENTS.md"]
    );
    assert_eq!(
        manifest
            .instructions()
            .iter()
            .map(|instruction| instruction.precedence)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    let manifest = resolve_named("03-project-walkup-nearest-agents", "default");
    assert_eq!(
        manifest
            .instructions()
            .iter()
            .map(|instruction| instruction.token.as_str())
            .collect::<Vec<_>>(),
        ["worktree:applications/AGENTS.md", "worktree:AGENTS.md"],
        "both levels of the winning name attach, nearest first"
    );
}

#[test]
fn duplicate_skill_names_make_the_manifest_non_reproducible() {
    // Upstream registers skills concurrently, so the winner of a contested
    // name is racy (live-confirmed 2026-10-19, `CONFORMANCE-DISCOVERY.md`
    // §2.3). The resolver must not claim a winner.
    let manifest = resolve_named("11-duplicate-name", "default");
    assert!(
        !manifest.reproducible(),
        "a duplicate-name layout is not reproducible"
    );
    assert_eq!(manifest.discovery_warnings().len(), 1);
    let warning = &manifest.discovery_warnings()[0];
    assert_eq!(warning.name, "dup-skill");
    assert_eq!(
        warning
            .candidates
            .iter()
            .map(|candidate| candidate.token.as_str())
            .collect::<Vec<_>>(),
        [
            "home:.claude/skills/dup/SKILL.md",
            "xdg:opencode/skills/dup/SKILL.md",
            "project:.opencode/skill/dup/SKILL.md"
        ],
        "every candidate is recorded; the warning names no winner"
    );
    assert_eq!(manifest.skills().len(), 1);
    assert!(
        manifest.skills()[0].ambiguous,
        "the effective skill is flagged ambiguous"
    );

    let unique = resolve_named("09-cross-agent-roots", "default");
    assert!(
        unique.reproducible(),
        "a layout with unique skill names stays reproducible"
    );
    assert!(unique.discovery_warnings().is_empty());
}

#[test]
fn project_config_flag_drops_project_opencode_but_keeps_home_opencode() {
    let case = "16-flags-project-config";
    let expected = load_expected(case);
    let disabled = resolve_scenario(
        case,
        context_support::scenario(&expected, "disable-project-config"),
    )
    .expect("flag scenario resolves");
    let names: Vec<&str> = disabled
        .skills()
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(names, ["global-skill"]);
}

#[test]
fn flags_are_part_of_the_manifest_hash_even_when_resolution_is_unchanged() {
    let case = "13-empty-nil";
    let base = resolve_named(case, "default");
    let expected = load_expected(case);
    let scenario = context_support::scenario(&expected, "default");
    let dir = case_dir(case);
    let roots = ContextRoots {
        home: &dir.join(&scenario.roots.home),
        xdg_config_home: &dir.join(&scenario.roots.xdg_config_home),
        project_root: &dir.join(&scenario.roots.project_root),
        worktree_root: &dir.join(&scenario.roots.worktree_root),
    };
    let flagged = resolve_manifest(
        &roots,
        &DiscoveryFlags {
            disable_claude_code_skills: true,
            ..DiscoveryFlags::default()
        },
    )
    .expect("flagged resolution succeeds");

    assert!(flagged.instructions().is_empty());
    assert!(flagged.skills().is_empty());
    assert!(flagged.discovery_errors().is_empty());
    assert_ne!(
        base.sha256(),
        flagged.sha256(),
        "changing a discovery gate must change the pinned context"
    );
}

#[test]
fn missing_root_is_a_typed_error_not_a_panic() {
    let missing = std::env::temp_dir().join("root-eval-missing-home-sentinel-do-not-create");
    let case = case_dir("13-empty-nil");
    let roots = ContextRoots {
        home: &missing,
        xdg_config_home: &case.join("home/config"),
        project_root: &case.join("project"),
        worktree_root: &case.join("project"),
    };
    let error = resolve_manifest(&roots, &DiscoveryFlags::default())
        .expect_err("a missing root must be refused");
    assert_eq!(error.kind(), ContextErrorKind::MissingRoot);
    assert!(error.to_string().contains("does not exist"));
}

#[test]
fn disjoint_roots_are_a_typed_error() {
    let case = case_dir("13-empty-nil");
    let roots = ContextRoots {
        home: &case.join("home"),
        xdg_config_home: &case.join("home/config"),
        project_root: &case.join("project"),
        worktree_root: &case.join("home"),
    };
    let error = resolve_manifest(&roots, &DiscoveryFlags::default())
        .expect_err("disjoint roots must be refused");
    assert_eq!(error.kind(), ContextErrorKind::RootsDisjoint);
}

#[test]
fn manifest_hash_projection_excludes_absolute_paths() {
    let manifest = resolve_named("12-ab-treatment", "treatment-added");
    let canonical = std::str::from_utf8(manifest.canonical_bytes()).expect("canonical is UTF-8");
    let case_root = case_dir("12-ab-treatment");
    assert!(
        !canonical.contains(&*case_root.to_string_lossy()),
        "the manifest hash must be root-relative and portable"
    );
    assert!(canonical.contains("project:.opencode/skill/root/SKILL.md"));
    assert!(canonical.contains("xdg:opencode/AGENTS.md"));
}

#[test]
fn ab_treatment_root_skill_is_byte_identical_to_the_repository_skill() {
    let fixture = read_case_file(
        "12-ab-treatment",
        "treatment-added/project/.opencode/skill/root/SKILL.md",
    );
    let repository = fs::read(repo_skill_path()).expect("repository skill file is readable");
    assert_eq!(
        fixture, repository,
        "the A/B delta skill must be a byte-for-byte copy of skills/claude/SKILL.md"
    );
}

#[test]
fn ab_trees_differ_only_by_the_declared_delta_directory() {
    let base_root = case_dir("12-ab-treatment").join("base");
    let treatment_root = case_dir("12-ab-treatment").join("treatment-added");
    let base = file_list(&base_root);
    let treatment = file_list(&treatment_root);

    let added: Vec<&str> = treatment
        .iter()
        .filter(|path| !base.contains(path))
        .map(String::as_str)
        .collect();
    let removed: Vec<&str> = base
        .iter()
        .filter(|path| !treatment.contains(path))
        .map(String::as_str)
        .collect();
    assert_eq!(
        added,
        vec!["project/.opencode/skill/root/SKILL.md"],
        "the treatment must add exactly the declared skill directory file"
    );
    assert!(removed.is_empty(), "the treatment must not remove files");

    let shared: Vec<&String> = base
        .iter()
        .filter(|path| treatment.contains(path))
        .collect();
    assert!(!shared.is_empty());
    for path in shared {
        let base_bytes = fs::read(base_root.join(path)).expect("base file is readable");
        let treatment_bytes =
            fs::read(treatment_root.join(path)).expect("treatment file is readable");
        assert_eq!(base_bytes, treatment_bytes, "shared file `{path}` changed");
    }
}
