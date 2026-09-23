//! Browser session grants.
//!
//! Observe and act are separate. A grant names a visible target and an expiry.
//! This does not attach to a desktop or a live browser.

use crate::auth;
use crate::policy;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    id: String,
    kind: String,
    target: String,
    expires_at: String,
    revoked: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct GrantFile {
    grants: Vec<Grant>,
}

#[derive(Debug, Serialize)]
pub struct GrantView {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub expires_at: String,
    pub active: bool,
}

pub fn grant(kind: &str, target: &str, minutes: u64) -> Result<GrantView> {
    if kind != "observe" && kind != "act" {
        anyhow::bail!("grant kind must be observe or act");
    }
    if !(1..=240).contains(&minutes) {
        anyhow::bail!("minutes must be 1..=240");
    }
    check_target(target)?;
    let mut file = read()?;
    let expires = Utc::now() + Duration::minutes(minutes as i64);
    let grant = Grant {
        id: format!("root_cs_{}", auth::random_hex(8)?),
        kind: kind.to_string(),
        target: target.to_string(),
        expires_at: expires.to_rfc3339(),
        revoked: false,
    };
    let view = view(&grant);
    file.grants.push(grant);
    write(&file)?;
    Ok(view)
}

pub fn session() -> Result<Vec<GrantView>> {
    Ok(read()?.grants.iter().map(view).collect())
}

pub fn revoke(id: &str) -> Result<GrantView> {
    let mut file = read()?;
    let Some(grant) = file.grants.iter_mut().find(|grant| grant.id == id) else {
        anyhow::bail!("unknown browser session {id}");
    };
    grant.revoked = true;
    let view = view(grant);
    write(&file)?;
    Ok(view)
}

pub fn require(kind: &str, target: &str) -> Result<()> {
    check_target(target)?;
    let needed = match kind {
        "observe" => "observe",
        "act" | "elevated" => "act",
        other => anyhow::bail!("unknown grant {other}"),
    };
    let file = read()?;
    let matched = file.grants.iter().any(|grant| {
        !grant.revoked
            && active(grant)
            && covers(&grant.target, target)
            && (grant.kind == needed || (needed == "observe" && grant.kind == "act"))
    });
    if !matched {
        anyhow::bail!("grant required: {needed} {target}");
    }
    Ok(())
}

fn covers(grant_target: &str, request: &str) -> bool {
    request == grant_target || request.starts_with(&format!("{grant_target}/"))
}

fn active(grant: &Grant) -> bool {
    DateTime::parse_from_rfc3339(&grant.expires_at)
        .map(|expires| expires > Utc::now())
        .unwrap_or(false)
}

fn check_target(target: &str) -> Result<()> {
    if root_work::secrets::detect(target).is_some() {
        anyhow::bail!("refusing a target that looks like a secret");
    }
    if !acceptable_origin(target) {
        anyhow::bail!("target must be an https origin or http://localhost");
    }
    if target.ends_with('/') {
        anyhow::bail!("target origin must not end with /");
    }
    Ok(())
}

/// `https://…`, or `http://localhost` with an optional numeric port, path, or query.
/// The host must stay localhost: `http://localhost.evil` and
/// `http://localhost:80@evil.com` are not localhost.
fn acceptable_origin(target: &str) -> bool {
    if let Some(rest) = target.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let Some(rest) = target.strip_prefix("http://localhost") else {
        return false;
    };
    match rest.chars().next() {
        None => true,
        Some('/' | '?') => true,
        Some(':') => localhost_port(&rest[1..]),
        _ => false,
    }
}

fn localhost_port(rest: &str) -> bool {
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || rest[..digits].parse::<u16>().is_err() {
        return false;
    }
    let after = &rest[digits..];
    after.is_empty() || matches!(after.chars().next(), Some('/' | '?'))
}

fn view(grant: &Grant) -> GrantView {
    GrantView {
        id: grant.id.clone(),
        kind: grant.kind.clone(),
        target: grant.target.clone(),
        expires_at: grant.expires_at.clone(),
        active: !grant.revoked && active(grant),
    }
}

fn path() -> Result<PathBuf> {
    let dir = policy::root_dir()?.join("computer");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("grants.json"))
}

fn read() -> Result<GrantFile> {
    let root = policy::root_dir()?;
    let path = root.join("computer").join("grants.json");
    if !path.exists() {
        return Ok(GrantFile::default());
    }
    let text = fs::read_to_string(&path)?;
    serde_json::from_str(&text).with_context(|| format!("could not read {}", path.display()))
}

fn write(file: &GrantFile) -> Result<()> {
    let path = path()?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(file)?)?;
    fs::rename(tmp, path).context("could not store browser grants")?;
    Ok(())
}
