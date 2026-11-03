//! The clap surface, daisy-cli's argv. Nothing here touches the ledger.

use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, ValueEnum};
use seamlane_core::cursor::DEFAULT_LIMIT;
use seamlane_core::ledger::{Actor, KINDS, PR_FLOWS};

#[derive(Parser, Debug)]
#[command(
    name = "seamlane",
    version,
    about,
    after_help = "Every command prints one {ok, data, meta} object on stdout, or an \
                  {ok: false, error, meta} object on stderr. Exit codes: 0 ok, 1 error, \
                  2 usage, 3 schema mismatch, 4 conflict, 5 runtime unavailable. Only \
                  `seamlane migrate` creates or migrates the ledger; every other command \
                  exits 3 on any other schema version."
)]
pub struct Cli {
    /// Path to the ledger (defaults to seamlane/ledger.db in the user data dir)
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,
    /// Output format: json is one {ok, data, meta} object; jsonl is one
    /// item per line followed by a trailer that is the only line with an
    /// `ok` key
    #[arg(long, global = true, value_enum, default_value_t = Format::Json)]
    pub format: Format,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Jsonl,
}

// Parsed once per process, never stored in bulk.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(subcommand)]
    Work(WorkCmd),
    /// Create the ledger, or migrate it to this binary's version after a
    /// backup. Refuses a file that is not a Seamlane ledger
    Migrate,
}

impl Cmd {
    /// The command's name when it writes, refused under
    /// `DAISY_CLI_READONLY=1` (a dry run included: it takes the write lock).
    pub fn write_name(&self) -> Option<&'static str> {
        match self {
            Cmd::Migrate => Some("migrate"),
            Cmd::Work(WorkCmd::Create { .. }) => Some("work create"),
            Cmd::Work(WorkCmd::Claim { .. }) => Some("work claim"),
            Cmd::Work(WorkCmd::Heartbeat { .. }) => Some("work heartbeat"),
            Cmd::Work(WorkCmd::Update { .. }) => Some("work update"),
            Cmd::Work(WorkCmd::Release { .. }) => Some("work release"),
            Cmd::Work(WorkCmd::Cancel { .. }) => Some("work cancel"),
            Cmd::Work(WorkCmd::Handoff { .. }) => Some("work handoff"),
            Cmd::Work(WorkCmd::Ready { .. }) => Some("work ready"),
            Cmd::Work(
                WorkCmd::Get { .. }
                | WorkCmd::List { .. }
                | WorkCmd::Handoffs { .. }
                | WorkCmd::Events { .. },
            ) => None,
        }
    }
}

/// Who is acting. Every write needs the (agent, session) pair; the ledger
/// refuses one without it.
#[derive(Args, Debug, Clone, Default)]
pub struct ActorArgs {
    /// Your agent name, [a-z0-9._-]{1,40} (default: $DAISY_AGENT)
    #[arg(long, global = true)]
    pub agent: Option<String>,
    /// Your session id, [A-Za-z0-9._:-]{1,120} (default: $DAISY_SESSION)
    #[arg(long, global = true)]
    pub session: Option<String>,
}

impl ActorArgs {
    /// Flags first, then the environment.
    pub fn resolve(&self) -> Actor {
        let pick = |flag: &Option<String>, var: &str| {
            flag.clone()
                .or_else(|| std::env::var(var).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Actor {
            agent: pick(&self.agent, "DAISY_AGENT"),
            session: pick(&self.session, "DAISY_SESSION"),
        }
    }
}

/// Leased work records. Claim an item before working on it, heartbeat
/// while you hold it, hand it off when you stop
#[derive(Subcommand, Debug)]
pub enum WorkCmd {
    /// Create a work item (queued). Idempotent when --idempotency-key is
    /// passed: a repeat returns the existing item and writes nothing
    Create {
        #[arg(long)]
        title: String,
        /// linear | github | note | issue | manual (default manual)
        #[arg(long)]
        source: Option<String>,
        /// CORE-123 for linear, owner/repo#N for github, PER-12 for issue,
        /// an id for note; stored as given
        #[arg(long)]
        source_id: Option<String>,
        /// The repo the work happens in, stored as given
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        next_action: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
        /// The spec this item is a slice of (its id or parent key)
        #[arg(long)]
        spec: Option<String>,
        /// The lane it runs in, e.g. CORE-4040:B (one herdr workspace)
        #[arg(long)]
        lane: Option<String>,
        /// Its order in the lane (a stack's layer)
        #[arg(long)]
        lane_pos: Option<i64>,
        /// What the lane delivers; picks the route at the first claim
        #[arg(long, value_parser = PossibleValuesParser::new(KINDS), default_value = "code")]
        kind: String,
        /// How it lands: through a PR, or direct
        #[arg(long, value_parser = PossibleValuesParser::new(PR_FLOWS), default_value = "pr")]
        pr_flow: String,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Take (or renew, or take over) the lease on an item. Needs an agent
    /// and a session
    Claim {
        id: i64,
        #[arg(long)]
        worktree: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        base_sha: Option<String>,
        /// Lease length in seconds, 30..=86400 (default 900)
        #[arg(long)]
        lease_secs: Option<i64>,
        /// Take an item another pair holds LIVE; recorded with their name
        #[arg(long)]
        takeover: bool,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Extend your lease by its own length. Writes no event
    Heartbeat {
        id: i64,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Change status or fields. --expected-updated-at is the item's current
    /// updated_at; a stale one is refused as a conflict (exit 4)
    Update {
        id: i64,
        /// queued | claimed | active | blocked | review | done | cancelled
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        /// Required (or --reason) when moving to blocked
        #[arg(long)]
        blocker: Option<String>,
        #[arg(long)]
        next_action: Option<String>,
        /// owner/repo#N
        #[arg(long)]
        pr: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        base_sha: Option<String>,
        /// The spec this item is a slice of (its id or parent key)
        #[arg(long)]
        spec: Option<String>,
        #[arg(long)]
        lane: Option<String>,
        #[arg(long)]
        lane_pos: Option<i64>,
        #[arg(long)]
        expected_updated_at: String,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Give the item up: clears the holder; claimed goes back to queued
    Release {
        id: i64,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Cancel an item (a status, not a delete)
    Cancel {
        id: i64,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        expected_updated_at: Option<String>,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Record a structured handoff (JSON: summary, status, changed_files,
    /// verification, smoke, blockers, open_questions, next_action, commit,
    /// pull_request, brief). Its status moves the item; a terminal status
    /// clears the holder. A ready handoff is refused until the handoff gate
    /// runs
    Handoff {
        id: i64,
        /// Path to the handoff JSON, or - for stdin; omit to read stdin
        file: Option<PathBuf>,
        /// Read the JSON from stdin (the default when no file is given)
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Every handoff on an item, oldest first
    Handoffs { id: i64 },
    /// The item's ledger, oldest first
    Events { id: i64 },
    /// Override whether the item is ready: --yes, --no, or --clear to go
    /// back to its handoffs. Refused from any session that has held it
    #[command(group = clap::ArgGroup::new("verdict").required(true).args(["yes", "no", "clear"]))]
    Ready {
        id: i64,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        no: bool,
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        reason: Option<String>,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// One item with its lease state
    Get { id: i64 },
    /// List items, newest activity first; done and cancelled hidden
    /// unless --all
    List {
        #[arg(long)]
        status: Option<String>,
        /// Only items held by this agent (any session)
        #[arg(long, conflicts_with = "mine")]
        agent: Option<String>,
        /// Only items held by my (agent, session) pair
        #[arg(long)]
        mine: bool,
        /// Include done and cancelled
        #[arg(long)]
        all: bool,
        /// Page size, 1..=500
        #[arg(long, default_value_t = DEFAULT_LIMIT)]
        limit: u32,
        /// Resume from a previous page's next_cursor
        #[arg(long)]
        cursor: Option<String>,
        /// Session for --mine (default: $DAISY_SESSION)
        #[arg(long)]
        session: Option<String>,
        /// Agent for --mine (default: $DAISY_AGENT)
        #[arg(long)]
        mine_agent: Option<String>,
    },
}
