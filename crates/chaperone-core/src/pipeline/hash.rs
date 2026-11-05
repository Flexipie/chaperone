//! The semantic hash. The input is the normalized limits, gates and
//! transitions as JSON with object keys sorted here (not by serde_json's map,
//! whose order depends on a Cargo feature any crate can turn on), gates and
//! transitions sorted, `requires` and `of` order kept. `published`, `cohort`
//! and comments are not part of it.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn semantic_hash(doc: &Value) -> String {
    format!("{:x}", Sha256::digest(canonical_string(doc)))
}

pub(crate) fn canonical_string(value: &Value) -> String {
    let mut out = String::new();
    canonical(value, &mut out);
    out
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::from(key.as_str()).to_string());
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}
