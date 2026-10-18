use sha2::{Digest as _, Sha256};

use crate::error::TaskError;

/// Prefix every v1 digest carries (§2.1).
pub const DIGEST_PREFIX: &str = "sha256:";

/// Reserved draft sentinel (§2.10.3).
pub const PENDING_SENTINEL: &str = "sha256:PENDING";

/// True for exactly `sha256:<64 lowercase hex>`.
///
/// Mirrors `is_canonical_sha256_digest` in `root-lockfile`: no bare hex, no
/// uppercase, no other algorithm.
pub fn is_canonical_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// True for the exact draft sentinel `sha256:PENDING`.
pub fn is_pending_sentinel(value: &str) -> bool {
    value == PENDING_SENTINEL
}

/// Compute the canonical `sha256:<hex>` digest of arbitrary bytes.
pub fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{DIGEST_PREFIX}{:x}", hasher.finalize())
}

/// Error out if `value` is the draft sentinel.
///
/// Draft records parse at the structural layer and are refused here so the
/// refusal is distinguishable from malformed digests.
pub(crate) fn require_sealed(field: &str, value: &str) -> Result<(), TaskError> {
    if is_pending_sentinel(value) {
        return Err(TaskError::SentinelRefusal {
            field: field.to_string(),
        });
    }
    Ok(())
}

/// Error out unless `value` is a canonical digest.
pub(crate) fn require_canonical_digest(field: &str, value: &str) -> Result<(), TaskError> {
    require_sealed(field, value)?;
    if !is_canonical_sha256_digest(value) {
        return Err(TaskError::DigestInvalid {
            field: field.to_string(),
            reason: "expected sha256:<64 lowercase hex>".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_root_lockfile_convention() {
        assert_eq!(
            sha256_digest(b"hello"),
            format!(
                "{}{}",
                DIGEST_PREFIX,
                root_lockfile::compute_sha256(b"hello")
            )
        );
    }

    #[test]
    fn canonical_digest_grammar() {
        assert!(is_canonical_sha256_digest(&sha256_digest(b"x")));
        assert!(!is_canonical_sha256_digest("sha256:PENDING"));
        assert!(!is_canonical_sha256_digest(
            &sha256_digest(b"x").to_uppercase()
        ));
    }
}
