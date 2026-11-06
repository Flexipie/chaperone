# CLI contract

2026-11-03. Flags follow the binary `0.y.z` axis and the payloads follow the shared
contract number ([CONTRIBUTING.md](../CONTRIBUTING.md) "Versions"). The `chaperone` binary keeps the argv, the `{ok, data, meta}`
envelope and the exit codes of `daisy-cli` 0.2.0 (contract 1), so a skill moves from
daisy to Chaperone by changing the binary path. That switch happens after the first demo
([ADR 0003](decisions/0003-chaperone-replaces-daisy.md)). The events each command writes
are in [events.md](events.md).

## Global flags and actor

| Flag or env | Rule |
| --- | --- |
| `--db <PATH>` | The ledger file. Without it, the CLI's default path in the user data dir (`dirs`, #7). Core takes a path and reads no environment. |
| `--format json\|jsonl` | `json` (default) is one object. `jsonl` is one item per line, then a trailer. |
| `--agent`, `DAISY_AGENT` | `[a-z0-9._-]{1,40}`. Required by every write. |
| `--session`, `DAISY_SESSION` | `[A-Za-z0-9._:-]{1,120}`. Required by every write. |
| `DAISY_CLI_READONLY=1` | Refuses every command that is not a read, `--dry-run` included (`read_only`). |
| `--dry-run` | Validates and prints what would be written. Writes nothing. |

The env names stay `DAISY_*` because `spawn` and the skills already export them.

## Envelope

- **Success:** one `{ok: true, data, meta}` on stdout.
- **Failure:** one `{ok: false, error: {code, message, retryable, details}, meta}` on stderr.
- **`meta`:** `{cli_version, schema_version, source_freshness}`. `schema_version` is the
  ledger's `PRAGMA user_version`. `source_freshness` is always `null`, because Chaperone
  has no mirrors.

`text` output and `--help` are for people and are not part of the contract. `--help`
output has snapshot tests.

## Exit codes

| Exit | Name | When |
| --- | --- | --- |
| 0 | `ok` | |
| 1 | `error` | Every error code without its own exit. |
| 2 | `usage` | Bad argv. |
| 3 | `schema_mismatch` | The ledger's `user_version` is not this binary's, or its `application_id` is not Chaperone's, or it is not SQLite at all. The file is left untouched. |
| 4 | `conflict` | A CAS miss, a held lease, or a stale `--expected-version`. |
| 5 | `runtime_unavailable` | Only passed through from `daisy-cli reviews get`. |

## Error codes

Daisy's ten codes, plus two from the engine.

| Code | Exit | `details` | Notes |
| --- | --- | --- | --- |
| `not_found` | 1 | `entity`, `id` (or `entity`, `reference`) | |
| `invalid_input` | 1 | | Validation, caps, and the check session rule. Writes nothing. |
| `busy` | 1 | | `retryable: true`. `SQLITE_BUSY` after the 5 s busy timeout. |
| `conflict` | 4 | `entity` (or `by`, `until` for a held lease) | Pass `--takeover` for a lease. |
| `schema_mismatch` | 3 | `found`, `supported`, `hint` | Newer file: refused, upgrade the binary. Older or missing file: `hint` says run `chaperone migrate`. Foreign file (wrong `application_id`): refused. |
| `usage` | 2 | `kind`, `help` | |
| `db` | 1 | | |
| `internal` | 1 | | |
| `runtime_unavailable` | 5 | `job_id`, `poll` | `retryable: true`. |
| `read_only` | 1 | `command`, `mutability` | |
| `gate_denied` | 1 | `transition`, `gate`, `fact_paths` | One `gate.denied` was appended. The failing gate id is also on stderr's message. |
| `pipeline_unknown` | 1 | `pipeline_id`, `pipeline_hash` | Claim or transition on a hash this binary cannot load. Writes nothing. |

## Lists

- **Shape:** `{items, next_cursor, has_more}`.
- **Paging:** `--limit` defaults to 50 and accepts 1 to 500. `--cursor` takes an opaque,
  command-specific value from `next_cursor`.
- **`jsonl`:** one item per line, then one trailer, which is the only line with an `ok`
  key and carries `meta.page`.

## Statuses

Wire statuses: `queued`, `claimed`, `active`, `blocked`, `review`, `done`, `cancelled`.
Ready is not a status (events.md, folded rows). `transition_ok(from, to)` is daisy's:

| From | Allowed to |
| --- | --- |
| `queued` | `blocked`, `done`, `cancelled` |
| `claimed` | `active`, `blocked`, `review`, `done`, `cancelled` |
| `active` | `blocked`, `review`, `done`, `cancelled` |
| `blocked` | `active`, `review`, `done`, `cancelled` |
| `review` | `active`, `done`, `cancelled` |
| `done`, `cancelled` | `queued` (reopen) |

`claim` moves `queued` to `claimed` and `release` moves it back. Moving to `blocked`
needs `--blocker` or `--reason`.

## Commands in M1

Every write except `migrate` also takes `--agent` and `--session`, and every write but `migrate`, `check record`, `reviews record`, `usage report` and `work merged` takes `--dry-run`. `[x]` is optional.

| Command | Argv | Kind |
| --- | --- | --- |
| `work create` | `--title <t> [--source] [--source-id] [--repo] [--next-action] [--idempotency-key] [--spec] [--lane] [--lane-pos] [--kind code\|docs\|measure\|investigation] [--pr-flow pr\|direct\|none]` | write |
| `work get` | `<id>` | read |
| `work list` | `[--status] [--agent] [--mine] [--all] [--session] [--mine-agent] [--limit] [--cursor]` | read, paged |
| `work claim` | `<id> [--worktree] [--branch] [--base-sha] [--lease-secs] [--takeover]` | write |
| `work update` | `<id> --expected-updated-at <t> [--status] [--reason] [--blocker] [--next-action] [--pr] [--branch] [--base-sha] [--spec] [--lane] [--lane-pos]` | write |
| `work heartbeat` | `<id>` | write, no event |
| `work release` | `<id>` | write |
| `work cancel` | `<id> [--reason] [--expected-updated-at]` | destructive |
| `work handoff` | `<id> [<file>] [--stdin]` | write |
| `work handoffs` | `<id>` | read |
| `work events` | `<id>` | read |
| `work ready` | `<id> [--yes] [--no] [--clear] [--reason]` | write |
| `work merged` | `<id> --pr <owner/repo#N> --merge-sha <sha> --head <sha> --merged-at <t> [--patch-id <hex>]` | write |
| `work board` | | read |
| `spec create` | `--parent <key> --file <path\|-> [--title]` | write |
| `spec get` | `[<spec>] [--key <slice key>] [--version <n>]` | read |
| `spec list` | | read |
| `spec update` | `<spec> --file <path\|-> --reason <r> [--expected-version <n>]` | write |
| `spec approve` | `<spec>` | write |
| `spec decide` | `<spec> [--slice <n>] [--key <k>] --question <q> --answer <a> [--where <w>]` | write |
| `check record` | `--work <id> --check plan_check\|quality_gate\|merge_verdict\|smoke\|kept_tests --head <sha> --outcome ran\|skipped [--reason <r>] [--decision yes\|no] [--findings <n>]` | write |
| `reviews record` | `--work <id> --job <daisy job id>` | write |
| `reviews get` | `<job id> [--wait] [--timeout-secs <n>]` (accepted for the path swap; a recorded job answers at once) | read |
| `jobs get` | `<job id>` | read |
| `usage report` | `--work <id> [--claude-dir <dir>] [--codex-dir <dir>]` | write |
| `usage get` | `<id>` | read |
| `migrate` | (uses `--db`; no actor flags) | write, no event |

`work create --kind` and `--pr-flow` are Chaperone's two additions to daisy's argv (#7). Both are optional and stored on `lane.created`. `--kind` defaults to `code` and `--pr-flow` to `pr`, so a skill that passes neither gets the strictest route. #14 reads them at the first claim, when the route is stamped.

`chaperone migrate` is the only command that creates the ledger or migrates it (`Ledger::open_and_migrate`, with a `VACUUM INTO` backup before an upgrade, #4). It creates a missing or empty file. It refuses a non-empty file whose `application_id` is not Chaperone's, a file that is not SQLite, and a version-0 file that already has tables, so it never applies 0001 over someone else's database. Every other command opens with `Ledger::open`, which never creates or migrates and refuses a mismatch with exit 3. Migration 0001 sets `PRAGMA application_id`, and open checks it, so a foreign SQLite file at version 1 is refused (#7).

`work handoff` reads the JSON from `<file>`, or from stdin when the file is omitted or `-`. A handoff with `ready: true` or to `done` runs the `handoff` gate (commands and transitions below). The write itself refuses a `lane.handed_off` that says ready and any event but `phase.entered` that moves a lane to `done`, so no path skips the gate.

`work merged` is new (#59). `/merge` runs it after the merge, from any session (it skips the
holder rule and leaves `updated_at` alone, as `check record` does). It appends one
`pr.merged` (events.md, PR). It refuses (`invalid_input`, nothing written) a `--pr` that is not
`owner/repo#N` or that the item has never held, a PR already merged on another item, a sha that
is not 7 to 40 lowercase hex digits, and a `--merged-at` that is not RFC 3339 UTC with
milliseconds. The same PR again replays. It prints `{event_id, work_id, kind, replayed, at,
payload}`.

`work ready` is the owner's override. It is refused (`invalid_input`) from any session that has held the lane, the same honor-system rule as `check record`. It answers daisy's way: `{id, ready, ready_reason}` from the board, or `{id, ready_override, on_board: false}` for a done or cancelled item.

`work board` is daisy's board shape, `{specs, other}`, one read statement. Every open item (not `done` or `cancelled`) is under `other`, ordered by `lane_key`, `lane_pos`, `id`; `specs` stays empty until items are grouped by spec. Each entry is `{item, handoff, ready_override, ready, ready_reason}`: `item` is `work get`'s, `handoff` the newest `work handoffs` row or null, and `ready_override` the newest `work ready` as `{ready, reason, by, at}`, null when there is none or it was cleared. The ready rule is daisy's first steps: the override wins (`marked ready by <agent>: <reason>`); else an item not in `review` is not ready (`status is <s>, not review`); else it is ready only when its newest handoff says ready (`the lane says ready`, else `the lane has not said ready`). A ready handoff exists only past the `handoff` gate, so a later `needs you` handoff unsays it. Daisy's further steps, a finished review on the PR head and no open thread from a person, need the PR and are the caller's to check; the board does not notice commits pushed after the gated handoff. `lanes` checks threads for every repo but the review on the head only for repos daisy does not review, so before it reads this board for a daisy-reviewed repo it has to check the review on the head there too. Daisy's other board fields (`pr_head`, `pr_state`, `pr_review`, `local_review`, `linear_outbox`, `threads`, `threads_checked`, `needs_you`) are not emitted.

Lease: `--lease-secs` defaults to 900, minimum 30, maximum 86400. Claiming a `done` or
`cancelled` item is `invalid_input`.

`check record` is new (#16, next to `reviews record`). The recorder passes its own
`--session`. The write is refused when that session is any session that has held the
lane (events.md, producer). `--head` is 7 to
40 lowercase hex digits. `--reason` is required with `skipped` and must be one the check allows
(events.md). `--decision` is required for `merge_verdict` and refused for every other
check. `--findings <n>` is required for a `plan_check` or `quality_gate` that ran and
refused elsewhere. It prints `{event_id, work_id, kind, replayed, at, payload}`.

`reviews record` reads `daisy-cli reviews get <job>`. That is Chaperone's only call into
daisy. A job that is not done (`data.done` false, status `queued` or `running`) appends
`check.requested`. A done job with status `reviewed` or `posted` appends `check.finished`
`ran` with `producer {kind: daisy_worker, job}`. A done job with status `failed` or
`cancelled` appends `check.finished` `skipped` with `reason` that status. It projects
`daisy_review.state` as `failed`, which `daisy_review_settled` rejects (a review that
never ran is a blocker, not a skip, R10), and adds no local round. M1's `handoff` does
not read `daisy_review_settled` (only the three stamps below), so this fact matters from
the first pipeline that requires it. `reviews get --wait` does not
block on a job that is only requested, so lanes keep `daisy-cli reviews get --wait` for
waiting. `reviews get` and `jobs get`
answer from what was recorded, in daisy's shape (the job's `check.finished`, else its
`check.requested`). Their keys are daisy's. Daisy's free text (`summary`, `impact`,
`intent_note`, `verdict_reason`, `error`, `report_path`, `worktree_path`) is not stored and
reads as null, and so does what only daisy knows in `jobs get` (cost, tokens, `job_dir`,
`round`). In `jobs get`, `work_id` is the lane the job is recorded on, `started_at` is when
Chaperone first recorded the job, and `finished_at` is when it recorded the finish (null
before), not daisy's times.
Findings come from the stored payload, and each one's `outcome` is its row's `fixed` or
`answered`, else null. `reviews record` prints the `reviews get` answer.

`reviews record` refuses (`invalid_input`, nothing written): a view whose `job_id` is not
`--job`, a view with no `done` or `status`, a done job with a status other than
`reviewed`, `posted`, `failed` or `cancelled`, a job recorded on another lane, a job whose repo, branch or PR is not the lane's (below), a finding with
no id, severity or title, and findings whose ids repeat.
A job's repo, branch or PR is not the lane's when its `repo` is not the lane's `repo_slug`, a local job's
`branch` is not the lane's `branch`, or a PR job's `pr_ref` is not the lane's `pr_ref`. Each is
checked only where the lane has it; `repo` and `pr_ref` ignore case. Daisy
is found at `/Applications/daisy.app/Contents/MacOS/daisy-cli`
([R15](research/r15-migration.md)), or at `CHAPERONE_DAISY_CLI`, and given 30 s. Its exit
5 (`runtime_unavailable`) and its `not_found` pass through. Any other failure, a timeout or output that is not daisy's envelope
is `internal`, with what daisy said in `details`, so daisy's exit codes never take
Chaperone's meanings.
`CHAPERONE_DAISY_CLI` is trusted like `--session` (an honor system until adapters): whoever
runs `reviews record` can point it at any program.

## Commands and transitions

Each command is one transaction that appends one event (events.md, write rules).

| Command | Transition | Event |
| --- | --- | --- |
| `work handoff` with `ready: true`, or with status `done` | `handoff` (gated) | `phase.entered`, or `gate.denied` |
| `work update --status done` | `handoff` (gated) | `phase.entered`, or `gate.denied` |
| `work handoff`, any other (a `needs you` or progress handoff) | ungated | `lane.handed_off` |
| `work update --status queued` from `done` or `cancelled` | ungated | `lane.reopened` |
| `work update --status cancelled`, `work cancel` | ungated | `lane.cancelled` |
| `work update`, any other change | ungated | `lane.updated`, or `pr.opened` when the PR goes from null to set |
| `work claim`, `release`, `ready` | ungated | the matching `lane.*` kind |
| `work heartbeat` | none | none |
| `migrate` | none | none (schema only) |

A gated command on a lane with no stamp (never claimed) is denied with gate `stamp_ok` and fact path `lane.pipeline_hash`. It is never ungated and never falls back to the latest pipeline. A gated command whose route has no matching transition is denied the same way, never passed.

`handoff` is the M1 gated transition. Its requires list is per route in
`pipelines/v17.1.toml` (#34; lanes stamped `v17` keep `pipelines/v17.toml`, #12). A
handoff with `ready: true` on `code_pr` or `direct` needs a `plan_check`, a
`quality_gate`, and a `merge_verdict` with decision `yes` on the handed-off sha
(`handoff.commit`, or for `update --status done` the commit of the lane's latest
handoff). The two shas match when the shorter, at least 7 hex characters, is a prefix of
the longer, whichever side it is on (v17: the verdict's sha must be the prefix). A
handoff to `done` or `update --status done`, on every route, needs the plan check and
quality gate (a lane that changes no code may skip them with an allowed reason), and on
a `code` lane the same YES verdict on the handed-off sha; a `docs`, `measure` or
`investigation` lane needs no verdict and no `commit`. A `code` lane without that verdict
is denied as `done_verdict`. A handoff with `ready: true` on `investigation` or `no_pr` is always
denied: an investigation hands off `needs you`, never ready (R7). `phase.entered.to` is `done` for a handoff to `done`
or `update --status done`, else `ready`, and `phase.entered.status` is the status the
command asked for, not R7's mapping from `to`. Ready comes from a
handoff's `ready` (or the owner's `work ready` override), so a handoff cannot mark a lane
ready without passing this gate. There is no `next_action` fallback for ready (events.md,
folded rows). A `needs you` handoff is not gated, so it always reaches the owner. A
missing fact fails closed. The route comes from `work create`'s `--kind` and `--pr-flow`
(events.md, lane facts). The
denial exits 1 with `gate_denied`, names the failing gate id on stderr, and leaves the
lane where it was.

`usage report` is new (#59). It reads the transcripts that started in the lane's
`worktree_path` and appends one `usage.reported` per provider message the lane does not
have (events.md, Usage). `/ship` is to run it after each handoff once lanes run on Chaperone, and anyone may run it on demand.
It skips the holder rule, as `check record` does. `--claude-dir` defaults to
`$CLAUDE_CONFIG_DIR`, else `~/.claude` when it exists. `--codex-dir` defaults to `$CODEX_HOME`,
else `~/.codex` when it exists. A folder from a flag or a variable is used as given. It
refuses (`invalid_input`, nothing written) a lane with no `worktree_path` or one that is not an
absolute folder below `/` (which would match every transcript or none), a folder that does
not exist, and a run with no folder at all. Entries in `projects` that are not folders are
skipped. A file or folder it cannot read is `internal`, naming the path, and the report
writes nothing. It prints `{work_id, written, existing, pending, sessions}`.
`sessions` lists each matched transcript as `{cli, session_id, agent_id, messages, pending,
unknown}`, with ids and counts only.

`usage get` prints `{work_id, known, totals, sources}`. `totals` is keyed by `source`
(`parent`, `subagent`), and `sources` has one row per transcript (`source`, `cli`, `session_id`,
`agent_id`, `agent_type`, `description`, `models`). Both carry `messages`, `peak_input_total`
and the sums of `cache_read`, `cache_write`, `output`, `reasoning`, `input_total` and
`output_total`. Only the transcript rows carry `uncached_input`, since its meaning differs
by CLI (Codex's includes the cache). A sum skips the messages where that field is null, and is
null when every message has it null. `known` is false when the lane has no usage event.

Where these numbers differ from `lane-stats`:

- `lane-stats`' `context_tokens` is the sum of `input_total`, but it counts a missing key as 0.
  Here a message with a missing key has a null `input_total`, which the sum skips.
- A message still streaming at the end of a transcript is counted by `lane-stats` and is
  pending here until it stops or the transcript has been idle for 10 minutes.
- `lane-stats` counts a message id once per transcript. Here it counts once per lane, so a
  resumed or forked session that copies earlier messages adds them once, and a lane total
  can be lower than `lane-stats`. Which transcript row a copied message lands on follows
  the scan order.
- `lane-stats` splits a session evenly between stack lanes that share a worktree. Here each
  lane that matches a session records its messages.
- `lane-stats` also matches old daisy items by branch name when they have no worktree path.
  Here a lane without `worktree_path` is refused.
- `lane-stats` reads Claude only. Codex rollouts are counted here, with Codex subagents
  under `subagent`.

## What lane-stats reads

These fields keep their daisy names and meaning:

- `work events`: `action` and `at` per item. `action` is daisy's word (`create`, `claim`,
  `renew`, `takeover`, `update`, `reopen`, `cancel`, `release`, `handoff`,
  `ready_override`), mapped from the event kind (`pr.opened` is `update`, `phase.entered`
  is the command's). `gate.denied`, `check.*`, `finding.updated` and `usage.reported` are not lane actions
  and do not appear in `work events`.
- `work handoffs`: `created_at` and `handoff.next_action`.

## Not in M1

- `reviews stats`: deferred to a follow-up before the skill switch.
- `reviews local`, `reviews pr`, `reviews run`, `reviews wait`: lanes keep calling
  `daisy-cli` for these. The worker stays in daisy.
- `jobs list`, `jobs events`, `capabilities`, `doctor`, `schema`.
- Daisy's personal hub (`issues`, `notes`, `inbox`, `settings`, `chat`, the mirrors,
  `memory`, `context`, `search`): never.
- No id floor, no router, no `linear_outbox`. Ids are plain integers. `source` and
  `--repo` are stored as given, and `linear_state` is always `null`.
