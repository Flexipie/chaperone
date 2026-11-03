# CLI contract

Version **0**, 2026-11-03. The `seamlane` binary keeps the argv, the `{ok, data, meta}`
envelope and the exit codes of `daisy-cli` 0.2.0 (contract 1), so a skill moves from
daisy to Seamlane by changing the binary path. That switch happens after the first demo
([ADR 0003](decisions/0003-seamlane-replaces-daisy.md)). The events each command writes
are in [events.md](events.md).

## Global flags and actor

| Flag or env | Rule |
| --- | --- |
| `--db <PATH>` | The ledger file. Without it, the default ledger path (#4). |
| `--format json\|jsonl` | `json` (default) is one object. `jsonl` is one item per line, then a trailer. |
| `--agent`, `DAISY_AGENT` | `[a-z0-9._-]{1,40}`. Required by every write. |
| `--session`, `DAISY_SESSION` | `[A-Za-z0-9._:-]{1,120}`. Required by every write. |
| `DAISY_CLI_READONLY=1` | Refuses every command that is not a read, `--dry-run` included (`read_only`). |
| `--dry-run` | Validates and prints what would be written. Writes nothing. |

The env names stay `DAISY_*` so lanes already running keep their identity.

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
| 3 | `schema_mismatch` | The ledger's `user_version` is newer than this binary. The file is left untouched. |
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
| `schema_mismatch` | 3 | `found`, `supported`, `hint` | |
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

Every write also takes `--agent`, `--session` and `--dry-run`. `[x]` is optional.

| Command | Argv | Kind |
| --- | --- | --- |
| `work create` | `--title <t> [--source] [--source-id] [--repo] [--next-action] [--idempotency-key] [--spec] [--lane] [--lane-pos]` | write |
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
| `spec decide` | `<spec> [--slice <n>] [--key <k>] --question <q> --answer <a> [--agreed-where <w>]` (alias `--where`) | write |
| `check record` | `--work <id> --check plan_check\|quality_gate\|merge_verdict\|smoke\|kept_tests --head <sha> --outcome ran\|skipped [--reason <r>] [--decision yes\|no] [--findings <n>]` | write |
| `reviews record` | `--work <id> --job <daisy job id>` | write |
| `reviews get` | `<job id>` | read |
| `jobs get` | `<job id>` | read |

Lease: `--lease-secs` defaults to 900, minimum 30, maximum 86400. Claiming a `done` or
`cancelled` item is `invalid_input`.

`check record` is new (#16, next to `reviews record`). The recorder passes its own
`--session`. The write is refused when that session equals the lane's. `--head` is 7 to
40 hex digits. `--reason` is required with `skipped` and must be one the check allows
(events.md). `--decision` is required for `merge_verdict` and refused for every other
check.

`reviews record` reads `daisy-cli reviews get <job>`. That is Seamlane's only call into
daisy. A queued or running job appends `check.requested`. A finished job appends
`check.finished` with `producer {kind: daisy_worker, job}`. `reviews get` and `jobs get`
answer from what was recorded, in daisy's shape.

## Commands and transitions

Each command is one transaction that appends one event (events.md, write rules).

| Command | Transition | Event |
| --- | --- | --- |
| `work handoff` with status `review` or `done` | `handoff` (gated) | `phase.entered`, or `gate.denied` |
| `work update --status done` | `handoff` (gated) | `phase.entered`, or `gate.denied` |
| `work handoff`, any other status or none | ungated | `lane.handed_off` |
| `work update`, any other change | ungated | `lane.updated`, or `pr.opened` when the PR goes from null to set |
| `work claim`, `release`, `cancel`, `ready`, reopen | ungated | the matching `lane.*` kind |
| `work heartbeat` | none | none |

`handoff` is the M1 gated transition in `pipelines/v17.toml` (#12). It requires a
`plan_check`, a `quality_gate`, and a `merge_verdict` with decision `yes` whose sha is a
prefix of the handed-off sha (`handoff.commit`, or for `update --status done` the commit
of the lane's latest handoff). A missing fact fails closed. The
denial exits 1 with `gate_denied`, names the failing gate id on stderr, and leaves the
lane where it was. Entering `blocked` keeps `resume_phase`.

## What lane-stats reads

These fields keep their daisy names and meaning:

- `work events`: `action` and `at` per item. `action` is daisy's word (`create`, `claim`,
  `renew`, `takeover`, `update`, `reopen`, `cancel`, `release`, `handoff`,
  `ready_override`), mapped from the event kind.
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
