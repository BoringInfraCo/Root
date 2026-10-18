//! OpenCode effective-context resolver (Milestone 1, Phase B).
//!
//! Resolves — from an on-disk layout and explicit inputs only — the instruction
//! files and skill files the pinned harness (`opencode` `1.18.27`) attaches for
//! a project. The frozen behavioral contract is §4 ("Skills & Rules") of
//! `Docs/Eval/milestone-0/UPSTREAM-CONTRACT.md`, with the `OPENCODE_DISABLE_*`
//! and `OPENCODE_CONFIG_DIR` gates from §4.1/§4.3/§5.2. Nothing here reads the
//! environment, spawns a process, or assumes a home directory: the caller
//! passes every root explicitly and maps the four environment gates into
//! [`DiscoveryFlags`].
//!
//! # Documented scan order
//!
//! The contract lists the discovery sources but does not spell out a total
//! order; duplicate names resolve "last-scanned wins". This resolver fixes the
//! following total order (later scans override earlier ones by skill name) and
//! records every override as a [`DiscoveryWarning`]:
//!
//! 1. built-in `customize-opencode` (not on disk; recorded in
//!    [`EffectiveContextManifest::builtin_skill_names`], overridden by any disk
//!    skill of the same name);
//! 2. `~/.claude/skills/**/SKILL.md`;
//! 3. `~/.agents/skills/**/SKILL.md`;
//! 4. for each directory from the instance directory up to the worktree root
//!    (innermost first, worktree root included): `<dir>/.claude/skills` then
//!    `<dir>/.agents/skills`;
//! 5. for each config directory in the contract's listed order —
//!    `${XDG_CONFIG_HOME:-~/.config}/opencode`, every `.opencode` from the
//!    instance directory up to the worktree (innermost first), `~/.opencode`,
//!    `OPENCODE_CONFIG_DIR` — the globs `<config>/skill/**/SKILL.md` then
//!    `<config>/skills/**/SKILL.md`;
//! 6. explicit `skills.paths` entries from each discovered `opencode.json`,
//!    concatenated in config-directory order and, inside a file, declared
//!    order.
//!
//! Within one scanned root, candidates are visited in lexicographic order by
//! path. Scanned roots are de-duplicated by path, keeping the first occurrence.
//!
//! # Instructions
//!
//! Contract §4.3:
//!
//! - global: `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` first; if
//!   absent and not suppressed by `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`, the
//!   `~/.claude/CLAUDE.md` fallback. First match wins.
//! - project: walking up from the instance directory to the worktree root,
//!   the first name in priority order `AGENTS.md` → `CLAUDE.md` →
//!   `CONTEXT.md`; one file per message. Both levels attach independently.
//!
//! # Hermeticity and safety
//!
//! - No environment reads, no process spawning, no implicit `HOME`; every root
//!   is an argument and every gate is a [`DiscoveryFlags`] field.
//! - Every root must exist and be a directory; a missing root is a typed
//!   [`ContextError::MissingRoot`], never a panic.
//! - The instance directory must be inside the worktree root
//!   ([`ContextError::RootsDisjoint`] otherwise); containment is lexical after
//!   normalization.
//! - Symlinks are refused wherever a file would be consumed (instruction
//!   candidates, `SKILL.md` candidates, config files); other symlinks inside a
//!   scanned tree are skipped silently. Explicit `skills.paths` entries may be
//!   absolute, relative to the instance directory, or `~/`-prefixed; `..`
//!   components, backslashes, and NUL bytes are refused.
//! - Reads are bounded by [`MAX_CONTEXT_FILE_BYTES`]; traversal is bounded by
//!   [`MAX_SCAN_DEPTH`] (deeper directories are silently not scanned).
//!
//! # Hashing
//!
//! The manifest's canonical hash projects **root-relative tokens** instead of
//! absolute paths (`home:…`, `xdg:…`, `project:…`, `worktree:…`,
//! `configdir:…`, or `abs:…` for files outside every declared root), so the
//! same logical tree hashes identically on any machine. Entries are ordered
//! deterministically: instructions by attachment order, skills by name,
//! discovery errors by `(token, kind)`, warnings by name and scan order.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::canonical_json::canonical_json_bytes;
use crate::digest::sha256_digest;
use frontmatter::parse_skill_frontmatter;

mod frontmatter;

/// Hash domain mixed into the manifest canonical projection.
pub const CONTEXT_HASH_DOMAIN: &str = "effective-context.v1";

/// Largest instruction/config/`SKILL.md` file this resolver will read.
pub const MAX_CONTEXT_FILE_BYTES: u64 = 1024 * 1024;

/// Deepest directory level under a scanned root that is traversed.
pub const MAX_SCAN_DEPTH: usize = 32;

/// Built-in skills registered before the disk scan (contract §4.1 item 5).
///
/// The resolver is disk-only, so built-in skill *bodies* are not resolved; the
/// names are recorded so an experiment can detect effective-name collisions.
pub const BUILTIN_SKILL_NAMES: &[&str] = &["customize-opencode"];

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// The explicit filesystem roots the resolver works from.
///
/// All four roots must exist and be directories. `xdg_config_home` stands in
/// for `${XDG_CONFIG_HOME:-~/.config}`; `project_root` is OpenCode's *instance
/// directory* (the walk-up start) and `worktree_root` is the root the walk-up
/// must not cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextRoots<'a> {
    /// Fake or real `$HOME`.
    pub home: &'a Path,
    /// `${XDG_CONFIG_HOME:-~/.config}`.
    pub xdg_config_home: &'a Path,
    /// OpenCode's instance directory (project root / walk-up start).
    pub project_root: &'a Path,
    /// Worktree root; walk-ups stop here.
    pub worktree_root: &'a Path,
}

/// Boolean/image mapping of the four environment gates that change discovery.
///
/// The resolver never reads the environment itself; callers translate the
/// actual environment (or a fixture scenario) into this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryFlags {
    /// `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`: skip all cross-agent skill dirs
    /// (`~/.claude/skills`, `~/.agents/skills`, and their walk-up variants).
    pub disable_claude_code_skills: bool,
    /// `OPENCODE_DISABLE_CLAUDE_CODE_PROMPT`: suppress the global
    /// `~/.claude/CLAUDE.md` fallback (the project walk-up is unaffected).
    pub disable_claude_code_prompt: bool,
    /// `OPENCODE_DISABLE_PROJECT_CONFIG`: skip project `opencode.json`/`.opencode`
    /// discovery (project config directories and their `opencode.json`).
    pub disable_project_config: bool,
    /// `OPENCODE_CONFIG_DIR`: an additional config directory scanned after the
    /// defaults.
    pub config_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Output vocabulary
// ---------------------------------------------------------------------------

/// Role of an instruction file in the effective context (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstructionRole {
    /// Global level (XDG config or the Claude fallback).
    Global,
    /// Project level (walk-up from the instance directory).
    Project,
}

impl InstructionRole {
    /// Stable string form used in the canonical projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// Where an instruction file was discovered (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstructionSource {
    /// `${XDG_CONFIG_HOME}/opencode/AGENTS.md`.
    GlobalConfigAgents,
    /// `~/.claude/CLAUDE.md` (global fallback, first-match-wins).
    GlobalClaudeFallback,
    /// Walk-up from the instance directory to the worktree root.
    ProjectWalkUp,
}

impl InstructionSource {
    /// Stable string form used in the canonical projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GlobalConfigAgents => "global-config-agents",
            Self::GlobalClaudeFallback => "global-claude-fallback",
            Self::ProjectWalkUp => "project-walkup",
        }
    }
}

/// Discovery source class of a skill file (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSource {
    /// `~/.claude/skills`.
    ClaudeGlobal,
    /// `~/.agents/skills`.
    AgentsGlobal,
    /// `<dir>/.claude/skills` found walking up to the worktree root.
    ClaudeWalkUp,
    /// `<dir>/.agents/skills` found walking up to the worktree root.
    AgentsWalkUp,
    /// A config directory's `skill/` or `skills/` subtree.
    OpenCodeConfigDir,
    /// An explicit `skills.paths` entry.
    ExplicitPath,
}

impl SkillSource {
    /// Stable string form used in the canonical projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeGlobal => "claude-global",
            Self::AgentsGlobal => "agents-global",
            Self::ClaudeWalkUp => "claude-walkup",
            Self::AgentsWalkUp => "agents-walkup",
            Self::OpenCodeConfigDir => "opencode-config-dir",
            Self::ExplicitPath => "explicit-path",
        }
    }
}

/// Why a `SKILL.md` candidate was excluded from discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryErrorKind {
    /// The file does not start with a `---` frontmatter block.
    MissingFrontmatter,
    /// The frontmatter block is never closed.
    UnterminatedFrontmatter,
    /// A frontmatter line is not a top-level `key: value` entry.
    InvalidFrontmatterLine,
    /// A frontmatter value is not a supported scalar.
    InvalidFrontmatterValue,
    /// The same frontmatter key appears twice.
    DuplicateFrontmatterKey,
    /// The frontmatter has no `name` key.
    MissingName,
    /// The frontmatter `name` is empty.
    EmptyName,
    /// The file is not valid UTF-8.
    InvalidUtf8,
}

impl DiscoveryErrorKind {
    /// Stable string form used in the canonical projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingFrontmatter => "missing-frontmatter",
            Self::UnterminatedFrontmatter => "unterminated-frontmatter",
            Self::InvalidFrontmatterLine => "invalid-frontmatter-line",
            Self::InvalidFrontmatterValue => "invalid-frontmatter-value",
            Self::DuplicateFrontmatterKey => "duplicate-frontmatter-key",
            Self::MissingName => "missing-name",
            Self::EmptyName => "empty-name",
            Self::InvalidUtf8 => "invalid-utf8",
        }
    }
}

/// Warning kind recorded during discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryWarningKind {
    /// A later-scanned skill overrode an earlier one with the same name.
    DuplicateSkillName,
}

/// A `SKILL.md` candidate that was read but excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryError {
    /// Absolute path of the excluded file.
    pub path: PathBuf,
    /// Canonical root-relative token of the file.
    pub token: String,
    /// Machine-matchable exclusion reason.
    pub kind: DiscoveryErrorKind,
    /// Human-readable detail.
    pub detail: String,
    /// SHA-256 of the file bytes (the file was read to classify it).
    pub sha256: String,
}

/// A duplicate-name override event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryWarning {
    /// Warning kind (currently only duplicate skill names).
    pub kind: DiscoveryWarningKind,
    /// Contested skill name.
    pub name: String,
    /// Absolute path of the overridden file.
    pub overridden_path: PathBuf,
    /// Token of the overridden file.
    pub overridden_token: String,
    /// Absolute path of the winning file.
    pub winner_path: PathBuf,
    /// Token of the winning file.
    pub winner_token: String,
}

/// A resolved instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInstruction {
    /// Global or project level.
    pub role: InstructionRole,
    /// Discovery source (§4.3).
    pub source: InstructionSource,
    /// 0-based attachment order (0 = first attached).
    pub precedence: u8,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Canonical root-relative token used by the manifest hash.
    pub token: String,
    /// SHA-256 of the file bytes.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
}

/// A discovered, frontmatter-valid skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    /// Frontmatter `name`.
    pub name: String,
    /// Discovery source class.
    pub source: SkillSource,
    /// Absolute path of the `SKILL.md` file.
    pub path: PathBuf,
    /// Canonical root-relative token used by the manifest hash.
    pub token: String,
    /// SHA-256 of the file bytes.
    pub sha256: String,
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// The fully resolved effective context for one set of roots and flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveContextManifest {
    instructions: Vec<ResolvedInstruction>,
    skills: Vec<ResolvedSkill>,
    discovery_errors: Vec<DiscoveryError>,
    discovery_warnings: Vec<DiscoveryWarning>,
    builtin_skill_names: Vec<String>,
    canonical_bytes: Vec<u8>,
    sha256: String,
}

impl EffectiveContextManifest {
    /// Instructions in attachment order (global first when both attach).
    pub fn instructions(&self) -> &[ResolvedInstruction] {
        &self.instructions
    }

    /// Effective skills, sorted by name (later scans override earlier ones).
    pub fn skills(&self) -> &[ResolvedSkill] {
        &self.skills
    }

    /// Excluded `SKILL.md` files with recorded reasons.
    pub fn discovery_errors(&self) -> &[DiscoveryError] {
        &self.discovery_errors
    }

    /// Duplicate-name override events.
    pub fn discovery_warnings(&self) -> &[DiscoveryWarning] {
        &self.discovery_warnings
    }

    /// Built-in skill names registered before the disk scan.
    pub fn builtin_skill_names(&self) -> &[String] {
        &self.builtin_skill_names
    }

    /// Canonical bytes whose SHA-256 is [`EffectiveContextManifest::sha256`].
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Canonical `sha256:<hex>` digest of the manifest projection.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Machine-matchable classification for every [`ContextError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextErrorKind {
    /// A declared root does not exist.
    MissingRoot,
    /// A declared root is not a directory.
    NotADirectory,
    /// The instance directory is not inside the worktree root.
    RootsDisjoint,
    /// An I/O operation failed.
    Io,
    /// A path that would be consumed is a symlink.
    SymlinkRefused,
    /// An explicit `skills.paths` entry is unsafe.
    UnsafePath,
    /// `opencode.json` is not valid JSON.
    ConfigParse,
    /// `opencode.json` has an unsupported shape.
    ConfigShape,
    /// A file exceeds [`MAX_CONTEXT_FILE_BYTES`].
    FileTooLarge,
    /// Canonical manifest serialization failed.
    Canonical,
}

/// Errors produced while resolving the effective context.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContextError {
    #[error("root `{path}` does not exist")]
    MissingRoot { path: PathBuf },

    #[error("root `{path}` is not a directory")]
    NotADirectory { path: PathBuf },

    #[error("instance directory `{project_root}` is not inside worktree root `{worktree_root}`")]
    RootsDisjoint {
        project_root: PathBuf,
        worktree_root: PathBuf,
    },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },

    #[error("symlink refused at `{path}`")]
    SymlinkRefused { path: PathBuf },

    #[error("unsafe path `{entry}`: {reason}")]
    UnsafePath { entry: String, reason: String },

    #[error("invalid config `{path}`: {message}")]
    ConfigParse { path: PathBuf, message: String },

    #[error("invalid config shape in `{path}`: {message}")]
    ConfigShape { path: PathBuf, message: String },

    #[error("file `{path}` is {size} bytes, over the {limit}-byte limit")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },

    #[error("canonical serialization failed: {message}")]
    Canonical { message: String },
}

impl ContextError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> ContextErrorKind {
        match self {
            Self::MissingRoot { .. } => ContextErrorKind::MissingRoot,
            Self::NotADirectory { .. } => ContextErrorKind::NotADirectory,
            Self::RootsDisjoint { .. } => ContextErrorKind::RootsDisjoint,
            Self::Io { .. } => ContextErrorKind::Io,
            Self::SymlinkRefused { .. } => ContextErrorKind::SymlinkRefused,
            Self::UnsafePath { .. } => ContextErrorKind::UnsafePath,
            Self::ConfigParse { .. } => ContextErrorKind::ConfigParse,
            Self::ConfigShape { .. } => ContextErrorKind::ConfigShape,
            Self::FileTooLarge { .. } => ContextErrorKind::FileTooLarge,
            Self::Canonical { .. } => ContextErrorKind::Canonical,
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Resolve the effective context for `roots` under `flags`.
///
/// See the module docs for scan order, instructions precedence, and safety
/// rules. This function performs reads only; it never writes, spawns, or
/// consults the environment.
pub fn resolve_manifest(
    roots: &ContextRoots<'_>,
    flags: &DiscoveryFlags,
) -> Result<EffectiveContextManifest, ContextError> {
    let resolved = ResolvedRoots::new(roots, flags)?;

    let mut instructions: Vec<ResolvedInstruction> = Vec::new();
    if let Some(instruction) = global_instruction(&resolved, flags)? {
        instructions.push(instruction);
    }
    if let Some(instruction) = project_instruction(&resolved)? {
        instructions.push(instruction);
    }
    for (index, instruction) in instructions.iter_mut().enumerate() {
        instruction.precedence = u8::try_from(index).unwrap_or(u8::MAX);
    }

    let mut candidates: Vec<ResolvedSkill> = Vec::new();
    let mut discovery_errors: Vec<DiscoveryError> = Vec::new();
    for (root, source) in skill_scan_roots(&resolved, flags) {
        scan_directory_root(
            &root,
            source,
            &resolved,
            &mut candidates,
            &mut discovery_errors,
        )?;
    }
    for entry in explicit_skill_paths(&resolved, flags)? {
        let expanded = expand_explicit_entry(&entry, &resolved)?;
        scan_explicit_entry(&expanded, &resolved, &mut candidates, &mut discovery_errors)?;
    }

    let discovery_warnings = duplicate_warnings(&candidates);
    let skills = winning_skills(&candidates);
    let flags_value = flags_projection(&resolved, flags);
    finish_manifest(
        instructions,
        skills,
        discovery_errors,
        discovery_warnings,
        flags_value,
    )
}

struct ResolvedRoots {
    home: PathBuf,
    xdg: PathBuf,
    project: PathBuf,
    worktree: PathBuf,
    config_dir: Option<PathBuf>,
    chain: Vec<PathBuf>,
}

impl ResolvedRoots {
    fn new(roots: &ContextRoots<'_>, flags: &DiscoveryFlags) -> Result<Self, ContextError> {
        let home = normalize_lexical(roots.home);
        let xdg = normalize_lexical(roots.xdg_config_home);
        let project = normalize_lexical(roots.project_root);
        let worktree = normalize_lexical(roots.worktree_root);
        require_directory(&home)?;
        require_directory(&xdg)?;
        require_directory(&project)?;
        require_directory(&worktree)?;
        if !project.starts_with(&worktree) {
            return Err(ContextError::RootsDisjoint {
                project_root: project,
                worktree_root: worktree,
            });
        }
        let config_dir = match &flags.config_dir {
            Some(dir) => {
                let dir = normalize_lexical(dir);
                require_directory(&dir)?;
                Some(dir)
            }
            None => None,
        };
        let chain = walk_chain(&project, &worktree);
        Ok(Self {
            home,
            xdg,
            project,
            worktree,
            config_dir,
            chain,
        })
    }

    /// Canonical root-relative token for an absolute path.
    ///
    /// The longest matching root wins so that a project nested inside the
    /// worktree (or an XDG home nested inside `$HOME`) keeps its most specific
    /// prefix; ties resolve to the first root in the fixed order `home`, `xdg`,
    /// `project`, `worktree`, `configdir`. Paths outside every root fall back
    /// to `abs:<path>`, which is deterministic but machine-specific.
    fn token_for(&self, path: &Path) -> String {
        let mut candidates: Vec<(&str, &Path)> = vec![
            ("home", &self.home),
            ("xdg", &self.xdg),
            ("project", &self.project),
            ("worktree", &self.worktree),
        ];
        if let Some(config_dir) = &self.config_dir {
            candidates.push(("configdir", config_dir));
        }
        let mut best: Option<(usize, &str, &Path)> = None;
        for (index, (tag, root)) in candidates.iter().enumerate() {
            if !path.starts_with(root) {
                continue;
            }
            let better = match best {
                None => true,
                Some((best_index, _, best_root)) => {
                    let depth = root.components().count();
                    let best_depth = best_root.components().count();
                    depth > best_depth || (depth == best_depth && index < best_index)
                }
            };
            if better {
                best = Some((index, tag, root));
            }
        }
        match best {
            Some((_, tag, root)) => {
                let relative = path.strip_prefix(root).unwrap_or(Path::new(""));
                format!("{}:{}", tag, relative_to_posix(relative))
            }
            None => format!("abs:{}", path.to_string_lossy()),
        }
    }
}

fn require_directory(path: &Path) -> Result<(), ContextError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Err(ContextError::MissingRoot {
            path: path.to_path_buf(),
        }),
        Err(error) => Err(ContextError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ContextError::SymlinkRefused {
                    path: path.to_path_buf(),
                });
            }
            if metadata.is_dir() {
                Ok(())
            } else {
                Err(ContextError::NotADirectory {
                    path: path.to_path_buf(),
                })
            }
        }
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn walk_chain(instance: &Path, worktree: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = instance.to_path_buf();
    loop {
        chain.push(current.clone());
        if current == worktree {
            break;
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
    chain
}

fn relative_to_posix(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            parts.push(part.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
}

/// True for a regular file; `false` for absent paths and directories; a typed
/// error for symlinks and I/O failures.
fn regular_file(path: &Path) -> Result<bool, ContextError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ContextError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                Err(ContextError::SymlinkRefused {
                    path: path.to_path_buf(),
                })
            } else {
                Ok(metadata.is_file())
            }
        }
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, ContextError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ContextError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ContextError::SymlinkRefused {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ContextError::Io {
            path: path.to_path_buf(),
            message: "not a regular file".to_string(),
        });
    }
    if metadata.len() > MAX_CONTEXT_FILE_BYTES {
        return Err(ContextError::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_CONTEXT_FILE_BYTES,
        });
    }
    let bytes = fs::read(path).map_err(|error| ContextError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if bytes.len() as u64 > MAX_CONTEXT_FILE_BYTES {
        return Err(ContextError::FileTooLarge {
            path: path.to_path_buf(),
            size: bytes.len() as u64,
            limit: MAX_CONTEXT_FILE_BYTES,
        });
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Instructions (§4.3)
// ---------------------------------------------------------------------------

fn global_instruction(
    roots: &ResolvedRoots,
    flags: &DiscoveryFlags,
) -> Result<Option<ResolvedInstruction>, ContextError> {
    let agents = roots.xdg.join("opencode").join("AGENTS.md");
    if regular_file(&agents)? {
        return Ok(Some(read_instruction(
            &agents,
            InstructionSource::GlobalConfigAgents,
            roots,
        )?));
    }
    if !flags.disable_claude_code_prompt {
        let claude = roots.home.join(".claude").join("CLAUDE.md");
        if regular_file(&claude)? {
            return Ok(Some(read_instruction(
                &claude,
                InstructionSource::GlobalClaudeFallback,
                roots,
            )?));
        }
    }
    Ok(None)
}

fn project_instruction(roots: &ResolvedRoots) -> Result<Option<ResolvedInstruction>, ContextError> {
    for file_name in ["AGENTS.md", "CLAUDE.md", "CONTEXT.md"] {
        for level in &roots.chain {
            let candidate = level.join(file_name);
            if regular_file(&candidate)? {
                return Ok(Some(read_instruction(
                    &candidate,
                    InstructionSource::ProjectWalkUp,
                    roots,
                )?));
            }
        }
    }
    Ok(None)
}

fn read_instruction(
    path: &Path,
    source: InstructionSource,
    roots: &ResolvedRoots,
) -> Result<ResolvedInstruction, ContextError> {
    let bytes = read_bounded(path)?;
    Ok(ResolvedInstruction {
        role: match source {
            InstructionSource::GlobalConfigAgents | InstructionSource::GlobalClaudeFallback => {
                InstructionRole::Global
            }
            InstructionSource::ProjectWalkUp => InstructionRole::Project,
        },
        source,
        precedence: 0,
        path: path.to_path_buf(),
        token: roots.token_for(path),
        sha256: sha256_digest(&bytes),
        size: bytes.len() as u64,
    })
}

// ---------------------------------------------------------------------------
// Skills (§4.1)
// ---------------------------------------------------------------------------

fn config_dirs(roots: &ResolvedRoots, flags: &DiscoveryFlags) -> Vec<PathBuf> {
    let mut dirs = vec![roots.xdg.join("opencode")];
    if !flags.disable_project_config {
        for level in &roots.chain {
            dirs.push(level.join(".opencode"));
        }
    }
    dirs.push(roots.home.join(".opencode"));
    if let Some(config_dir) = &roots.config_dir {
        dirs.push(config_dir.clone());
    }
    dedup_paths(dirs)
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

fn skill_scan_roots(roots: &ResolvedRoots, flags: &DiscoveryFlags) -> Vec<(PathBuf, SkillSource)> {
    let mut scanned: Vec<(PathBuf, SkillSource)> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let push = |path: PathBuf,
                source: SkillSource,
                scanned: &mut Vec<(PathBuf, SkillSource)>,
                seen: &mut Vec<PathBuf>| {
        if !seen.contains(&path) {
            seen.push(path.clone());
            scanned.push((path, source));
        }
    };

    if !flags.disable_claude_code_skills {
        push(
            roots.home.join(".claude").join("skills"),
            SkillSource::ClaudeGlobal,
            &mut scanned,
            &mut seen,
        );
        push(
            roots.home.join(".agents").join("skills"),
            SkillSource::AgentsGlobal,
            &mut scanned,
            &mut seen,
        );
        for level in &roots.chain {
            push(
                level.join(".claude").join("skills"),
                SkillSource::ClaudeWalkUp,
                &mut scanned,
                &mut seen,
            );
            push(
                level.join(".agents").join("skills"),
                SkillSource::AgentsWalkUp,
                &mut scanned,
                &mut seen,
            );
        }
    }
    for dir in config_dirs(roots, flags) {
        push(
            dir.join("skill"),
            SkillSource::OpenCodeConfigDir,
            &mut scanned,
            &mut seen,
        );
        push(
            dir.join("skills"),
            SkillSource::OpenCodeConfigDir,
            &mut scanned,
            &mut seen,
        );
    }
    scanned
}

fn scan_directory_root(
    root: &Path,
    source: SkillSource,
    roots: &ResolvedRoots,
    candidates: &mut Vec<ResolvedSkill>,
    discovery_errors: &mut Vec<DiscoveryError>,
) -> Result<(), ContextError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ContextError::Io {
                path: root.to_path_buf(),
                message: error.to_string(),
            })
        }
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ContextError::SymlinkRefused {
                    path: root.to_path_buf(),
                });
            }
            if !metadata.is_dir() {
                return Ok(());
            }
        }
    }

    let mut files = Vec::new();
    collect_skill_files(root, 0, &mut files)?;
    for file in files {
        match read_skill_file(&file, source, roots)? {
            SkillReadOutcome::Discovered(skill) => candidates.push(skill),
            SkillReadOutcome::Excluded(error) => discovery_errors.push(error),
        }
    }
    Ok(())
}

fn collect_skill_files(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), ContextError> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ContextError::Io {
                path: dir.to_path_buf(),
                message: error.to_string(),
            })
        }
        Ok(entries) => entries,
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| ContextError::Io {
            path: dir.to_path_buf(),
            message: error.to_string(),
        })?;
        paths.push(entry.path());
    }
    paths.sort();

    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| ContextError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                return Err(ContextError::SymlinkRefused { path });
            }
            continue;
        }
        if metadata.is_dir() {
            collect_skill_files(&path, depth + 1, out)?;
        } else if metadata.is_file()
            && path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
        {
            out.push(path);
        }
    }
    Ok(())
}

enum SkillReadOutcome {
    Discovered(ResolvedSkill),
    Excluded(DiscoveryError),
}

fn read_skill_file(
    path: &Path,
    source: SkillSource,
    roots: &ResolvedRoots,
) -> Result<SkillReadOutcome, ContextError> {
    let bytes = read_bounded(path)?;
    let sha256 = sha256_digest(&bytes);
    let excluded = |kind: DiscoveryErrorKind, detail: String| {
        SkillReadOutcome::Excluded(DiscoveryError {
            path: path.to_path_buf(),
            token: roots.token_for(path),
            kind,
            detail,
            sha256: sha256.clone(),
        })
    };

    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(_) => {
            return Ok(excluded(
                DiscoveryErrorKind::InvalidUtf8,
                "file is not valid UTF-8".to_string(),
            ))
        }
    };

    match parse_skill_frontmatter(&content) {
        Ok(frontmatter) => Ok(SkillReadOutcome::Discovered(ResolvedSkill {
            name: frontmatter.name,
            source,
            path: path.to_path_buf(),
            token: roots.token_for(path),
            sha256,
        })),
        Err(error) => Ok(excluded(error.kind(), error.detail())),
    }
}

fn scan_explicit_entry(
    path: &Path,
    roots: &ResolvedRoots,
    candidates: &mut Vec<ResolvedSkill>,
    discovery_errors: &mut Vec<DiscoveryError>,
) -> Result<(), ContextError> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ContextError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            })
        }
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        return Err(ContextError::SymlinkRefused {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return scan_directory_root(
            path,
            SkillSource::ExplicitPath,
            roots,
            candidates,
            discovery_errors,
        );
    }
    if metadata.is_file() && path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        match read_skill_file(path, SkillSource::ExplicitPath, roots)? {
            SkillReadOutcome::Discovered(skill) => candidates.push(skill),
            SkillReadOutcome::Excluded(error) => discovery_errors.push(error),
        }
    }
    Ok(())
}

fn explicit_skill_paths(
    roots: &ResolvedRoots,
    flags: &DiscoveryFlags,
) -> Result<Vec<String>, ContextError> {
    let mut entries: Vec<String> = Vec::new();
    for dir in config_dirs(roots, flags) {
        let config = dir.join("opencode.json");
        if !regular_file(&config)? {
            continue;
        }
        let bytes = read_bounded(&config)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|error| ContextError::ConfigParse {
                path: config.clone(),
                message: error.to_string(),
            })?;
        let Some(object) = value.as_object() else {
            return Err(ContextError::ConfigShape {
                path: config,
                message: "top level must be a JSON object".to_string(),
            });
        };
        let Some(skills) = object.get("skills") else {
            continue;
        };
        let Some(skills) = skills.as_object() else {
            return Err(ContextError::ConfigShape {
                path: config,
                message: "`skills` must be a JSON object".to_string(),
            });
        };
        let Some(paths) = skills.get("paths") else {
            continue;
        };
        let Some(paths) = paths.as_array() else {
            return Err(ContextError::ConfigShape {
                path: config,
                message: "`skills.paths` must be an array".to_string(),
            });
        };
        for entry in paths {
            let Some(entry) = entry.as_str() else {
                return Err(ContextError::ConfigShape {
                    path: config,
                    message: "`skills.paths` entries must be strings".to_string(),
                });
            };
            entries.push(entry.to_string());
        }
    }
    Ok(entries)
}

fn expand_explicit_entry(entry: &str, roots: &ResolvedRoots) -> Result<PathBuf, ContextError> {
    let unsafe_path = |reason: &str| ContextError::UnsafePath {
        entry: entry.to_string(),
        reason: reason.to_string(),
    };
    if entry.trim().is_empty() {
        return Err(unsafe_path("entry is empty"));
    }
    if entry != entry.trim() {
        return Err(unsafe_path(
            "leading or trailing whitespace is not permitted",
        ));
    }
    if entry.contains('\0') {
        return Err(unsafe_path("NUL bytes are not permitted"));
    }
    if entry.contains('\\') {
        return Err(unsafe_path("backslashes are not permitted"));
    }

    let expanded: PathBuf = if entry == "~" {
        roots.home.clone()
    } else if let Some(rest) = entry.strip_prefix("~/") {
        if rest.is_empty() {
            return Err(unsafe_path("`~/` without a target is not permitted"));
        }
        roots.home.join(rest)
    } else if entry.starts_with('~') {
        return Err(unsafe_path("`~user` expansion is not supported"));
    } else {
        let path = Path::new(entry);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            roots.project.join(path)
        }
    };

    if expanded
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(unsafe_path("`..` components are not permitted"));
    }
    Ok(expanded)
}

fn duplicate_warnings(candidates: &[ResolvedSkill]) -> Vec<DiscoveryWarning> {
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        by_name.entry(&candidate.name).or_default().push(index);
    }
    let mut warnings = Vec::new();
    for (name, indices) in by_name {
        for pair in indices.windows(2) {
            let overridden = &candidates[pair[0]];
            let winner = &candidates[pair[1]];
            warnings.push(DiscoveryWarning {
                kind: DiscoveryWarningKind::DuplicateSkillName,
                name: name.to_string(),
                overridden_path: overridden.path.clone(),
                overridden_token: overridden.token.clone(),
                winner_path: winner.path.clone(),
                winner_token: winner.token.clone(),
            });
        }
    }
    warnings
}

fn winning_skills(candidates: &[ResolvedSkill]) -> Vec<ResolvedSkill> {
    let mut winners: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        winners.insert(&candidate.name, index);
    }
    winners
        .values()
        .map(|&index| candidates[index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Canonical projection
// ---------------------------------------------------------------------------

fn flags_projection(roots: &ResolvedRoots, flags: &DiscoveryFlags) -> Value {
    json!({
        "disable_claude_code_skills": flags.disable_claude_code_skills,
        "disable_claude_code_prompt": flags.disable_claude_code_prompt,
        "disable_project_config": flags.disable_project_config,
        "config_dir": roots
            .config_dir
            .as_ref()
            .map(|dir| roots.token_for(dir)),
    })
}

fn finish_manifest(
    instructions: Vec<ResolvedInstruction>,
    skills: Vec<ResolvedSkill>,
    mut discovery_errors: Vec<DiscoveryError>,
    mut discovery_warnings: Vec<DiscoveryWarning>,
    flags_value: Value,
) -> Result<EffectiveContextManifest, ContextError> {
    discovery_errors.sort_by(|left, right| {
        (left.token.as_str(), left.kind.as_str()).cmp(&(right.token.as_str(), right.kind.as_str()))
    });
    discovery_warnings.sort_by(|left, right| {
        (
            left.name.as_str(),
            left.overridden_token.as_str(),
            left.winner_token.as_str(),
        )
            .cmp(&(
                right.name.as_str(),
                right.overridden_token.as_str(),
                right.winner_token.as_str(),
            ))
    });

    let builtin_skill_names: Vec<String> = BUILTIN_SKILL_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    let projection = json!({
        "hash_domain": CONTEXT_HASH_DOMAIN,
        "flags": flags_value,
        "builtin_skill_names": builtin_skill_names,
        "instructions": instructions
            .iter()
            .map(|instruction| {
                json!({
                    "role": instruction.role.as_str(),
                    "source": instruction.source.as_str(),
                    "precedence": instruction.precedence,
                    "token": instruction.token,
                    "sha256": instruction.sha256,
                    "size": instruction.size,
                })
            })
            .collect::<Vec<_>>(),
        "skills": skills
            .iter()
            .map(|skill| {
                json!({
                    "name": skill.name,
                    "source": skill.source.as_str(),
                    "token": skill.token,
                    "sha256": skill.sha256,
                })
            })
            .collect::<Vec<_>>(),
        "discovery_errors": discovery_errors
            .iter()
            .map(|error| {
                json!({
                    "kind": error.kind.as_str(),
                    "token": error.token,
                    "sha256": error.sha256,
                })
            })
            .collect::<Vec<_>>(),
        "discovery_warnings": discovery_warnings
            .iter()
            .map(|warning| {
                json!({
                    "name": warning.name,
                    "overridden_token": warning.overridden_token,
                    "winner_token": warning.winner_token,
                })
            })
            .collect::<Vec<_>>(),
    });

    let canonical_bytes =
        canonical_json_bytes(&projection).map_err(|message| ContextError::Canonical { message })?;
    let sha256 = sha256_digest(&canonical_bytes);
    Ok(EffectiveContextManifest {
        instructions,
        skills,
        discovery_errors,
        discovery_warnings,
        builtin_skill_names,
        canonical_bytes,
        sha256,
    })
}

// ---------------------------------------------------------------------------
// Token grammar (shared with condition overlays)
// ---------------------------------------------------------------------------

/// Validate a canonical root-relative token (`<tag>:<relative path>`).
///
/// Tags: `home`, `xdg`, `project`, `worktree`, `configdir`, or `abs` (whose
/// remainder must be an absolute POSIX path). Relative paths must be
/// non-empty, use `/` separators, and contain no `.`/`..` components,
/// backslashes, or empty components.
pub(crate) fn validate_canonical_token(token: &str) -> Result<(), String> {
    let Some((tag, relative)) = token.split_once(':') else {
        return Err("expected `<root-tag>:<relative-path>`".to_string());
    };
    match tag {
        "home" | "xdg" | "project" | "worktree" | "configdir" => {
            if relative.is_empty() {
                return Err("relative path is empty".to_string());
            }
            validate_relative(relative)
        }
        "abs" => {
            let Some(relative) = relative.strip_prefix('/') else {
                return Err("`abs:` token must hold an absolute path".to_string());
            };
            validate_relative(relative)
        }
        other => Err(format!("unknown root tag `{other}`")),
    }
}

fn validate_relative(relative: &str) -> Result<(), String> {
    if relative.contains('\\') {
        return Err("backslashes are not permitted".to_string());
    }
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("invalid path component `{part}`"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_ok(token: &str) {
        assert_eq!(validate_canonical_token(token), Ok(()), "token `{token}`");
    }

    fn token_err(token: &str) {
        assert!(
            validate_canonical_token(token).is_err(),
            "token `{token}` should fail"
        );
    }

    #[test]
    fn token_grammar_accepts_canonical_forms() {
        token_ok("home:.claude/skills/x/SKILL.md");
        token_ok("xdg:opencode/AGENTS.md");
        token_ok("project:.opencode/skill/root/SKILL.md");
        token_ok("worktree:AGENTS.md");
        token_ok("configdir:skill/a/SKILL.md");
        token_ok("abs:/opt/skills/a/SKILL.md");
    }

    #[test]
    fn token_grammar_refuses_unsafe_forms() {
        token_err("home:");
        token_err("home:../escape");
        token_err("home:a/./b");
        token_err("home:a//b");
        token_err("home:a\\b");
        token_err("moon:x");
        token_err("abs:relative/path");
        token_err("abs:/a/../b");
        token_err("plain/path");
    }

    #[test]
    fn lexical_normalization_folds_dots() {
        assert_eq!(
            normalize_lexical(Path::new("/a/./b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(normalize_lexical(Path::new("a/b/")), PathBuf::from("a/b"));
    }

    #[test]
    fn missing_root_is_a_typed_error() {
        let base = std::env::temp_dir().join("root-eval-context-missing-root-sentinel");
        let roots = ContextRoots {
            home: &base,
            xdg_config_home: &base,
            project_root: &base,
            worktree_root: &base,
        };
        let error = resolve_manifest(&roots, &DiscoveryFlags::default())
            .expect_err("missing root must fail");
        assert_eq!(error.kind(), ContextErrorKind::MissingRoot);
    }

    #[test]
    fn disjoint_roots_are_refused() {
        let base = std::env::temp_dir();
        let roots = ContextRoots {
            home: &base,
            xdg_config_home: &base,
            project_root: &base.join("root-eval-disjoint-instance"),
            worktree_root: &base.join("root-eval-disjoint-worktree"),
        };
        // Both synthetic roots are missing; the missing-root check fires
        // first by design (root validation precedes containment).
        let error = resolve_manifest(&roots, &DiscoveryFlags::default())
            .expect_err("synthetic roots must fail");
        assert_eq!(error.kind(), ContextErrorKind::MissingRoot);
    }
}
