-- Ledger v1. R5 tables and column names. Statuses follow the M1
-- vocabularies (wire statuses, ran|skipped, open|fixed|answered); the check
-- name stays in gate_results.gate because CHECK is an SQL keyword. Work-item
-- columns arrive with the CLI slices.
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
  spec_id          INTEGER,
  lane_key         TEXT NOT NULL,
  pipeline_id      TEXT,                -- pipeline_id, pipeline_hash, route: set at claim
  pipeline_hash    TEXT,
  route            TEXT,
  status           TEXT NOT NULL,       -- queued|claimed|active|blocked|review|done|cancelled
  agent            TEXT,
  session          TEXT,
  lease_expires_at TEXT,
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
  updated_at       TEXT NOT NULL,
  UNIQUE (spec_id, lane_key)
);
CREATE INDEX lanes_status ON lanes (status, updated_at);
CREATE INDEX lanes_holder ON lanes (agent, session);

-- Latest check.finished per (lane, gate, sha). A result on an older sha is a
-- different row, so it cannot satisfy the head.
CREATE TABLE gate_results (
  lane_id      INTEGER NOT NULL,
  gate         TEXT NOT NULL,           -- the check name
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
