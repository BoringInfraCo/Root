//! Trial isolation verification, residue scanning, and cleanup (M2, Track A).
//!
//! - [`verify_isolation`] proves two materialized trials share no files (every
//!   entry is canonicalized, so a symlink escape into the sibling trial is
//!   caught, not just identical roots);
//! - [`residue_scan`] lists trial directories, stale lock files, and
//!   leftover process markers under a base directory;
//! - [`cleanup_trial`] removes a trial root and verifies the removal;
//! - [`check_no_leak`] proves a before/after file listing contains no new
//!   file outside the trial root.
//!
//! This module performs reads and deletions only; it never spawns a process.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::trial::TrialLayout;

/// Machine-matchable classification for every [`IsolationError`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsolationErrorKind {
    /// Two trial roots overlap or are identical.
    OverlappingRoots,
    /// The two trials share at least one canonicalized file.
    SharedFiles,
    /// A trial file canonicalizes outside its own trial root.
    EscapeDetected,
    /// A before/after listing shows a new file outside the trial root.
    LeakDetected,
    /// Trial cleanup failed or the root is still present.
    CleanupFailed,
    /// An I/O operation failed.
    Io,
}

/// Errors produced by isolation verification, residue scanning, and cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IsolationError {
    #[error("trial roots overlap: `{left}` and `{right}`")]
    OverlappingRoots { left: PathBuf, right: PathBuf },

    #[error("trials share {count} file(s), e.g. `{example}`")]
    SharedFiles { count: usize, example: PathBuf },

    #[error("trial file `{path}` escapes its trial root `{root}`")]
    EscapeDetected { path: PathBuf, root: PathBuf },

    #[error("leak detected: {count} new file(s) outside the trial root, e.g. `{example}`")]
    LeakDetected { count: usize, example: PathBuf },

    #[error("cleanup failed for `{path}`: {reason}")]
    CleanupFailed { path: PathBuf, reason: String },

    #[error("i/o error for `{path}`: {message}")]
    Io { path: PathBuf, message: String },
}

impl IsolationError {
    /// The machine-matchable rule classification.
    pub fn kind(&self) -> IsolationErrorKind {
        match self {
            Self::OverlappingRoots { .. } => IsolationErrorKind::OverlappingRoots,
            Self::SharedFiles { .. } => IsolationErrorKind::SharedFiles,
            Self::EscapeDetected { .. } => IsolationErrorKind::EscapeDetected,
            Self::LeakDetected { .. } => IsolationErrorKind::LeakDetected,
            Self::CleanupFailed { .. } => IsolationErrorKind::CleanupFailed,
            Self::Io { .. } => IsolationErrorKind::Io,
        }
    }
}

/// Residue found by [`residue_scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residue {
    /// A trial directory still present under the base directory.
    TrialDir { path: PathBuf },
    /// A stale lock file (`*.lock`) or lock-heartbeat marker.
    StaleLock { path: PathBuf },
    /// A `pids.txt` marker whose content is not the clean `none` sentinel.
    LeftoverProcessMarker { path: PathBuf },
}

impl Residue {
    /// The offending path.
    pub fn path(&self) -> &Path {
        match self {
            Self::TrialDir { path }
            | Self::StaleLock { path }
            | Self::LeftoverProcessMarker { path } => path,
        }
    }
}

// ---------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------

/// Prove two trial layouts share no files.
///
/// Both roots are canonicalized and must be disjoint (neither equal nor a
/// prefix of the other). Every entry under each root is then canonicalized —
/// symlinks resolve — and any entry escaping its own root is an
/// [`IsolationError::EscapeDetected`]; any canonical path present in both
/// trees is [`IsolationError::SharedFiles`].
pub fn verify_isolation(a: &TrialLayout, b: &TrialLayout) -> Result<(), IsolationError> {
    let root_a = canonicalize(&a.root)?;
    let root_b = canonicalize(&b.root)?;
    if root_a == root_b || root_a.starts_with(&root_b) || root_b.starts_with(&root_a) {
        return Err(IsolationError::OverlappingRoots {
            left: root_a,
            right: root_b,
        });
    }
    let files_a = canonical_tree(&root_a)?;
    let files_b = canonical_tree(&root_b)?;
    let set_b: HashSet<&PathBuf> = files_b.iter().collect();
    let shared: Vec<&PathBuf> = files_a.iter().filter(|path| set_b.contains(path)).collect();
    if let Some(example) = shared.first() {
        return Err(IsolationError::SharedFiles {
            count: shared.len(),
            example: (*example).clone(),
        });
    }
    Ok(())
}

/// Remove a trial root recursively and verify it is gone.
///
/// Idempotent: a missing root is already clean. Any other failure, or a root
/// that still exists afterwards, is [`IsolationError::CleanupFailed`].
pub fn cleanup_trial(layout: &TrialLayout) -> Result<(), IsolationError> {
    match fs::remove_dir_all(&layout.root) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(IsolationError::CleanupFailed {
                path: layout.root.clone(),
                reason: error.to_string(),
            });
        }
    }
    if layout.root.exists() {
        return Err(IsolationError::CleanupFailed {
            path: layout.root.clone(),
            reason: "trial root still exists after removal".to_string(),
        });
    }
    Ok(())
}

/// Prove no file outside the trial root appeared between two listings.
///
/// Every path in `after` that is absent from `before` (compared
/// canonicalized when the file exists, lexically otherwise) must live under
/// the canonical trial root; otherwise [`IsolationError::LeakDetected`]
/// names an offending path. Callers snapshot whatever scope they police
/// (trial-scoped dirs for leak tests; wider scopes for runner integration).
pub fn check_no_leak(
    layout: &TrialLayout,
    before: &[PathBuf],
    after: &[PathBuf],
) -> Result<(), IsolationError> {
    let root = canonicalize(&layout.root)?;
    let before_set: HashSet<PathBuf> = before.iter().map(|path| key_of(path)).collect();
    let mut leaks: Vec<PathBuf> = Vec::new();
    for path in after {
        if before_set.contains(&key_of(path)) {
            continue;
        }
        let key = key_of(path);
        if !key.starts_with(&root) {
            leaks.push(path.clone());
        }
    }
    if let Some(example) = leaks.first() {
        return Err(IsolationError::LeakDetected {
            count: leaks.len(),
            example: example.clone(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Residue scan
// ---------------------------------------------------------------------------

/// Scan `base_dir` for trial residue.
///
/// Reports every immediate subdirectory as [`Residue::TrialDir`], every
/// `*.lock` file and `*heartbeat*` marker found by a bounded recursive walk
/// as [`Residue::StaleLock`], and every `pids.txt` whose trimmed content is
/// not `none` as [`Residue::LeftoverProcessMarker`]. Results are sorted by
/// path. A missing base directory is an I/O error, never an empty pass.
pub fn residue_scan(base_dir: &Path) -> Result<Vec<Residue>, IsolationError> {
    let entries = fs::read_dir(base_dir).map_err(|error| io_error(base_dir, error.to_string()))?;
    let mut residues: Vec<Residue> = Vec::new();
    let mut trial_dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(base_dir, error.to_string()))?;
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            residues.push(Residue::TrialDir { path: path.clone() });
            trial_dirs.push(path);
        }
    }
    for dir in trial_dirs {
        collect_lock_residue(&dir, 0, &mut residues)?;
    }
    residues.sort_by(|left, right| left.path().cmp(right.path()));
    Ok(residues)
}

fn collect_lock_residue(
    dir: &Path,
    depth: usize,
    out: &mut Vec<Residue>,
) -> Result<(), IsolationError> {
    const MAX_DEPTH: usize = 32;
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|error| io_error(dir, error.to_string()))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(dir, error.to_string()))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let file_type = fs::symlink_metadata(&path)
            .map_err(|error| io_error(&path, error.to_string()))?
            .file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_lock_residue(&path, depth + 1, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".lock") || name.contains("heartbeat") {
            out.push(Residue::StaleLock { path: path.clone() });
        } else if name == "pids.txt" {
            let content = fs::read_to_string(&path).unwrap_or_default();
            if content.trim() != "none" {
                out.push(Residue::LeftoverProcessMarker { path: path.clone() });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn io_error(path: &Path, message: String) -> IsolationError {
    IsolationError::Io {
        path: path.to_path_buf(),
        message,
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, IsolationError> {
    fs::canonicalize(path).map_err(|error| io_error(path, error.to_string()))
}

/// Canonical key for set comparison: canonicalized when the file exists,
/// lexical otherwise (covers deleted or never-created entries).
fn key_of(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Every entry under `root` (files, dirs, symlinks), canonicalized.
///
/// Symlink traversal is disabled during the walk; each entry is canonicalized
/// individually so escapes are attributed, not followed.
fn canonical_tree(root: &Path) -> Result<Vec<PathBuf>, IsolationError> {
    let mut out = vec![root.to_path_buf()];
    walk_canonical(root, 0, root, &mut out)?;
    Ok(out)
}

fn walk_canonical(
    dir: &Path,
    depth: usize,
    root: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), IsolationError> {
    const MAX_DEPTH: usize = 32;
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|error| io_error(dir, error.to_string()))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(dir, error.to_string()))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let canonical = canonicalize(&path)?;
        if !canonical.starts_with(root) {
            return Err(IsolationError::EscapeDetected {
                path: canonical,
                root: root.to_path_buf(),
            });
        }
        out.push(canonical.clone());
        let is_dir = fs::symlink_metadata(&path)
            .map_err(|error| io_error(&path, error.to_string()))?
            .file_type()
            .is_dir();
        // `file_type().is_dir()` is false for symlinks (symlink_metadata
        // does not follow), so symlinked directories are recorded but never
        // traversed: no infinite loops, no cross-trial walks.
        if is_dir {
            walk_canonical(&path, depth + 1, root, out)?;
        }
    }
    Ok(())
}
