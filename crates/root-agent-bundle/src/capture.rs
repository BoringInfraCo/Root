//! Capture: read-only proposal + names-only `agent.toml` write.
//!
//! Plan-first: default is a proposal (no writes, `mutated=false`). `--apply`
//! writes canonical TOML via [`crate::project::emit_agent_toml`] (regular
//! file only, no symlink follow, refuse-overwrite without `--force`, `.root/`
//! containment by default). Exact SOURCE gate on write; unsupported source
//! still proposes with `supported=false` + warning.

use crate::canonical::{canonical_adapter_id, version_supported, CanonicalEnv};
use crate::manifest::{supported_versions_for, SECRET_DISCLOSURE};
use crate::plan::HeldOut;
use crate::translate::build_canonical_from_inspect;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct InstructionSummary {
    pub role: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureProposal {
    pub from: String,
    pub version: Option<String>,
    pub supported: bool,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub instructions: Option<InstructionSummary>,
    pub env_vars: Vec<String>,
    pub policies: Vec<String>,
    pub held: Vec<HeldOut>,
    pub warnings: Vec<String>,
    pub disclosure: String,
    pub mutated: bool,
}

fn harness_main_for(adapter: &str) -> &'static str {
    match adapter {
        crate::manifest::CLAUDE_ADAPTER_ID => "CLAUDE.md",
        _ => "AGENTS.md",
    }
}

fn instruction_path_for(adapter: &str) -> Result<(crate::scope::Scope, String)> {
    match adapter {
        crate::manifest::ADAPTER_ID => {
            Ok((crate::scope::Scope::CodexHome, "AGENTS.md".to_string()))
        }
        crate::manifest::OPENCODE_ADAPTER_ID => {
            Ok((crate::scope::Scope::OpenCodeHome, "AGENTS.md".to_string()))
        }
        crate::manifest::CLAUDE_ADAPTER_ID => {
            Ok((crate::scope::Scope::ClaudeHome, "CLAUDE.md".to_string()))
        }
        other => Err(crate::manifest::unsupported_adapter_error(other)),
    }
}

fn secret_held_for(adapter: &str) -> Vec<HeldOut> {
    match adapter {
        crate::manifest::ADAPTER_ID => vec![
            HeldOut {
                source: "auth.json".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "sessions/, *.sqlite*, history.jsonl, cache/, logs/".to_string(),
                reason: "volatile: never inspected or exported".to_string(),
            },
        ],
        crate::manifest::OPENCODE_ADAPTER_ID => vec![
            HeldOut {
                source: "auth tokens, mcp-auth.json".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "sessions/, *.sqlite*, logs/, share state".to_string(),
                reason: "volatile: never inspected or exported".to_string(),
            },
        ],
        crate::manifest::CLAUDE_ADAPTER_ID => vec![
            HeldOut {
                source: ".credentials.json, macOS Keychain, ANTHROPIC_* tokens".to_string(),
                reason: "secret: never inspected or exported".to_string(),
            },
            HeldOut {
                source: "claude global state (sign-in, OAuth, projects, MCP)".to_string(),
                reason: crate::claude::CLAUDE_MCP_HELD_ERROR.to_string(),
            },
        ],
        _ => vec![],
    }
}

/// Read-only proposal (never writes, never locks). Builds via
/// `build_canonical_from_inspect` + counts; unsupported source yields
/// `supported=false` + warning but still proposes.
pub fn propose_capture(from: &str) -> Result<CaptureProposal> {
    let norm = canonical_adapter_id(from)?.to_string();
    let env: CanonicalEnv = build_canonical_from_inspect(&norm)?;
    let version = if env.source_agent_version == "unknown" {
        None
    } else {
        Some(env.source_agent_version.clone())
    };
    let supported = version
        .as_deref()
        .map(|v| version_supported(&norm, v))
        .unwrap_or(false);

    let mut warnings = Vec::new();
    if !supported {
        let supported_list = supported_versions_for(&norm)
            .map(|s| format!("{s:?}"))
            .unwrap_or_else(|_| "[]".to_string());
        warnings.push(format!(
            "source {norm} version '{}' is not in supported exact versions {}; capture proposal only",
            version.clone().unwrap_or_else(|| "unknown".to_string()),
            supported_list
        ));
    }

    let mut skills: Vec<String> = env.skills.iter().map(|s| s.name.clone()).collect();
    skills.sort();
    let mut mcp_servers: Vec<String> = env.mcp_servers.keys().cloned().collect();
    mcp_servers.sort();
    let env_vars = env.environment.needs_env.clone();
    let mut policies: Vec<String> = env.policies.iter().map(|p| p.key.clone()).collect();
    policies.sort();

    // Instructions summary (size + hash, names only otherwise).
    let (scope, rel) = instruction_path_for(&norm)?;
    let instructions = match crate::scope::resolve_target(scope, &rel) {
        Err(e) => {
            // Resolve failures (symlink/escape) are source errors, not holds.
            // Absent files are handled below; other errors propagate.
            // resolve_target Ok for missing tails, so Err here is fail-closed.
            return Err(e);
        }
        Ok(target) => match std::fs::symlink_metadata(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("Failed to stat instructions file")?,
            Ok(m) => {
                if m.file_type().is_symlink() {
                    anyhow::bail!(
                        "Refusing to capture symlink {} (bundle v1 rejects all symlinks)",
                        target.display()
                    );
                }
                if !m.is_file() {
                    anyhow::bail!("instructions path is not a regular file");
                }
                if m.len() > crate::manifest::MAX_FILE_BYTES {
                    anyhow::bail!("instructions file exceeds per-file limit");
                }
                let bytes = std::fs::read(&target).context("Failed to read instructions")?;
                Some(InstructionSummary {
                    role: harness_main_for(&norm).to_string(),
                    size: bytes.len() as u64,
                    sha256: root_lockfile::compute_sha256(&bytes),
                })
            }
        },
    };

    let mut held = secret_held_for(&norm);
    for p in &env.policies {
        held.push(HeldOut {
            source: format!("policy:{}", p.key),
            reason: if p.reason.is_empty() {
                "held; never auto-applied".to_string()
            } else {
                format!("held; never auto-applied: {}", p.reason)
            },
        });
    }
    for s in &env.skills {
        held.push(HeldOut {
            source: format!("skill:{}", s.name),
            reason:
                "skill content requires hash-bound approval at apply; content-approval deferred"
                    .to_string(),
        });
    }

    Ok(CaptureProposal {
        from: norm,
        version,
        supported,
        skills,
        mcp_servers,
        instructions,
        env_vars,
        policies,
        held,
        warnings,
        disclosure: SECRET_DISCLOSURE.to_string(),
        mutated: false,
    })
}

/// Walk ancestors of `start` looking for a `.git` entry (directory OR file,
/// symlinks included) using `symlink_metadata`; stop at the filesystem root.
/// `start` may be a file, a directory, or a not-yet-existing destination.
pub fn resolve_repo_root(start: &Path) -> Option<PathBuf> {
    let base = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    // Begin at the path's parent when it names an existing regular file.
    let mut dir = if base.is_file() {
        base.parent().map(Path::to_path_buf).unwrap_or(base)
    } else {
        base
    };
    loop {
        if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(dir);
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// Lexically normalize `path`, rejecting any `..` that would escape above the
/// filesystem root (or the leading relative base).
fn lexical_normalize(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    anyhow::bail!(
                        "refusing to write agent environment outside repo: '{}' escapes via '..'",
                        path.display()
                    );
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    Ok(out)
}

/// Real containment check for capture writes: no repo root found → Err unless
/// `allow_outside_repo`; otherwise the destination must be strictly inside
/// `<repo>/.root/` with no `..` escape and no symlink component between the
/// repo root and the destination, and its nearest existing ancestor must
/// canonicalize inside `<repo>/.root/`.
fn check_capture_containment(out: &Path, allow_outside_repo: bool) -> Result<()> {
    if allow_outside_repo {
        return Ok(());
    }
    let outside = || {
        anyhow::anyhow!(
            "refusing to write agent environment outside repo: '{}' is not inside a .root/ directory (pass --allow-outside-repo to override)",
            out.display()
        )
    };
    let absolute = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to determine current directory")?
            .join(out)
    };
    let normalized = lexical_normalize(&absolute)?;
    let Some(repo) = resolve_repo_root(&absolute) else {
        return Err(outside());
    };
    let repo_norm = lexical_normalize(&repo)?;
    let dot_root = repo_norm.join(".root");
    // Strictly inside <repo>/.root/ and not .root itself.
    if normalized == dot_root || !normalized.starts_with(&dot_root) {
        return Err(outside());
    }
    // Reject any symlink component between repo root and destination.
    if let Ok(rest) = normalized.strip_prefix(&repo_norm) {
        let mut prefix = repo_norm.clone();
        for comp in rest.components() {
            prefix.push(comp);
            if let Ok(m) = std::fs::symlink_metadata(&prefix) {
                if m.file_type().is_symlink() {
                    anyhow::bail!(
                        "refusing to write agent environment outside repo: '{}' passes through symlink component '{}' (pass --allow-outside-repo to override)",
                        out.display(),
                        prefix.display()
                    );
                }
            }
        }
    }
    // Canonicalize repo root + nearest existing ancestor; require containment.
    if let Ok(canon_repo) = std::fs::canonicalize(&repo_norm) {
        let mut nearest = normalized.clone();
        while std::fs::symlink_metadata(&nearest).is_err() {
            match nearest.parent() {
                Some(p) if !p.as_os_str().is_empty() => nearest = p.to_path_buf(),
                _ => break,
            }
        }
        if let Ok(canon_nearest) = std::fs::canonicalize(&nearest) {
            // `.root` may not exist yet, so the nearest existing ancestor can be
            // the repo root itself. The lexical check above already guarantees
            // the destination sits under `<repo>/.root/`; here we only need to
            // confirm no symlinked ancestor redirected us outside the repo.
            if !canon_nearest.starts_with(&canon_repo) {
                anyhow::bail!(
                    "refusing to write agent environment outside repo: '{}' resolves outside .root/ (symlink escape)",
                    out.display()
                );
            }
        }
    }
    Ok(())
}

/// Exact SOURCE gate + names-only write. Refuses existing without `force`.
/// Default requires `out` inside a `.root/` ancestor; `allow_outside_repo`
/// bypasses (explicit flag only). Writes `emit_agent_toml` bytes (regular
/// file only, no symlink follow). Literal secrets refuse with work-state
/// style message. Returns the env written.
pub fn write_agent_toml(
    from: &str,
    out: &Path,
    force: bool,
    allow_outside_repo: bool,
) -> Result<CanonicalEnv> {
    let norm = canonical_adapter_id(from)?.to_string();
    let env = build_canonical_from_inspect(&norm)?;
    // Exact SOURCE gate (no write on unsupported).
    let supported_list = supported_versions_for(&norm)?;
    if !supported_list.contains(&env.source_agent_version.as_str()) {
        anyhow::bail!(
            "unsupported source agent version '{}' for adapter '{}'. Supported exact versions: {:?}",
            env.source_agent_version,
            norm,
            supported_list
        );
    }
    // Containment (default).
    check_capture_containment(out, allow_outside_repo)?;
    // Refuse overwrite without force; never follow symlinks.
    match std::fs::symlink_metadata(out) {
        Ok(m) => {
            if m.file_type().is_symlink() {
                anyhow::bail!(
                    "refusing to write agent environment through symlink '{}'",
                    out.display()
                );
            }
            if !force {
                anyhow::bail!(
                    "refusing to overwrite existing agent environment '{}' without --force",
                    out.display()
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("Failed to stat output path")?,
    }
    let text = crate::project::emit_agent_toml(&env)?;
    // Names-only grep guard (defense-in-depth; emit already validates).
    for pat in ["sk-", "ghp_", "github_pat_", "Bearer ", "--token="] {
        if text.contains(pat) {
            anyhow::bail!(
                "refusing to write agent environment: emitted TOML looks like a secret value ({pat} present); canonical files carry names only"
            );
        }
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).context("Failed to create output directory")?;
            // Ensure parent chain has no symlink escape for the final parent?
            // Best-effort: if parent exists and is symlink, refuse.
            if let Ok(m) = std::fs::symlink_metadata(parent) {
                if m.file_type().is_symlink() {
                    anyhow::bail!(
                        "refusing to write agent environment through symlink parent '{}'",
                        parent.display()
                    );
                }
            }
        }
    }
    // Atomic write beside target (temp + rename), 0644 for checked-in file.
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        out.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        let mut f = opts
            .open(&tmp)
            .context("Failed to create temp agent.toml")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        }
        use std::io::Write;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, out).context("Failed to rename agent.toml into place")?;
    if let Ok(d) = std::fs::File::open(parent) {
        let _ = d.sync_all();
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvRestore {
        fn save(keys: &[&'static str]) -> Self {
            Self {
                saved: keys.iter().map(|k| (*k, std::env::var_os(k))).collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn create(label: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "root-capture-012-{}-{}-{}-{}",
                label,
                std::process::id(),
                nanos,
                id
            ));
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(&path)
                    .unwrap();
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&path).unwrap();
            }
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    const ISOLATED_KEYS: &[&str] = &[
        "HOME",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "CLAUDE_CONFIG_DIR",
        "ROOT_DIR",
        "TMPDIR",
        "PATH",
    ];

    fn isolate_env(tmp: &TempDir, extra_path: Option<&Path>) {
        let home = tmp.path().join("home");
        let codex = tmp.path().join("codex");
        let xdg = tmp.path().join("xdg");
        let oc = tmp.path().join("oc");
        let cl = tmp.path().join("cl");
        let root = tmp.path().join("root");
        let tdir = tmp.path().join("tmp");
        for d in [&home, &codex, &xdg, &oc, &cl, &root, &tdir] {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(d)
                    .unwrap();
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(d).unwrap();
            }
        }
        std::env::set_var("HOME", &home);
        std::env::set_var("CODEX_HOME", &codex);
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::set_var("XDG_DATA_HOME", &xdg);
        std::env::set_var("OPENCODE_CONFIG_DIR", &oc);
        std::env::set_var("OPENCODE_DISABLE_AUTOUPDATE", "1");
        std::env::set_var("CLAUDE_CONFIG_DIR", &cl);
        std::env::set_var("ROOT_DIR", &root);
        std::env::set_var("TMPDIR", &tdir);
        if let Some(bin) = extra_path {
            let mut dirs: Vec<PathBuf> = vec![bin.to_path_buf()];
            if let Some(old) = std::env::var_os("PATH") {
                if !old.is_empty() {
                    dirs.extend(std::env::split_paths(&old));
                }
            }
            if let Ok(joined) = std::env::join_paths(&dirs) {
                std::env::set_var("PATH", joined);
            } else {
                std::env::set_var("PATH", bin);
            }
        } else {
            std::env::set_var("PATH", "/usr/bin:/bin");
        }
    }

    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn capture_proposal_changes_nothing() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("proposal");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        write_executable(
            &bindir.join("codex"),
            "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n",
        );
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# agents\n").unwrap();
        std::fs::write(codex_home.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
        let root = PathBuf::from(std::env::var_os("ROOT_DIR").unwrap());
        let before = dir_snapshot(&root);

        let proposal = propose_capture("codex").unwrap();
        assert!(!proposal.mutated);
        assert_eq!(proposal.from, "codex");
        assert!(proposal.supported);
        assert_eq!(proposal.disclosure, SECRET_DISCLOSURE);

        let after = dir_snapshot(&root);
        assert_eq!(before, after, "proposal must write nothing under ROOT_DIR");
        // No agent.toml written anywhere near tmp (outside .root).
        assert!(!tmp.path().join("agent.toml").exists());
    }

    fn dir_snapshot(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if !dir.exists() {
            return out;
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(cur) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&cur) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                out.push(p.display().to_string());
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn capture_write_names_only_overwrite_and_outside_repo() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("write");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        write_executable(
            &bindir.join("codex"),
            "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n",
        );
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# agents\n").unwrap();
        std::fs::write(
            codex_home.join("config.toml"),
            "model = \"gpt-5\"\n[mcp_servers.github]\ncommand = \"npx\"\nargs = [\"-y\", \"pkg\"]\nenv_vars = [\"GITHUB_TOKEN\"]\n",
        )
        .unwrap();

        // Inside repo .root → ok.
        let repo = tmp.path().join("repo");
        let dotroot = repo.join(".root");
        std::fs::create_dir_all(&dotroot).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let out = dotroot.join("agent.toml");
        let env = write_agent_toml("codex", &out, false, false).unwrap();
        assert_eq!(env.source_agent, "codex");
        let text = std::fs::read_to_string(&out).unwrap();
        for pat in ["sk-", "ghp_", "Bearer", "--token="] {
            assert!(
                !text.contains(pat),
                "written TOML must be names-only, found {pat}"
            );
        }
        assert!(
            text.contains("GITHUB_TOKEN"),
            "names must be present, got:\n{text}"
        );
        assert!(
            text.starts_with(&format!("# {}", SECRET_DISCLOSURE)),
            "disclosure comment on top"
        );

        // Refuse overwrite without force.
        let err = write_agent_toml("codex", &out, false, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("without --force"), "got: {err}");
        // Force ok.
        write_agent_toml("codex", &out, true, false).unwrap();

        // Outside repo → default Err, allow flag ok.
        let outside = tmp.path().join("outside.toml");
        let err = write_agent_toml("codex", &outside, false, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("outside repo") || err.contains(".root"),
            "got: {err}"
        );
        write_agent_toml("codex", &outside, false, true).unwrap();
        assert!(outside.exists());
    }

    #[test]
    fn capture_write_unsupported_source_fails_closed() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("unsupported");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        write_executable(
            &bindir.join("codex"),
            "#!/bin/sh\nprintf 'codex-cli 9.9.9\\n'\n",
        );
        isolate_env(&tmp, Some(&bindir));
        // Proposal still prints (warn) with supported=false.
        let proposal = propose_capture("codex").unwrap();
        assert!(!proposal.supported);
        assert!(!proposal.warnings.is_empty());
        // Write refuses.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        let out = repo.join(".root").join("agent.toml");
        let err = write_agent_toml("codex", &out, false, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported source agent version"),
            "got: {err}"
        );
        assert!(!out.exists(), "no write on unsupported source");
    }

    #[test]
    fn work_state_secret_note_refused() {
        // Direct emit refusal for secret-shaped free text (policy reason).
        use crate::canonical::{
            CanonicalEnv, CanonicalEnvironment, CanonicalInstructions, CanonicalPolicy,
            CanonicalProvenance, CANONICAL_SCHEMA_VERSION,
        };
        use std::collections::BTreeMap;
        let env = CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
            source_agent: "codex".to_string(),
            source_agent_version: "0.150.1".to_string(),
            instructions: CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: vec![],
            tools: vec![],
            mcp_servers: BTreeMap::new(),
            policies: vec![CanonicalPolicy {
                key: "note".to_string(),
                disposition: "held".to_string(),
                reason: "api_secret = 'abcdefghij'".to_string(),
            }],
            environment: CanonicalEnvironment { needs_env: vec![] },
            provenance: CanonicalProvenance {
                captured_at: "1234567890".to_string(),
                bundle_hash: String::new(),
                disclosure: SECRET_DISCLOSURE.to_string(),
            },
        };
        let err = crate::project::emit_agent_toml(&env)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("refusing to write agent environment")
                && err.contains("looks like a secret"),
            "got: {err}"
        );
    }

    #[test]
    fn capture_containment_rejects_dotdot_escape() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-dotdot");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let escape = repo.join(".root").join("..").join("outside.toml");
        let err = check_capture_containment(&escape, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside repo"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn capture_containment_rejects_symlinked_ancestor() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-symlink");
        let repo = tmp.path().join("repo");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // .root is a symlink pointing outside the repo.
        std::os::unix::fs::symlink(&outside, repo.join(".root")).unwrap();
        let out = repo.join(".root").join("agent.toml");
        let err = check_capture_containment(&out, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("outside repo") || err.contains("symlink"),
            "got: {err}"
        );
    }

    #[test]
    fn capture_containment_accepts_repo_dot_root() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-ok");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".root")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let out = repo.join(".root").join("agent.toml");
        check_capture_containment(&out, false).unwrap();
        // .root itself is not a valid file destination.
        assert!(check_capture_containment(&repo.join(".root"), false).is_err());
    }

    #[test]
    fn capture_containment_accepts_missing_dot_root() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-missing-dotroot");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // `.root` does not exist yet (the common fresh-clone case). The
        // destination must still be accepted, including on macOS where the
        // temp dir is reached through a symlinked ancestor.
        let out = repo.join(".root").join("agent.toml");
        check_capture_containment(&out, false).unwrap();
    }

    #[test]
    fn write_agent_toml_creates_missing_dot_root() {
        let _lock = crate::lock_env();
        let _restore = EnvRestore::save(ISOLATED_KEYS);
        let tmp = TempDir::create("write-missing-dotroot");
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        write_executable(
            &bindir.join("codex"),
            "#!/bin/sh\nprintf 'codex-cli 0.150.1\\n'\n",
        );
        isolate_env(&tmp, Some(&bindir));
        let codex_home = PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        std::fs::write(codex_home.join("AGENTS.md"), "# agents\n").unwrap();

        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // No `.root` directory exists yet.
        let out = repo.join(".root").join("agent.toml");
        write_agent_toml("codex", &out, false, false).unwrap();
        assert!(
            out.is_file(),
            "capture must create the missing .root directory"
        );
    }

    #[test]
    fn capture_containment_requires_repo() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-norepo");
        let dotroot = tmp.path().join("norepo").join(".root");
        std::fs::create_dir_all(&dotroot).unwrap();
        let out = dotroot.join("agent.toml");
        assert!(
            resolve_repo_root(&out).is_none(),
            "test fixture must not sit inside a git repo"
        );
        let err = check_capture_containment(&out, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside repo"), "got: {err}");
    }

    #[test]
    fn allow_outside_repo_bypasses() {
        let _lock = crate::lock_env();
        let tmp = TempDir::create("contain-bypass");
        let out = tmp.path().join("anywhere.toml");
        // No repo root, no .root component: still allowed when bypassed.
        check_capture_containment(&out, true).unwrap();
    }
}
