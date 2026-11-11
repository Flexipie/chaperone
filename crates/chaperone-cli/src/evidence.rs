//! `chaperone evidence show <ref>`: a stored file's text, hashed again
//! (`Evidence::get`). Piped, the stored bytes as they are; on a terminal,
//! control characters but newline and tab escaped, so a stored escape
//! sequence cannot change what is shown.

use std::io::{ErrorKind, IsTerminal, Write};
use std::path::Path;

use chaperone_core::ledger::{Evidence, LedgerError, Refusal};

use crate::output::{CliError, Outcome};

pub fn show(db: &Path, reference: &str) -> Result<Outcome, CliError> {
    let store = Evidence::beside(db);
    let text = store
        .resolve(reference)
        .and_then(|hash| store.get(&hash))
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => CliError::from(Refusal::NotFoundRef {
                entity: "evidence",
                reference: reference.into(),
            }),
            ErrorKind::InvalidInput => Refusal::InvalidInput(e.to_string()).into(),
            _ => LedgerError::from(e).into(),
        })?;
    let mut out = std::io::stdout().lock();
    let text = if out.is_terminal() {
        let shown = |c: char| match c {
            '\n' | '\t' => c.to_string(),
            c if c.is_control() => c.escape_unicode().to_string(),
            c => c.to_string(),
        };
        text.chars().map(shown).collect()
    } else {
        text
    };
    let _ = out.write_all(text.as_bytes());
    Ok(Outcome::Written)
}
