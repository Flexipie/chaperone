-- Ledger v4. `dossier get` and `dossier note` find a key's notes through
-- this instead of scanning every event (ADR 0007). Partial, like
-- events_job (0002): dossier.rs NOTES must match its expression and kind
-- term for term. The key is stored lowercased, so a plain index serves a
-- match that ignores case.
CREATE INDEX events_dossier ON events (json_extract(payload_json, '$.key'))
  WHERE kind = 'dossier.noted';
