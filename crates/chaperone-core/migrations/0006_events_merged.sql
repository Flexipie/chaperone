-- Ledger v6. The stats cohort (`--merged-since`), the board's recently
-- merged lanes and the lane page's repo median find merges by `merged_at`
-- through this instead of scanning every event (#127). Partial, like
-- events_backlog (0005): stats.rs MERGED_SINCE and MERGED_LANES and
-- lane_page.rs REPO_MERGED must match its expression and kind term for
-- term. A v5 binary refuses the file.
CREATE INDEX events_merged ON events (json_extract(payload_json, '$.merged_at'))
  WHERE kind = 'pr.merged';
