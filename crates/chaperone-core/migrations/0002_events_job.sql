-- Ledger v2. Job lookups (`reviews get`, `jobs get`, `reviews record`)
-- search this instead of scanning every event.
-- Partial, so only check events pay for it. The planner uses it only when
-- the query's filter is this expression and this IN list, term for term:
-- checks.rs JOB_EVENTS must match it exactly.
CREATE INDEX events_job ON events (json_extract(payload_json, '$.job'))
  WHERE kind IN ('check.requested', 'check.finished');
