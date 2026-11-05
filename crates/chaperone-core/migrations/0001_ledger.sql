-- Ledger v1. R5 tables and column names. Statuses follow the M1
-- vocabularies (wire statuses, ran|skipped, open|fixed|answered); the check
-- name stays in gate_results.gate because CHECK is an SQL keyword. The lanes
-- columns are daisy's work item fields plus Chaperone's kind, pr_flow, phase
-- and pipeline stamp (events.md "Folded rows").
-- Editable until the first demo, then frozen: later changes are new files.

-- Append-only log. No AUTOINCREMENT: a rolled-back insert must not burn an
-- id. No foreign keys, so history survives a projection being rebuilt.
CREATE TABLE events (
  id              INTEGER PRIMARY KEY,
  lane_id         INTEGER,
  spec_id         INTEGER,
  kind            TEXT NOT NULL,
  actor_agent     TEXT,
  actor_session   TEXT,
  idempotency_key TEXT UNIQUE,          -- optional; same key replays the row
  payload_json    TEXT NOT NULL,
  at              TEXT NOT NULL
);
CREATE INDEX events_lane ON events (lane_id, id);

CREATE TABLE specs (
  id            INTEGER PRIMARY KEY,
  external_key  TEXT NOT NULL UNIQUE,   -- tracker key, any tracker
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
  title            TEXT NOT NULL,
  source           TEXT NOT NULL,       -- linear|github|note|issue|manual
  source_id        TEXT,                -- stored as given
  repo_slug        TEXT,                -- --repo, stored as given
  kind             TEXT NOT NULL,       -- code|docs|measure|investigation
  pr_flow          TEXT NOT NULL,       -- pr|direct
  phase            TEXT,                -- null until the first phase.entered
  next_action      TEXT,
  blocker          TEXT,
  pr_ref           TEXT,
  base_sha         TEXT,
  spec_id          INTEGER,
  lane_key         TEXT,
  lane_pos         INTEGER,
  pipeline_id      TEXT,                -- pipeline_id, pipeline_hash, route: set at claim
  pipeline_hash    TEXT,
  route            TEXT,
  status           TEXT NOT NULL,       -- queued|claimed|active|blocked|review|done|cancelled
  agent            TEXT,
  session          TEXT,
  claimed_at       TEXT,
  lease_secs       INTEGER,
  lease_expires_at TEXT,             -- lease_expires_at, last_heartbeat_at: outside the fold
  last_heartbeat_at TEXT,
  head_sha         TEXT,
  branch           TEXT,
  worktree_path    TEXT,
  -- Runtime (R8), written by supervise.
  pid              INTEGER,
  exit_code        INTEGER,
  cli_session_id   TEXT,
  port_base        INTEGER,
  port_count       INTEGER,
  viewer_hint      TEXT,
  head_event_id    INTEGER NOT NULL,
  created_at       TEXT NOT NULL,
  updated_at       TEXT NOT NULL
);
-- One item per slot of a lane: a stack's layers differ by lane_pos. A
-- null lane_pos is one slot, not a distinct value per row.
CREATE UNIQUE INDEX lanes_slot ON lanes (spec_id, lane_key, ifnull(lane_pos, ''));
CREATE INDEX lanes_status ON lanes (status, updated_at);
CREATE INDEX lanes_holder ON lanes (agent, session);

-- Latest check.finished per (lane, gate, sha). A result on an older sha is a
-- different row, so it cannot satisfy the head.
CREATE TABLE gate_results (
  lane_id      INTEGER NOT NULL,
  gate         TEXT NOT NULL,
  head_sha     TEXT NOT NULL,
  status       TEXT NOT NULL,           -- ran | skipped
  evidence_ref TEXT,
  event_id     INTEGER NOT NULL,
  at           TEXT NOT NULL,
  PRIMARY KEY (lane_id, gate, head_sha)
);

CREATE TABLE findings (
  id             INTEGER PRIMARY KEY,
  lane_id        INTEGER NOT NULL,
  check_event_id INTEGER NOT NULL,
  finding_key    TEXT NOT NULL,
  severity       TEXT NOT NULL,         -- blocker|convention|risk|nit|question
  status         TEXT NOT NULL,         -- open|fixed|answered
  head_sha       TEXT,
  path           TEXT,
  title          TEXT NOT NULL,
  body           TEXT NOT NULL,
  event_id       INTEGER NOT NULL,
  updated_at     TEXT NOT NULL,
  UNIQUE (check_event_id, finding_key)
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

-- 'CHAP'. Open refuses a file without it, so another SQLite file that
-- happens to be at user_version 1 is never read or written as a ledger.
PRAGMA application_id = 1128808784;
