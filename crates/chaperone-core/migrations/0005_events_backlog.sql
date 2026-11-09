-- Ledger v5. The backlog finds a repo's newest priority order through this
-- instead of scanning every event (#119). Partial, like events_dossier
-- (0004): backlog.rs ORDERS must match its expression and kind term for
-- term. The repo is stored lowercased. A v4 binary refuses the file, so
-- only one that knows `backlog.prioritised` writes beside it.
CREATE INDEX events_backlog ON events (json_extract(payload_json, '$.repo'))
  WHERE kind = 'backlog.prioritised';
