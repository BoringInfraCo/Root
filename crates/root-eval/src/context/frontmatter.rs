//! Conservative frontmatter reader for `SKILL.md` files.
//!
//! Contract §4.1 (frozen at OpenCode `1.18.27`): a skill's frontmatter must
//! contain a string `name`; `description` is an optional string; invalid
//! frontmatter excludes the file with a recorded error.
//!
//! The resolver does not depend on a full YAML engine. This module implements
//! a deliberately small, fail-closed subset:
//!
//! - the file must start with a `---` line and contain a closing `---` line;
//! - inside, only top-level `key: value` mapping entries are accepted —
//!   indented (nested) lines are refused;
//! - blank lines and full-line `# comments` are ignored;
//! - keys match `[A-Za-z0-9_-]+`, must be unique, and are not indented;
//! - values are double-quoted (`\n \t \r \" \\` escapes only), single-quoted
//!   (`''` escape only), or plain scalars that do not start with a YAML
//!   indicator character; comments inside plain scalars are **not** stripped;
//! - anything else is an error, and the skill is excluded.
//!
//! Unknown keys are ignored (frontmatter commonly carries `license`,
//! `allowed-tools`, …). Line endings: a single trailing `\r` per line is
//! tolerated; everything else is taken literally.

use super::DiscoveryErrorKind;

/// Parsed frontmatter fields the resolver consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillFrontmatter {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
}

/// Reasons a `SKILL.md` file is excluded from discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontmatterError {
    /// The first line is not `---`.
    Missing,
    /// A frontmatter block was opened but never closed.
    Unterminated,
    /// A line is not a top-level `key: value` entry.
    InvalidLine { line: usize, detail: String },
    /// A value is not a supported scalar.
    InvalidValue { line: usize, key: String },
    /// The same key appears twice.
    DuplicateKey { key: String },
    /// No `name` key is present.
    MissingName,
    /// `name` is present but empty.
    EmptyName,
}

impl FrontmatterError {
    /// Stable machine-matchable kind recorded in the manifest.
    pub(crate) fn kind(&self) -> DiscoveryErrorKind {
        match self {
            Self::Missing => DiscoveryErrorKind::MissingFrontmatter,
            Self::Unterminated => DiscoveryErrorKind::UnterminatedFrontmatter,
            Self::InvalidLine { .. } => DiscoveryErrorKind::InvalidFrontmatterLine,
            Self::InvalidValue { .. } => DiscoveryErrorKind::InvalidFrontmatterValue,
            Self::DuplicateKey { .. } => DiscoveryErrorKind::DuplicateFrontmatterKey,
            Self::MissingName => DiscoveryErrorKind::MissingName,
            Self::EmptyName => DiscoveryErrorKind::EmptyName,
        }
    }

    /// Human-readable detail recorded alongside the kind.
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::Missing => "file does not start with a `---` frontmatter block".to_string(),
            Self::Unterminated => "frontmatter block has no closing `---`".to_string(),
            Self::InvalidLine { line, detail } => {
                format!("line {line}: {detail}")
            }
            Self::InvalidValue { line, key } => {
                format!("line {line}: value of `{key}` is not a supported scalar")
            }
            Self::DuplicateKey { key } => format!("duplicate frontmatter key `{key}`"),
            Self::MissingName => "frontmatter has no `name` key".to_string(),
            Self::EmptyName => "frontmatter `name` is empty".to_string(),
        }
    }
}

/// Parse the frontmatter block of a skill file.
pub(crate) fn parse_skill_frontmatter(content: &str) -> Result<SkillFrontmatter, FrontmatterError> {
    let mut lines = content.split('\n').enumerate();
    let Some((_, first)) = lines.next() else {
        return Err(FrontmatterError::Missing);
    };
    if strip_cr(first) != "---" {
        return Err(FrontmatterError::Missing);
    }

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut closed = false;
    for (index, raw) in lines {
        let line = strip_cr(raw);
        if line == "---" {
            closed = true;
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(FrontmatterError::InvalidLine {
                line: index + 1,
                detail: "indented lines (nested structures) are not supported".to_string(),
            });
        }
        let Some((key, raw_value)) = line.split_once(':') else {
            return Err(FrontmatterError::InvalidLine {
                line: index + 1,
                detail: "expected `key: value`".to_string(),
            });
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(FrontmatterError::InvalidLine {
                line: index + 1,
                detail: format!("invalid key `{key}`"),
            });
        }
        if entries.iter().any(|(existing, _)| existing == key) {
            return Err(FrontmatterError::DuplicateKey {
                key: key.to_string(),
            });
        }
        let value = parse_scalar(raw_value.trim(), key, index + 1)?;
        entries.push((key.to_string(), value));
    }
    if !closed {
        return Err(FrontmatterError::Unterminated);
    }

    let mut name = None;
    let mut description = None;
    for (key, value) in entries {
        match key.as_str() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }

    let name = match name {
        None => return Err(FrontmatterError::MissingName),
        Some(name) if name.is_empty() => return Err(FrontmatterError::EmptyName),
        Some(name) => name,
    };
    Ok(SkillFrontmatter {
        name,
        description: description.filter(|value| !value.is_empty()),
    })
}

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

fn parse_scalar(raw: &str, key: &str, line: usize) -> Result<String, FrontmatterError> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    let invalid = || FrontmatterError::InvalidValue {
        line,
        key: key.to_string(),
    };
    let first = raw.chars().next().expect("non-empty checked above");
    match first {
        '"' => {
            let Some(inner) = raw
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
            else {
                return Err(invalid());
            };
            if inner.contains('"') {
                return Err(invalid());
            }
            unescape_double_quoted(inner).ok_or_else(invalid)
        }
        '\'' => {
            let Some(inner) = raw
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
            else {
                return Err(invalid());
            };
            unescape_single_quoted(inner).ok_or_else(invalid)
        }
        '[' | '{' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`' | '#' => Err(invalid()),
        _ => {
            if raw.contains('\t') {
                return Err(invalid());
            }
            Ok(raw.to_string())
        }
    }
}

fn unescape_double_quoted(inner: &str) -> Option<String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let escape = chars.next()?;
        match escape {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            _ => return None,
        }
    }
    Some(out)
}

fn unescape_single_quoted(inner: &str) -> Option<String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\'' {
            out.push(character);
            continue;
        }
        // YAML single-quote escaping: `''` is a literal quote.
        if chars.peek() == Some(&'\'') {
            chars.next();
            out.push('\'');
        } else {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_repo_root_skill_shape() {
        let content = "---\nname: root\ndescription: Safe deterministic package management via Root\n---\n\n# Root Skill\n";
        let parsed = parse_skill_frontmatter(content).expect("valid frontmatter");
        assert_eq!(parsed.name, "root");
        assert_eq!(
            parsed.description.as_deref(),
            Some("Safe deterministic package management via Root")
        );
    }

    #[test]
    fn missing_frontmatter_is_refused() {
        assert_eq!(
            parse_skill_frontmatter("# no frontmatter\n").expect_err("refused"),
            FrontmatterError::Missing
        );
    }

    #[test]
    fn unterminated_frontmatter_is_refused() {
        assert_eq!(
            parse_skill_frontmatter("---\nname: x\n").expect_err("refused"),
            FrontmatterError::Unterminated
        );
    }

    #[test]
    fn missing_and_empty_name_are_distinct() {
        assert_eq!(
            parse_skill_frontmatter("---\ndescription: d\n---\n").expect_err("refused"),
            FrontmatterError::MissingName
        );
        assert_eq!(
            parse_skill_frontmatter("---\nname:\n---\n").expect_err("refused"),
            FrontmatterError::EmptyName
        );
    }

    #[test]
    fn duplicate_keys_are_refused() {
        assert_eq!(
            parse_skill_frontmatter("---\nname: a\nname: b\n---\n").expect_err("refused"),
            FrontmatterError::DuplicateKey {
                key: "name".to_string()
            }
        );
    }

    #[test]
    fn quoted_values_are_supported() {
        let parsed = parse_skill_frontmatter("---\nname: \"a\\nb\"\n---\n").expect("valid");
        assert_eq!(parsed.name, "a\nb");
        let parsed = parse_skill_frontmatter("---\nname: 'it''s'\n---\n").expect("valid");
        assert_eq!(parsed.name, "it's");
    }

    #[test]
    fn yaml_indicator_values_are_refused() {
        for value in ["[a, b]", "{a: b}", "|", "&anchor", "*alias"] {
            let content = format!("---\nname: {value}\n---\n");
            assert!(
                parse_skill_frontmatter(&content).is_err(),
                "value `{value}` must be refused"
            );
        }
    }

    #[test]
    fn nested_and_blank_lines() {
        assert!(parse_skill_frontmatter("---\nname: ok\n  nested: no\n---\n").is_err());
        assert_eq!(
            parse_skill_frontmatter("---\n\n# comment\nname: ok\n\n---\n")
                .expect("valid")
                .name,
            "ok"
        );
    }

    #[test]
    fn blank_frontmatter_block_is_missing_name() {
        assert_eq!(
            parse_skill_frontmatter("---\n---\n").expect_err("refused"),
            FrontmatterError::MissingName
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let parsed = parse_skill_frontmatter("---\nlicense: MIT\nname: ok\n---\n").expect("valid");
        assert_eq!(parsed.name, "ok");
        assert_eq!(parsed.description, None);
    }
}
