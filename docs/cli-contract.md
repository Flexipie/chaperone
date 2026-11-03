# CLI contract

2026-11-03. Flags follow the binary `0.y.z` axis and the payloads follow the shared
contract number ([CONTRIBUTING.md](../CONTRIBUTING.md) "Versions"). The `seamlane` binary keeps the argv, the `{ok, data, meta}`
envelope and the exit codes of `daisy-cli` 0.2.0 (contract 1), so a skill moves from
daisy to Seamlane by changing the binary path. That switch happens after the first demo
([ADR 0003](decisions/0003-seamlane-replaces-daisy.md)). The events each command writes
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
  ledger's `PRAGMA user_version`. `source_freshness` is always `null`, because Seamlane
  has no mirrors.

`text` output and `--help` are for people and are not part of the contract. `--help`
output has snapshot tests.

## Exit codes

| Exit | Name | When |
| --- | --- | --- |
| 0 | `ok` | |
| 1 | `error` | Every error code without its own exit. |
| 2 | `usage` | Bad argv. |
| 3 | `schema_mismatch` | The ledger's `user_version` is not this binary's, or its `application_id` is not Seamlane's, or it is not SQLite at all. The file is left untouched. |
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
| `schema_mismatch` | 3 | `found`, `supported`, `hint` | Newer file: refused, upgrade the binary. Older or missing file: `hint` says run `seamlane migrate`. Foreign file (wrong `application_id`): refused. |
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

Every write except `migrate` also takes `--agent`, `--session` and `--dry-run`. `[x]` is optional.

| Command | Argv | Kind |
| --- | --- | --- |
| `work create` | `--title <t> [--source] [--source-id] [--repo] [--next-action] [--idempotency-key] [--spec] [--lane] [--lane-pos] [--kind code\|docs\|measure\|investigation] [--pr-flow pr\|direct]` | write |
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
| `migrate` | (uses `--db`; no actor flags) | write, no event |

`work create --kind` and `--pr-flow` are Seamlane's two additions to daisy's argv (#7). Both are optional and stored on `lane.created`. `--kind` defaults to `code` and `--pr-flow` to `pr`, so a skill that passes neither gets the strictest route. #14 reads them at the first claim, when the route is stamped.

`seamlane migrate` is the only command that creates the ledger or migrates it (`Ledger::open_and_migrate`, with a `VACUUM INTO` backup before an upgrade, #4). It creates a missing or empty file. It refuses a non-empty file whose `application_id` is not Seamlane's, a file that is not SQLite, and a version-0 file that already has tables, so it never applies 0001 over someone else's database. Every other command opens with `Ledger::open`, which never creates or migrates and refuses a mismatch with exit 3. Migration 0001 sets `PRAGMA application_id`, and open checks it, so a foreign SQLite file at version 1 is refused (#7).

`work handoff` reads the JSON from `<file>`, or from stdin when the file is omitted or `-`. A handoff with `ready: true` or to `done` runs the `handoff` gate (commands and transitions below). The write itself refuses a `lane.handed_off` that says ready and any event but `phase.entered` that moves a lane to `done`, so no path skips the gate.

`work ready` is the owner's override. It is refused (`invalid_input`) from any session that has held the lane, the same honor-system rule as `check record`.

Lease: `--lease-secs` defaults to 900, minimum 30, maximum 86400. Claiming a `done` or
`cancelled` item is `invalid_input`.

`check record` is new (#16, next to `reviews record`). The recorder passes its own
`--session`. The write is refused when that session is any session that has held the
lane (events.md, producer). `--head` is 7 to
40 hex digits. `--reason` is required with `skipped` and must be one the check allows
(events.md). `--decision` is required for `merge_verdict` and refused for every other
check. `--findings <n>` is required for a `plan_check` or `quality_gate` that ran and
refused elsewhere. It prints `{event_id, work_id, kind, replayed, at, payload}`.

`reviews record` reads `daisy-cli reviews get <job>`. That is Seamlane's only call into
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
Seamlane first recorded the job, and `finished_at` is when it recorded the finish (null
before), not daisy's times.
Findings come from the stored payload, and each one's `outcome` is its row's `fixed` or
`answered`, else null. `reviews record` prints the `reviews get` answer.

`reviews record` refuses (`invalid_input`, nothing written): a view whose `job_id` is not
`--job`, a job recorded on another lane, a job older than the lane's newest recorded job for
the same check (daisy's ids only go up), a request for a job already recorded finished, and
a finding with no id, severity or title. Daisy is found at
`/Applications/daisy.app/Contents/MacOS/daisy-cli` ([R15](research/r15-migration.md)), or
at `SEAMLANE_DAISY_CLI`, and given 30 s. Its exit 5 (`runtime_unavailable`) and its
`not_found` pass through. Any other failure, a timeout or output that is not daisy's envelope
is `internal`, with what daisy said in `details`, so daisy's exit codes never take
Seamlane's meanings.

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
`pipelines/v17.toml` (#12): on `code_pr`, a `plan_check`, a `quality_gate`, and a
`merge_verdict` with decision `yes` whose sha is a prefix of the handed-off sha
(`handoff.commit`, or for `update --status done` the commit of the lane's latest
handoff). `direct` has the same list as `code_pr` (none of the three is a PR gate). On
`investigation`, a handoff to `done` or `update --status done` needs the same three checks (plan check and quality gate may skip with an allowed reason), and a handoff with `ready: true` is always denied: an investigation hands
off `needs you`, never ready (R7). `phase.entered.to` is `done` for a handoff to `done`
or `update --status done`, else `ready`, and `phase.entered.status` is the status the
command asked for, not R7's mapping from `to`. Ready comes from a
handoff's `ready` (or the owner's `work ready` override), so a handoff cannot mark a lane
ready without passing this gate. There is no `next_action` fallback for ready (events.md,
folded rows). A `needs you` handoff is not gated, so it always reaches the owner. A
missing fact fails closed. The route comes from `work create`'s `--kind` and `--pr-flow`
(events.md, lane facts). The
denial exits 1 with `gate_denied`, names the failing gate id on stderr, and leaves the
lane where it was.

## What lane-stats reads

These fields keep their daisy names and meaning:

- `work events`: `action` and `at` per item. `action` is daisy's word (`create`, `claim`,
  `renew`, `takeover`, `update`, `reopen`, `cancel`, `release`, `handoff`,
  `ready_override`), mapped from the event kind (`pr.opened` is `update`, `phase.entered`
  is the command's). `gate.denied`, `check.*` and `finding.updated` are not lane actions
  and do not appear in `work events`.
- `work handoffs`: `created_at` and `handoff.next_action`.

## Not in M1

- `work board` and `reviews stats`: deferred to a follow-up before the skill switch.
- `reviews local`, `reviews pr`, `reviews run`, `reviews wait`: lanes keep calling
  `daisy-cli` for these. The worker stays in daisy.
- `jobs list`, `jobs events`, `capabilities`, `doctor`, `schema`.
- Daisy's personal hub (`issues`, `notes`, `inbox`, `settings`, `chat`, the mirrors,
  `memory`, `context`, `search`): never.
- No id floor, no router, no `linear_outbox`. Ids are plain integers. `source` and
  `--repo` are stored as given, and `linear_state` is always `null`.
