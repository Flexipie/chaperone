# R5: State model and storage

> Superseded in part by [ADR 0003](../decisions/0003-seamlane-replaces-daisy.md) and [events.md](../events.md): daisy does not read the ledger (no `reader_contract`), event kinds and folded statuses (`ran | skipped`, `open | fixed | answered`, no `ready` status) are in events.md.

Status: draft, 2026-11-02. Storage decision for the ledger. Language and the pipeline format stay with R6 and R7. This feeds a decision record when those land.

## Recommendation

One SQLite file, WAL mode, `synchronous=FULL`, separate from daisy's database. Many agent processes write it. There is no storage daemon.

Each accepted mutation is one `BEGIN IMMEDIATE` transaction that does two things: append one row to an event log, and update the current-state tables (lane, spec, gate, finding, message) to match. The log is the history. The tables are the fold of that log, written in the same transaction, so a reader never sees one without the other. A rebuild test folds the log back into empty tables and compares.

Heartbeats and lease renewals update the lane row only. They are clock ticks. A crash that drops one makes the lease expire sooner, which is the safe direction. Putting them in the log would bury the rows metrics need. Daisy already made that split for `work_events` versus `last_heartbeat_at` (`daisy-core` migration `0028_work_items.sql`).

Daisy opens this file read-only and never migrates it. Closing daisy does not stop a lane.

## Why this shape

VISION asks for an append-only ledger: metrics are queries, a lane can be replayed, and nothing is reconstructed from transcripts. It also asks for gates that refuse a transition. Those two needs pull in different directions, and the same-transaction fold is how both hold.

| Approach | What it gives | Why it loses here |
|----------|----------------|-------------------|
| **State tables plus a log, one transaction** | Current lane, lease, and gate in one read. History for metrics and replay. Crash cannot commit one without the other. | Every write path has to go through one function. A second path that updates a table and skips the log will lie, and only a rebuild test catches it. |
| **Pure event sourcing (fold on read, or an async projection)** | One source of truth, replay is the product. | A gate check is a compare-and-swap on current status, lease, pipeline version, and head sha. Folding on every check holds the write lock longer. An async projection can show daisy a gate that has not landed yet. Daisy already dropped a tail-replay projection for jobs: `job_events` "updates the jobs row in the same write, so there is nothing to project" (migration `0010_jobs.sql`). |
| **State tables only** | Simple reads, leases, conditional updates. | This is the current measuring problem. `lane-stats` reconstructs plan checks, quality gates, and verdicts from handoff text after the fact (pipeline changelog, v17). |
| **JSONL per lane** | Easy to append, easy to tail. | A torn last line on crash. No transaction across a lane row and a message to a sibling. Claims and gate CAS need a lock manager anyway. |
| **Postgres** | Real concurrent writers, mature tooling. | A daemon, a port, backups, and a process that must be up. Against "one local binary, works offline, no daemon zoo" (VISION). Revisit only if the ledger ever leaves one machine. |
| **DuckDB, redb, LMDB** | Fast local storage. | DuckDB is the wrong writer model. Key-value stores give up the SQL daisy and the metrics queries already speak. |

SQLite is the fit because daisy-core has run this pattern since the work ledger: WAL, `busy_timeout` 5000, `BEGIN IMMEDIATE`, `user_version` migrations in the same transaction as the DDL (`daisy-core/src/db.rs`). The harness ledger is that pattern pulled out of the personal app, not a new database theory.

## What was measured

Probe on this machine, 2026-11-02: SQLite 3.54.1 (Homebrew Python 3.15), local APFS. Shape under test: 50 processes, each appending 40 events. Every event is its own `BEGIN IMMEDIATE` that inserts one log row (about 200 bytes of JSON) and updates that lane's `head_event_id` and `event_count`. Each writer then retries its last idempotency key. Heartbeat-style lease updates run on every fifth event and do not append. `PRAGMA busy_timeout=5000`.

| Case | Result |
|------|--------|
| `synchronous=FULL`, 50 writers, 2000 events | 0.971 s, about 2060 commits/s. `integrity_check` ok. 2000 distinct keys. 0 lanes where `event_count` or `head_event_id` disagreed with the log. 0 errors. The retry of each last key raised `UNIQUE` and did not insert. |
| `synchronous=NORMAL`, same load | 0.631 s, about 3170 commits/s. Same correctness result. |
| `SIGKILL` after insert and lane update, before `COMMIT` | Uncommitted event absent. Previously committed seed still present. Integrity ok. Projection not drifted. |
| `SIGKILL` during a tight commit loop | 985 committed events, 985 distinct keys, integrity ok, projection matched. The in-flight transaction is gone. |
| Two `BEGIN` (deferred) readers that then both write | Second writer got `SQLITE_BUSY_SNAPSHOT` (extended code 517, message "database is locked") immediately. `busy_timeout` of 5 seconds did not wait. |
| One transaction held open for 1.5 s | The other writer blocked, then committed after 1.60 s. |
| One transaction held open for 6 s | The other writer failed at 5.18 s with "database is locked". |

A real lane does not write 40 events a second. It writes on claim, a few gates, findings, and messages, and it renews a lease on a timer. Fifty lanes are far under 2060 commits/s. The number that matters is the other one: a transaction that stays open for longer than `busy_timeout` fails every other writer.

`FULL` is the setting to ship. SQLite documents that WAL plus `synchronous=NORMAL` survives an application crash without corruption, and that an OS crash or power loss can roll back commits that had already returned. A gate acknowledgement is one of those commits. `FULL` syncs the WAL on commit. The probe's gap (0.97 s versus 0.63 s for 2000 commits) is not worth a gate that the process believed had landed.

Sources for the SQLite behaviour: [WAL](https://www.sqlite.org/wal.html), [synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous), [SQLITE_BUSY_SNAPSHOT](https://www.sqlite.org/rescode.html#busy_snapshot). WAL is one writer, many readers, and it does not work on a network filesystem. The file stays on the local disk, not in iCloud or Dropbox, same rule as a database that depends on file locks.

## Data model

Draft schema. Names of gates are the v17 ones (`plan_check`, `quality_gate`, `merge_verdict` from `pipeline-changelog.md`); R7 owns the real list. Payload schemas per `kind` are a later contract. This is the shape, not a migration to copy in unchanged.

Invariants:

- One write function. It inserts the event and applies the fold. Rebuild uses the same apply function on an empty database. Shipping a second update path is a bug.
- A refused transition writes nothing. No event, no row change. Daisy's work ledger already does this.
- `events.id` is the subscribe cursor (`WHERE id > ?`). No `AUTOINCREMENT`: a rolled-back insert must not burn an id (daisy's `dry_run` note in `db.rs`). Gaps are fine. Order is the id.
- Events have no foreign keys, so history survives a projection being rebuilt. Daisy's `work_events` is the same.
- Idempotency key is unique on the event, globally, chosen by the caller before the call and reused on retry. Scope is not the session: a crash that starts a new session must still dedupe. Natural keys also apply where the domain has them (below).
- A secret does not go in `payload_json`. Typed payloads per kind. R13 writes the rule; the column is not a junk drawer.

```sql
CREATE TABLE events (
  id              INTEGER PRIMARY KEY,
  lane_id         INTEGER,
  spec_id         INTEGER,
  kind            TEXT NOT NULL,
  actor_agent     TEXT,
  actor_session   TEXT,
  idempotency_key TEXT NOT NULL UNIQUE,
  payload_json    TEXT NOT NULL,
  at              TEXT NOT NULL
);
CREATE INDEX events_lane ON events (lane_id, id);

CREATE TABLE specs (
  id            INTEGER PRIMARY KEY,
  external_key  TEXT NOT NULL UNIQUE,   -- tracker id, not only Linear
  title         TEXT NOT NULL,
  status        TEXT NOT NULL,          -- draft | approved
  head_version  INTEGER NOT NULL,
  head_event_id INTEGER NOT NULL,
  updated_at    TEXT NOT NULL
);

CREATE TABLE spec_versions (
  spec_id  INTEGER NOT NULL,
  version  INTEGER NOT NULL,
  body     TEXT NOT NULL,
  author   TEXT,
  event_id INTEGER NOT NULL,
  at       TEXT NOT NULL,
  PRIMARY KEY (spec_id, version)
);

CREATE TABLE lanes (
  id               INTEGER PRIMARY KEY,
  spec_id          INTEGER NOT NULL,
  lane_key         TEXT NOT NULL,
  pipeline_version TEXT NOT NULL,       -- pinned when the lane is created
  status           TEXT NOT NULL,       -- queued|claimed|active|blocked|review|ready|done|cancelled
  agent            TEXT,
  session          TEXT,
  lease_expires_at TEXT,
  head_sha         TEXT,
  branch           TEXT,
  worktree_path    TEXT,
  head_event_id    INTEGER NOT NULL,
  updated_at       TEXT NOT NULL,
  UNIQUE (spec_id, lane_key)
);
CREATE INDEX lanes_status ON lanes (status, updated_at);
CREATE INDEX lanes_holder ON lanes (agent, session);

-- Current outcome only. A pass on an older sha is a different row, so it
-- cannot satisfy the head. That is the v17 failure mode (verdict not on head)
-- stored as a key.
CREATE TABLE gate_results (
  lane_id          INTEGER NOT NULL,
  gate             TEXT NOT NULL,
  head_sha         TEXT NOT NULL,
  pipeline_version TEXT NOT NULL,
  status           TEXT NOT NULL,       -- passed | failed | skipped
  evidence_ref     TEXT,
  event_id         INTEGER NOT NULL,
  at               TEXT NOT NULL,
  PRIMARY KEY (lane_id, gate, head_sha)
);

CREATE TABLE findings (
  id              INTEGER PRIMARY KEY,
  lane_id         INTEGER NOT NULL,
  review_event_id INTEGER NOT NULL,
  finding_key     TEXT NOT NULL,
  severity        TEXT NOT NULL,        -- blocker|convention|risk|nit|question
  status          TEXT NOT NULL,        -- open|fixed|declined
  head_sha        TEXT,
  path            TEXT,
  title           TEXT NOT NULL,
  body            TEXT NOT NULL,
  event_id        INTEGER NOT NULL,
  updated_at      TEXT NOT NULL,
  UNIQUE (review_event_id, finding_key)
);

CREATE TABLE messages (
  id           INTEGER PRIMARY KEY,
  event_id     INTEGER NOT NULL UNIQUE,
  spec_id      INTEGER NOT NULL,
  from_lane_id INTEGER NOT NULL,
  to_lane_id   INTEGER,                 -- null: every lane on the spec
  type         TEXT NOT NULL,
  body_json    TEXT NOT NULL,
  at           TEXT NOT NULL
);
CREATE INDEX messages_to ON messages (to_lane_id, id);
```

Event kinds that must be enough to rebuild the tables: `spec.created`, `spec.revised`, `spec.approved`, `lane.created`, `lane.claimed`, `lane.taken_over`, `lane.released`, `lane.status_changed`, `gate.recorded`, `finding.opened`, `finding.updated`, `message.sent`.

Recording a gate when the same `(lane, gate, sha)` already has the same status returns the existing `event_id` and does not append. A changed status appends and updates the row. Findings follow the same rule on `(review_event_id, finding_key)`.

Evidence files (screenshots, logs) sit beside the database, named by content hash. The transaction stores the hash in `evidence_ref`. Write and fsync the file first, then commit the event. A crash in between leaves an orphan file, which is harmless. A large blob inside the row would inflate the WAL for every concurrent reader. R10 owns the evidence formats; the rule here is that the log row stays small.

## Concurrency and leases

Writers are agent processes, each with its own connection. SQLite allows one writer. Readers, including daisy, do not block that writer and are not blocked by it, in WAL mode.

Every mutation:

1. `BEGIN IMMEDIATE`, so the write lock is taken up front. A deferred transaction that reads and then writes can fail with `SQLITE_BUSY_SNAPSHOT`, and `busy_timeout` does not absorb that error. The probe reproduced it (code 517, returned immediately). `daisy-core` `write_tx` exists for this reason.
2. Do the conditional update. `changes() = 1` means this caller won. Anything else rolls back and returns a conflict. No event row.
3. Insert the event. Commit.

Claim, copied from daisy's work lease (`work.rs`: the holder is the agent and session pair, because one agent name runs many sessions):

```sql
UPDATE lanes
   SET agent = ?1, session = ?2,
       lease_expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', ?now, '+' || ?secs || ' seconds'),
       status = 'claimed', updated_at = ?now
 WHERE id = ?3
   AND status NOT IN ('done', 'cancelled')
   AND (lease_expires_at IS NULL OR lease_expires_at <= ?now
        OR (agent = ?1 AND session = ?2));
```

`changes() = 1` then the event. A live lease held by someone else leaves the row as it was. Renew extends `lease_expires_at` for the same pair and writes no event. Takeover of an expired lease is an event. Default, floor, and ceiling can start at daisy's numbers (900 seconds, 30 seconds, 86400 seconds) until a reason appears to change them.

A status change also checks `updated_at` (or `head_event_id`) against the value the caller read, the same optimistic check daisy uses on issues. The gate tables make the v17 rule structural: a lane cannot be marked ready while `gate_results` lacks `plan_check`, `quality_gate`, and `merge_verdict` passed on the current `head_sha` for this `pipeline_version`. The engine that decides which gates apply is R7. The storage rule is that the check and the status event commit together.

`busy_timeout` 5000, plus a short retry loop around the whole call for the busy errors that are not snapshot conflicts (daisy's `retry_on_busy`: a few attempts, doubling backoff). Callers treat a conflict as logic (someone else holds the lease, or the sha moved), and a busy timeout as "try again".

The transaction may contain the statements above and nothing that waits: no process spawn, no network, no file copy of a screenshot. Those happen before `BEGIN`.

## Crash safety

A committed transaction is one event plus the matching fold, and `synchronous=FULL` puts that commit on disk before the call returns. `SIGKILL` before commit left the uncommitted event invisible and the earlier seed intact. `SIGKILL` during a commit loop left a consistent prefix: integrity ok, no duplicate keys, lane counters matched the log.

What the client must do: generate the idempotency key first, send the call, and on any uncertainty (process died, timeout, "database is locked") send the same key again. `UNIQUE` plus the same-status gate rule makes the retry a no-op when the first attempt committed.

What this does not prove: a power pull in the middle of `fsync`. That is SQLite's promise under WAL and `synchronous=FULL`, not something this probe pulled the plug to show. The in-language test, once R6 picks a language, should repeat the kill test against the real write function. This probe used CPython's `sqlite3`.

## Migrations

Same machinery as daisy-core, on this file only:

- `PRAGMA user_version`, bumped in the same transaction as that migration's SQL, so a failed migration does not leave half-applied DDL.
- A binary whose schema is older refuses to write. A binary whose schema is newer is the only migrator.
- Before an upgrade, `VACUUM INTO` a snapshot (daisy's `backup_before_upgrade`). Agents do not migrate.
- Expand, then backfill, then enforce. New columns nullable first.
- Daisy is never the migrator.

## How daisy reads it

The ledger is not inside `daisy.db`. Lanes stop today when the desktop app is closed because the work ledger lives there (VISION). A separate file means the app being closed is irrelevant to writers.

Daisy, when it wants lanes, gates, or findings:

- Opens the file with read-only flags and `query_only=ON`. It does not set `journal_mode` (a read-only connection cannot, and must not, be the one that creates the file).
- Uses short transactions. A long read does not block writers in WAL mode. It does pin the oldest snapshot, and the WAL file cannot checkpoint past that reader, so the file grows until the read ends. Daisy already keeps a read-only companion connection for this reason (`open_read_only` in `db.rs`). The harness side should set a normal `wal_autocheckpoint` and treat a multi-gigabyte `-wal` as a stuck reader, not as a writer bug.
- Reads views, not base tables. The views are the contract. Adding a column to `lanes` must not break daisy.

```sql
CREATE TABLE reader_contract (
  id      INTEGER PRIMARY KEY CHECK (id = 1),
  version INTEGER NOT NULL
);

CREATE VIEW reader_lanes AS
  SELECT id, spec_id, lane_key, pipeline_version, status, agent, session,
         lease_expires_at, head_sha, branch, head_event_id, updated_at
  FROM lanes;

CREATE VIEW reader_gates AS
  SELECT lane_id, gate, head_sha, pipeline_version, status, evidence_ref, event_id, at
  FROM gate_results;

CREATE VIEW reader_findings AS
  SELECT id, lane_id, review_event_id, finding_key, severity, status,
         head_sha, path, title, body, updated_at
  FROM findings;

CREATE VIEW reader_events AS
  SELECT id, lane_id, spec_id, kind, actor_agent, actor_session, at
  FROM events;
```

`reader_events` omits `payload_json` so a tail of the log is not an accidental dump of review text. Finding bodies are on `reader_findings`, where daisy already knows they are untrusted input (VISION's security bar, and R13).

Daisy checks `reader_contract.version` the way `open_existing` checks `user_version`: a contract it does not know is a clear error, not a guess. It does not take a lock whose meaning is "the app is running". That lock is why the personal app and the work ledger are coupled today.

Until R15 moves writes off `daisy-cli work` and `reviews`, daisy keeps its own tables. This file is the one those writes move to. Daisy does not become a second writer during the overlap.

## Main risk

One connection that holds the write lock stalls every lane. The 50-writer burst succeeded because each transaction was an insert and an update. The same database, with one transaction left open for 6 seconds and `busy_timeout` 5000, failed the next writer at 5.18 seconds. A 1.5 second hold delayed the next commit by 1.6 seconds. The failure mode is an agent, a migration, or a future helper that starts `BEGIN IMMEDIATE` and then does anything that waits.

The check that keeps the fold honest (one write function, rebuild in tests) does not shrink this window. The rule that does is: nothing that waits runs inside the transaction. That rule has to be enforced in review and in a test that fails when a write is held open, because SQLite will not enforce it for us.

A long daisy read is the paired risk. Writers continue, and the WAL grows until that read ends. Short read transactions on the daisy side are part of this decision, not a polish item.

## What this leaves open

- R6: which language embeds SQLite. The hot path to benchmark is the one measured here (immediate append plus fold, a tail of `events.id`, process startup), not a new storage question.
- R7: which gates exist, and the pipeline document. Columns `pipeline_version` and `gate_results.gate` are the hooks.
- R9: message types. The table is a log of typed messages, not the overlap rules.
- R10: evidence formats. Only the hash-and-file rule is fixed here.
- R15: moving daisy's work, spec, and review writes onto this file without stopping a running lane.
