//! seamlane: the lane CLI, with daisy-cli's argv, envelope and exit codes
//! (docs/cli-contract.md), so a skill switches by changing the binary path.

mod cli;
mod output;
mod spec;
mod work;

use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use clap::error::ErrorKind;
use seamlane_core::ledger::{Ledger, SCHEMA_VERSION};
use serde_json::json;

use cli::{Cli, Cmd, Format};
use output::{CliError, Meta, Outcome};

fn main() {
    let meta = Meta::default();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => e.exit(),
            _ => finish(Err(CliError::from_clap(&e)), &meta, Format::Json),
        },
    };
    let format = cli.format;
    let mut meta = meta;
    let result = run(cli, &mut meta);
    finish(result, &meta, format)
}

fn finish(result: Result<Outcome, CliError>, meta: &Meta, format: Format) -> ! {
    let (text, code) = match &result {
        Ok(outcome) => (output::render_ok(outcome, meta, format), 0),
        Err(e) => (output::render_err(e, meta), output::exit_code(e)),
    };
    let _ = if result.is_ok() {
        std::io::stdout().write_all(text.as_bytes())
    } else {
        std::io::stderr().write_all(text.as_bytes())
    };
    std::process::exit(code);
}

fn run(cli: Cli, meta: &mut Meta) -> Result<Outcome, CliError> {
    // Before the ledger opens, so a refused write never takes its lock.
    if let Some(command) = cli.cmd.write_name()
        && std::env::var_os("DAISY_CLI_READONLY").is_some_and(|v| v == "1")
    {
        return Err(CliError::ReadOnly(command));
    }
    let path = match cli.db {
        Some(path) => path,
        None => default_db()?,
    };
    match cli.cmd {
        Cmd::Migrate => {
            Ledger::open_and_migrate(&path)?;
            Ok(Outcome::One(
                json!({ "path": path, "schema_version": SCHEMA_VERSION }),
            ))
        }
        Cmd::Work(cmd) => work::run(&Ledger::open(&path)?, cmd, meta),
        Cmd::Spec(cmd) => spec::run(&Ledger::open(&path)?, cmd, meta),
    }
}

fn default_db() -> Result<PathBuf, CliError> {
    dirs::data_dir()
        .map(|d| d.join("seamlane").join("ledger.db"))
        .ok_or_else(|| CliError::Internal("no user data directory; pass --db".into()))
}
