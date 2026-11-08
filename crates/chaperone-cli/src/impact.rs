//! `chaperone impact <files>`: the files that depend on them, nearest
//! first, from the code graph (`chaperone-graph`), and with
//! `--entry-points` the entry points they reach, from the pipeline
//! profile. Reads the worktree and git, never the ledger.

use std::path::{Path, PathBuf};

use chaperone_core::ledger::Refusal;
use chaperone_graph::Error;

use crate::cli::Format;
use crate::output::{self, CliError, Meta, Outcome};

pub fn run(
    targets: Vec<String>,
    cache: Option<PathBuf>,
    entry_points: bool,
    profile: Option<PathBuf>,
    declared: Option<Vec<String>>,
    meta: &Meta,
    format: Format,
) -> Result<Outcome, CliError> {
    let dir = Path::new(".");
    match chaperone_graph::impact_in(dir, targets, cache, entry_points, profile, declared) {
        Ok(data) => output::write_one(&data, meta, format),
        Err(Error::Invalid(e)) => Err(Refusal::InvalidInput(e).into()),
        Err(Error::Internal(e)) => Err(CliError::Internal(e)),
    }
}
