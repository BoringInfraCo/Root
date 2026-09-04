//! Constrained bundle targets (S1 amendment).
//!
//! Manifest targets MUST use this closed scope enum plus a validated relative
//! path. Interpolated strings such as `"$CODEX_HOME/AGENTS.md"` are forbidden
//! in bundle v1.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Closed set of writable scopes for S1/S2 (Codex + OpenCode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum Scope {
    /// `$CODEX_HOME` (`$CODEX_HOME` env or `~/.codex`).
    CodexHome,
    /// OpenCode global config dir (`$OPENCODE_CONFIG_DIR`, else
    /// `$XDG_CONFIG_HOME/opencode`, else `$HOME/.config/opencode`).
    OpenCodeHome,
    /// `~/.agents/skills` (shared skill library).
    SharedSkills,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::CodexHome => "codex_home",
            Scope::OpenCodeHome => "opencode_home",
            Scope::SharedSkills => "shared_skills",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "codex_home" => Ok(Scope::CodexHome),
            "opencode_home" => Ok(Scope::OpenCodeHome),
            "shared_skills" => Ok(Scope::SharedSkills),
            other => anyhow::bail!(
                "unsupported bundle scope '{}'. Supported scopes: codex_home, opencode_home, shared_skills",
                other
            ),
        }
    }
}

/// Resolve the filesystem root for a scope.
///
/// Honors `$CODEX_HOME` for `CodexHome` and the OpenCode config-dir
/// resolution order for `OpenCodeHome`. Returns an error if the home
/// directory cannot be determined.
pub fn scope_root(scope: Scope) -> Result<PathBuf> {
    match scope {
        Scope::CodexHome => {
            if let Some(val) = std::env::var_os("CODEX_HOME") {
                let p = PathBuf::from(val);
                if p.as_os_str().is_empty() {
                    anyhow::bail!("CODEX_HOME is set but empty");
                }
                return Ok(p);
            }
            let home = dirs::home_dir().context("Could not determine home directory")?;
            Ok(home.join(".codex"))
        }
        Scope::OpenCodeHome => opencode_config_dir(),
        Scope::SharedSkills => {
            let home = dirs::home_dir().context("Could not determine home directory")?;
            Ok(home.join(".agents").join("skills"))
        }
    }
}

/// OpenCode global config dir. Never uses macOS `dirs::config_dir()`
/// (`~/Library/Application Support`); OpenCode uses `~/.config/opencode`.
///
/// Order: `$OPENCODE_CONFIG_DIR` if non-empty, else `$XDG_CONFIG_HOME/opencode`
/// if `XDG_CONFIG_HOME` is non-empty, else `$HOME/.config/opencode`.
pub fn opencode_config_dir() -> Result<PathBuf> {
    if let Some(val) = std::env::var_os("OPENCODE_CONFIG_DIR") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
    if let Some(val) = std::env::var_os("XDG_CONFIG_HOME") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join("opencode"));
        }
    }
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".config").join("opencode"))
}

/// Validate a bundle-relative path (the `rel` half of a target).
///
/// Rejects: empty, absolute, `..`/prefix/root/curdir components (only
/// `Normal` allowed), backslashes, NUL bytes, length > 1024 bytes,
/// trailing slashes, and `.tmp` staging suffixes.
pub fn validate_rel(rel: &str) -> Result<()> {
    if rel.is_empty() {
        anyhow::bail!("invalid bundle path: empty relative path");
    }
    if rel.len() > 1024 {
        anyhow::bail!("invalid bundle path '{}': exceeds 1024 bytes", rel);
    }
    if rel.contains('\0') {
        anyhow::bail!("invalid bundle path: contains NUL byte");
    }
    if rel.contains('\\') {
        anyhow::bail!("invalid bundle path '{}': backslashes are forbidden", rel);
    }
    // Reject `$` (env interpolation) and `~` (home expansion): targets are
    // `{scope, rel}` only; interpolated strings are forbidden in bundle v1.
    if rel.contains('$') || rel.contains('~') {
        anyhow::bail!(
            "invalid bundle path '{}': interpolation characters are forbidden; use {{scope, rel}} targets",
            rel
        );
    }
    if rel.ends_with('/') {
        anyhow::bail!("invalid bundle path '{}': trailing slash", rel);
    }
    if rel.ends_with(".tmp") {
        anyhow::bail!(
            "invalid bundle path '{}': .tmp suffix is reserved for staging",
            rel
        );
    }
    // Reject `.` segments lexically (Path::components normalizes them away).
    if rel == "." || rel.starts_with("./") || rel.contains("/./") || rel.ends_with("/.") {
        anyhow::bail!("invalid bundle path '{}': non-normal component", rel);
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        anyhow::bail!(
            "invalid bundle path '{}': absolute paths are forbidden",
            rel
        );
    }
    let mut has_normal = false;
    for comp in path.components() {
        match comp {
            Component::Normal(_) => has_normal = true,
            Component::ParentDir => {
                anyhow::bail!("invalid bundle path '{}': '..' is forbidden", rel);
            }
            Component::RootDir | Component::Prefix(_) | Component::CurDir => {
                anyhow::bail!("invalid bundle path '{}': non-normal component", rel);
            }
        }
    }
    if !has_normal {
        anyhow::bail!("invalid bundle path '{}': no normal component", rel);
    }
    Ok(())
}

/// Resolve `(scope, rel)` to an absolute target path.
///
/// Validates `rel`, then joins onto the scope root. Lexical containment is
/// NOT sufficient: every *existing* path component at or below the scope root
/// is checked with `symlink_metadata` and symlinks are rejected, so an
/// attacker-planted ancestor symlink (e.g. `~/.agents/skills/foo` pointing
/// outside the scope) cannot redirect writes. Components above the scope root
/// (e.g. a symlinked `/tmp` on macOS) are intentionally not inspected.
/// Canonicalization is used only to verify containment of the nearest
/// existing ancestor; nothing is created by this function.
pub fn resolve_target(scope: Scope, rel: &str) -> Result<PathBuf> {
    validate_rel(rel)?;
    let root = scope_root(scope)?;
    let joined = root.join(rel);
    if !joined.starts_with(&root) {
        anyhow::bail!("invalid bundle target: escapes scope root");
    }
    validate_descendants(&root, Path::new(rel))?;
    Ok(joined)
}

/// Revalidate an already-resolved target immediately before filesystem I/O.
///
/// The scope root itself is intentionally allowed to be a symlink (for
/// example, a user may place `$CODEX_HOME` on another volume). Every existing
/// component *below* that root is required to be a real directory, except for
/// the final component which may be a regular file. Callers should invoke this
/// as close as possible to each read, create, rename, or delete; the global
/// Root mutation lock only excludes other Root writers, not arbitrary local
/// processes.
pub fn revalidate_target(scope: Scope, rel: &str, expected: &Path) -> Result<()> {
    validate_rel(rel)?;
    let root = scope_root(scope)?;
    let joined = root.join(rel);
    if joined != expected {
        anyhow::bail!(
            "refusing target '{}': resolved path changed (expected '{}')",
            joined.display(),
            expected.display()
        );
    }
    validate_descendants(&root, Path::new(rel))
}

fn validate_descendants(root: &Path, rel: &Path) -> Result<()> {
    // Collect rel components (all Normal, per validate_rel).
    let rel_comps: Vec<_> = rel.components().map(|c| c.as_os_str()).collect();
    // Check each existing prefix at/below root for symlinks or file-in-middle.
    let mut prefix = root.to_path_buf();
    for (i, comp) in rel_comps.iter().enumerate() {
        prefix.push(comp);
        match std::fs::symlink_metadata(&prefix) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Nonexistent tail: nothing further can be a symlink.
                // (TOCTOU between check and write is best-effort; the global
                // mutation lock excludes concurrent Root writers.)
                break;
            }
            Err(e) => {
                return Err(e).context(format!("Cannot stat {}", prefix.display()));
            }
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    anyhow::bail!(
                        "refusing target '{}': ancestor component is a symlink (bundle v1 rejects all symlinks)",
                        prefix.display()
                    );
                }
                let last = i + 1 == rel_comps.len();
                if !last && !meta.is_dir() {
                    anyhow::bail!(
                        "refusing target '{}': ancestor component is not a directory",
                        prefix.display()
                    );
                }
            }
        }
    }
    // Containment of the nearest existing ancestor against the canonical root.
    if let Ok(canon_root) = std::fs::canonicalize(root) {
        let mut nearest_existing = root.to_path_buf();
        let mut probe = root.to_path_buf();
        for comp in &rel_comps {
            let next = probe.join(comp);
            if std::fs::symlink_metadata(&next).is_ok() {
                probe = next;
                nearest_existing = probe.clone();
            } else {
                break;
            }
        }
        if let Ok(canon_nearest) = std::fs::canonicalize(&nearest_existing) {
            if !canon_nearest.starts_with(&canon_root) {
                anyhow::bail!("invalid bundle target: escapes scope root");
            }
        }
    }
    Ok(())
}

/// Detect duplicate `scope:rel` targets, including case-fold collisions
/// (macOS APFS is case-insensitive; Linux ext4 is not).
pub fn check_duplicates(targets: &[(Scope, String)]) -> Result<()> {
    let mut exact: HashSet<String> = HashSet::new();
    let mut folded: HashSet<String> = HashSet::new();
    for (scope, rel) in targets {
        let key = format!("{}:{}", scope.as_str(), rel);
        if !exact.insert(key.clone()) {
            anyhow::bail!("invalid bundle: duplicate target '{}'", key);
        }
        let fold_key = format!("{}:{}", scope.as_str(), rel.to_lowercase());
        if !folded.insert(fold_key.clone()) {
            anyhow::bail!(
                "invalid bundle: casing collision on target '{}' (macOS/Linux portability)",
                key
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "root_scope_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn scope_parse_roundtrip() {
        assert_eq!(Scope::parse("codex_home").unwrap(), Scope::CodexHome);
        assert_eq!(Scope::parse("opencode_home").unwrap(), Scope::OpenCodeHome);
        assert_eq!(Scope::parse("shared_skills").unwrap(), Scope::SharedSkills);
        assert!(Scope::parse("$CODEX_HOME/AGENTS.md").is_err());
        assert!(Scope::parse("codex").is_err());
        assert!(Scope::parse("opencode").is_err());
    }

    #[test]
    fn rel_validation_rejects_traversal_and_abs() {
        for bad in [
            "",
            "/abs/path",
            "../escape",
            "a/../../b",
            "a\\b",
            "trail/",
            "x.tmp",
            ".",
            "a/./b",
        ] {
            assert!(validate_rel(bad).is_err(), "should reject '{}'", bad);
        }
        for good in ["AGENTS.md", "skills/foo/SKILL.md", "a/b/c.txt"] {
            assert!(validate_rel(good).is_ok(), "should accept '{}'", good);
        }
    }

    #[test]
    fn duplicates_and_case_collisions_rejected() {
        let dup = vec![
            (Scope::CodexHome, "AGENTS.md".to_string()),
            (Scope::CodexHome, "AGENTS.md".to_string()),
        ];
        assert!(check_duplicates(&dup).is_err());
        let case = vec![
            (Scope::CodexHome, "AGENTS.md".to_string()),
            (Scope::CodexHome, "agents.md".to_string()),
        ];
        assert!(check_duplicates(&case).is_err());
        let ok = vec![
            (Scope::CodexHome, "AGENTS.md".to_string()),
            (Scope::SharedSkills, "foo/SKILL.md".to_string()),
        ];
        assert!(check_duplicates(&ok).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn descendant_symlink_is_rejected_but_scope_root_symlink_is_allowed() {
        use std::os::unix::fs::symlink;

        let tmp = unique_tmp("symlink");
        let real_root = tmp.join("real-root");
        let root_link = tmp.join("root-link");
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&real_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&real_root, &root_link).unwrap();

        validate_descendants(&root_link, Path::new("safe/file.txt")).unwrap();

        symlink(&outside, real_root.join("escape")).unwrap();
        let err = validate_descendants(&root_link, Path::new("escape/file.txt")).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_in_middle_is_rejected() {
        let tmp = unique_tmp("file_middle");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("not-a-dir"), b"x").unwrap();
        let err = validate_descendants(&tmp, Path::new("not-a-dir/file.txt")).unwrap_err();
        assert!(err.to_string().contains("not a directory"), "got: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
