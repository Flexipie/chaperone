//! chaperone: the lane CLI, with daisy-cli's argv, envelope and exit codes
//! (docs/cli-contract.md), so a skill switches by changing the binary path.

mod checks;
mod cli;
mod doctor;
mod impact;
mod output;
mod overlap;
mod spec;
mod work;

use std::io::Write;
use std::path::PathBuf;

use chaperone_core::ledger::{Ledger, SCHEMA_VERSION};
use chaperone_core::pipeline;
use chaperone_core::stats::Cohort;
use clap::Parser;
use clap::error::ErrorKind;
use serde_json::json;

use cli::{Cli, Cmd, Format, PipelineCmd};
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
        Cmd::Work(cmd) => work::run(&Ledger::open(&path)?, cmd, meta, cli.format),
        Cmd::Spec(cmd) => spec::run(&Ledger::open(&path)?, cmd, meta),
        Cmd::Check(cmd) => checks::check(&Ledger::open(&path)?, cmd),
        Cmd::Reviews(cmd) => checks::reviews(&Ledger::open(&path)?, cmd),
        Cmd::Jobs(cmd) => checks::jobs(&Ledger::open(&path)?, cmd),
        Cmd::Usage(cmd) => checks::usage(&Ledger::open(&path)?, cmd),
        Cmd::Attention(cmd) => checks::attention(&Ledger::open(&path)?, cmd),
        Cmd::Dossier(cmd) => checks::dossier(&Ledger::open(&path)?, cmd),
        Cmd::Stats { merged_since, work } => {
            let cohort = match merged_since {
                Some(since) => Cohort::MergedSince(since),
                None => Cohort::Work(work),
            };
            Ok(Outcome::One(Ledger::open(&path)?.stats(&cohort)??))
        }
        Cmd::Impact {
            files,
            cache,
            entry_points,
            profile,
            declared,
        } => impact::run(
            files,
            cache,
            entry_points,
            profile,
            declared,
            meta,
            cli.format,
        ),
        Cmd::Pipeline(PipelineCmd::Current) => {
            let p = pipeline::current();
            Ok(Outcome::One(json!({ "id": p.id, "hash": p.hash })))
        }
        Cmd::Doctor {
            skills,
            repo,
            profile,
        } => doctor::run(&skills, repo.as_deref(), profile),
        Cmd::Overlap { repo, uncommitted } => {
            overlap::run(&Ledger::open(&path)?, repo, uncommitted, meta, cli.format)
        }
    }
}

fn default_db() -> Result<PathBuf, CliError> {
    dirs::data_dir()
        .map(|d| d.join("chaperone").join("ledger.db"))
        .ok_or_else(|| CliError::Internal("no user data directory; pass --db".into()))
}
