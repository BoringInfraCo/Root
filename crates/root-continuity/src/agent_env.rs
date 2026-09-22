//! Agent-environment capture summary (Sprint 013).
//!
//! A durable, names-only projection of the resolved canonical agent
//! environment (`root-agent-bundle`). It records adapter id, version, role
//! names, skill names, MCP server ids, credential *names*, and policy keys.
//! Secret values are never captured; free text is scanned with the canonical
//! secret guard and refused if it looks like a value.

use anyhow::{bail, Result};
use root_agent_bundle::canonical::{looks_like_secret_value, CanonicalEnv};
use root_agent_bundle::project::resolve_env_source;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Distinguishable agent-environment capture outcome (Sprint 013 blocker 5).
///
/// `Absent` means no canonical environment exists (a genuine, benign absence).
/// `Captured` carries the names-only summary and the validated canonical
/// environment. Any present-but-unusable environment (malformed, unreadable,
/// secret-shaped, oversized) is an `Err` so the caller can abort atomically
/// rather than record a misleadingly complete checkpoint.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CaptureOutcome {
    Absent,
    Captured {
        summary: AgentEnvSummary,
        canonical: CanonicalEnv,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentEnvSummary {
    pub adapter: Option<String>,
    pub source_agent_version: Option<String>,
    pub instructions: Option<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub credential_refs: Vec<String>,
    pub policies: Vec<String>,
    pub path: Option<String>,
}

impl AgentEnvSummary {
    /// A `(none)`-style summary used when no canonical environment exists.
    pub fn empty() -> Self {
        Self {
            adapter: None,
            source_agent_version: None,
            instructions: None,
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            credential_refs: Vec::new(),
            policies: Vec::new(),
            path: None,
        }
    }

    /// True when nothing was captured (no adapter and no references).
    pub fn summary_is_empty(&self) -> bool {
        self.adapter.is_none()
            && self.source_agent_version.is_none()
            && self.instructions.is_none()
            && self.skills.is_empty()
            && self.mcp_servers.is_empty()
            && self.credential_refs.is_empty()
            && self.policies.is_empty()
            && self.path.is_none()
    }

    fn secret_shaped(&self) -> Option<&'static str> {
        let text = [
            self.adapter.as_deref(),
            self.source_agent_version.as_deref(),
            self.instructions.as_deref(),
            self.path.as_deref(),
        ];
        for value in text.into_iter().flatten() {
            if looks_like_secret_value(value) {
                return Some("summary field");
            }
        }
        for value in self
            .skills
            .iter()
            .chain(self.mcp_servers.iter())
            .chain(self.credential_refs.iter())
            .chain(self.policies.iter())
        {
            if looks_like_secret_value(value) {
                return Some("summary reference");
            }
        }
        None
    }
}

/// Capture the canonical agent environment as a distinguishable outcome.
///
/// Missing environment (`no canonical environment found`) yields
/// [`CaptureOutcome::Absent`], not an error. A present but unusable environment
/// (malformed, unreadable, secret-shaped, oversized) yields an `Err` whose
/// message is sanitized: it names what failed and where, but never echoes the
/// offending secret value.
pub fn capture(cwd: &Path, explicit: Option<&Path>) -> Result<CaptureOutcome> {
    match resolve_env_source(explicit, cwd) {
        Ok((path, env)) => {
            let summary = summary_from(&path, &env)?;
            Ok(CaptureOutcome::Captured {
                summary,
                canonical: env,
            })
        }
        Err(error) => {
            let text = format!("{error:#}");
            if text.contains("no canonical environment found") {
                Ok(CaptureOutcome::Absent)
            } else {
                Err(sanitize_capture_error(&text, cwd, explicit))
            }
        }
    }
}

/// Map a raw `resolve_env_source` failure onto a sanitized, category-only
/// error. The raw text is inspected but never included: TOML parse errors can
/// quote the offending line, which may itself be a secret value.
fn sanitize_capture_error(raw: &str, cwd: &Path, explicit: Option<&Path>) -> anyhow::Error {
    let location = match explicit {
        Some(path) => path.display().to_string(),
        None => cwd.join(".root").join("agent.toml").display().to_string(),
    };
    if raw.contains("looks like a secret") {
        anyhow::anyhow!(
            "Refusing to capture agent environment at {location}: a field looks like a secret \
             value. Root records names and references only, never credential values."
        )
    } else if raw.contains("exceeds size limit") {
        anyhow::anyhow!(
            "Refusing to capture agent environment at {location}: the file exceeds the size limit."
        )
    } else if raw.contains("symlink") {
        anyhow::anyhow!(
            "Refusing to capture agent environment at {location}: symlinked files are not allowed."
        )
    } else if raw.contains("not valid UTF-8") {
        anyhow::anyhow!(
            "Failed to capture agent environment at {location}: the file is not valid UTF-8."
        )
    } else if raw.contains("NotFound") || raw.contains("No such file") {
        anyhow::anyhow!(
            "Failed to capture agent environment at {location}: the file could not be read."
        )
    } else {
        anyhow::anyhow!(
            "Failed to capture agent environment at {location}: the canonical environment is \
             malformed or unreadable."
        )
    }
}

fn summary_from(path: &Path, env: &CanonicalEnv) -> Result<AgentEnvSummary> {
    let summary = AgentEnvSummary {
        adapter: Some(env.source_agent.clone()),
        source_agent_version: Some(env.source_agent_version.clone()),
        instructions: Some(env.instructions.main.clone()),
        skills: env.skills.iter().map(|skill| skill.name.clone()).collect(),
        mcp_servers: env.mcp_servers.keys().cloned().collect(),
        credential_refs: env.environment.needs_env.clone(),
        policies: env
            .policies
            .iter()
            .map(|policy| policy.key.clone())
            .collect(),
        path: Some(path.display().to_string()),
    };
    if let Some(field) = summary.secret_shaped() {
        bail!(
            "Refusing to capture agent environment: a {field} looks like a secret value. \
             Root records names and references only, never credential values."
        );
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use root_agent_bundle::canonical::{
        CanonicalEnvironment, CanonicalInstructions, CanonicalMcp, CanonicalPolicy,
        CanonicalProvenance, CanonicalSkill, CanonicalTool, CANONICAL_SCHEMA_VERSION,
    };
    use root_agent_bundle::manifest::SECRET_DISCLOSURE;
    use root_agent_bundle::project::emit_agent_toml;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    struct HomeGuard {
        previous: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let lock = crate::test_lock();
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", home);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    struct TempRepo {
        base: PathBuf,
        repo: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "root_continuity_agent_env_{tag}_{}",
                std::process::id()
            ));
            let repo = base.join("campfire");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(base.join("home")).unwrap();
            Self { base, repo }
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn base_env() -> CanonicalEnv {
        CanonicalEnv {
            schema_version: CANONICAL_SCHEMA_VERSION,
            source_agent: "codex".to_string(),
            source_agent_version: "0.150.1".to_string(),
            instructions: CanonicalInstructions {
                main: "AGENTS.md".to_string(),
            },
            skills: vec![CanonicalSkill {
                name: "docs-writer".to_string(),
                files: vec!["SKILL.md".to_string()],
            }],
            tools: vec![CanonicalTool {
                name: "model".to_string(),
                value: "gpt-5".to_string(),
            }],
            mcp_servers: {
                let mut map = BTreeMap::new();
                map.insert(
                    "github".to_string(),
                    CanonicalMcp {
                        id: "github".to_string(),
                        transport: "stdio".to_string(),
                        command: vec!["npx".to_string()],
                        args: vec!["-y".to_string(), "pkg".to_string()],
                        cwd: None,
                        credential_refs: vec!["GITHUB_TOKEN".to_string()],
                        environment: vec!["GITHUB_TOKEN".to_string()],
                        enabled: false,
                    },
                );
                map
            },
            policies: vec![CanonicalPolicy {
                key: "shell_environment_policy".to_string(),
                disposition: "held".to_string(),
                reason: String::new(),
            }],
            environment: CanonicalEnvironment {
                needs_env: vec!["GITHUB_TOKEN".to_string()],
            },
            provenance: CanonicalProvenance {
                captured_at: "1234567890".to_string(),
                bundle_hash: String::new(),
                disclosure: SECRET_DISCLOSURE.to_string(),
            },
        }
    }

    fn write_env(repo: &Path, env: &CanonicalEnv) {
        let dir = repo.join(".root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.toml"), emit_agent_toml(env).unwrap()).unwrap();
    }

    #[test]
    fn valid_env_file_populates_names_only() {
        let fixture = TempRepo::new("valid");
        let _guard = HomeGuard::set(&fixture.base.join("home"));
        let env = base_env();
        write_env(&fixture.repo, &env);

        let outcome = capture(&fixture.repo, None).unwrap();
        let CaptureOutcome::Captured { summary, canonical } = outcome else {
            panic!("expected Captured, got {outcome:?}");
        };
        assert_eq!(summary.adapter.as_deref(), Some("codex"));
        assert_eq!(summary.source_agent_version.as_deref(), Some("0.150.1"));
        assert_eq!(summary.instructions.as_deref(), Some("AGENTS.md"));
        assert_eq!(summary.skills, vec!["docs-writer".to_string()]);
        assert_eq!(summary.mcp_servers, vec!["github".to_string()]);
        assert_eq!(summary.credential_refs, vec!["GITHUB_TOKEN".to_string()]);
        assert_eq!(
            summary.policies,
            vec!["shell_environment_policy".to_string()]
        );
        assert!(summary.path.as_ref().unwrap().ends_with("agent.toml"));
        assert!(!summary.summary_is_empty());
        assert_eq!(canonical, env);
    }

    #[test]
    fn absent_env_file_is_absent_not_error() {
        let fixture = TempRepo::new("absent");
        let _guard = HomeGuard::set(&fixture.base.join("home"));
        assert_eq!(
            capture(&fixture.repo, None).unwrap(),
            CaptureOutcome::Absent
        );
    }

    #[test]
    fn secret_shaped_field_is_refused_without_echoing_value() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        let fixture = TempRepo::new("secret");
        let _guard = HomeGuard::set(&fixture.base.join("home"));
        let mut env = base_env();
        env.source_agent_version = secret.to_string();
        write_env(&fixture.repo, &env);

        let error = capture(&fixture.repo, None).unwrap_err().to_string();
        assert!(error.contains("looks like a secret"), "{error}");
        assert!(!error.contains(secret), "secret value leaked: {error}");
    }

    #[test]
    fn malformed_env_file_is_refused_with_sanitized_error() {
        let fixture = TempRepo::new("malformed");
        let _guard = HomeGuard::set(&fixture.base.join("home"));
        let dir = fixture.repo.join(".root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.toml"), "{ this is not valid toml").unwrap();

        let error = capture(&fixture.repo, None).unwrap_err().to_string();
        assert!(
            error.contains("malformed or unreadable"),
            "expected sanitized category, got: {error}"
        );
        assert!(
            error.contains("agent.toml"),
            "error must name where: {error}"
        );
    }
}
