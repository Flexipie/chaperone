//! `seamlane spec ...`: the dispatch spec of a parent issue, versioned in
//! the ledger (`seamlane_core::spec`), printed in daisy-cli's shapes.

use std::io::Read;
use std::path::Path;

use seamlane_core::cursor::Page;
use seamlane_core::ledger::{Ledger, Refusal, SPEC_BODY_MAX_BYTES};
use seamlane_core::spec::{Spec, parse};
use serde_json::{Value, json};

use crate::cli::SpecCmd;
use crate::output::{CliError, Meta, Outcome};
use crate::work::value;

pub fn run(ledger: &Ledger, cmd: SpecCmd, meta: &mut Meta) -> Result<Outcome, CliError> {
    let now = ledger.now()?;
    let spec = match cmd {
        SpecCmd::Create {
            parent,
            file,
            title,
            dry_run,
            actor,
        } => {
            let body = read_body(&file)?;
            meta.dry_run = dry_run.then_some(true);
            ledger.spec_create(
                &actor.resolve(),
                &parent,
                title.as_deref(),
                &body,
                &now,
                dry_run,
            )??
        }
        SpecCmd::Get { spec, version } => ledger.spec_get(&spec, version)??,
        SpecCmd::List => {
            let items = ledger.spec_list()?;
            return Ok(Outcome::Page(Page {
                items: items.iter().map(value).collect::<Result<_, _>>()?,
                next_cursor: None,
                has_more: false,
            }));
        }
        SpecCmd::Update {
            spec,
            file,
            reason,
            expected_version,
            dry_run,
            actor,
        } => {
            let body = read_body(&file)?;
            meta.dry_run = dry_run.then_some(true);
            ledger.spec_update(
                &actor.resolve(),
                &spec,
                &body,
                &reason,
                expected_version,
                &now,
                dry_run,
            )??
        }
        SpecCmd::Approve {
            spec,
            dry_run,
            actor,
        } => {
            meta.dry_run = dry_run.then_some(true);
            ledger.spec_approve(&actor.resolve(), &spec, &now, dry_run)??
        }
    };
    Ok(Outcome::One(spec_out(&spec)?))
}

/// The markdown from a file, or stdin for `-`. Reads at most one byte past
/// the cap, so an oversized body is refused without being read whole.
fn read_body(path: &Path) -> Result<String, CliError> {
    let invalid = |m: String| CliError::from(Refusal::InvalidInput(m));
    let cap = SPEC_BODY_MAX_BYTES as u64 + 1;
    let mut buf = Vec::new();
    let read = if path == Path::new("-") {
        std::io::stdin().take(cap).read_to_end(&mut buf)
    } else {
        std::fs::File::open(path).and_then(|f| f.take(cap).read_to_end(&mut buf))
    };
    read.map_err(|e| invalid(format!("could not read {}: {e}", path.display())))?;
    if buf.len() > SPEC_BODY_MAX_BYTES {
        return Err(invalid(format!(
            "spec body is over the {SPEC_BODY_MAX_BYTES} byte limit"
        )));
    }
    String::from_utf8(buf).map_err(|_| invalid(format!("{} is not UTF-8", path.display())))
}

/// A spec as daisy's `get` prints it: the version, plus its slices (without
/// their bodies, which are in `body`) and its decisions, parsed.
fn spec_out(s: &Spec) -> Result<Value, CliError> {
    let slices: Vec<Value> = parse::parse_spec_sections(&s.body)
        .into_iter()
        .map(|sec| json!({ "n": sec.n, "key": sec.key, "title": sec.title, "done_when": sec.done_when }))
        .collect();
    let mut v = value(s)?;
    v["slices"] = json!(slices);
    v["decisions"] = value(parse::parse_decisions(&s.body))?;
    Ok(v)
}
