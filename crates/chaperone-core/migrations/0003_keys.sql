-- Ledger v3. Key lookups search an index instead of scanning.
-- Any-case matching stays: these indexes are NOCASE, the collation the
-- queries name. specs_key is not UNIQUE; the spec.created refusal in
-- write.rs stays the guard against a second spec for a parent.
CREATE INDEX specs_key ON specs (external_key COLLATE NOCASE);
CREATE INDEX lanes_source ON lanes (source_id COLLATE NOCASE);

-- The slice keys of each spec's head version, for `spec get --key` with no
-- --spec. Unlike events_job (0002) this needs the fold: keys come out of
-- the Rust spec parser, so the fold writes them and the migration's Rust
-- step fills them for existing specs.
CREATE TABLE spec_keys (
  spec_id INTEGER NOT NULL,
  key     TEXT NOT NULL COLLATE NOCASE,
  PRIMARY KEY (spec_id, key)
) WITHOUT ROWID;
CREATE INDEX spec_keys_key ON spec_keys (key);
