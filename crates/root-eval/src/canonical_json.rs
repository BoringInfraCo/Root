//! Canonical JSON serialization shared by the Phase B hashes.
//!
//! Mirrors the conventions of [`crate::canonical`] (the TOML world): keys are
//! sorted lexicographically at every level, arrays keep declaration order, and
//! the serialized bytes are the hash input. Unlike TOML, the effective-context
//! manifest and the experiment plan are hashed as compact JSON so the canonical
//! projection is a plain data tree.
//!
//! Rules:
//!
//! 1. object keys are sorted byte-wise at every nesting level;
//! 2. arrays keep their given order (callers sort semantically meaningful
//!    collections before hashing);
//! 3. output is compact (`serde_json` default, no whitespace);
//! 4. floating-point numbers are refused — every hash projection uses strings,
//!    integers, and booleans only, so a float is a programming error, not
//!    data.

use serde_json::{Map, Value};

/// Serialize `value` to canonical JSON bytes, or return a message describing
/// why the value is not canonicalizable.
///
/// The error is a plain `String` so each caller can map it into its own error
/// taxonomy (`ContextError`, `EvalError`).
pub(crate) fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let sorted = sort_value(value)?;
    serde_json::to_vec(&sorted).map_err(|error| format!("canonical serialization failed: {error}"))
}

fn sort_value(value: &Value) -> Result<Value, String> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sort_value(&map[key])?);
            }
            Ok(Value::Object(sorted))
        }
        Value::Array(values) => {
            let mut sorted = Vec::with_capacity(values.len());
            for value in values {
                sorted.push(sort_value(value)?);
            }
            Ok(Value::Array(sorted))
        }
        Value::Number(number) => {
            if number.is_f64() {
                return Err("floating-point numbers are not canonical JSON".to_string());
            }
            Ok(Value::Number(number.clone()))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_are_sorted_at_every_level() {
        let bytes = canonical_json_bytes(&json!({
            "b": 1,
            "a": { "z": true, "y": [ { "q": "1", "p": "2" } ] },
        }))
        .expect("object is canonicalizable");
        assert_eq!(
            String::from_utf8(bytes).expect("utf-8"),
            r#"{"a":{"y":[{"p":"2","q":"1"}],"z":true},"b":1}"#
        );
    }

    #[test]
    fn arrays_keep_order() {
        let bytes = canonical_json_bytes(&json!({ "x": [3, 1, 2] })).expect("canonicalizable");
        assert_eq!(String::from_utf8(bytes).expect("utf-8"), r#"{"x":[3,1,2]}"#);
    }

    #[test]
    fn floats_are_refused() {
        let error = canonical_json_bytes(&json!({ "x": 1.5 })).expect_err("float refused");
        assert!(error.contains("floating-point"), "{error}");
    }
}
