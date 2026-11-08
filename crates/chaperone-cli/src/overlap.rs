//! `chaperone overlap`: per file, the open lanes touching it
//! (`chaperone_core::overlap`).

use chaperone_core::ledger::Ledger;

use crate::cli::Format;
use crate::output::{self, CliError, Meta, Outcome};

pub fn run(
    ledger: &Ledger,
    repo: Option<String>,
    uncommitted: bool,
    meta: &Meta,
    format: Format,
) -> Result<Outcome, CliError> {
    let data = chaperone_core::overlap::overlap(ledger, repo, uncommitted)?;
    output::write_one(&data, meta, format)
}
