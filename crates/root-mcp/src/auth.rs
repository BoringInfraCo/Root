//! Local bearer token for non-stdio transports.
//!
//! The token is a file under the Root directory, mode 0600. It is not an OAuth
//! access token and it must never be put in a URL. The stdio shim reads it and
//! presents it to rootd; HTTP clients send it in `Authorization`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn token_path(root_dir: &Path) -> PathBuf {
    root_dir.join("rootd.token")
}

/// Create the token file if needed and return the secret.
pub fn load_or_create(root_dir: &Path) -> Result<String> {
    let path = token_path(root_dir);
    if let Some(existing) = read_file(&path)? {
        return Ok(existing);
    }
    let token = random_hex(32)?;
    write_private(&path, &token)?;
    Ok(token)
}

/// Read a token the daemon has already written.
pub fn read_existing(root_dir: &Path) -> Result<String> {
    let path = token_path(root_dir);
    let started = Instant::now();
    loop {
        if let Some(token) = read_file(&path)? {
            return Ok(token);
        }
        if started.elapsed() > Duration::from_secs(2) {
            anyhow::bail!("rootd token is missing at {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn bearer_matches(presented: Option<&str>, token: &str) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    let expected = format!("Bearer {token}");
    constant_eq(presented.trim(), &expected)
}

pub fn random_hex(nbytes: usize) -> Result<String> {
    let mut bytes = vec![0u8; nbytes];
    let mut file = std::fs::File::open("/dev/urandom").context("could not read /dev/urandom")?;
    std::io::Read::read_exact(&mut file, &mut bytes)?;
    let mut out = String::with_capacity(nbytes * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

fn read_file(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim();
            if token.is_empty() {
                Ok(None)
            } else {
                Ok(Some(token.to_string()))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn write_private(path: &Path, token: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    writeln!(file, "{token}")?;
    Ok(())
}

fn constant_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_match_requires_the_prefix_and_the_whole_secret() {
        assert!(bearer_matches(Some("Bearer abc"), "abc"));
        assert!(!bearer_matches(Some("Bearer abcd"), "abc"));
        assert!(!bearer_matches(Some("abc"), "abc"));
        assert!(!bearer_matches(None, "abc"));
    }
}
