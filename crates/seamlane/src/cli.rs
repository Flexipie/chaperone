//! The clap surface, daisy-cli's argv. Nothing here touches the ledger.

use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, ValueEnum};
use seamlane_core::cursor::DEFAULT_LIMIT;
use seamlane_core::ledger::{Actor, KINDS, PR_FLOWS, SESSION_CHECKS};

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
    #[command(subcommand)]
    Spec(SpecCmd),
    #[command(subcommand)]
    Check(CheckCmd),
    #[command(subcommand)]
    Reviews(ReviewsCmd),
    #[command(subcommand)]
    Jobs(JobsCmd),
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
            Cmd::Check(CheckCmd::Record { .. }) => Some("check record"),
            Cmd::Reviews(ReviewsCmd::Record { .. }) => Some("reviews record"),
            Cmd::Reviews(ReviewsCmd::Get { .. }) | Cmd::Jobs(JobsCmd::Get { .. }) => None,
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
                | WorkCmd::Events { .. }
                | WorkCmd::Board,
            ) => None,
            Cmd::Spec(SpecCmd::Create { .. }) => Some("spec create"),
            Cmd::Spec(SpecCmd::Update { .. }) => Some("spec update"),
            Cmd::Spec(SpecCmd::Approve { .. }) => Some("spec approve"),
            Cmd::Spec(SpecCmd::Decide { .. }) => Some("spec decide"),
            Cmd::Spec(SpecCmd::Get { .. } | SpecCmd::List) => None,
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
        /// How it lands: through a PR, direct, or none (no PR at all)
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
    /// clears the holder. A ready or done handoff runs the handoff gate
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
        /// Mark the item ready, whatever its handoffs say
        #[arg(long)]
        yes: bool,
        /// Mark the item not ready, whatever its handoffs say
        #[arg(long)]
        no: bool,
        /// Drop the override; ready comes from the latest handoff again
        #[arg(long)]
        clear: bool,
        /// Why, kept on the event
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
    /// Every open item with its newest handoff and whether it is ready: the
    /// owner's override, else status review and a ready handoff. Every item
    /// is under `other`; specs are not grouped yet
    Board,
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

/// The dispatch spec of a parent issue, versioned. /dispatch writes it,
/// /issue and /ship read it
#[derive(Subcommand, Debug)]
pub enum SpecCmd {
    /// Create the spec for a parent, at version 1 (draft). One per parent:
    /// a second create is refused and names the spec to update
    Create {
        /// The parent: CORE-1234, PER-12 or owner/repo#12. `--linear` is
        /// daisy's older name and still works
        #[arg(long, visible_alias = "linear")]
        parent: String,
        /// The markdown body, or - for stdin
        #[arg(long)]
        file: PathBuf,
        /// Defaults to the body's first `# ` heading, else the parent key
        #[arg(long)]
        title: Option<String>,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// A spec with its slices and decisions parsed from the body. With
    /// --key, that slice alone: its section, its decisions and the
    /// spec-wide ones, and its siblings with their work item status and PR.
    /// --key without a spec finds the spec that has the slice
    Get {
        /// The spec id or its parent key (any case)
        #[arg(required_unless_present = "key")]
        spec: Option<String>,
        /// An older version (default: the newest)
        #[arg(long)]
        version: Option<i64>,
        /// A slice's issue key: CORE-1235, PER-12 or owner/repo#12
        #[arg(long)]
        key: Option<String>,
    },
    /// Every spec at its newest version, most recently changed first
    List,
    /// Append a new version
    Update {
        /// The spec id or its parent key (any case)
        spec: String,
        /// The markdown body, or - for stdin
        #[arg(long)]
        file: PathBuf,
        /// Why it changed, kept with the version
        #[arg(long)]
        reason: String,
        /// Exit 4 unless the newest version is still this one
        #[arg(long)]
        expected_version: Option<i64>,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Mark the spec approved (draft -> approved); approving again is a
    /// no-op
    Approve {
        /// The spec id or its parent key (any case)
        spec: String,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Record a decision: one line appended to `## decisions` as a new
    /// version, `- [slice N] Q: ... A: ... (agreed: <where>, <date>)`.
    /// Without --slice or --key it is spec-wide
    Decide {
        /// The spec id or its parent key (any case)
        spec: String,
        /// The slice by its number
        #[arg(long, conflicts_with = "key")]
        slice: Option<u32>,
        /// The slice by its issue key instead of its number
        #[arg(long)]
        key: Option<String>,
        /// The question, on one line
        #[arg(long)]
        question: String,
        /// The answer, on one line
        #[arg(long)]
        answer: String,
        /// Where it was agreed: a PR thread URL, a Slack link, "in chat"
        #[arg(long = "where")]
        agreed_where: Option<String>,
        /// Run the real write and roll it back; meta.dry_run is true
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
}

/// Check results a fresh session records for a lane: plan check, quality
/// gate, merge verdict, smoke and kept tests
#[derive(Subcommand, Debug)]
pub enum CheckCmd {
    /// Record one check result on a work item. Refused from any session
    /// that has held it: pass your own --session. The same result again
    /// replays and writes nothing
    Record {
        #[arg(long)]
        work: i64,
        #[arg(long, value_parser = PossibleValuesParser::new(SESSION_CHECKS))]
        check: String,
        /// The commit checked, 7 to 40 lowercase hex digits
        #[arg(long)]
        head: String,
        #[arg(long, value_parser = PossibleValuesParser::new(["ran", "skipped"]))]
        outcome: String,
        /// Why it was skipped; required with skipped, from the check's list
        #[arg(long)]
        reason: Option<String>,
        /// merge_verdict only, required there
        #[arg(long, value_parser = PossibleValuesParser::new(["yes", "no"]))]
        decision: Option<String>,
        /// Findings raised; plan_check and quality_gate that ran only,
        /// required there
        #[arg(long)]
        findings: Option<u64>,
        #[command(flatten)]
        actor: ActorArgs,
    },
}

/// Daisy's review jobs, as recorded in the ledger. The worker stays in
/// daisy; lanes keep `daisy-cli reviews local` and `reviews get --wait`
#[derive(Subcommand, Debug)]
pub enum ReviewsCmd {
    /// Read a daisy review job with `daisy-cli reviews get` and record it on
    /// a work item: requested while it runs, finished (with its findings)
    /// once done. Recording the same state again writes nothing
    Record {
        #[arg(long)]
        work: i64,
        /// The daisy review job id
        #[arg(long)]
        job: i64,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// A recorded review job in daisy's shape. Daisy's free text (summary,
    /// impact, report path) is null; read it from daisy
    Get {
        job: i64,
        /// Accepted for daisy's argv; a recorded job answers at once
        #[arg(long)]
        wait: bool,
        /// Accepted for daisy's argv; unused
        #[arg(long)]
        timeout_secs: Option<u64>,
    },
}

/// Recorded daisy jobs
#[derive(Subcommand, Debug)]
pub enum JobsCmd {
    /// A recorded review job in daisy's job shape; what only daisy knows
    /// (cost, tokens, job dir) is null
    Get { id: i64 },
}
