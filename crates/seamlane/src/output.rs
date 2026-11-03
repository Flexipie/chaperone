//! The machine contract (docs/cli-contract.md): one `{ok, data, meta}`
//! envelope on stdout, or one `{ok: false, error, meta}` on stderr, the
//! exit-code table and the two renderers.

use seamlane_core::cursor::Page;
use seamlane_core::ledger::{LedgerError, Refusal, SCHEMA_VERSION};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::Format;

#[derive(Debug)]
pub enum Outcome {
    One(Value),
    Page(Page<Value>),
}

/// `schema_version` is the ledger's `user_version`: every command but
/// `migrate` runs only on a file at this binary's version. Seamlane has no
/// mirrors, so `source_freshness` is always null.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub cli_version: &'static str,
    pub schema_version: i64,
    pub source_freshness: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageMeta>,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            cli_version: env!("CARGO_PKG_VERSION"),
            schema_version: SCHEMA_VERSION,
            source_freshness: None,
            dry_run: None,
            page: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PageMeta {
    pub count: usize,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug)]
pub enum CliError {
    Refused(Refusal),
    Ledger(LedgerError),
    Usage {
        message: String,
        kind: String,
        help: String,
    },
    ReadOnly(&'static str),
    Internal(String),
}

impl From<Refusal> for CliError {
    fn from(r: Refusal) -> Self {
        CliError::Refused(r)
    }
}

impl From<LedgerError> for CliError {
    fn from(e: LedgerError) -> Self {
        CliError::Ledger(e)
    }
}

impl CliError {
    /// A clap parse failure. Help and version never reach here.
    pub fn from_clap(e: &clap::Error) -> CliError {
        let help = e.to_string();
        CliError::Usage {
            message: help
                .lines()
                .next()
                .unwrap_or("usage error")
                .trim_start_matches("error: ")
                .to_string(),
            kind: format!("{:?}", e.kind()),
            help,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

fn body(code: &'static str, message: String, details: Option<Value>) -> ErrorBody {
    ErrorBody {
        code,
        message,
        retryable: false,
        details,
    }
}

impl ErrorBody {
    pub fn from_cli(e: &CliError) -> ErrorBody {
        match e {
            CliError::Refused(r) => match r {
                Refusal::InvalidInput(m) => body("invalid_input", m.clone(), None),
                Refusal::NotFound { entity, id } => body(
                    "not_found",
                    format!("{entity} {id} not found"),
                    Some(json!({ "entity": entity, "id": id })),
                ),
                Refusal::NotFoundRef { entity, reference } => body(
                    "not_found",
                    format!("{entity} {reference} not found"),
                    Some(json!({ "entity": entity, "reference": reference })),
                ),
                Refusal::Conflict { entity, message } => body(
                    "conflict",
                    message.clone(),
                    Some(json!({ "entity": entity })),
                ),
                Refusal::LeaseHeld { by, until } => body(
                    "conflict",
                    format!("held by {by} until {until}; pass --takeover to take it"),
                    Some(json!({ "by": by, "until": until })),
                ),
                Refusal::PipelineUnknown {
                    pipeline_id,
                    pipeline_hash,
                } => body(
                    "pipeline_unknown",
                    format!("this binary cannot load pipeline {pipeline_id} {pipeline_hash}"),
                    Some(json!({ "pipeline_id": pipeline_id, "pipeline_hash": pipeline_hash })),
                ),
            },
            CliError::Ledger(l) => {
                let mismatch = |found: Value, hint: &str| {
                    body(
                        "schema_mismatch",
                        l.to_string(),
                        Some(json!({ "found": found, "supported": SCHEMA_VERSION, "hint": hint })),
                    )
                };
                match l {
                    LedgerError::SchemaMismatch { found, supported } => mismatch(
                        json!(found),
                        if found > supported {
                            "update seamlane"
                        } else {
                            "run `seamlane migrate`"
                        },
                    ),
                    LedgerError::Missing(_) => mismatch(json!(0), "run `seamlane migrate`"),
                    LedgerError::NotALedger(_) => {
                        mismatch(Value::Null, "not a Seamlane ledger; check --db")
                    }
                    LedgerError::Sqlite(e)
                        if matches!(
                            e.sqlite_error_code(),
                            Some(
                                rusqlite::ErrorCode::DatabaseBusy
                                    | rusqlite::ErrorCode::DatabaseLocked
                            )
                        ) =>
                    {
                        ErrorBody {
                            retryable: true,
                            ..body("busy", l.to_string(), None)
                        }
                    }
                    LedgerError::Sqlite(_) => body("db", l.to_string(), None),
                    LedgerError::Io(_) => body("internal", l.to_string(), None),
                }
            }
            CliError::Usage {
                message,
                kind,
                help,
            } => body(
                "usage",
                message.clone(),
                Some(json!({ "kind": kind, "help": help })),
            ),
            CliError::ReadOnly(command) => {
                // Cancel is daisy's one destructive work command.
                let (mutability, writes) = if *command == "work cancel" {
                    ("destructive", "writes (destructive)")
                } else {
                    ("write", "writes")
                };
                body(
                    "read_only",
                    format!(
                        "seamlane is read-only here (DAISY_CLI_READONLY=1): `{command}` {writes}"
                    ),
                    Some(json!({ "command": command, "mutability": mutability })),
                )
            }
            CliError::Internal(m) => body("internal", m.clone(), None),
        }
    }
}

pub fn exit_code(e: &CliError) -> i32 {
    match ErrorBody::from_cli(e).code {
        "usage" => 2,
        "schema_mismatch" => 3,
        "conflict" => 4,
        _ => 1,
    }
}

pub fn render_ok(outcome: &Outcome, meta: &Meta, format: Format) -> String {
    let (data, lines, page) = match outcome {
        Outcome::One(v) => (
            v.clone(),
            vec![v],
            PageMeta {
                count: 1,
                next_cursor: None,
                has_more: false,
            },
        ),
        Outcome::Page(p) => (
            json!({ "items": p.items, "next_cursor": p.next_cursor, "has_more": p.has_more }),
            p.items.iter().collect(),
            PageMeta {
                count: p.items.len(),
                next_cursor: p.next_cursor.clone(),
                has_more: p.has_more,
            },
        ),
    };
    match format {
        Format::Json => {
            let env = json!({ "ok": true, "data": data, "meta": meta });
            serde_json::to_string_pretty(&env).unwrap_or_default() + "\n"
        }
        Format::Jsonl => {
            let mut out = String::new();
            for line in lines {
                out += &serde_json::to_string(line).unwrap_or_default();
                out.push('\n');
            }
            let meta = Meta {
                page: Some(page),
                ..meta.clone()
            };
            out += &serde_json::to_string(&json!({ "ok": true, "meta": meta })).unwrap_or_default();
            out.push('\n');
            out
        }
    }
}

pub fn render_err(err: &CliError, meta: &Meta) -> String {
    let env = json!({ "ok": false, "error": ErrorBody::from_cli(err), "meta": meta });
    serde_json::to_string_pretty(&env).unwrap_or_default() + "\n"
}
