//! Shared corpus-replay helpers for the Phase B integration tests.
//!
//! Tests read fixture files only: case trees live under
//! `tests/fixtures/opencode-discovery/<case>/`, expectations live in each
//! case's `expected.json`. Nothing is written and nothing outside the fixtures
//! and the repository's `skills/claude/SKILL.md` is read.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use root_eval::{
    resolve_manifest, ConditionOverlay, ContextErrorKind, ContextRoots, DiscoveryFlags,
    EffectiveContextManifest, InstructionRole, InstructionSource, SkillSource,
};

/// Root of the discovery fixture corpus.
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("opencode-discovery")
}

/// Absolute path of one fixture case.
pub fn case_dir(case: &str) -> PathBuf {
    fixtures_root().join(case)
}

/// Repository's real Claude skill file (the A/B treatment payload).
pub fn repo_skill_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("skills")
        .join("claude")
        .join("SKILL.md")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFile {
    pub case: String,
    pub contract: String,
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub roots: RootsSpec,
    #[serde(default)]
    pub flags: FlagsSpec,
    #[serde(default)]
    pub expect: Option<Expectation>,
    #[serde(default)]
    pub expect_error: Option<ExpectedError>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootsSpec {
    pub home: String,
    pub xdg_config_home: String,
    pub project_root: String,
    pub worktree_root: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagsSpec {
    #[serde(default)]
    pub disable_claude_code_skills: bool,
    #[serde(default)]
    pub disable_claude_code_prompt: bool,
    #[serde(default)]
    pub disable_project_config: bool,
    #[serde(default)]
    pub config_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub instructions: Vec<ExpectedInstruction>,
    pub skills: Vec<ExpectedSkill>,
    #[serde(default)]
    pub discovery_errors: Vec<ExpectedDiscoveryError>,
    #[serde(default)]
    pub discovery_warnings: Vec<ExpectedWarning>,
    pub manifest_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedInstruction {
    pub role: String,
    pub source: String,
    pub precedence: u8,
    pub file: String,
    pub token: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedSkill {
    pub name: String,
    pub source: String,
    pub file: String,
    pub token: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedDiscoveryError {
    pub file: String,
    pub token: String,
    pub kind: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedWarning {
    pub name: String,
    pub overridden_file: String,
    pub overridden_token: String,
    pub winner_file: String,
    pub winner_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedError {
    pub kind: String,
    pub message_contains: String,
}

/// Load a case's `expected.json`.
pub fn load_expected(case: &str) -> ExpectedFile {
    let path = case_dir(case).join("expected.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

/// Resolve one declarative scenario.
pub fn resolve_scenario(
    case: &str,
    scenario: &Scenario,
) -> Result<EffectiveContextManifest, root_eval::ContextError> {
    let dir = case_dir(case);
    let home = dir.join(&scenario.roots.home);
    let xdg = dir.join(&scenario.roots.xdg_config_home);
    let project = dir.join(&scenario.roots.project_root);
    let worktree = dir.join(&scenario.roots.worktree_root);
    let roots = ContextRoots {
        home: &home,
        xdg_config_home: &xdg,
        project_root: &project,
        worktree_root: &worktree,
    };
    let flags = DiscoveryFlags {
        disable_claude_code_skills: scenario.flags.disable_claude_code_skills,
        disable_claude_code_prompt: scenario.flags.disable_claude_code_prompt,
        disable_project_config: scenario.flags.disable_project_config,
        config_dir: scenario
            .flags
            .config_dir
            .as_ref()
            .map(|relative| dir.join(relative)),
    };
    resolve_manifest(&roots, &flags)
}

/// Replay a whole case: every scenario, full expectation comparison.
pub fn run_case(case: &str) {
    let expected = load_expected(case);
    assert_eq!(expected.case, case, "expected.json case id");
    assert_eq!(
        expected.contract, "Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md",
        "every case cites the frozen contract"
    );
    assert!(!expected.scenarios.is_empty(), "case {case} has scenarios");
    for scenario in &expected.scenarios {
        run_scenario(case, scenario);
    }
}

/// Replay a single scenario.
pub fn run_scenario(case: &str, scenario: &Scenario) {
    let resolved = resolve_scenario(case, scenario);
    match (resolved, &scenario.expect_error) {
        (Err(error), Some(expected_error)) => {
            assert_eq!(
                error.kind(),
                context_error_kind(&expected_error.kind),
                "{case}/{}: unexpected error kind: {error}",
                scenario.name
            );
            assert!(
                error.to_string().contains(&expected_error.message_contains),
                "{case}/{}: error message `{error}` does not contain `{}`",
                scenario.name,
                expected_error.message_contains
            );
        }
        (Ok(manifest), Some(expected_error)) => panic!(
            "{case}/{}: expected error kind `{}` but resolved manifest {}",
            scenario.name,
            expected_error.kind,
            manifest.sha256()
        ),
        (Err(error), None) => {
            panic!("{case}/{}: resolution failed: {error}", scenario.name)
        }
        (Ok(manifest), None) => {
            let expected = scenario
                .expect
                .as_ref()
                .unwrap_or_else(|| panic!("{case}/{}: no expectation", scenario.name));
            assert_manifest(case, &scenario.name, expected, &manifest);
        }
    }
}

/// Find a named scenario in a case's expectation file.
pub fn scenario<'a>(expected: &'a ExpectedFile, name: &str) -> &'a Scenario {
    expected
        .scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("case {} has no scenario `{name}`", expected.case))
}

/// Resolve a named scenario and require success.
pub fn resolve_named(case: &str, scenario_name: &str) -> EffectiveContextManifest {
    let expected = load_expected(case);
    let scenario = scenario(&expected, scenario_name);
    resolve_scenario(case, scenario)
        .unwrap_or_else(|error| panic!("{case}/{scenario_name}: resolution failed: {error}"))
}

fn assert_manifest(
    case: &str,
    scenario_name: &str,
    expected: &Expectation,
    actual: &EffectiveContextManifest,
) {
    let dir = case_dir(case);

    assert_eq!(
        actual.instructions().len(),
        expected.instructions.len(),
        "{case}/{scenario_name}: instruction count"
    );
    for (found, wanted) in actual.instructions().iter().zip(&expected.instructions) {
        assert_eq!(
            found.role,
            instruction_role(&wanted.role),
            "{case}/{scenario_name}: instruction role for {}",
            wanted.file
        );
        assert_eq!(
            found.source,
            instruction_source(&wanted.source),
            "{case}/{scenario_name}: instruction source for {}",
            wanted.file
        );
        assert_eq!(
            found.precedence, wanted.precedence,
            "{case}/{scenario_name}: precedence for {}",
            wanted.file
        );
        assert_eq!(
            found.path,
            dir.join(&wanted.file),
            "{case}/{scenario_name}: absolute instruction path"
        );
        assert_eq!(
            found.token, wanted.token,
            "{case}/{scenario_name}: instruction token for {}",
            wanted.file
        );
        assert_eq!(
            found.sha256, wanted.sha256,
            "{case}/{scenario_name}: instruction digest for {}",
            wanted.file
        );
        assert_eq!(
            found.size, wanted.size,
            "{case}/{scenario_name}: instruction size for {}",
            wanted.file
        );
    }

    assert_eq!(
        actual.skills().len(),
        expected.skills.len(),
        "{case}/{scenario_name}: skill count"
    );
    for (found, wanted) in actual.skills().iter().zip(&expected.skills) {
        assert_eq!(
            found.name, wanted.name,
            "{case}/{scenario_name}: skill name for {}",
            wanted.file
        );
        assert_eq!(
            found.source,
            skill_source(&wanted.source),
            "{case}/{scenario_name}: skill source for {}",
            wanted.file
        );
        assert_eq!(
            found.path,
            dir.join(&wanted.file),
            "{case}/{scenario_name}: absolute skill path"
        );
        assert_eq!(
            found.token, wanted.token,
            "{case}/{scenario_name}: skill token for {}",
            wanted.file
        );
        assert_eq!(
            found.sha256, wanted.sha256,
            "{case}/{scenario_name}: skill digest for {}",
            wanted.file
        );
    }

    assert_eq!(
        actual.discovery_errors().len(),
        expected.discovery_errors.len(),
        "{case}/{scenario_name}: discovery error count"
    );
    for (found, wanted) in actual
        .discovery_errors()
        .iter()
        .zip(&expected.discovery_errors)
    {
        assert_eq!(
            found.path,
            dir.join(&wanted.file),
            "{case}/{scenario_name}: error path"
        );
        assert_eq!(
            found.token, wanted.token,
            "{case}/{scenario_name}: error token"
        );
        assert_eq!(
            found.kind.as_str(),
            wanted.kind,
            "{case}/{scenario_name}: error kind for {}",
            wanted.file
        );
        assert_eq!(
            found.sha256, wanted.sha256,
            "{case}/{scenario_name}: error digest for {}",
            wanted.file
        );
        assert!(
            !found.detail.is_empty(),
            "{case}/{scenario_name}: error detail is recorded"
        );
    }

    assert_eq!(
        actual.discovery_warnings().len(),
        expected.discovery_warnings.len(),
        "{case}/{scenario_name}: discovery warning count"
    );
    for (found, wanted) in actual
        .discovery_warnings()
        .iter()
        .zip(&expected.discovery_warnings)
    {
        assert_eq!(
            found.name, wanted.name,
            "{case}/{scenario_name}: warning name"
        );
        assert_eq!(
            found.overridden_path,
            dir.join(&wanted.overridden_file),
            "{case}/{scenario_name}: overridden path"
        );
        assert_eq!(
            found.overridden_token, wanted.overridden_token,
            "{case}/{scenario_name}: overridden token"
        );
        assert_eq!(
            found.winner_path,
            dir.join(&wanted.winner_file),
            "{case}/{scenario_name}: winner path"
        );
        assert_eq!(
            found.winner_token, wanted.winner_token,
            "{case}/{scenario_name}: winner token"
        );
    }

    assert_eq!(
        actual.builtin_skill_names(),
        root_eval::BUILTIN_SKILL_NAMES,
        "{case}/{scenario_name}: built-in skill names"
    );

    if expected.manifest_sha256 == "TBD" {
        println!("TBD-HASH\t{case}\t{scenario_name}\t{}", actual.sha256());
    } else {
        assert_eq!(
            actual.sha256(),
            expected.manifest_sha256,
            "{case}/{scenario_name}: manifest hash"
        );
    }
}

/// Load a declared overlay JSON file from a case directory.
pub fn load_overlay(case: &str, file: &str) -> ConditionOverlay {
    let path = case_dir(case).join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

/// All regular files under `root`, as `/`-separated relative paths, sorted.
pub fn file_list(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
            .map(|entry| {
                entry
                    .unwrap_or_else(|error| panic!("failed to read entry: {error}"))
                    .path()
            })
            .collect();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()));
            if metadata.is_dir() {
                walk(&path, root, out);
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("walked path is under root")
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(relative);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files
}

/// Read a case-relative file.
pub fn read_case_file(case: &str, relative: &str) -> Vec<u8> {
    let path = case_dir(case).join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn instruction_role(value: &str) -> InstructionRole {
    match value {
        "global" => InstructionRole::Global,
        "project" => InstructionRole::Project,
        other => panic!("unknown role `{other}`"),
    }
}

fn instruction_source(value: &str) -> InstructionSource {
    match value {
        "global-config-agents" => InstructionSource::GlobalConfigAgents,
        "global-claude-fallback" => InstructionSource::GlobalClaudeFallback,
        "project-walkup" => InstructionSource::ProjectWalkUp,
        other => panic!("unknown instruction source `{other}`"),
    }
}

fn skill_source(value: &str) -> SkillSource {
    match value {
        "claude-global" => SkillSource::ClaudeGlobal,
        "agents-global" => SkillSource::AgentsGlobal,
        "claude-walkup" => SkillSource::ClaudeWalkUp,
        "agents-walkup" => SkillSource::AgentsWalkUp,
        "opencode-config-dir" => SkillSource::OpenCodeConfigDir,
        "explicit-path" => SkillSource::ExplicitPath,
        other => panic!("unknown skill source `{other}`"),
    }
}

fn context_error_kind(value: &str) -> ContextErrorKind {
    match value {
        "missing-root" => ContextErrorKind::MissingRoot,
        "not-a-directory" => ContextErrorKind::NotADirectory,
        "roots-disjoint" => ContextErrorKind::RootsDisjoint,
        "io" => ContextErrorKind::Io,
        "symlink-refused" => ContextErrorKind::SymlinkRefused,
        "unsafe-path" => ContextErrorKind::UnsafePath,
        "config-parse" => ContextErrorKind::ConfigParse,
        "config-shape" => ContextErrorKind::ConfigShape,
        "file-too-large" => ContextErrorKind::FileTooLarge,
        "canonical" => ContextErrorKind::Canonical,
        other => panic!("unknown context error kind `{other}`"),
    }
}
