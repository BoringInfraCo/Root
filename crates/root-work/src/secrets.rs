//! Conservative detection of obvious, high-confidence secrets in text that is
//! about to become durable work state.
//!
//! This is a guard rail, not a secrets scanner. It recognizes a small set of
//! unambiguous credential shapes and stays deliberately narrow so that normal
//! engineering prose is not rejected. Root does not claim complete detection.

const PEM_LABEL: &str = "PEM private key";
const AWS_LABEL: &str = "AWS access key id";
const GITHUB_LABEL: &str = "GitHub token";
const SLACK_LABEL: &str = "Slack token";
const OPENAI_LABEL: &str = "OpenAI-style API key";
const BEARER_LABEL: &str = "bearer token";
const PASSWORD_LABEL: &str = "password assignment";
const SECRET_LABEL: &str = "secret assignment";
const TOKEN_LABEL: &str = "token assignment";

/// Human-readable refusal used by the store when a value looks like a secret.
pub fn refusal(label: &str) -> String {
    format!(
        "Refusing to persist what looks like a secret ({label}). \
         Root does not store credentials in work state."
    )
}

/// Return a human label when `text` contains an obvious high-confidence secret.
pub fn detect(text: &str) -> Option<&'static str> {
    detect_pem(text)
        .or_else(|| detect_aws(text))
        .or_else(|| detect_github(text))
        .or_else(|| detect_slack(text))
        .or_else(|| detect_openai(text))
        .or_else(|| detect_bearer(text))
        .or_else(|| detect_assignment(text))
}

fn detect_pem(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        Some(PEM_LABEL)
    } else {
        None
    }
}

fn detect_aws(text: &str) -> Option<&'static str> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index + 4 <= bytes.len() {
        if &bytes[index..index + 4] == b"AKIA" && boundary_before(bytes, index) {
            let body = index + 4;
            if body + 16 <= bytes.len()
                && bytes[body..body + 16]
                    .iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
                && !bytes
                    .get(body + 16)
                    .is_some_and(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            {
                return Some(AWS_LABEL);
            }
        }
        index += 1;
    }
    None
}

fn detect_github(text: &str) -> Option<&'static str> {
    for prefix in ["ghp_", "github_pat_"] {
        let mut search = 0;
        while let Some(offset) = text[search..].find(prefix) {
            let start = search + offset;
            let body = start + prefix.len();
            if boundary_before(text.as_bytes(), start) && alnum_run(&text[body..]) >= 20 {
                return Some(GITHUB_LABEL);
            }
            search = body;
        }
    }
    None
}

fn detect_slack(text: &str) -> Option<&'static str> {
    for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
        let mut search = 0;
        while let Some(offset) = text[search..].find(prefix) {
            let start = search + offset;
            let body = start + prefix.len();
            if boundary_before(text.as_bytes(), start) && alnum_run(&text[body..]) >= 8 {
                return Some(SLACK_LABEL);
            }
            search = body;
        }
    }
    None
}

fn detect_openai(text: &str) -> Option<&'static str> {
    let mut search = 0;
    while let Some(offset) = text[search..].find("sk-") {
        let start = search + offset;
        let body = start + 3;
        let candidate: Vec<u8> = text[body..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-' || *byte == b'_')
            .collect();
        if boundary_before(text.as_bytes(), start)
            && candidate.len() >= 10
            && candidate.iter().any(u8::is_ascii_digit)
        {
            return Some(OPENAI_LABEL);
        }
        search = body;
    }
    None
}

fn detect_bearer(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0;
    while index + 6 <= bytes.len() {
        if &bytes[index..index + 6] == b"bearer" && boundary_before(bytes, index) {
            let mut cursor = index + 6;
            if cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                    cursor += 1;
                }
                let token = &text[cursor..];
                let len = token
                    .bytes()
                    .take_while(|byte| !byte.is_ascii_whitespace())
                    .count();
                if len >= 20 {
                    return Some(BEARER_LABEL);
                }
            }
        }
        index += 1;
    }
    None
}

fn detect_assignment(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    for (keyword, label) in [
        ("password", PASSWORD_LABEL),
        ("passwd", PASSWORD_LABEL),
        ("secret", SECRET_LABEL),
        ("token", TOKEN_LABEL),
    ] {
        let key = keyword.as_bytes();
        let mut index = 0;
        while index + key.len() <= bytes.len() {
            if &bytes[index..index + key.len()] == key && assignment_boundary_before(bytes, index) {
                let mut cursor = index + key.len();
                while cursor < bytes.len() && is_key_char(bytes[cursor]) {
                    cursor += 1;
                }
                while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                    cursor += 1;
                }
                if cursor < bytes.len() && (bytes[cursor] == b'=' || bytes[cursor] == b':') {
                    let mut value = cursor + 1;
                    while value < bytes.len() && (bytes[value] == b' ' || bytes[value] == b'\t') {
                        value += 1;
                    }
                    if value < bytes.len() {
                        let candidate = &text[value..];
                        if value_looks_secret(candidate) {
                            return Some(label);
                        }
                    }
                }
            }
            index += 1;
        }
    }
    None
}

fn value_looks_secret(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        let mut cursor = 1;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        return cursor > 1;
    }
    let token: String = value
        .chars()
        .take_while(|character| !character.is_whitespace())
        .collect();
    if token.len() < 4 {
        return false;
    }
    token.len() >= 10
        || token
            .bytes()
            .any(|byte| byte.is_ascii_digit() || !byte.is_ascii_alphanumeric())
}

fn alnum_run(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric())
        .count()
}

fn boundary_before(bytes: &[u8], index: usize) -> bool {
    index == 0 || !is_key_char(bytes[index - 1])
}

fn assignment_boundary_before(bytes: &[u8], index: usize) -> bool {
    index == 0 || !bytes[index - 1].is_ascii_alphanumeric()
}

fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_pem_private_key() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow==\n-----END RSA PRIVATE KEY-----";
        assert_eq!(detect(text), Some(PEM_LABEL));
    }

    #[test]
    fn detects_aws_access_key_id() {
        assert_eq!(detect("AKIAIOSFODNN7EXAMPLE"), Some(AWS_LABEL));
        assert_eq!(
            detect("key id AKIAIOSFODNN7EXAMPLE trailing"),
            Some(AWS_LABEL)
        );
    }

    #[test]
    fn detects_github_tokens() {
        assert_eq!(
            detect("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            Some(GITHUB_LABEL)
        );
        assert_eq!(
            detect("github_pat_abcdefghijklmnopqrstuvwxyz0123456789"),
            Some(GITHUB_LABEL)
        );
    }

    #[test]
    fn detects_slack_tokens() {
        assert_eq!(detect("xoxb-1234567890-abcdef"), Some(SLACK_LABEL));
        assert_eq!(detect("xoxp-0987654321-xyzzy"), Some(SLACK_LABEL));
    }

    #[test]
    fn detects_openai_style_keys() {
        assert_eq!(
            detect("sk-abcdefghijklmnopqrstuvwxyz0123456789"),
            Some(OPENAI_LABEL)
        );
        assert_eq!(
            detect("client secret is sk-live-abc123, use it"),
            Some(OPENAI_LABEL)
        );
        assert_eq!(detect("sk-proj-example1234567890"), Some(OPENAI_LABEL));
    }

    #[test]
    fn detects_bearer_tokens() {
        assert_eq!(
            detect("Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345"),
            Some(BEARER_LABEL)
        );
    }

    #[test]
    fn detects_credential_assignments() {
        assert_eq!(detect("password = hunter2"), Some(PASSWORD_LABEL));
        assert_eq!(detect("PASSWORD: s3cr3t-value"), Some(PASSWORD_LABEL));
        assert_eq!(detect("api_secret = 'abcdefghij'"), Some(SECRET_LABEL));
        assert_eq!(detect("auth_token: abcdef123456"), Some(TOKEN_LABEL));
    }

    #[test]
    fn normal_engineering_prose_is_not_sensitive() {
        for text in [
            "Tokens expire after 24 hours",
            "password reset flow",
            "The secret is out",
            "tokenization = true",
            "sk-learn is a Python library",
            "AKIA is a prefix",
        ] {
            assert_eq!(detect(text), None, "false positive for {text:?}");
        }
    }

    #[test]
    fn refusal_names_the_label() {
        let message = refusal(PASSWORD_LABEL);
        assert!(message.contains(PASSWORD_LABEL));
        assert!(message.contains("does not store credentials"));
    }
}
