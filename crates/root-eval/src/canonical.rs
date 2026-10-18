use toml::map::Map;
use toml::Value;

use crate::digest::sha256_digest;
use crate::error::TaskError;

/// Canonical projection and digest of a parsed task record (§2.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalRecord {
    bytes: Vec<u8>,
    sha256: String,
}

impl CanonicalRecord {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Build the canonical hash over the parsed document minus `[integrity]`.
///
/// Rules implemented:
/// 1. keys sorted lexicographically in every table;
/// 2. arrays keep declaration order, except `fixture.files`, which the spec's
///    canonical serializer sorts by `path`;
/// 3. comments and formatting are already gone after TOML parsing;
/// 4. the canonical re-serialization must be a parse/re-serialize fixed point;
/// 5. the digest is SHA-256 over the UTF-8 canonical bytes.
pub(crate) fn canonical_record(document: &Value) -> Result<CanonicalRecord, TaskError> {
    let projection = canonical_projection(document)?;
    let bytes = serialize_canonical(&projection)?;

    // Fixed-point rule (§2.10.4): parse(canonical bytes) must re-serialize to
    // the byte-identical form. The canonical projection has no `[integrity]`
    // table, so a re-parse is structurally valid only as a document projection;
    // canonical_projection handles that absence.
    let reparsed: Value =
        toml::from_str(
            std::str::from_utf8(&bytes).map_err(|_| TaskError::CanonicalDrift {
                detail: "canonical bytes are not UTF-8".to_string(),
            })?,
        )
        .map_err(|error| TaskError::CanonicalDrift {
            detail: format!("canonical bytes do not re-parse: {error}"),
        })?;
    let second = serialize_canonical(&canonical_projection(&reparsed)?)?;
    if second != bytes {
        return Err(TaskError::CanonicalDrift {
            detail: "canonical bytes changed on re-serialization".to_string(),
        });
    }

    Ok(CanonicalRecord {
        sha256: sha256_digest(&bytes),
        bytes,
    })
}

fn canonical_projection(document: &Value) -> Result<Value, TaskError> {
    let table = document.as_table().ok_or_else(|| TaskError::TypeInvalid {
        field: "<document>".to_string(),
        expected: "TOML table".to_string(),
    })?;
    let mut table = table.clone();
    table.remove("integrity");
    sort_declared_fixture_files(&mut table);
    Ok(sort_value(&Value::Table(table)))
}

fn sort_declared_fixture_files(table: &mut Map<String, Value>) {
    if let Some(Value::Table(fixture)) = table.get_mut("fixture") {
        if let Some(Value::Array(files)) = fixture.get_mut("files") {
            files.sort_by(|left, right| file_path(left).cmp(file_path(right)));
        }
    }
}

fn file_path(value: &Value) -> &str {
    value
        .as_table()
        .and_then(|table| table.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sort_value(&table[key]));
            }
            Value::Table(sorted)
        }
        Value::Array(values) => Value::Array(values.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

fn serialize_canonical(value: &Value) -> Result<Vec<u8>, TaskError> {
    let mut text = toml::to_string_pretty(value).map_err(|error| TaskError::Syntax {
        message: format!("canonical serialization failed: {error}"),
    })?;
    while text.ends_with('\n') {
        text.pop();
    }
    text.push('\n');
    Ok(text.into_bytes())
}
