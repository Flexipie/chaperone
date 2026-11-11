//! The clap surface, daisy-cli's argv. Nothing here touches the ledger.

use std::path::PathBuf;

use chaperone_core::checks::{ATTENTION_BY, ATTENTION_TYPES, QUESTION_REASONS};
use chaperone_core::cursor::DEFAULT_LIMIT;
use chaperone_core::ledger::{Actor, KINDS, PR_FLOWS, SESSION_CHECKS};
use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "chaperone",
    version,
    about,
    after_help = "Every command prints one {ok, data, meta} object on stdout, or an \
                  {ok: false, error, meta} object on stderr. Exit codes: 0 ok, 1 error, \
                  2 usage, 3 schema mismatch, 4 conflict, 5 runtime unavailable. Only \
                  `chaperone migrate` creates or migrates the ledger; every other command \
                  exits 3 on any other schema version."
)]
pub struct Cli {
    /// Path to the ledger (defaults to chaperone/ledger.db in the user data dir)
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
    #[command(subcommand)]
    Usage(UsageCmd),
    #[command(subcommand)]
    Attention(AttentionCmd),
    #[command(subcommand)]
    Finding(FindingCmd),
    #[command(subcommand)]
    Dossier(DossierCmd),
    #[command(subcommand)]
    Evidence(EvidenceCmd),
    /// Per file, the open lanes touching it: each lane's newest handoff
    /// (changed_files), then the commits on its worktree's HEAD since the
    /// merge-base with origin, all git calls within 5 s. Shared files
    /// first. Call it before starting work
    Overlap {
        /// Only lanes in this repo (owner/repo, any case)
        #[arg(long)]
        repo: Option<String>,
        /// Also each worktree's uncommitted and untracked files (stats
        /// every tracked file: slower, more so in large repos)
        #[arg(long)]
        uncommitted: bool,
    },
    /// R11's lane and cohort numbers, read from the ledger: claim to
    /// merge, tokens, peak context, attention, checks and the verdict on
    /// the merged head. A number nothing records yet is null, and
    /// `missing` names what it needs
    #[command(group(clap::ArgGroup::new("cohort").required(true)))]
    Stats {
        /// Lanes with a PR merged at or after this: YYYY-MM-DD (00:00 UTC)
        /// or RFC 3339 UTC with milliseconds
        #[arg(long, group = "cohort")]
        merged_since: Option<String>,
        /// These work items (repeat the flag for more)
        #[arg(long, group = "cohort")]
        work: Vec<i64>,
    },
    /// The files that depend on these Rust files, nearest first: module
    /// nodes and import edges read with tree-sitter from the worktree,
    /// facts cached by content per origin. Each depth says if it is exact
    /// or a lower bound; at most 500 files listed
    Impact {
        /// Files, from the repo root or the current directory
        #[arg(required = true)]
        files: Vec<String>,
        /// The cache file (default: one per origin in the user cache
        /// directory)
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Also list the entry points (commands, routes, screens) the files
        /// reach, each with its area and evidence, from the pipeline
        /// profile's `[[entry_points]]` and `[areas]`
        #[arg(long)]
        entry_points: bool,
        /// The profile (default: .chaperone/profile.toml on the repo's
        /// base branch, else the origin's in
        /// ~/dotfiles/claude/pipeline/profiles)
        #[arg(long, requires = "entry_points")]
        profile: Option<PathBuf>,
        /// Compare these areas (comma-separated, from the profile's
        /// `[areas]`, as a handoff's `areas` declares them) with the areas
        /// the files are in and the entry points they reach
        #[arg(long, requires = "entry_points", value_delimiter = ',')]
        declared: Option<Vec<String>>,
    },
    /// Run `check run` diff, tests, review and verdict on the lane's commit
    /// in that order, under one hold of the lane's run lock (a second run is
    /// a conflict, exit 4), stopping at the first step refused or not
    /// passed; the verdict runs only after a complete review that leaves no
    /// finding blocking. Prints each step and where and why it stopped. Plan
    /// and smoke are not steps
    Verify {
        #[arg(long)]
        work: i64,
        #[command(flatten)]
        actor: ActorArgs,
    },
    #[command(subcommand)]
    Pr(PrCmd),
    /// The owner's merge of the lane's PR, as chaperone-ui's Merge: on a
    /// YES verdict on --head, the lane ready and GitHub's PR head still
    /// --head, `gh pr merge --squash --delete-branch --match-head-commit`,
    /// then `pr.merged`, then the done handoff unless the lane's lease is
    /// live, when `herdr agent prompt` tells its agent to hand off done.
    /// Refused from any session or agent that has held the lane
    Merge {
        #[arg(long)]
        work: i64,
        /// The head you confirm (default: the lane's handoff head)
        #[arg(long)]
        head: Option<String>,
        #[command(flatten)]
        actor: ActorArgs,
    },
    #[command(subcommand)]
    Pipeline(PipelineCmd),
    /// Check skill and script files against this binary: every chaperone
    /// command, flag and value they call, the handoff fields they write and
    /// the pipeline version they name. With --repo, also the repo's profile
    /// areas and whether this binary's code differs from the repo's origin/main.
    /// Any mismatch is a `drift` error (exit 1) listing each with file:line.
    /// Reads files and git, never the ledger
    Doctor {
        /// Directories (searched recursively) or files to read: .md, .sh,
        /// .py, and scripts with a shell or python shebang
        #[arg(long, required = true, num_args = 1..)]
        skills: Vec<PathBuf>,
        /// A checkout of the repo: check its profile's `[areas]` and
        /// `[[entry_points]]` against its tracked files, and this binary's
        /// commit against its origin/main
        #[arg(long)]
        repo: Option<PathBuf>,
        /// The profile (default: .chaperone/profile.toml on the --repo
        /// base branch, else its origin's in
        /// ~/dotfiles/claude/pipeline/profiles)
        #[arg(long, requires = "repo")]
        profile: Option<PathBuf>,
    },
    /// Create the ledger, or migrate it to this binary's version after a
    /// backup. Refuses a file that is not a Chaperone ledger
    Migrate,
    /// Write the Claude Code plugin embedded in this binary as a
    /// marketplace in the user data dir, replacing what was there, and
    /// register it with `claude plugin marketplace add` and `claude plugin
    /// install chaperone@chaperone --scope user`. Safe to run again.
    /// Never reads the ledger
    Install,
    /// `claude plugin uninstall` and `claude plugin marketplace remove` for
    /// what `install` registered (a refusal of what is not there is
    /// reported, not an error), then delete the marketplace directory
    Uninstall,
    /// The plugin's Claude Code hook for the lane $CHAPERONE_WORK: reads
    /// the hook's JSON on stdin and prints the hook's answer, not an
    /// envelope. Nothing outside an open lane on a v20 or newer pipeline;
    /// the plugin's shim keeps every failure silent
    Hook { event: HookEvent },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum HookEvent {
    /// The lane's state, gates not passing yet and commands, as context
    SessionStart,
    /// Deny `gh pr create`, `gh pr merge` and `chaperone work ready --yes`
    PreBash,
    /// Block a stop once the branch moved past the lane's ready handoff
    Stop,
    /// Record the API error the turn stopped on, once per error kind
    StopFailure,
}

/// The lane's pull request
#[derive(Subcommand, Debug)]
pub enum PrCmd {
    /// Under the lane's run lock (a second run is a conflict, exit 4), on
    /// a clean worktree at the lane's branch tip: `check run` diff and
    /// tests (a pass on the same tree replays), refused past the
    /// pipeline's code line limit, with a file git counts no lines of, or
    /// when the tests fail. Then `git push origin <commit>:refs/heads/<branch>`
    /// and `gh pr create`, or `gh pr edit` when the lane holds a PR or gh
    /// lists one for the branch, with a body made from ledger facts; a PR
    /// new to the ledger is recorded as `pr.opened` on that commit
    Open {
        #[arg(long)]
        work: i64,
        /// Default on create: the work item's title; on edit, kept
        #[arg(long)]
        title: Option<String>,
        /// Open it as a draft (create only)
        #[arg(long)]
        draft: bool,
        #[command(flatten)]
        actor: ActorArgs,
    },
}

/// The pipeline documents this binary runs
#[derive(Subcommand, Debug)]
pub enum PipelineCmd {
    /// The pipeline a new claim stamps: {id, hash, newer}, `newer` the
    /// embedded pipelines a first claim may stamp instead (`work claim
    /// --pipeline`). Skills read the version from here rather than naming
    /// one
    Current,
    /// The `[limits]` table of the lane's pipeline, else of the one a new
    /// claim stamps: {pipeline: {id, hash}, limits}
    Limits {
        /// The lane (default: the pipeline a new claim stamps)
        #[arg(long)]
        work: Option<i64>,
    },
    /// Each version of the pipeline changelog (`## vN <time>` in
    /// <dotfiles>/claude/pipeline-changelog.md), newest first, with its
    /// entry, engine ids and `stats`' cohort numbers over its lanes: the
    /// first `pipeline vN` a handoff summary names, else the version
    /// active at the lane's first claim
    History {
        /// The dotfiles checkout (default: ~/dotfiles)
        #[arg(long)]
        dotfiles: Option<PathBuf>,
        /// Only lanes in this repo (owner/repo, any case)
        #[arg(long)]
        repo: Option<String>,
    },
}

impl Cmd {
    /// The command's name when it writes, refused under
    /// `DAISY_CLI_READONLY=1` (a dry run included: it takes the write lock).
    pub fn write_name(&self) -> Option<&'static str> {
        match self {
            Cmd::Migrate => Some("migrate"),
            Cmd::Check(CheckCmd::Record { .. }) => Some("check record"),
            Cmd::Check(CheckCmd::Run { .. }) => Some("check run"),
            Cmd::Check(CheckCmd::Waive { .. }) => Some("check waive"),
            Cmd::Check(CheckCmd::Prune) => None,
            Cmd::Verify { .. } => Some("verify"),
            Cmd::Pr(PrCmd::Open { .. }) => Some("pr open"),
            Cmd::Merge { .. } => Some("merge"),
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
            Cmd::Work(WorkCmd::Merged { .. }) => Some("work merged"),
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
            Cmd::Usage(UsageCmd::Report { .. }) => Some("usage report"),
            Cmd::Usage(UsageCmd::Get { .. }) => None,
            Cmd::Attention(AttentionCmd::Record { .. }) => Some("attention record"),
            Cmd::Finding(FindingCmd::Dispute(_)) => Some("finding dispute"),
            Cmd::Finding(FindingCmd::Waive(_)) => Some("finding waive"),
            Cmd::Finding(FindingCmd::Accept(_)) => Some("finding accept"),
            Cmd::Finding(FindingCmd::Answer(_)) => Some("finding answer"),
            Cmd::Finding(FindingCmd::List { .. }) => None,
            Cmd::Dossier(DossierCmd::Note { .. }) => Some("dossier note"),
            Cmd::Dossier(DossierCmd::Get { .. }) => None,
            Cmd::Evidence(EvidenceCmd::Show { .. }) => None,
            Cmd::Hook {
                event: HookEvent::StopFailure,
            } => Some("hook stop-failure"),
            Cmd::Hook { .. }
            | Cmd::Overlap { .. }
            | Cmd::Stats { .. }
            | Cmd::Impact { .. }
            | Cmd::Pipeline(_)
            | Cmd::Doctor { .. }
            | Cmd::Install
            | Cmd::Uninstall => None,
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
        /// Absolute path of the lane's worktree
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
        /// At the first claim, stamp this embedded pipeline in place of the
        /// default: one newer than it (`pipeline current`'s `newer`), for a
        /// pilot lane
        #[arg(long)]
        pipeline: Option<String>,
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
    /// areas, verification, smoke, blockers, open_questions, next_action,
    /// commit, pull_request, brief). Its status moves the item; a terminal status
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
    /// Record that one of the item's PRs merged (for /merge). The
    /// same PR again replays and writes nothing
    Merged {
        id: i64,
        /// owner/repo#N, a PR the item has held
        #[arg(long)]
        pr: String,
        /// The merge commit on the base
        #[arg(long)]
        merge_sha: String,
        /// The PR head that was merged
        #[arg(long)]
        head: String,
        /// When the forge merged it, RFC 3339 UTC with milliseconds
        #[arg(long)]
        merged_at: String,
        /// git patch-id of the merged diff
        #[arg(long)]
        patch_id: Option<String>,
        #[command(flatten)]
        actor: ActorArgs,
    },
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
    /// owner's override, else status review and a ready handoff, and the
    /// paths its handoff shares with other open lanes' (`chaperone overlap`
    /// adds worktrees). Every item is under `other`; specs are not grouped yet
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
/// gate, merge verdict, smoke, kept tests and review rounds; and the checks
/// the binary runs itself
#[derive(Subcommand, Debug)]
pub enum CheckCmd {
    /// Record one check result on a work item. Refused from any session
    /// that has held it, except a smoke claim on a v20 lane: pass your own
    /// --session. The same result again replays and writes nothing
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
        /// smoke only: a text file the smoke wrote or showed, inside the
        /// lane's worktree (a relative path is read from its root), not under
        /// .git or .chaperone, at most 1 MiB; repeat for up to 12
        #[arg(long)]
        evidence: Vec<PathBuf>,
        /// smoke that ran only: a script, by the --evidence file rules, that
        /// the CLI runs with bash in the worktree's root; the claim keeps
        /// the script, its output and exit code as the first evidence file.
        /// Required for a smoke that ran on a v20 lane
        #[arg(long)]
        run: Option<PathBuf>,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Run one check on the lane's commit in its worktree and record the
    /// result as the harness's. One run per lane at a time: a second is a
    /// conflict (exit 4). A run that fails records nothing; the same tree
    /// again replays
    Run {
        kind: RunKind,
        #[arg(long)]
        work: i64,
        /// The plan file, for plan only: regular UTF-8 text of at most 64 KiB
        #[arg(long)]
        file: Option<PathBuf>,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Waive a check the harness runs for a lane that cannot pass it now:
    /// the plan check of a lane that coded before its plan was attacked.
    /// The owner's: refused from any session that has held the lane
    Waive {
        kind: WaiveKind,
        #[arg(long)]
        work: i64,
        /// Why; one line, kept on the event and redacted at write
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// Remove the build directories and checkouts beside the ledger of lanes
    /// that are done, cancelled or not in the ledger, as every `check run`
    /// and `verify` does first; a lane whose run is in progress is skipped.
    /// Writes nothing to the ledger
    Prune,
}

/// The checks the owner may waive
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum WaiveKind {
    /// The plan check, on the commit the lane is on (none before v20)
    Plan,
}

/// The checks `check run` does itself
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum RunKind {
    /// Files and lines the commit changes against the merge base
    Diff,
    /// The base branch profile's `[tests]` commands, in a fresh checkout of
    /// the commit the harness makes and removes each run; the checkout
    /// must stay at the commit and clean
    Tests,
    /// Four review lenses on the commit's diff, in that checkout, each on
    /// the CLI its pipeline runner names, then a verifier that tries to
    /// refute each finding; the lane's pipeline names them (none before v20)
    Review,
    /// One fresh session on whether the commit should merge, from the
    /// change, the lane's goal and the harness's own diff, tests and
    /// complete review of it; a verdict stands for its tree, so the same
    /// tree and results replay it, yes or no (none before v20)
    Verdict,
    /// One fresh session attacking the plan in --file, before any code: a
    /// commit that changes a code file is refused (the owner can waive the
    /// check); the same plan on the same tree replays (none before v20)
    Plan,
    /// The base branch profile's `[smoke]` commands, run as `tests` runs
    /// its; no `[smoke]` table is refused, never a pass (none before v20)
    Smoke,
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

/// Token usage per provider message, read from a lane's transcripts
#[derive(Subcommand, Debug)]
pub enum UsageCmd {
    /// Read the transcripts that started in the work item's worktree and
    /// record one usage event per message it does not have yet. Counts and
    /// ids only. Reporting again writes nothing new
    Report {
        #[arg(long)]
        work: i64,
        /// Claude Code's config dir (default: $CLAUDE_CONFIG_DIR, else
        /// ~/.claude, when it exists)
        #[arg(long)]
        claude_dir: Option<PathBuf>,
        /// Codex's home (default: $CODEX_HOME, else ~/.codex, when it
        /// exists)
        #[arg(long)]
        codex_dir: Option<PathBuf>,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// The work item's usage: totals per source (parent, subagent) and one
    /// row per transcript. known is false when nothing was recorded
    Get { id: i64 },
}

/// A lane's review and plan check findings: list them, the lane disputes
/// or accepts one, the owner waives one
#[derive(Subcommand, Debug)]
pub enum FindingCmd {
    /// Every finding of the lane's complete harness reviews, of any commit,
    /// and of its newest plan check, oldest first, with the reason of each
    /// one's newest move; blocks is true for those the no_blocking_findings
    /// gate counts
    List {
        #[arg(long)]
        work: i64,
    },
    /// Say an open or verified finding is wrong. Only from a session that
    /// has held the lane; it still blocks until a session that never has
    /// rules on it
    Dispute(FindingArgs),
    /// Stop an open, verified or disputed finding blocking. The owner's:
    /// refused from any session that has held the lane
    Waive(FindingArgs),
    /// Say the lane will act on an open plan check finding, and how. Only
    /// from a session that has held the lane; the review is shown it
    Accept(FindingArgs),
    /// Answer an open question finding, as anyone may
    Answer(FindingArgs),
}

/// One finding, addressed as the ledger does, and why
#[derive(Args, Debug)]
pub struct FindingArgs {
    /// The event id of the check.finished that raised it (`work events`)
    pub event: i64,
    /// The finding's key in that check (its id in `reviews get`)
    pub key: String,
    /// Why; kept on the event and redacted at write
    #[arg(long)]
    pub reason: String,
    /// Run the real write and roll it back; meta.dry_run is true
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub actor: ActorArgs,
}

/// Interventions on a lane by the orchestrator or the owner
#[derive(Subcommand, Debug)]
pub enum AttentionCmd {
    /// Record one intervention: a question asked, its answer, a nudge, a
    /// send-back or a revive. The same record again with nothing new on the
    /// lane replays; pass --idempotency-key for a real repeat
    Record {
        #[arg(long)]
        work: i64,
        #[arg(long = "type", value_parser = PossibleValuesParser::new(ATTENTION_TYPES))]
        kind: String,
        /// Who handled it (for asked, who the question went to)
        #[arg(long, value_parser = PossibleValuesParser::new(ATTENTION_BY))]
        by: String,
        /// asked only, required there: R12's reason id
        #[arg(long, value_parser = PossibleValuesParser::new(QUESTION_REASONS))]
        reason: Option<String>,
        /// sent_back only, required there: the send-back round, 1 or more
        #[arg(long)]
        round: Option<u64>,
        /// answered only, required there: the event id of the question
        #[arg(long)]
        answers: Option<i64>,
        /// One line, at most 500 characters; required for answered (the
        /// answer). Redacted at write
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[command(flatten)]
        actor: ActorArgs,
    },
}

/// The agent-only record of an issue key: notes, and one capped read of
/// what the ledger knows about the key. Data for agents, never a gate
#[derive(Subcommand, Debug)]
pub enum DossierCmd {
    /// Leave a note on a key: what you learned that the issue does not say.
    /// The same note again from the same session replays. Notes are never
    /// edited; --supersedes hides an older one from `dossier get`
    Note {
        /// The issue key: CORE-1234, PER-12 or owner/repo#12
        key: String,
        /// At most 600 characters. Redacted at write
        #[arg(long)]
        note: String,
        /// <kind>:<ref>, up to 5: issue:CORE-1, pr:owner/repo#12,
        /// adr:docs/decisions/0007-issue-dossier.md, doc:<path>, commit:<sha>
        #[arg(long = "link")]
        links: Vec<String>,
        /// The event id of an earlier note on the same key that this one
        /// replaces
        #[arg(long)]
        supersedes: Option<i64>,
        #[command(flatten)]
        actor: ActorArgs,
    },
    /// The key's slice and spec decisions, lanes, handoffs, files and notes,
    /// capped (48,000 bytes at most as printed); every cut is counted in its
    /// section's `omitted`. not_found when the ledger has none of them
    Get {
        /// The issue key: CORE-1234, PER-12 or owner/repo#12
        key: String,
    },
}

/// The evidence store beside the ledger: text a check or a harness session
/// stored, named by its sha256
#[derive(Subcommand, Debug)]
pub enum EvidenceCmd {
    /// Print the text stored under a hash (a `prompt_ref`, `answer_ref` or
    /// `evidence_ref` of a check event, as `check run` prints it), hashed
    /// again, not in an envelope:
    /// piped, its bytes; on a terminal, control characters other than
    /// newline and tab escaped. Refused: an unknown ref (not_found), a ref that
    /// is not 12 to 64 lowercase hex or a prefix two files share
    /// (invalid_input), a file whose bytes no longer match its name
    /// (internal). Never opens the ledger
    Show {
        /// The sha256, or a unique prefix of at least 12 of its digits
        #[arg(value_name = "REF")]
        reference: String,
    },
}
