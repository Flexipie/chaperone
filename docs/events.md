# Event schema

Version **0**, 2026-11-03. The event schema and the integration contract share this
number ([CONTRIBUTING.md](../CONTRIBUTING.md) "Versions"). A removed or renamed kind or
field bumps it. A new nullable field does not. Kind and field names become one-way once
a real ledger exists (after the first demo).

This file is the source the `EventKind` enum is checked against (the drift test, #5).
The CLI that writes these events is [cli-contract.md](cli-contract.md). Evidence:
[R5](research/r5-state-and-storage.md) (log and fold), [R7](research/r7-process-engine.md)
(engine), [R10](research/r10-verification-review-layer.md) (checks).
[ADR 0003](decisions/0003-chaperone-replaces-daisy.md) is why daisy reads none of this.

## Envelope

One row in `events`. The log is the truth. Every other table is a fold of it. The ledger file carries `PRAGMA user_version` (migrations) and a `PRAGMA application_id` set in 0001 (#7). Open checks both and refuses a mismatch ([cli-contract.md](cli-contract.md) exit 3).

| Field | Type | Rule |
| --- | --- | --- |
| `id` | integer | Log order. Assigned by the write, never reused. |
| `kind` | text | One of the M1 kinds below. A reserved kind is refused. |
| `lane_id` | integer, nullable | Null for spec events, `dossier.noted` and `backlog.prioritised`. |
| `spec_id` | integer, nullable | Null for a lane created without `--spec`. |
| `actor` | `{agent, session, tool}` | From `--agent` / `--session` (or `DAISY_AGENT` / `DAISY_SESSION`). `tool` is always `chaperone`. |
| `idempotency_key` | text, nullable, unique | The same key returns the existing row and appends nothing (by key and kind, never the payload). A key already used by another kind is `invalid_input`. |
| `at` | RFC 3339 UTC, ms | Passed in, or for a CLI command read once the write lock is held and moved to 1 ms past the newest event's `at` when the clock is not later (two commits in one millisecond, or a wall clock that stepped back). So clock-timed events are strictly ordered by `at`, each lane's `updated_at` only goes up, and a write between a read and its `--expected-updated-at` always shows. After a wall clock that ran ahead, `at` stays ahead of it until the clock catches up, and a lease's expiry is counted from `at`, so it reads live for that much longer. The `spec` commands read the clock under the lock too. |
| `payload` | JSON object | Typed per kind, below. |

## Write rules

These rules keep every write small, single and fail-closed.

- **One command, one event, one transaction.** Each command appends exactly one event and
  folds it inside one `BEGIN IMMEDIATE` transaction. A crash leaves the event and its
  fold both there or both absent. The exceptions are `usage report`, `check run review`,
  `check run verdict` and `check run plan`.
  `usage report` appends one `usage.reported` per message the lane lacks, each in its own
  transaction, so a crash leaves whole events, and the next report adds the rest. `check run
  review` appends a `usage.reported` per session it started, then one `check.finished`, then
  one `finding.updated` per verifier ruling, each in its own transaction; the next run writes
  rulings a crash left out before anything else (cli-contract.md). `check run verdict` and `check run
  plan` append a `usage.reported` for their session, then one `check.finished`.
- **Refusals.** A validation or CAS refusal writes nothing. A gate denial appends one
  `gate.denied` and nothing else. Its key is
  `deny:<lane_id>:<transition>:<to>:<gate>:<head_sha, or - for none>:<id of the lane's latest event that is not a gate.denied, usage.reported, pr.merged or attention.recorded>`,
  so a retry of the same target and commit with no new facts replays the same denial instead of appending again, and the same command on another commit appends its own. A
  replayed denial still exits 1 with `gate_denied`.
- **Lane commands other than `work ready` bump `updated_at`.** `gate.denied`, `check.*`,
  `finding.updated`, `lane.ready_overridden`, `usage.reported`, `pr.merged` and `attention.recorded` do not touch the lane row's `updated_at`, so a subagent
  recording a check never makes the lane's next `--expected-updated-at` stale.
- **Replays.** A retry with the same key and kind returns the stored event, even if the new
  payload differs. `work create` stores its `--idempotency-key` as `create:<key>` and shows it without the prefix, so a
  caller's key can never take a derived key below and block that write; a create stored under the bare key by an older
  binary still replays under it, and a key that itself starts with `create:` is `invalid_input`. Commands that take no `--idempotency-key` derive one: `check record`
  and `check waive` use `check:` plus the sha256 of the lane id, its canonical payload as the write stores it (redacted) and the id of the lane's newest
  `check.finished` for that check with a different payload (so a retry replays unless another session recorded the same check since, and a
  result that flips back, such as no, yes, no, appends), and `reviews record` uses
  `review:<lane_id>:<job>:requested` or `review:<lane_id>:<job>:finished`. A job already
  recorded on another lane is `invalid_input`. `work merged` uses `merged:<lane_id>:<pr>`
  (the PR lowercased), and a PR already merged on another lane is `invalid_input`.
  `check run` uses `run:<lane_id>:<check>:<head_sha>:<profile>:<attempt>`: `<head_sha>` is the
  measured commit, `<profile>` the sha256 of the profile the check ran by (`-` for `diff`, which
  reads none; the pipeline's hash for `review`; for `smoke` the profile's, as `tests`), and `<attempt>` is the id of the lane's newest `check.finished` for that check
  that is not a pass on this commit and profile (0 for none; a `diff` has no `result`, so it
  counts as a pass; a `review` that is not `complete` counts as no pass). So a run on the same
  commit replays, a `tests` run that failed or a `review` a lens or the verifier missed runs
  again (flaky tests, a fixed machine, an API error), a new commit appends even when its tree is the same (an amend
  that only rewords), and a lane that comes back to an earlier commit after another appends, as
  `check record`'s flip back does. `tests` looks its key up before it runs anything.
  `check run verdict` holds for a tree whatever the commit: it uses
  `run:<lane_id>:merge_verdict:<tree>:<pipeline hash>:<facts_sha256>:<attempt>`, `<attempt>` the
  id of the lane's newest `merge_verdict` (0 for none), and before it runs it looks up the
  lane's newest harness `merge_verdict` that ran on the same tree, pipeline and facts digest.
  On the same commit that one replays, yes or no; on another commit (an amend that kept the
  tree) it is carried: a new event on this commit with the same decision and `carried_from`,
  so `verdict.sha` is the new head, and no session starts. So a verdict is never run again
  until it says yes. On purpose, only a change in the harness's facts lets a new session
  run on the same tree: a different `tests` result, a newest complete `review` with another
  finding count, a change in the lane's blocking findings (a verifier's ruling, a
  dispute, a waive, a new blocking finding), or a new `check run smoke` result. A session's
  smoke claim is shown to the verdict but is not in the digest, so a new claim cannot buy
  a session. A skipped verdict (no answer) is not looked up,
  so the next run starts a session. `check run plan` is keyed the same way, as
  `run:<lane_id>:plan_check:<tree>:<pipeline hash>:<plan_ref>:<attempt>`: the same plan text on
  the same tree and pipeline replays, a changed plan runs.
  `attention record` scopes its `--idempotency-key` to the lane and type (`attention:<lane_id>:<type>:<key>`);
  without one it uses `attention:` plus the
  sha256 of the lane id, its canonical payload and the id of the lane's newest event that is
  not a `gate.denied`, `usage.reported`, `pr.merged` or `attention.recorded` (the anchor the
  denial key uses), so a retry replays and the same record after new lane activity appends.
- **Small payloads.** No secrets (redacted at write, [R13](research/r13-security-and-trust.md)),
  no blobs. Evidence files sit beside the ledger at `evidence/sha256/<first two hex>/<hash>`
  (the directories 0700, each file 0600), and the payload carries only the hash
  (`evidence_ref`). The hash is the sha256 of the bytes stored: a text is redacted, then cut
  to its last 256 KiB, then hashed, so the raw bytes are never stored (a cut cannot leave
  the end of a secret whose start it removed) and a retry names the same file. A file is
  written and synced before the event that names it commits, and a read hashes it again:
  one that no longer matches its name is an error. Spec bodies are capped at 256 KiB, handoff
  summaries at 4000 characters, and finding bodies at daisy's 600 characters. Over a
  cap is `invalid_input`.
- **Redaction.** The write redacts before it appends, so the log never holds a
  pre-redaction copy and a retry replays the redacted bytes. Strings under a prose key
  (`title`, `body`, `summary`, `reason`, `note`, `command`, `what`, `shows`, `showed`,
  `error`, `evidence`, `blocker`, `blockers`, `open_questions`, `next_action`, `goal`,
  `merge_case`, `decisions`, `description`, `check_by_hand`, `why`, `proven`, `not_proven`, `verdict`,
  `shape`, `message`, `question`, `answer`, `where`, at any depth) have R13's patterns
  replaced with `[redacted:<kind>]` (`private_key`, `github_token`, `aws_key`,
  `slack_token`, `gitlab_token`, `bearer`, `api_key`, `url_credentials`,
  `assignment`), and the payload gets `redactions`,
  the count. `changed.<column>.to` counts as `<column>`; `.from`, the value compared with
  the row, is left as sent. A match in any other string is an identifier of the wrong
  type, and a match in an object key is a bad key: both are `invalid_input`, naming the
  key, not the value. `redactions` is the write's to set.
  The actor's `agent` and `session` and the idempotency key are columns, stored as
  they are, so one that matches a pattern is refused the same way (`invalid_input`,
  naming the kind, never the value) before the key is looked up.
- **File mode.** A read-write open makes the ledger and its `-wal` and `-shm` 0600
  (owner only) before it writes, and fails if it cannot; backups are created 0600.
  A read-only open changes no mode.
- **Heartbeat writes no event.** It touches only `last_heartbeat_at` and
  `lease_expires_at` on the lane row. Those two columns are outside the fold:
  `rebuild()` copies their live values from the old row and folds everything else from
  the log, and the rebuild property compares every column except these two.
- **One event folds status.** A gated move folds status through `phase.entered`. An
  ungated move folds it through the command's own event (`lane.updated`, and so on).
  Status is never folded twice by one command.

## M1 kinds

### Spec

| Kind | Command | Payload |
| --- | --- | --- |
| `spec.created` | `spec create` | `parent_key`, `title`, `version` (1), `body` |
| `spec.revised` | `spec update`, `spec decide` | `version`, `body`, `reason`, optional `title` (the body's first `# ` heading; folded into `specs.title` when present), optional `decision {slice, key, question, answer, where, date}` (set by `decide`) |
| `spec.approved` | `spec approve` | `version` |

A spec's `parent_key` is unique ignoring case; a second `spec.created` for it is
`invalid_input` naming the existing spec. The write fills in a missing `version` (the
next one for `spec.revised`, the head for `spec.approved`); a given `version` that is not
that one is `conflict` (exit 4), the `--expected-version` check.

### Lane

A lane is a work item. `work events` shows the daisy action in the second column, so
`lane-stats` reads the ledger unchanged. Every kind that changes a lane column other
than the lease and the stamp carries `changed {field: {from, to}}`, which is how
`rebuild()` reproduces the row. `lane.renewed` carries the lease, `lane.claimed` the
stamp's top-level fields, and `lane.ready_overridden` changes no column.

| Kind | Action | Payload |
| --- | --- | --- |
| `lane.created` | `create` | `title`, `source`, `source_id`, `repo`, `next_action`, `spec_id`, `lane_key`, `lane_pos`, `kind`, `pr_flow` |
| `lane.claimed` | `claim` | `lease_secs`, `lease_expires_at`, `pipeline_id`, `pipeline_hash`, `route`, `changed` (holder, `queued` to `claimed`, and any `--worktree` as `worktree_path`, `--branch`, `--base-sha`) |
| `lane.renewed` | `renew` | `lease_secs`, `lease_expires_at`, optional `changed` (`worktree_path`, `branch`, `base_sha`) |
| `lane.taken_over` | `takeover` | `from {agent, session}`, `reason` (`lease expired` or `forced`), `expired_at` or `live_until`, `lease_secs`, `lease_expires_at`, `changed` |
| `lane.updated` | `update` | `changed`, optional `reason` |
| `lane.reopened` | `reopen` | `changed` (status `done` or `cancelled` to `queued`), `reason` |
| `lane.cancelled` | `cancel` | `changed`, `reason` |
| `lane.released` | `release` | `changed` (holder cleared; status `claimed` to `queued`, any other status kept) |
| `lane.handed_off` | `handoff` | `handoff_id` (the id of its own event, set by the write on any lane event that carries `handoff`), `handoff` (daisy's `HandoffInput`: `summary`, `status`, `changed_files`, `areas` (absent when not declared), `verification`, `smoke`, `blockers`, `open_questions`, `next_action`, `commit`, `pull_request`, `ready`, `brief`), `changed` |
| `lane.ready_overridden` | `ready_override` | `ready` (`true`, `false`, or `null` for `--clear`), `reason` |

The stamp (`pipeline_id`, `pipeline_hash`, `route`) is written once, on the first
`lane.claimed`, by the write itself: the newest pipeline the binary loads and the route
from the lane's kind and flow (lane facts below). Any other event that carries a stamp
field is `invalid_input`. A takeover keeps it. A claim, renewal or takeover on a lane
stamped with a hash this binary cannot load is refused (`pipeline_unknown`) and writes
nothing.

`worktree_path` and `branch` are frozen once the lane has a harness check (`producer.kind`
`harness`): a claim, renewal, takeover or update that changes a set one, or clears it, is
`invalid_input` and writes nothing, so a later run reads the folder and branch the earlier one
did. One not set yet may be set once (a lane checked before it named a branch). One that names
the same value, a trailing slash aside, still writes.

### Engine

| Kind | Written when | Payload |
| --- | --- | --- |
| `phase.entered` | A gated command passes (`work handoff` with `ready: true` or to `done`, `work update --status done`) | `transition`, `from`, `to`, `status`, `head_sha`, plus the command's fields: `changed`, and `handoff_id` and `handoff` for a handoff |
| `gate.denied` | A gated command fails | `transition`, `gate` (the first failing gate id), `fact_paths` (what that gate read), `head_sha` |

`work events` shows `phase.entered` with the command's action (`handoff` or `update`).

### Checks

| Kind | Command | Payload |
| --- | --- | --- |
| `check.requested` | `reviews record`, job not done (`data.done` false) | `check` (`local_review` or `pr_review`), `job`, `head_sha`, `review` |
| `check.finished` | `check record`, `check run`, `check waive`, `reviews record` (job finished) | `check`, `outcome`, `head_sha`, `producer`, the check's fields below, optional `evidence_ref` |
| `finding.updated` | `finding dispute`, `finding waive`; the other moves are core's (`Ledger::finding_update`) for the harness | `check_event_id`, `finding_key`, `status`, `reason` (required to dispute or waive; one non-blank line of at most 500 characters, as `attention record --note`), `head_sha` (required for `fixed`: the commit that fixes it, 7 to 40 lowercase hex digits) |

**Finding statuses.** A finding is folded `open`. Each `finding.updated` makes one of these
moves, checked by the write; any other move, another status, or the same status again
(`finding <event>/<key> is already <status>`) is `invalid_input`, an unknown finding is
`not_found` with reference `<event>/<key>`, and nothing is written. `refuted`, `waived`,
`fixed` and `answered` are final. "Lane" is a session that has held the lane (a
`lane.claimed` or `lane.taken_over`); "independent" is one that never has: the verifier,
or the owner (the rule `work ready` uses). The lane's only move is to dispute, so it never
settles its own finding. A `finding.updated` whose `producer.kind` is `harness` is the
verifier's ruling inside `check run review`: the binary ran the verifier, so it may make the
`verified` and `refuted` moves whatever session started the run (usually the lane's own, in
the background), and only on a finding whose `check.finished` is itself a harness `review`.
It makes no other move from a session that has held the lane, and none on another check's
finding. Only that code path sets the producer; no command takes it as input.

| From | To | Who |
| --- | --- | --- |
| `open` | `answered` | anyone, and only when the severity is `question` |
| `open`, `verified` | `fixed` | independent (the verifier confirms the fix on `head_sha`) |
| `open`, `disputed` | `verified`, `refuted` | independent |
| `open`, `verified` | `disputed` | lane |
| `open`, `verified`, `disputed` | `waived` | independent (the owner) |

There is no `disputed` to `fixed`: a lane that concedes a disputed finding fixes it and
waits for the verifier to rule. `open`, `verified` and `disputed` are unsettled.

A requested review with no finish projects the fact `daisy_review.state` as `running`, never `absent`, and `reviews get` reports it running.

### PR

| Kind | Command | Payload |
| --- | --- | --- |
| `pr.opened` | `work update --pr` when the PR goes from null to set | `pr`, `head_sha` (the latest handoff's `commit`, null if none), `changed` |
| `pr.merged` | `work merged` | `pr`, `merge_sha`, `head_sha` (the PR head that merged), `merged_at` (the forge's merge time; R11's `at`, kept apart from the envelope `at`, which is when it was recorded), `patch_id` (nullable) |

It is that update's one event, in place of `lane.updated`, and `work events` shows it as `update`. When the PR first arrives in
a handoff, the handoff's event carries `changed.pr_ref` from null, and the fact
`pr.opened_at` is the `at` of whichever event first set the PR.

`pr.merged` names one PR the lane has held (a `pr.opened` or any `changed.pr_ref.to` on its
events), spelled as the lane held it, so a lane with several PRs records each merge. It changes no column but
`head_event_id`, is not a lane action in `work events`, and no gate reads it. The ready and
handoff numbers are read from the log, never stored on it. Commits after the handoff are
`git rev-list --count <handoff commit>..<head_sha>`, and ready to merge is `merged_at` minus
the handoff's `at`. The handoff is R11's first real one (its `next_action` does not start
with `reviewing`; if every one is a progress note, the earliest stands in) among the lane's
handoffs while it held this PR: from the event that set `pr_ref` to it (`pr.opened` or a
`changed.pr_ref.to`) until the next `pr_ref` change or this `pr.merged`, whichever is first.
So each PR of a lane is measured from its own handoffs, even when a later PR was opened
before an earlier one merged. When the owner made it ready, it is the newest
`lane.ready_overridden` with `ready: true` in that window.

The `--merged-since` cohort, the board's recently merged lanes and the lane page's repo median
find merges on the partial index `events_merged` (migration 0006, ledger v6),
`json_extract(payload_json, '$.merged_at') WHERE kind = 'pr.merged'`.


### Attention

| Kind | Command | Payload |
| --- | --- | --- |
| `attention.recorded` | `attention record` | `type` (`asked`, `answered`, `nudged`, `sent_back`, `revived`), `by` (`orchestrator` or `owner`: who handled it; for `asked`, who the question went to), `reason` (`asked` only: one of R12's question ids, below), `round` (`sent_back` only, 1 or more), `answers` (`answered` only: the event id of the `asked` it answers, on the same lane), `note` (one non-blank line of at most 500 characters as sent, redacted at write; the answer itself for `answered`) |

One intervention on a lane by the orchestrator or the owner. It changes no column but
`head_event_id`, is not a lane action in `work events`, and no gate reads it. Time to an
answer is the `answered` event's `at` minus its question's. The owner's attention is the
events with `by: owner`. R12's planned `chaperone attention answer` writes this same kind
with `type: answered`. `question.opened` stays reserved for R12's question lifecycle; an
`asked` carries the question's reason id.

### Dossier

| Kind | Command | Payload |
| --- | --- | --- |
| `dossier.noted` | `dossier note` | `key` (`normalize_key`'s form, lowercased), `note` (at most 600 characters as sent, redacted at write), `links` (at most 5 `{kind, ref}`: `issue`, `pr`, `adr`, `doc` or `commit` with an identifier of that kind, at most 300 characters, never redacted; left out when none), `supersedes` (the event id of an earlier note on the same key; left out when none) |

An agent-only note on an issue key ([ADR 0007](decisions/0007-issue-dossier.md)). `lane_id` and
`spec_id` are null, so it is no lane's event: it folds nothing, never changes a lane's
`updated_at` and is never a gate anchor. The writer is the actor. The idempotency key is
`dossier:` plus the sha256 of the key, the actor's agent and session, the payload as stored
(redacted, with `redactions`, canonical) and the id of the key's newest note whose stored payload
differs, so a retry replays, a restored note appends and another session's identical note
appends. Notes are never edited: a correction supersedes. No fact reads `dossier.*`.
`dossier get` finds a key's notes on the partial index `events_dossier` (migration 0004),
`json_extract(payload_json, '$.key') WHERE kind = 'dossier.noted'`.

### Backlog

| Kind | Command | Payload |
| --- | --- | --- |
| `backlog.prioritised` | chaperone-ui's Backlog tab (`Ledger::backlog_prioritise`, no CLI command) | `repo` (`owner/repo`, lowercased), `order` (at most 500 distinct issue keys of that repo, `owner/repo#N` lowercased, first first), `after` (the event id of the repo's newest `backlog.prioritised` the order was built from, or null when it had none) |

The owner's order of a repo's open issues (#119). `lane_id` and `spec_id` are null: it folds
nothing, never changes a lane's `updated_at` and is never a gate anchor. The newest one on a
repo stands; the Backlog tab lists its keys first, in its order, then the rest by issue number.
The write refuses it (`conflict`, nothing written) when `after` is not the repo's newest order,
so an order another window wrote since is never lost. It takes no idempotency key. The newest
order is read on the partial index `events_backlog` (migration 0005, ledger v5),
`json_extract(payload_json, '$.repo') WHERE kind = 'backlog.prioritised'`.

### Usage

| Kind | Command | Payload |
| --- | --- | --- |
| `usage.reported` | `usage report` | One provider message, in [R11](research/r11-observability-cost-metrics.md)'s fields: `message_id`, `source` (`parent` or `subagent`), `model`, `uncached_input`, `cache_read`, `cache_write`, `output`, `reasoning`, `input_includes_cache`, `output_includes_reasoning`, `input_total`, `output_total`, `cost_usd`, `raw` (the CLI's usage object, counts only: R13's allowlist, strings dropped). Plus `origin` (`transcript` in M1; the R4 adapters write the same kind later), `cli` (`claude` or `codex`), `session_id`, and for a subagent `agent_id`, `agent_type` and `description` |

The key is `usage:<lane_id>:<message_id>`, so a message is recorded once per lane, and a report
that finds nothing new writes nothing. The write refuses a `usage.reported` without a
`message_id`. A missing count is null, never 0. It folds nothing but `head_event_id`, it is
not a lane action in `work events`, and no gate reads it. Totals are sums over these
events (`usage get`), never a stored counter. Message text never enters the payload.

- **Claude Code.** The transcripts whose first `cwd` is the lane's `worktree_path` or a folder
  under it, under `<claude dir>/projects/*/<session>.jsonl`, and their subagents under
  `<session>/subagents/agent-<id>.jsonl` with `agent_type` and `description` from the sibling
  `.meta.json`. A response is one line per content block, and earlier lines can carry partial
  usage, so the last line of a message id counts. A message is recorded once a line of it
  has a `stop_reason`, a later message has begun, or the transcript has not been written for
  10 minutes (an interrupt or a killed subagent never writes a stop). Until then it is
  pending, and the next report records it. `input_includes_cache` is false and `input_total` is
  `uncached_input + cache_read + cache_write` (null if any is null). `output_includes_reasoning`
  is true, because Anthropic counts thinking inside `output_tokens`.
- **Codex.** The rollouts under `<codex home>/sessions/**/rollout-*.jsonl` whose `session_meta`
  `cwd` matches. Each `token_usage_record` is one response (`message_id` is its
  `response_id`), with the model from the latest `turn_context`. `input_tokens` already holds
  the cached count. In local records `total_tokens` is input plus output, and input is never
  below cached. So `input_includes_cache` is true, `uncached_input` and `input_total` are
  `input_tokens`, and `cache_read` is `cached_input_tokens`. A rollout with no
  `token_usage_record` (older Codex) is reported as `unknown` and writes nothing. A rollout
  whose `session_meta.source` has `subagent` is a subagent: `session_id` is its
  `parent_thread_id`, `agent_id` its own id, `agent_type` the spawn's `agent_role` and
  `description` its `agent_nickname`.
- **Harness sessions.** A headless session the harness starts (`check run review`'s lenses
  and verifier, `check run verdict`'s and `check run plan`'s session) is one `usage.reported` with `source` and `origin` `harness`, `message_id`
  `run:<session_id>` (so the key is `usage:<lane_id>:run:<session_id>`), the session's
  `session_id` and `cost_usd` (the CLI's own figure, null for Codex, which reports none), `raw`
  (the CLI's usage object, counts only), and labels saying what ran it: `phase` (`review`, `verdict` or `plan`),
  `lens`, `cli` and `model` (the runner's). The token fields are null: unknown, never 0. A
  session that left no id (it never started, or Codex printed no `thread.started`) writes none.
- **Cursor** and other CLIs are not read. Their lanes have no usage events, so `usage get`
  says `known: false`, never a guessed zero.

Usage is matched by worktree path, so a redo lane that reuses a path, or a worktree nested
under another lane's, also counts those sessions. That is lane-stats' rule, kept on purpose.

## `check.finished` payloads

`producer` says who ran the check:

- `{kind: "session", agent, session}`: a fresh subagent, through `check record`. The
  write is refused (`invalid_input`, nothing written) when `producer.session` equals the
  session of anyone who has held the lane (every `lane.claimed` and `lane.taken_over`). A subagent that
  inherits `DAISY_SESSION` is refused, so it fails closed until it passes its own
  `--session`. This is an honor system until adapters own sessions
  ([R4](research/r4-agent-cli-integration.md)).
- `{kind: "daisy_worker", job}`: daisy's review worker, through `reviews record`. The
  session rule does not apply, whoever runs the command. The job is checkable with
  `daisy-cli reviews get <job>`.
- `{kind: "harness", bin, version}`: this binary, through `check run`, which measured
  the result itself: `bin` is the executable's path and `version` the package version, plus
  `+<commit>` when it was built in a git checkout. The session rule does not apply. Only
  `check run` writes this producer; no write refuses another producer's `diff` yet. A harness
  check named like a session check (`merge_verdict`, `plan_check`, `smoke`) is the binary's too, so the session rule
  does not apply to it either.

| Check | Producer | Fields (after `check`, `outcome`, `head_sha`, `producer`) |
| --- | --- | --- |
| `plan_check` | session | `ran`: `findings` (count). `skipped`: `reason` `measure`, `investigation`, `docs` or `tests`; or, through `check waive plan` (the owner's, refused from any session that has held the lane), `reason` `waived` with `note`, the owner's one line, redacted. |
| `plan_check` | harness | `check run plan`, before any code. `ran` with `findings[]` in the harness `review`'s shape (keys `plan-<first 12 hex of sha256(path, NUL, title)>`, title and body cut the same way), folded into `findings` like a review's, or `skipped` with `reason` `no_answer` and no findings (never "no findings"). Both carry `tree`, `profile` (the lane's pipeline hash), `base` (the merge base), `plan_ref` (the evidence hash of the plan text as stored, redacted) and `session` (the lens row for lens `plan`, as the verdict's). |
| `quality_gate` | session | `ran`: `findings` (count). `skipped`: `reason` `no_code` or `tests`. |
| `merge_verdict` | session | `ran` only. `decision` `yes` or `no`. The verdict sha is `head_sha`, from `--head`, never the model's prose. |
| `merge_verdict` | harness | `check run verdict`. `ran` with `decision` `yes` or `no`, `why` (cut to 600 characters), `proven` and `not_proven` (at most 20 items, each cut to 500), or `skipped` with `reason` `no_answer` (the session errored, stopped on its budget or timeout, or answered outside the schema) and no decision, which no fact reads. Both carry `tree`, `profile` (the lane's pipeline hash), `base` (the merge base shown), `diff_cut`, `facts` (the ids of the harness `diff`, `tests` and complete `review` events read, all on `head_sha`), `facts_sha256` (the sha256 of the canonical JSON of the results the session was shown: those events' key fields, the lane's blocking findings once each, and the newest harness `smoke` on the commit, nothing that names the commit and not the session's smoke claim it was also shown, so a later gate can require a verdict on the current facts) and `session` (the review's lens row for lens `verdict`, without `findings`: `ran`, `error`, `bin`, `version`, `runner`, `model`, `prompt_sha256`, `prompt_ref`, `answer_ref`, `evidence_truncated`, `cost_usd`). A carried verdict copies them and adds `carried_from`, the id of the event whose session decided. |
| `smoke` | session | `ran`, or `skipped` with `reason` `nothing_runs` or `attempt_failed`. Optional `evidence_files` (`check record --evidence`, at most 12): `path` (inside the lane's worktree), `sha256` (the evidence file as stored), `bytes` (as read), `truncated`, `redactions`. The recording session's claim, not the harness's. |
| `kept_tests` | session | `ran`, or `skipped` with `reason` `none` (kept tests: none). |
| `review_round` | session | `ran` only. One review round daisy did not run (a review agent's, `ship-wait round`). It adds a round and nothing else: no `review.local_head`, no findings, no `daisy_review.state`. |
| `local_review` | daisy_worker | `job`, `round`, `score`, `verdict`, `findings[]` of `finding_key`, `severity` (`blocker`, `convention`, `risk`, `nit`, `question`), `path`, `line`, `title`, `body`, `verified`, `lenses`, `body_truncated`; `review` |
| `diff` | harness | `ran` only. `tree` (the commit's tree), `base` (the first of `origin/HEAD`, `origin/main`, `origin/master` that this clone has as a remote-tracking ref, never a local branch of that name, else the claimed `base_sha`), `files`, `added`, `removed`, `code_lines` (added plus removed over code files), `changes_code` (some changed file is code), `uncounted_files` (changed files git gave no line counts for: binary, or text an attributes rule such as `-diff` hides; they add nothing to the line totals, so a size gate should not trust them while this is above 0), `files_truncated` (the evidence kept only the end of the list). A file is code unless it is under `docs/`, ends in `.md` or ends in `.lock`. `evidence_ref`: the per-file list, one JSON object `{path, added, removed}` per line, so a list cut to its end still reads line by line once a partial first line is skipped. |
| `tests` | harness | `ran` only, pass or fail. `tree`, `profile` (sha256 of the base branch's profile text), `profile_base` (`ref`, the base it was read from such as `origin/main`, and `commit`, where that ref pointed in the lane's clone: compare it with the real remote, as the lane can move it), `result` (`pass` or `fail`), `commands` (those that ran, in order: `argv`, `exit` (null when killed, by a signal or the deadline), `ms`; a failure stops the rest), `timed_out` (the run passed `timeout_secs`: a fail, as the tests did not pass on that tree). After commands that all passed (each null when not checked): `head_moved` (the checkout's `HEAD` is no longer the commit under test, as when a test committed: a fail), `dirty` (`git status --porcelain --untracked-files=normal --ignored` showed a tracked change or a new untracked file or directory: a fail), `ignored_files` (new entries the commit's ignore rules match, such as build output inside the checkout, a whole ignored directory counting once: counted and listed in the evidence, not a failure), `status_cut` (the listing passed 1 MiB, so it was not read: a fail). These target accidental writes; an index flag such as `assume-unchanged` hides a change from them. `output_truncated`: some command's stdout or stderr kept only its last 16 KiB, or the evidence kept only its last 256 KiB. `evidence_ref`: the status listing, then each command's argv and the end of its stderr and stdout. |
| `smoke` | harness | `check run smoke`: the base profile's `[smoke]` commands, with the fields of `tests` above (`result` pass or fail, `commands`, `timed_out`, the checks after, `profile`, `profile_base`). It sets `smoke.recorded` as any smoke does, pass or fail (a skipped one already did). |
| `review` | harness | `ran` only. `tree`, `profile` (the hash of the lane's pipeline, whose `review_*` checks name the prompts, models and budgets), `base` (the merge base the lenses were shown), `diff_bytes`, `diff_cut` (the diff passed 128 KiB, so each prompt carried its first whole lines and said where the rest is), `uncounted_files` (changed files git gave no line counts for, as `diff` counts them: binary, or hidden by the commit's own attributes rule such as `-diff`, so no lens saw their text), `lenses` (one row per lens, `correctness`, `design_tests`, `intent`, `codex`: `lens`, `ran`, `error` when it did not run (cut to 500 characters), `findings` (count), `cost_usd` when the CLI reported one, `bin` (the agent's path, found on `PATH`) and `version` (its `--version` line), `runner`, `model`, `prompt_sha256` (the pipeline's prompt file), `prompt_ref` (the prompt the session was given) and `answer_ref` (the CLI's whole answer), each its own evidence file, and `evidence_truncated` (the store kept only the end of one of them)), `verifier` (the same row for the verifier, `lens` `verifier` and `findings` its count of rulings, plus `rulings` when it ran: `key`, `verdict` `verified` or `refuted`, `why` (never blank; cut to 600 characters); null when no lens found anything, so no session was started), `complete` (every lens ran, and the verifier ran or had nothing to rule on), `findings[]` (every finding of the lenses that ran: `finding_key` `<lens>-<first 12 hex of sha256(path, NUL, title)>`, then `-2`, `-3` for a repeat in one answer by its place, so a finding the lens words with the same path and title on another run keeps its key, and one it rewords gets a new one; `severity` one of `blocker`, `risk`, `convention`, `question`, `nit`, `improvement`; `title` (cut to 500 characters), `body` (cut to 600; the evidence has both whole), `path`, `line`, `lens`). A lens that errors, hits its budget, times out or answers outside the schema (prose, a field or severity it does not know) did not run: it never reads as no findings. The verifier's rulings follow as `finding.updated` (finding statuses, above). |
| `pr_review` | daisy_worker | Same as `local_review`, without `round`, plus `pr`. |
| `local_review`, `pr_review` (job `failed` or `cancelled`) | daisy_worker | `skipped`, `reason` `failed` or `cancelled`, `job`. `review`. No score or findings. It sets no `review.local_head`, adds no round, and projects `daisy_review.state` as `failed`, which no gate accepts. |

`job` is daisy's job id as a JSON integer. `round` is the lane's review rounds including this one
(`review.local_rounds`: `ran` local reviews and `review_round` checks), not daisy's round, so `jobs get` reports `round` null. `review` holds daisy's scalars only (`status`, `phase`,
`source`, `pr_ref`, `branch`, `repo`), for `reviews get` and `jobs get`; daisy's free text
is not stored.

Finding bodies are untrusted text. They are stored, and they never satisfy a gate. R10's
richer fields (smoke cases with evidence hashes, kept-test items with bars, verdict
reasons) are reserved for a `--file` input after M1.

## Folded rows

| Table | M1 rule |
| --- | --- |
| `lanes` | `status` is a wire status only (`queued`, `claimed`, `active`, `blocked`, `review`, `done`, `cancelled`). There is no `ready` status. Ready is the `work ready` override, which wins over every handoff until `--clear`; else a lane not in `review` is not ready; else the latest handoff's `ready` field, with no `next_action` fallback (daisy's `lane_says_ready` falls back; M1 does not). `work board` applies it (cli-contract.md). The columns are daisy's `work get` fields (`title`, `source`, `source_id`, `source_key`, `repo_slug`, `next_action`, `blocker`, `pr_ref`, `branch`, `base_sha`, `worktree_path`, `agent`, `session`, `claimed_at`, `lease_secs`, `lease_expires_at`, `last_heartbeat_at`, `spec_id`, `lane_key`, `lane_pos`, `status`, `created_at`, `updated_at`) plus `kind`, `pr_flow`, `phase`, `pipeline_id`, `pipeline_hash`, `route` and `head_event_id`. `id` is the lane id, `idempotency_key` comes from `lane.created` (less the `create:` it is stored under), `lease_state` is computed at read time, and `linear_outbox` and `repo_id` are always null. `phase` is null until the first `phase.entered`, then its `to`; M1's `handoff` has no `from` constraint. `head_event_id` is the id of the lane's latest event of any kind. `spec_id`, `lane_key` and the pipeline columns are nullable until set. One item per `(spec_id, lane_key, lane_pos)` slot (a null `lane_pos` is one slot), so a stack's layers share a lane. `events.idempotency_key` is nullable. |
| `gate_results` | One row per `(lane_id, gate, head_sha)`, where column `gate` holds the `check.finished` payload's `check` (named `gate` because `CHECK` is an SQL keyword). `status` is `ran` or `skipped`. The latest `check.finished` on that key wins. |
| `findings` | One row per `(check_event_id, finding_key)`. `status` is one of the finding statuses (checks, above). `Ledger::blocking_findings` reads a lane's unsettled findings from every round whose severity is not `nit`, `improvement`, `question` or `convention` (compared exactly, so an unknown or misspelt severity blocks; `convention` may block under a stricter profile later). This is not `review.previous_round_closed`, which reads every severity but only the newest round. Which of the two a v20 gate reads, and whether a superseded round's unsettled blocker still blocks, is decided with pipeline v20. |
| `spec_keys` | Since ledger v3. One row per `(spec_id, key)`: the slice keys of the spec's head version, as the spec parser reads them, replaced on every `spec.created` and `spec.revised`. Keys match in any ASCII case (`COLLATE NOCASE`), so two sections naming one key in different cases are one row. `spec get --key` without `--spec` reads it. |
| handoffs | A handoff is a `lane.handed_off`, or a `phase.entered` that carries `handoff`. `work handoffs`, "the latest handoff" (ready, `pr.opened.head_sha`, the `update --status done` commit) and lane-stats read both. |

R5's `passed | failed`, `declined` and `ready` values, its NOT NULL `spec_id` and `pipeline_version`, and its `findings.review_event_id` name are replaced by the ones above.

**Lane facts in M1.** `lane.kind` and `pr.flow` come from `work create --kind` (`code`, `docs`, `measure` or `investigation`; default `code`, so a lane that says nothing fails closed) and `--pr-flow` (`pr`, `direct` or `none`; default `pr`), stored on `lane.created`. The route is chosen once, at the first claim, by R7's rule: `investigation` if the kind is `investigation`, else `no_pr` if the flow is `none`, else `direct` if the flow is `direct`, else `code_pr`. The first claim also stamps the newest pipeline (`v19`); a lane keeps the stamp it has, so a lane stamped `v17` or `v17.1` stays on it. Until a pipeline reads the `diff` check, `lane.changes_code` is true exactly when the kind is `code`. Skip reasons are checked by list membership: when `changes_code` is false, any reason the check lists is accepted, and when it is true, `skipped` fails the gate. So a `docs`, `measure` or `investigation` lane may skip its plan check and quality gate with an allowed reason, and a `code` lane may not. No M1 input marks a lane test-only, so a test-only lane is `code` and runs both checks; the `tests` reason waits for that input.

**Facts in M1.** The projection reads the lane row's stamp and kind and the lane's
events, never prose. `commit.sha` is the head the gated command names (the handoff's
`commit`, or for `update --status done` the latest handoff's), never a stored one.
`plan_check.*` and `quality_gate.outcome`/`reason` come from the latest `check.finished`
of that check, and `verdict.*` from the latest `merge_verdict` that ran, a session's or the
harness's (a skipped harness verdict is none).
`quality_gate.at` is the `at` of the first quality gate with the latest one's outcome
and reason, so recording the same result again after the PR does not fail
`quality_gate_before_pr`, and a skip before the PR does not time a gate that ran after
it. `smoke.recorded` and `kept_tests.recorded` are true once any such check is recorded.
`review.local_rounds` counts `ran` local reviews and `review_round` checks, so the cap
binds only rounds that were recorded; `review.local_head` is the newest
one's sha, and `review.previous_round_closed` holds when that newest round has no
unsettled finding of any severity (so a dispute never closes it; no transition that runs
today reads it). `daisy_review.state` is `running` while a requested job (or a request with no
job) has no finish, else the latest finished review's (`finished`, or `failed` when
skipped), else `absent`. A finish recorded after a newer job's finish of the same check
closes its request and counts its round, but leaves the state, head and newest round on
the newer job (daisy's job ids only go up). Facts whose producer is reserved stay absent, so a gate that
reads one denies: `kept_tests.bar_ok`, `verification.unexplained_failures`,
`brief.oversize`, `review.code_changed_since`, `diff.code_lines`, `ci.state` (no v19
gate reads it: CI is shown, never a gate),
`threads.human_open`, `pr.is_draft`, `pr.conflicting`, `human.merge_yes`,
`handoff.decisions`.

## Reserved

Named so nothing else takes the name. The write refuses these kinds in M1.

| Kind | From | Why not M1 |
| --- | --- | --- |
| `session.started`, `turn.finished`, `session.exited`, `tool.started`, `tool.finished`, `permission.denied` | [R4](research/r4-agent-cli-integration.md) v0 lifecycle | No adapter in M1. M1's `usage.reported` comes from transcripts. |
| `agent.stalled`, `agent.blocked`, `agent.killed` | [R8](research/r8-process-terminal-runtime.md) | Needs `chaperone supervise`. |
| `reservation.written`, `intent.granted`, `intent.denied`, `path.released`, `seam.published`, `seam.acked`, `seam.rejected`, `message.sent`, `stack.rebased`, `stack.layer_opened` | [R9](research/r9-coordination-between-lanes.md) | Coordination is Phase 3. |
| `evidence.recorded`, `escape.judged`, `price.published`, `pr.commit`, `pr.threads_observed`, `issue.opened`, `guard.blocked` | [R11](research/r11-observability-cost-metrics.md) | Metrics stay in `lane-stats` for M1. |
| `question.opened` | [R12](research/r12-interfaces-and-attention.md) | R12's question lifecycle is later; M1 records a question as `attention.recorded` with `type: asked`. Its reason ids (`question.done_ambiguous`, `question.scope`, `question.one_way`, `question.recorded_decision`, `question.size`, `question.investigation`, `question.verdict_no`, `question.product`, `question.seam_reject`, `question.overlap`) are payload values, not kinds. |
| `human.merge_yes` | [R10](research/r10-verification-review-layer.md) | Needs a user-attributed adapter. M1's done is the `handoff` transition. |

Reserved `check.finished` values. No M1 command produces them, so they are not
accepted yet:

| Check | R10 record |
| --- | --- |
| `verification` | `/ship` step 1 command list |
| `commit_classified` | `commit.classified`, host classifier |
| `ci_snapshot`, `threads_snapshot`, `pr_snapshot` | `ci.snapshot`, `threads.snapshot`, `pr.snapshot` |
| `brief`, `decision_list` | Lane-written brief and investigation list |

Not event kinds, though research spells them with a dot:

- MCP tools: `lane.read`, `lane.message`, `evidence.attach`. R9's agent calls
  `intent.submit`, `seam.publish`, `seam.ack` and `seam.reject` produce the reserved
  `seam.*` and `intent.*` events above.
- R9 inbox message types (carried by `message.sent`): `seam.revised`, `base.moved`,
  `dependency.cleared`, `overlap.needs_human`.
- Fact paths (`plan_check.outcome`, `verdict.sha`, `commit.sha`, `smoke.recorded`, ...)
  and table columns (`gate_results.evidence_ref`). Facts are R7's fact bundle.

## Replaced names

| Old name | Where | Now |
| --- | --- | --- |
| `gate.recorded` | R5, R11, ROADMAP | `check.finished`, folded into `gate_results` |
| `review.finished` | R11 | `check.finished` with `check` `local_review` or `pr_review` |
| `finding.opened`, `finding.recorded` | R5, R11 | `findings[]` on `check.finished`, then `finding.updated` |
| `lane.status_changed` | R5, R11 | `lane.updated` (`changed.status`), or `phase.entered` on a gated move |
| `diff_measured` | R10 | `check.finished` with `check` `diff` (`check run diff`) |
| `handoff.recorded` | R11 | `lane.handed_off`, or `phase.entered` on a gated handoff |
