//! Keyset pagination, daisy's `c1` cursor. A cursor is the sort key of the
//! last row a page returned, encoded as `"c1" + hex(json array)` so the
//! values keep their JSON types and stay opaque to consumers.

use serde::Serialize;
use serde_json::Value;

use crate::ledger::Refusal;

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 500;

const PREFIX: &str = "c1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(pub Vec<Value>);

impl Cursor {
    pub fn encode(&self) -> String {
        let bytes = serde_json::to_vec(&self.0).unwrap_or_default();
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!("{PREFIX}{hex}")
    }

    /// Anything that is not a cursor this crate produced with `arity` parts
    /// is `invalid_input`, never a panic and never a silently empty page.
    pub fn decode(s: &str, arity: usize) -> Result<Cursor, Refusal> {
        let malformed = || Refusal::InvalidInput("malformed cursor".into());
        let hex = s.strip_prefix(PREFIX).ok_or_else(malformed)?;
        if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(malformed());
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| malformed()))
            .collect::<Result<Vec<u8>, _>>()?;
        match serde_json::from_slice(&bytes) {
            Ok(Value::Array(parts)) if parts.len() == arity => Ok(Cursor(parts)),
            _ => Err(malformed()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass back as `--cursor` for the next page; None on the last one.
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

pub fn check_limit(limit: u32) -> Result<u32, Refusal> {
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(Refusal::InvalidInput(format!(
            "limit must be between 1 and {MAX_LIMIT}, got {limit}"
        )))
    }
}

/// A page from rows read with `LIMIT limit + 1`: the extra row proves there
/// is more without a count. `key_of` is the row's sort key in ORDER BY order.
pub fn from_rows<T>(mut rows: Vec<T>, limit: u32, key_of: impl Fn(&T) -> Vec<Value>) -> Page<T> {
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more
        .then(|| rows.last().map(|r| Cursor(key_of(r)).encode()))
        .flatten();
    Page {
        items: rows,
        next_cursor,
        has_more,
    }
}
