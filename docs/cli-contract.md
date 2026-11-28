# CLI contract

2026-11-03. Flags follow the binary `0.y.z` axis and the payloads follow the shared
contract number ([CONTRIBUTING.md](../CONTRIBUTING.md) "Versions"). The `chaperone` binary keeps the argv, the `{ok, data, meta}`
envelope and the exit codes of `daisy-cli` 0.2.0 (contract 1), so a skill moves from
daisy to Chaperone by changing the binary path. That switch happens after the first demo
([ADR 0003](decisions/0003-chaperone-replaces-daisy.md)). The events each command writes
are in [events.md](events.md). The contract stays at 0 until the first demo, removals
included (events.md, owner decision 2026-11-23): `impact`, `overlap` and `dossier` were
removed in #311.

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

Daisy's ten codes, plus two from the engine and `doctor`'s `drift`.

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
| `pipeline_unknown` | 1 | `pipeline_id`, `pipeline_hash` | Claim or transition on a pipeline id this binary has no document for. Writes nothing. |
| `drift` | 1 | `findings` | `doctor` only: each mismatch as `{file, line, kind, message}`. |

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
`claim --worktree` takes an absolute path: every reader of `worktree_path` (`check run`,
`verify`, `pr open`, the hooks) would resolve a relative one against its own directory,
so a relative one is refused (`invalid_input`, nothing written).

## Commands in M1

Every write except `migrate` and `hook` also takes `--agent` and `--session`, and every write but `migrate`, `hook`, `check record`, `check run`, `check waive`, `check overturn`, `verify`, `pr open`, `merge`, `reviews record`, `usage report`, `work merged` and `attention record` takes `--dry-run`. `[x]` is optional.

| Command | Argv | Kind |
| --- | --- | --- |
| `work create` | `--title <t> [--source] [--source-id] [--repo] [--next-action] [--idempotency-key] [--spec] [--lane] [--lane-pos] [--kind code\|docs\|measure\|investigation] [--pr-flow pr\|direct\|none]` | write |
| `work get` | `<id>` | read |
| `work list` | `[--status] [--agent] [--mine] [--all] [--session] [--mine-agent] [--limit] [--cursor]` | read, paged |
| `work claim` | `<id> [--worktree] [--branch] [--base-sha] [--lease-secs] [--takeover] [--pipeline <id>]` | write |
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
| `stats` | `--merged-since <date> \| --work <id>...` | read |
| `pipeline current` | | read, no ledger |
| `pipeline limits` | `[--work <id>]` | read (the ledger only with `--work`) |
| `pipeline history` | `[--dotfiles <dir>] [--repo <owner/repo>]` | read |
| `doctor` | `--skills <dir\|file>... [--repo <dir>]` | read, no ledger |
| `spec create` | `--parent <key> --file <path\|-> [--title]` | write |
| `spec get` | `[<spec>] [--key <slice key>] [--version <n>]` | read |
| `spec list` | | read |
| `spec update` | `<spec> --file <path\|-> --reason <r> [--expected-version <n>]` | write |
| `spec approve` | `<spec>` | write |
| `spec decide` | `<spec> [--slice <n>] [--key <k>] --question <q> --answer <a> [--where <w>]` | write |
| `check record` | `--work <id> --check plan_check\|quality_gate\|merge_verdict\|smoke\|kept_tests\|review_round\|size_waiver\|rounds_waiver --head <sha> --outcome ran\|skipped [--reason <r>] [--decision yes\|no] [--findings <n>] [--evidence <file>]... [--run <script>]` (a `size_waiver` or `rounds_waiver` is refused here, having no `--lines` or `--rounds`: `check waive size` and `check waive rounds` record them) | write |
| `check run` | `diff\|tests\|review\|verdict\|plan\|smoke --work <id> [--file <plan>]` | write |
| `check waive` | `plan --work <id> --reason <r>`, `size --work <id> [--lines <n>] --reason <r>`, `rounds --work <id> --rounds <n> --reason <r>` | write |
| `check overturn` | `plan\|size --work <id> --reason <r>` | write |
| `check prune` | | read, removes directories |
| `verify` | `--work <id>` | write |
| `pr open` | `--work <id> [--title <t>] [--draft]` | write, pushes, calls gh |
| `merge` | `--work <id> [--head <sha>]` | write, calls gh and herdr |
| `reviews record` | `--work <id> --job <daisy job id>` | write |
| `reviews get` | `<job id> [--wait] [--timeout-secs <n>]` (accepted for the path swap; a recorded job answers at once) | read |
| `review eval` | `--repos <dir> [--set <file>] [--only <name>] [--runs <n>]` | read, runs review sessions |
| `jobs get` | `<job id>` | read |
| `usage report` | `--work <id> [--claude-dir <dir>] [--codex-dir <dir>]` | write |
| `usage get` | `<id>` | read |
| `attention record` | `--work <id> --type asked\|answered\|nudged\|sent_back\|revived\|stopped\|prompted --by orchestrator\|owner [--reason <R12 id>] [--round <k>] [--answers <event id>] [--note <one line>] [--idempotency-key]` | write |
| `finding list` | `--work <id>` | read |
| `finding dispute` | `<event> <key> --reason <r>` | write |
| `finding waive` | `<event> <key> --reason <r> --kind <kind> [--ref <ref>]` | write |
| `finding accept` | `<event> <key> --reason <r>` | write |
| `finding answer` | `<event> <key> --reason <r>` | write |
| `finding overturn` | `<event> <key> --reason <r>` | write |
| `evidence show` | `<sha256 or prefix>` | read, no ledger |
| `migrate` | (uses `--db`; no actor flags) | write, no event |
| `install` | | no ledger, writes the marketplace dir, calls claude |
| `uninstall` | | no ledger, removes the marketplace dir, calls claude |
| `hook` | `session-start\|pre-bash\|stop\|stop-failure\|prompt` (stdin: the hook's JSON) | read; `stop-failure` and `prompt` write |

`work create --kind` and `--pr-flow` are Chaperone's two additions to daisy's argv (#7). Both are optional and stored on `lane.created`. `--kind` defaults to `code` and `--pr-flow` to `pr`, so a skill that passes neither gets the strictest route. #14 reads them at the first claim, when the route is stamped.

`chaperone migrate` is the only command that creates the ledger or migrates it (`Ledger::open_and_migrate`, with a `VACUUM INTO` backup before an upgrade, #4). It creates a missing or empty file. It refuses a non-empty file whose `application_id` is not Chaperone's, a file that is not SQLite, and a version-0 file that already has tables, so it never applies 0001 over someone else's database. Every other command opens with `Ledger::open`, which never creates or migrates and refuses a mismatch with exit 3. Migration 0001 sets `PRAGMA application_id`, and open checks it, so a foreign SQLite file at version 1 is refused (#7).

`work handoff` reads the JSON from `<file>`, or from stdin when the file is omitted or `-`. An `areas` field, which older skills send, is accepted and dropped, as any field the ledger does not keep (#311). A handoff with `ready: true` or to `done` runs the `handoff` gate (commands and transitions below). Before a handoff that enters ready (`ready: true` and a `status` other than `done`) is written, the CLI reads, in the lane's `worktree_path`, the tip of the lane's `branch` (stack layers share one worktree, so `HEAD` may be a later layer's), or `HEAD` when the item names no branch or the worktree does not have it. The write refuses (`invalid_input`, nothing written, before the gate) a `commit` that is missing or is not that commit or a 7 to 64 character prefix of it, so the checks the gate reads are checks on what the branch holds. A handoff to `done` is not checked (after a merge the branch may have moved on), nor is a lane with no worktree, a relative `worktree_path`, or a directory that is gone; a worktree that is there and cannot be read refuses. The ledger makes no git call: other callers of the write pass the head they observed, or none. The write itself refuses a `lane.handed_off` that says ready and any event but `phase.entered` that moves a lane to `done`, so no path skips the gate. After a handoff that entered ready (not a dry run), on a lane holding a PR (`pr_ref`, from `pr open` or the handoff's `pull_request`; with none, the handoff's `pull_request` URL `https://github.com/<owner>/<repo>/pull/<n>`) with an absolute `worktree_path`, the CLI requests the base profile's `[pr] reviewers` (read as `pr open` reads them): `gh pr view <n> --repo <owner/repo> --json author,reviewRequests,latestReviews` (found on `PATH`, 120 s per call), then, leaving out (any case) the author, each requested login or team slug (a profile's `org/team` matches its slug) and each latest review's author, `gh pr edit <n> --repo <owner/repo> --add-reviewer <a,b>` when anyone is left. GitHub's record decides, so a repeat ready handoff asks no one twice. It runs after the write, so a failure never loses the handoff: the output is the handoff with `reviewers` `{requested: [..]}` (empty when no one is left or the profile names none), or `{error}` with gh's or the profile's message, and the same ready handoff again retries. The owner's `work ready` override requests no one.

`work merged` is new (#59). `/merge` runs it after the merge, from any session (it skips the
holder rule and leaves `updated_at` alone, as `check record` does). It appends one
`pr.merged` (events.md, PR). It refuses (`invalid_input`, nothing written) a `--pr` that is not
`owner/repo#N` or that the item has never held, a PR already merged on another item, a sha that
is not 7 to 40 lowercase hex digits, and a `--merged-at` that is not RFC 3339 UTC with
milliseconds (GitHub's `mergedAt`, `2026-11-05T10:00:00Z`, is passed as
`2026-11-05T10:00:00.000Z`); off v21 those three flags are required. On a v21 item (#308) it
asks GitHub every time, held PR or not, once the ref is `owner/repo#N`: one `gh pr view <n> -R
<owner/repo> --json state,headRefName,headRefOid,isCrossRepository,mergeCommit,mergedAt` (a gh
failure is `internal`). On an item with a `pr_ref`, any other PR is refused before gh. A PR not in the item's `repo`, with its head in another repository, whose
head branch is not the item's `branch`, or not `MERGED` is refused, the message saying which. The
merge commit, head and time recorded are GitHub's, never the flags'. The ledger checks "never
held" last, so when that is its one refusal on an item with no `pr_ref`, the PR is made its `pr_ref` (with the write
`work update --pr` makes, its own event, under the holder rule) and the merge is recorded; any
other refusal leaves `pr_ref` as it was. The same PR again replays. It prints `{event_id, work_id, kind, replayed, at,
payload}`.

`attention record` is new (#59). `/orchestrate` and the owner run it for each intervention on
a lane, from any session (it skips the holder rule and leaves `updated_at` alone). It appends
one `attention.recorded` (events.md, Attention). It refuses (`invalid_input`, nothing written)
an `asked` without `--reason` (one of `question.done_ambiguous`, `question.scope`, `question.one_way`,
`question.recorded_decision`, `question.size`, `question.investigation`, `question.verdict_no`,
`question.product`, `question.seam_reject` and `question.overlap`), a `--reason` on another type, a `sent_back` without `--round` of
1 or more, an `answered` without `--answers` and `--note` or whose `--answers` is not a question
asked on that item, `--answers` or `--round` on another type, and a note that is blank, has a line break or is
over 500 characters. `--idempotency-key` is trimmed (blank is no key, over 200 characters is refused, as for
`work create`) and scoped to the item and type, so the same key on another item, or on a
question and its answer, is a separate record. Without `--idempotency-key`, a record repeated with nothing new on the lane
replays; a real repeat (a second nudge on a stalled lane) passes its own key. It prints
`{event_id, work_id, kind, replayed, at, payload}`.

`work update` keeps daisy's owner rule (a live lease held by another pair refuses everyone
else) with one exception, the owner's pause (#119; the removed board sent it): agent `owner-ui` may move
a `claimed` or `active` item to `blocked` with `--blocker "paused by owner"` and nothing else
(no `--reason`, no other field) over another pair's live lease. It is an ordinary `lane.updated`
that keeps the holder and bumps `updated_at`, so the lane's next `--expected-updated-at` is
stale and it reads the pause. Any other change from `owner-ui`, and the same pause from any
other agent, is `conflict` (lease held). The write's lease guard checks the event itself, so
no other path gets past it.

`finding dispute` and `finding waive` are new (M2 PR 14). Each appends one `finding.updated`
(events.md, finding statuses) on the finding's own lane, addressed as the ledger addresses it:
`<event>` is the id of the `check.finished` that raised it (`work events`) and `<key>` its key
there (its `id` in `reviews get`). `dispute` moves an `open` or `verified` finding to `disputed`
and only a session that has held the lane may; it still blocks until a session that never has
rules on it: on a pipeline with the limit `dispute_attempts` (v21 on; on v20 the owner
waives it), the verifier at the next `check run review` (so `verify`, #339) rules it
`refuted` (the dispute holds) or `verified` (it blocks again), inside a fresh review, or on a
tree whose review replays as a harness `dispute_ruling` (events.md). Not one titled `owner:`
(a one-way door or a gate that fails open, as the lenses title them), one the owner moved to
`verified`, or one disputed again after its ruling: those are the owner's. `waive` moves an `open`, `verified` or `disputed` finding to `waived` and is the
owner's: refused from any session that has held the lane, as `work ready` is. `--reason` is
required, one non-blank line of at most 500 characters (as `attention record --note`), and
redacted. A move the table does not allow, the wrong session or a bad reason is
`invalid_input`, and an unknown finding is `not_found` with `details.reference`
`<event>/<key>`; nothing is written. The same move again (a retry after a lost answer) is
refused as `finding <event>/<key> is already <status>`, not replayed. Both print
`{event_id, work_id, kind, replayed, at, payload}`.

The lane itself may make one `waive` (#340): `--kind deferred`, with `--ref` the repo rule that
forbids the edit or the follow-up issue, on an `open` or `verified` finding of a harness review
it cannot fix in this lane. Refused on a disputed one (it waits for its ruling), another check's,
and one that is the owner's (titled `owner:`, overturned, or disputed twice). It stops blocking,
a lane's deferral never reopens its own NO verdict, `pr open` lists it under "Resolved without
the owner", and `finding overturn` makes it block again.

`finding waive` takes `--kind` (#235), required: `fixed`, `disproved`, `scope`, `deferred` or
`accepted`, and `--ref`, what backs it (the commit and test, the file:line, the owner text or
the follow-up issue; one non-blank line of at most 500 characters). `fixed` and `disproved`
need `--ref`. `scope`, `deferred` and `accepted` are countersigned by whoever writes them:
the writing session must have no event on the lane but its own earlier waivers of those three
kinds. So the orchestrator, which wants the lane through, is never the only judge: it briefs a
fresh agent that never saw the lane, and that agent judges and writes the waiver under its own
session. The ledger reads sessions as they are declared, as it does for the owner's
independence, so it cannot tell a real fresh agent from a made-up id; it refuses one that has
touched the lane. `--kind scope` also releases an `accepted` plan check finding whose work
moved to another issue, with `--ref` naming it; no other kind may, nor one with no `--ref`. Waiving it changes the plan
answers the review is shown, so the next `verify` reviews the head again; a blocker an
earlier review raised on that point is a review finding and is waived on its own. Never waive
a finding where a gate fails open on a path a lane can reach: fix it or ask the owner. An
`accepted` waiver with no `--ref` is listed in `stats` (`accepted_without_ref`).

`finding accept` is new (#221): the lane's answer to a finding of its harness plan check, the
other being `finding dispute`. It moves an `open` plan check finding to `accepted`, only from a
session that has held the lane, and is refused on any other check's finding. `--reason` is
required as for `dispute` (how the lane will act on it) and the review lenses are shown it. It
prints as `dispute` does. `finding answer` makes the `open` to `answered` move on a
`question`, from any session, with the answer as `--reason`: how the lane answers a plan
check's question. While a finding of the lane's newest plan check (a replayed older plan is
carried, so it is the newest again) is `open`, `check run
diff` and `check run review` (so `verify`) are refused (events.md, plan findings).

`finding overturn` is new (#339): the owner's answer to a stop the lane resolved without
them. It moves a `refuted` or `waived` finding to `verified`, so it blocks again, refused from
any session that has held the lane, and the verifier never rules on its dispute again. It
prints as `dispute` does. `pr open` lists under "Resolved without the owner" each dispute the
verifier upheld and each `deferred` waiver (its ref and reason), with `finding overturn`, and
the lane's own plan waiver and size reason (#340), with `check overturn`, and the harness's
extra review round (#341), with `check waive rounds`. Its Size line adds
`over the cap of <cap>: <why>` when the diff adds more code lines than the cap a pipeline's
review asks about (v21 on), the why the newest size waiver's note, or `no reason recorded`.

`finding list` is new (#212). It prints, oldest first, every finding of the lane's complete
harness reviews (`check.finished` `review` from the harness with `complete: true`), of any
commit, and of its newest plan check (#221), as items `{check_event_id, check, finding_key,
severity, title, body, path, line, head_sha, status, reason, waiver, waiver_ref, blocks, disputed}`. `check` is `review` or
`plan_check`; `reason` is that of the finding's newest move, null when it has none, and `waiver`
that move's waiver kind (#235) and `waiver_ref` its ref (#340), null when it has none; `disputed` is whether the lane has ever disputed it (#339); a plan
check's finding never `blocks`. `check_event_id` and `finding_key` address the finding for `finding dispute`
and `finding waive`; `head_sha` is the commit the review read; `line` is the lens's, from the
review's payload. `blocks` is the `no_blocking_findings` gate's rule: unsettled (`open`,
`verified` or `disputed`) and of a severity that blocks, so once the head has a complete
review the count of `blocks` is `findings.open_blocking` (before that the gate stops at
`review_complete` and the fact is absent). An incomplete review's findings, and those of other checks (an
older plan check's included), are not listed. An unknown work id lists nothing, as `work events`.

`evidence show` is new (#213). It prints a file from the evidence store beside `--db`
(`evidence/sha256/<2>/<hash>`): a harness session's `prompt_ref` or `answer_ref`, a check's
`evidence_ref`, a smoke claim's `sha256`, as the `check.finished` event names them (`check run`
prints the event). `<ref>` is the sha256 or a
prefix of 12 to 64 of its lowercase hex digits that names one file. The file is hashed again
before anything is printed. The answer is the stored text, not an envelope: piped, its exact bytes;
on a terminal, control characters other than newline and tab are printed escaped (`\u{1b}`), so a
stored escape sequence cannot change what is shown. Errors are the usual envelope on stderr: a ref
that names no file is `not_found` (`details.entity` `evidence`, `details.reference` the ref as
given), a ref that is not 12 to 64 lowercase hex digits or a prefix two files share is
`invalid_input`, and a file whose bytes no longer match its name is `internal`. It never opens the
ledger.

`work ready` is the owner's override. It is refused (`invalid_input`) from any session that has held the lane, the same honor-system rule as `check record`. It answers daisy's way: `{id, ready, ready_reason}` from the board (read for that item only), or `{id, ready_override, on_board: false}` for a done or cancelled item.

`work board` is daisy's board shape, `{specs, other}`, one read statement. Its rows are written as they are read to an unlinked, owner-only temp file, so the CLI never holds the whole board; stdout gets the file once the read has ended, so a slow or stalled reader never holds a WAL snapshot, and a ledger error leaves stdout empty. It needs a writable temp dir (`TMPDIR`); without one it fails `internal` with `temp file: ...`. Every open item (not `done` or `cancelled`) is under `other`, ordered by `lane_key`, `lane_pos`, `id`; `specs` stays empty until items are grouped by spec. Each entry is `{item, handoff, ready_override, ready, ready_reason}`: `item` is `work get`'s, `handoff` the newest `work handoffs` row or null, and `ready_override` the newest `work ready` as `{ready, reason, by, at}`, null when there is none or it was cleared. The ready rule is daisy's first steps: the override wins (`marked ready by <agent>: <reason>`); else an item not in `review` is not ready (`status is <s>, not review`); else it is ready only when its newest handoff says ready (`the lane says ready`, else `the lane has not said ready`). A ready handoff exists only past the `handoff` gate, so a later `needs you` handoff unsays it. Ready is also derived at read time: for an item the rule would call ready on the lane's word, the board evaluates the `handoff` gate again on the handoff's `commit` with the facts the lane's events project now, so a record written after the handoff (a NO verdict on that commit, say) unsays it as `the lane said ready, but <gate> no longer holds`. Every gate of the lane's ready row is asked, so a verdict recorded on a newer commit (even a yes) or a review round past the cap unsays it too, and a lane stamped with a pipeline this binary cannot load reads `the lane said ready, but this binary does not know pipeline <id>`. That read writes nothing (no `gate.denied`), leaves the lane's `phase` alone, runs only for those items, and makes no git call. The override is not re-evaluated. Daisy's further steps, a finished review on the PR head and no open thread from a person, need the PR and are the caller's to check; the board does not notice commits pushed after the gated handoff unless a check was recorded on them. `lanes` checks threads for every repo but the review on the head only for repos daisy does not review, so before it reads this board for a daisy-reviewed repo it has to check the review on the head there too. Daisy's other board fields (`pr_head`, `pr_state`, `pr_review`, `local_review`, `linear_outbox`, `threads`, `threads_checked`, `needs_you`) are not emitted.

`stats` is new (#89). It prints [R11](research/r11-observability-cost-metrics.md)'s numbers for a cohort of lanes, read from the log in one statement (the cohort's events in lane order, each lane folded once); nothing is stored, so every total is a sum over events. The cohort is the lanes with a `pr.merged` whose `merged_at` is at or after `--merged-since` (`YYYY-MM-DD`, read as 00:00 UTC, or RFC 3339 UTC with milliseconds; anything else is `invalid_input`), or the `--work` ids (repeat the flag; a repeated id counts once, an unknown one is `not_found`). An empty cohort is `lanes: 0` with null medians. `data` is `{cohort, lanes}`.

Each lane is `{id, title, kind, pipeline_id, claimed_at, merged_at, claim_to_merge_h, with_pr, merged, tokens, peak_context, attention, waivers, accepted_without_ref, send_back_rounds, plan_check, quality_gate, size_waiver (the newest owner size waiver's `lines` and `head_sha`, null without one or after its overturn; a lane's own size reason raises nothing and is not one), over_cap (a counted harness review of the lane has `over_cap`), cut_findings (the design_tests lens's findings titled `cut:` on those reviews), size_estimate (`{estimate, measured}`: the newest `estimate` a plan check stated (an owner's plan waiver states none), as held at the lane's first harness `diff`, and that diff's `code_lines`; null when either is missing), merge_verdict, verdict_on_head, head_reviewed, flat_rounds, reraised_findings, verified_findings, verified_findings_after_handoff, commits_after_handoff, sources}`:

- `claimed_at` is the first `lane.claimed` (a reopened lane keeps it, so its wait counts), `merged_at` the latest `pr.merged.merged_at`, and `claim_to_merge_h` the hours between them, only when every PR the lane held (`pr.opened` or a `changed.pr_ref.to`) has merged (`merged`).
- `tokens` is `{total, steps}`, null when the lane has no usage event. `steps` is keyed by step: `parent` (the lane's own agent), `subagent`, and for a harness session its `lens` (`plan`, `correctness`, `design_tests`, `intent`, `codex`, a required lens's name, `verifier`, `verdict`; `harness` for one with no lens). Each, and `total` over them, has the four token counts `uncached_input` (fresh input, as `usage get` counts it), `cache_write`, `cache_read` and `output`, summed skipping nulls as `usage get` does (a count no session of the step reported, as Codex's `cache_write`, is null, never 0), and `sessions`, the step's distinct sessions (a subagent's `agent_id`, else `session_id`). No dollar figure. `peak_context` is the largest parent `input_total`.
- `attention` counts `attention.recorded` by `type`, plus `waived`: each `finding.updated` to `waived` and each `check waive` (a `check.finished` with `reason: waived`), the owner's decisions; `send_back_rounds` is the highest `sent_back` round.
- `waivers` counts the lane's `finding.updated` to `waived` by `waiver.kind` (`none` for one written before waivers had kinds), so a waived finding that later escapes shows which kind failed; `accepted_without_ref` lists each `accepted` waiver with no `ref` as `<event>/<key>`, for the owner.
- `plan_check` and `quality_gate` are the latest `check.finished` of that check: `{outcome, head_sha, findings}` when it ran, `{outcome, head_sha, reason}` when skipped, null when absent.
- `merge_verdict` is the latest `merge_verdict` that ran, `{decision, head_sha}`, null when none ran, so a missing verdict reads apart from a NO or a YES on an older head.
- `verdict_on_head` is true when, for every merged PR, the latest `merge_verdict` that ran on its merged head (the gate's rule: the shorter sha, at least 7 hex, is a prefix of the longer) said yes; null until every PR the lane held has merged (a stack lane with a layer still open is null, not true).
- The review numbers read `check.finished` `local_review` and `pr_review` that ran (R11's `review.finished` and `finding.recorded`, events.md "Replaced names"). `head_reviewed`: every merged head has a local review on it (null until every PR has merged, as `verdict_on_head`). `flat_rounds`: adjacent local scores, recorded after the lane's first PR and ordered by daisy's job id, where the later is not higher. `reraised_findings`: verified findings grouped by trimmed lowercased title, the distinct jobs in each group minus one, summed, plus each finding a counted harness review wrote with `same_as` (the verifier ruled it a repeat of an earlier one). `verified_findings`: findings (by `finding_key`, once each) whose latest record in job order is verified. `verified_findings_after_handoff`: those whose latest record is on a review recorded after the first real handoff (one whose `next_action` does not start with `reviewing`, else the earliest; all of them when there is no handoff). The time is when Chaperone recorded the review, since daisy's finish time is not stored, so a review recorded late counts as after.
- The harness's `review` counts as a review too: only a complete one from the harness producer, as `findings.open_blocking` reads them, and a carried one only when the review it names counts (its findings stay that one's). Its finding is verified while its latest `finding.updated` move, by anyone, in log order, is `verified` (a `refuted` clears it; `disputed`, `fixed` and `waived` keep it). It counts for `head_reviewed` as a local review does, and for `reraised_findings` each harness review is its own job. Its `flat_rounds`: a complete, not carried harness review on another commit than the one before it that finds again every verified `finding_key` of that one (which had at least one).
- On a lane stamped with a schema 2 pipeline (v20, by the stamped id whatever its hash: a prompt edit moves the hash), the handoff of `verified_findings_after_handoff` is the ready point instead, since its first handoff is at planning: the first harness `merge_verdict` that said yes, else the first `pr.opened`.
- `commits_after_handoff` is always null: it is a git count (events.md, PR), not a ledger read.
- `sources` names where the numbers came from: `reviews`, the checks its reviews came from (`local_review`, `pr_review`, `review`), and `ready_point`, which point the after-handoff count used (`handoff`, `verdict`, `pr_opened`, or null with none). `tokens.steps` is keyed by its step.

`cohort` has `lanes`, `with_pr`, `merged`, `too_few_lanes` (under 25: flagged, not scored), `claim_to_merge_h_median`, `tokens` (the lanes' `{total, steps}` summed per step, null with no usage; a message recorded on two lanes of the cohort, which usage's worktree match allows, counts on each), `attention` (summed per type), `waivers` (summed per kind), `accepted_without_ref` (every lane's, as `{work_id, finding}`), R11's first five (`lanes_with_usage` (lanes with a parent `input_total`), `peak_context_median`, `context_tokens_per_merged_pr_median` (parent `input_total` over the PR count, merged lanes with usage), `lanes_peak_over_500k`; `lanes_reviewed` (lanes with a recorded review: the coverage of row 2's sums), `flat_rounds`, `reraised_findings`; `lanes_head_reviewed`; `lanes_with_commits_after_handoff`, `verified_findings`, `verified_findings_after_handoff`; `lanes_plan_checked`, `plan_check_skipped` and `quality_gate_skipped` (counts by reason), `lanes_quality_gated`, `lanes_verdict_on_head`, the last two over lanes with a PR as R11 defines, `lanes_size_waived` (lanes the owner raised the size cap for, to compare their rounds and escapes), `lanes_over_cap` and `lanes_with_cut_finding` (lanes reviewed past the cap, and those of them with a `cut:` finding), `lanes_estimated` and `lanes_over_estimate` (lanes with a `size_estimate`, and those of them whose measured lines are past 1.5 times the estimate)), `finding_outcomes`, and `missing`. `finding_outcomes` counts, per lens (`none` when a finding names none), each finding of a counted, not carried harness `review` on a merged lane (`merged`) by outcome, from its newest `finding.updated` (#279): `fixed` (status `fixed`, or waived `fixed`), `wrong` (`refuted`, a dispute the verifier upheld included (#339), or waived `disproved`), `intentional` (waived `accepted`, `scope` or `deferred`: real, deliberately not changed in the PR), `did_not_matter` (ruled `unproven` by the verifier and not settled since) and `unlabelled` (anything else: a waiver written before waivers had kinds, an answered question, or any finding still unsettled when it merged, since the verifier rules `fixed` only on blocking ones). A `same_as` repeat is left out; `reraised_findings` counts it. Every lens lists all five, so its false-alarm rate is `wrong` plus `did_not_matter` over their sum, and `unlabelled` shows how much is unknown. Nothing is guessed: a finding gone in a later round is not read as fixed. A median is over the non-null lane values, rounded to two decimals, and null when there are none. `missing` maps each cohort number printed as null because nothing records its input (by its cohort name; the lane fields of the same rows are null for the same reason) to what it needs: `lanes_with_commits_after_handoff` always, the three token numbers of row 1 when no lane in the cohort has a parent `input_total` (`lanes_peak_over_500k` is then null, not 0), and every review number when no lane in the cohort has a recorded review (with one, a lane without a review counts as unreviewed, not unknown). `sources` counts the lanes per review check (`reviews`) and per ready point (`ready_point`).

`pipeline current` (#106) prints `{id, hash, newer}`: the pipeline a new claim stamps by default (`v21`), and in `newer` the `{id, hash}` of each embedded document newer than it that a first claim may opt into with `work claim --pipeline` (none today). Skills read the version from here rather than naming one, so a lane's stamp and its skill cannot disagree.

`pipeline limits` prints `{pipeline: {id, hash}, limits}`: the `[limits]` table of the lane's stamped pipeline with `--work` (a lane with no stamp this binary loads is `invalid_input`), else of `pipeline current`'s; `{}` when the table is empty.

`pipeline history` (#112) prints each version of the pipeline changelog with the lanes that ran on it, so the pipeline's own performance can be shown (the UI's Pipeline page, `lane-stats`). It reads `<dotfiles>/claude/pipeline-changelog.md` (`--dotfiles`, default `~/dotfiles`; `~/.claude/pipeline-changelog.md` links to it): every heading that starts `## v` and a digit must be `## v<N> <YYYY-MM-DDTHH:MMZ>` (UTC), N and the time rising in file order, else `invalid_input` naming the line, as is a missing file; a time that has that shape but is not a real one (`2026-13-40T25:61Z`) is `invalid_input` naming its version; a version's `entry` is the text under its heading, up to the next `## ` heading. Lanes are every lane not cancelled (`--repo` keeps one repo's, any case), read from the log in one statement as `stats` does, after one query per version that checks its time is real, all in one read transaction; each lane's review rounds are then projected from its events (one projection per lane). A lane's version is the first `pipeline vN` (as words; `pipeline v17.1` names v17) a handoff summary names, in log order, else the version whose time is at or before the lane's first `lane.claimed` (`manual` before the first version), as `lane-stats` assigns them; so an open lane can move at its first handoff. A lane never claimed and never stamped ran no pipeline and is left out. `data` is `{versions}`, one row per changelog version (newest first, a version with no lanes too), then any stamped version the changelog lacks, then `manual` when it has lanes: `{version, started_at, entry, engine_ids, lane_ids, version_from: {stamp, claim}, lanes_review_rounds, review_rounds_median, cohort}`. `started_at` and `entry` are null for a version not in the changelog. `engine_ids` are the distinct engine documents its lanes were claimed under (`work get`'s `pipeline_id`, such as v17.1 next to changelog v17). `cohort` is `stats --work <lane_ids>`'s `cohort`, so its `claim_to_merge_h_median` is the per-version median and its `attention.sent_back` the version's send-backs. `review_rounds_median` is the median of the engine's `review.local_rounds` (the count the gates read) over the `lanes_review_rounds` lanes with at least one local round (a local review that ran or a `review_round` check; a daisy PR review alone is not one), null when none had one; a lane never reviewed is left out rather than counted as 0.

`doctor` (#106) checks skill and script files against this binary, so a skill that drifts from the CLI fails before a lane runs it. It never opens the ledger. `--skills` takes directories (read recursively, hidden entries skipped) or files: `.md`, `.sh`, `.py`, and extensionless scripts with a shell or python shebang (only their first 128 bytes are read to tell). A call is the binary's argv after `$L`, `"$L"`, `$chaperone`, `"$chaperone"`, or the word `chaperone` followed by a command (after a space, `/` or `(`); in Markdown only in fenced blocks and code spans (which may wrap within a paragraph), never prose; shell lines join `\` continuations and skip comments; a word stops at `|`, `;`, `&`, `)`, a backtick or a redirect. In Python, a call is a run of string literals whose first is a chaperone command, on a line that names `ledger` or `chaperone` (`[ledger, "work", "get", id]`, `cli(ledger, ["work", ...])`, `daisy("work", "list", cli=ledger)`). A placeholder (`<id>`, `"$PR"`, `$(...)`, an f-string, any other expression) stands for any value. Each call goes through this binary's own parser; an unknown command, flag or value is a finding, and so, in a fenced block or a shell file (whole commands), is a missing required argument or a value that does not parse (`--limit lots`); a missing argument or bad number in a code span or a Python list (either may be a fragment), or in a call with a word that may expand to several (`"$@"`, `$*`, `"${a[@]}"`, an unquoted `$x`), and a placeholder's refusal are not. `--flag=<placeholder>` keeps its flag, so the rest of the call is still checked. `--status` of `work update` and `work list`, and `--reason` of `check record` for its `--check`, are checked against the ledger's lists, as `--flag value` or `--flag=value`. A fenced `json` block with a `changed_files` key is a handoff: each key, nested ones too, must be a field the ledger keeps (it drops the rest). In `.md` files, every `pipeline vX` and `Pipeline version: **vX**` must be `pipeline current`'s id. A path with no such file, no call in any file, or an unreadable file is a finding too, so a wrong path never passes. With `--repo <checkout>` it also compares the commit this binary was built from (`build`, embedded at build time, empty outside a git checkout) with the repo's `origin/main` on `crates`, `pipelines`, `Cargo.toml` and `Cargo.lock`, so a docs-only merge does not flag it; it does not fetch. No findings: `data` is `{pipeline: {id, hash}, build, invocations, files: [{path, invocations}], origin_main?}`. Any finding: `drift` (exit 1).

Lease: `--lease-secs` defaults to 900, minimum 30, maximum 86400. Claiming a `done` or
`cancelled` item is `invalid_input`.

`--pipeline <id>` names the pipeline a lane's first claim stamps: the default (`v21`, the
same as no flag) or an embedded one newer than it, for a pilot lane (none today): recorded on
`lane.claimed` as the default stamp is. An older id (`v20`, `v19`) or one not embedded is
`invalid_input`, and so is `--pipeline` on a renewal, a takeover, or a claim of a lane already
stamped. `v19` and `v20` stay embedded only for the lanes already stamped with them. On a lane
stamped `v20`, `check record` refuses `plan_check` and `merge_verdict` (events.md, producer):
the harness runs them (`check run plan`, `check run verdict`); `check waive plan` and a smoke
claim stay.

`check record` is new (#16, next to `reviews record`). The recorder passes its own
`--session`. The write is refused when that session is any session that has held the
lane (events.md, producer), except a `smoke` on a lane stamped `v20`, which is only the lane's
claim there and may come from the lane's own session. `--head` is 7 to
40 lowercase hex digits. `--reason` is required with `skipped` and must be one the check allows
(events.md). `--decision` is required for `merge_verdict` and refused for every other
check. `--findings <n>` is required for a `plan_check` or `quality_gate` that ran and
refused elsewhere. `--evidence <file>` (repeat for up to 12) goes with `smoke` only: the files
the smoke wrote or showed, the claim of the session recording it, never the harness's. Each is
read by the CLI by R13's rules: a relative path is read from the lane's worktree root, its real
path (links resolved) must be inside the worktree and not under `.git` or `.chaperone` there,
and it must be a regular file of at most 1 MiB of UTF-8 text with no NUL byte (the type is
checked before the file is opened, so a FIFO is refused, not waited on; this guards against
accidents, not a lane that swaps the file between the check and the open). Every rule is checked
first: the check and the count, then each file is read and named, then the record is tried and
rolled back (so a session that held the lane, or any other refusal, stops here); only then is
each file stored in the evidence store (redacted, its last 256 KiB), before the event that
carries `evidence_files` commits. A refusal is `invalid_input` and writes no event and no file.
`--run <script>` (#225) goes with a `smoke` that ran only, never with `--evidence`: the script
is read by the same rules, then, after the trial record (which carries the run as if it passed),
the CLI runs `bash -- <script>` in the worktree's root as `check run` runs a command (stdin empty,
the session's and git's repository variables removed, the end of each stream kept, a 30-minute
deadline). The script's text, `$ ["bash", "--", "<script>"]`, its stderr and stdout, then
`exit <code>` (`null`, and `(timed out)` at the deadline, when it has no code) is the claim's
evidence file, named by the script's path, and the payload carries `run: {argv, exit}`. A run
that exits non-zero is recorded too, and claims nothing (`smoke.claimed` needs `exit` 0); a
script that cannot start writes nothing. On a lane stamped `v20`, a session `smoke` that ran
without `run` is refused at the write (`invalid_input`), so a claim's evidence is what its
command printed and how it exited, never a script the lane names as evidence. The output is part
of the payload's hash, so a run claim replays only when its output is the same.
(A write that fails only between the trial and the real record, such as a lane claimed in that
moment, leaves its content-addressed files in the store.) It prints `{event_id, work_id, kind, replayed, at, payload}`.

`check run diff` is a check the binary runs itself rather than one a session reports
(events.md, producer `harness`). It first takes the lane's run lock: a file
`<ledger>.lane-<id>.lock` beside the ledger, locked as `migrate` locks its own but without
waiting, so a second `check run` on the lane while one runs is `conflict` (exit 4) and writes
nothing. The lock goes when the command ends, however it ends. The lock file itself stays next
to the ledger, one per lane, and is never removed (removing it would race the next run). Then it
reads the lane's `worktree_path`, `branch` and `base_sha`, and in the worktree the commit the
lane is on (the tip of its `branch`, else `HEAD`, as a ready handoff reads it, within 5 s), then
that commit's numstat against the merge base (committed files only, no renames; the bases are
read as remote-tracking refs, so a local branch named `origin/main` is never taken for one) and
its tree, those two within one 10 s deadline. A lane with no worktree, a relative or missing
one, a git call that fails or times out (not marked retryable yet), a numstat past 16 MiB and
a base profile that is a link or no file, is not TOML (with or without `[diff]`) or whose
`[diff]`, `[size]`, `[assets]`, `[[companions]]` or `[[review.required]]` does not load (a required lens
needs a `name` with no space or `:`, a non-empty `when` and a prompt) are `invalid_input` and
write nothing. So is a
commit that breaks a companion rule of the base profile: each `[[companions]]` has a `name`
(no space or `:`), `when` and `needs` (non-empty glob lists), and a commit that changes a file
`when` names must change one `needs` names. The refusal names each broken rule, the file that
triggered it and the files it needs. A lane commit (merge base to the lane's commit) whose
message has the trailer `No-companion: <name> <why>`, the why not empty, waives that rule, so a change that truly
needs no doc edit says so in its history; git's log is read for it only when a rule is broken, and a log past 1 MiB refuses rather than
read as no waiver.
A binary older than the table ignores it. The per-file list goes to the
evidence store before the one `check.finished` that names it is written. It prints what
`check record` prints.

`check run tests` takes the same lock and reads the commit the same way, then the `[tests]`
table of `.chaperone/profile.toml` on the base branch (the first of `origin/HEAD`,
`origin/main`, `origin/master` as remote-tracking refs, read at one commit, which the event
records), never the lane's copy. Those refs live in the lane's own clone, which the lane can
move, so under the same-user trust model this keeps a lane's edit out but is a speed bump, not a
guarantee. The table: `commands`, a list of argv arrays run in order with no shell;
`target_dir`, optional, the name of a variable (`[A-Z_][A-Z0-9_]*`, not `PATH` or `HOME`; for
cargo `CARGO_TARGET_DIR`) the harness sets to `targets/lane-<id>` beside the ledger, the lane's
own build directory, never shared with another lane, whose builds would replace the binaries its
tests run; and `timeout_secs`, optional, 1 to 86400, default 1800, for the checkout, every
command and the checks after them together. Unknown keys in `[tests]` are refused. No profile,
no `[tests]`, no command or a bad key is `invalid_input` and writes nothing. A result already
stored under its key (events.md) is printed with `replayed` true and nothing runs.

Else every run starts from a fresh repository the harness owns at `checkouts/lane-<id>` beside
the ledger: whatever is there (a killed run's) is deleted first, then `git init` with hooks off
(`core.hooksPath=/dev/null`), the lane's commit fetched by sha from the lane's repository
(`git fetch --no-tags <path> <sha>`: no remote, no other ref), and checked out detached. The
whole directory is deleted again when the run ends, however it ends, and no git state is kept
between runs, as a test command can write the repository's `.git` (config, sparse checkout,
excludes) and so change what a later run would check out. One that cannot be deleted refuses
the next run. So the lane's hooks, config, filters, excludes and replace refs never reach the
checkout, and the lane's repository is only read. Every git call the harness makes sets
`GIT_NO_REPLACE_OBJECTS=1` and drops `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT` and the
`GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` variables, whose config would beat the repository's
own (the user's global config is still read). The build directory `targets/lane-<id>` is kept
between runs for speed: it is a cache an earlier commit's build could have written, a trade-off
under the same-user trust model. It and a leftover `checkouts/lane-<id>` (exactly `lane-<digits>`, a
directory, never a symlink) are removed once the lane is done or cancelled, or its current PR has a `pr.merged` whatever its
status, or for an id the ledger does not have, under that lane's run lock, skipping a lane whose lock is held: at the start of every
`check run` and `verify` (whatever is not removed is left for the next), or now with `check prune`,
which writes nothing to the ledger and prints `{removed: [path], skipped: [{path, why}]}`.

Each command runs in the checkout with stdin empty and the environment less the session's
variables, the ones that point git at another repository and the config variables above, in
its own process group: at the deadline SIGTERM, then the group killed 2 s later, and when the
command exits its group is killed too, so a process it left in its own group neither holds its
output nor outlives it (one it moved to another group or session is not reached). Only the last
16 KiB of each output stream is kept, and the run stops at the first command that fails. If all
pass, the checkout's `HEAD` must still be the commit, and `git status --porcelain
--untracked-files=normal --ignored` must show no tracked change and no new untracked file or
directory; new ignored entries (a whole ignored directory counts once) are counted, not failed,
and a listing past 1 MiB is not read and fails. These checks target accidental writes; an index
flag such as `assume-unchanged` hides a change from them. A failing command, a command killed by
a signal, a moved `HEAD`, a dirty tree, a cut listing and a timeout are a `check.finished` with
`result` `fail`; a command that cannot start, or a checkout that cannot be made (git's own
message is kept), writes nothing.

Each command runs as `nice -n 10 <argv>`, so the repo's commands run at low priority; the event
and evidence name the repo's argv. A program that is not a file with an exec bit (a path from
the checkout, else a name on `PATH`) is a command that cannot start; one that passes that check
and still fails to exec under nice (such as a script whose interpreter is missing) is nice's exit,
126 or 127, recorded as a failed run.

A `check run tests` (and the tests step of `verify`) that has to run takes one of the test
slots shared by every run on the same ledger (one per machine with the default ledger) first,
after the replay check and before its timeout starts, so the wait is not timed:
`<ledger>.slot-<n>.lock` beside the ledger, n below `CHAPERONE_SLOTS` (a whole number of 1 or
more, default 2; another value is `invalid_input`), each a file lock the OS releases when its
holder ends, so a killed run never keeps a slot. A run that finds every slot held writes one line
to stderr, `chaperone: lane <id> waits for a test slot (<n> held, by lanes <ids>)` (`?` for a
holder that has not written its id yet), before any error envelope, and tries again every
200 ms, in no first-come order. The slot is released once the commands and the checks after them
end, before the event is written; the event's `slot` has `slots` and `wait_ms`. The commit under
test is the one read before the wait, so a lane that commits while waiting has its earlier commit
tested and recorded. A replay and `check run smoke` take no slot.

`check run smoke` is `check run tests` on the base profile's `[smoke]` table, which has the
same keys and rules as `[tests]`, recorded as check `smoke` with the harness producer (events.md).
A profile with no `[smoke]` is refused (`invalid_input`, nothing written), never a pass: whether
a lane needs a smoke is a gate's to say, not this command's. The lane's stamped pipeline must
define the `verdict` check (v20 on); a lane on v19 is refused (`invalid_input`, nothing
written, the message says the lane records smoke with `check record`), as a harness `smoke`
would meet v19's `smoke_recorded` from the lane's own session. The lane's own account of its smoke stays a session's `check record
--check smoke`, with its evidence files.

`check run review` takes the same lock and reads the commit the same way. The lane's stamped
pipeline must name the review's checks, each with a runner (`[runners]`: the CLI, model,
tools, `tokens_limit` (Claude only), timeout and an optional `fallback`, another runner with none of its own) and a prompt file: `review_correctness`, `review_design_tests`,
`review_intent` and `review_codex` (the lenses) and `review_verifier`; a Claude runner must
name a model and a `tokens_limit`; and `[limits]` must hold `review_attempts`, 1 or more. v19 names none, so
a lane on it is refused (`invalid_input`, nothing written) and keeps recording reviews with
`check record` and `reviews record`. Each `[[review.required]]` lens of the base profile
(`name`, `when` globs, `prompt`, the repo's own question) whose `when` names a changed file is
one more lens on the `review_correctness` runner, never skipped for size, its row in `lenses`
under its name, so the review is not complete until it has run; a name that is a pipeline
lens's or `verifier`, or is given twice, is `invalid_input`, nothing written. With any, the
review's `<profile>` below is the sha256 of the pipeline's hash and each lens's name and
prompt digest, so a review from before the rule replays or carries nothing; with none it is the
pipeline's hash. Then it writes the rulings a crash left unwritten on the
lane's newest review, and a complete review already stored under its key (events.md;
`<profile>` is the hash above) is printed with `replayed` true and nothing runs. A
complete review by this pipeline on another commit of the same tree (an amend that kept the
tree), or whose diff against its merge base has the same `patch_id` (a clean merge or rebase
onto a newer base; events.md), is carried to this commit, `carried_from` (and `carried_by`
`patch_id` for the second) and no findings of its own (events.md), and no session starts:
the lenses would only word their findings anew. The `patch_id` is read in the lane's
worktree before anything runs (a diff or patch-id git cannot give is refused, `invalid_input`,
nothing written), three lines of context and submodule pointer changes shown whatever its diff
config or `GIT_DIFF_OPTS`, and any change to the lane's diff, whitespace or a file mode
included, gives a new one. A commit that already has it
replays it, so flipping between commits of a tree appends nothing after the first carry. Else,
on a pipeline whose `[limits]` holds `review_rounds` (v21: 3, 1 or more), a lane that already
has that many fresh reviews (complete harness reviews numbered `round`, not carried; one from
before the limit has no `round` and does not count; an incomplete one never counts, as it
leaves nothing to fix and `review_attempts` bounds it), or the newest `rounds_waiver`'s (the owner's or the harness's below)
`rounds`, is refused (`invalid_input`, naming the limit, "needs you", and `check waive
rounds`): two fix rounds, then the owner decides. Once per lane the harness grants one fresh
review more by itself instead (#341), when the limit is the pipeline's (no `rounds_waiver`, the
owner's or an earlier grant, sets it: the owner's limit is theirs) and the fresh review before the newest raised a blocking
finding and none of its findings still blocks (each ruled `fixed` or `refuted`, or `waived`,
deferred included): a harness `rounds_waiver` with `rounds` the limit plus one and `note`
"round <n> settled every blocking finding of round <n-1>", written only once the tree's
`review_attempts` refusal below has passed. `verify` stopping on findings that still
block at that limit adds "the lane is at its review_rounds limit of <n> fresh reviews: needs
you", or, when that grant is due, "... and has one more by itself: <note>". Else a tree that already has
`review_attempts` reviews by this pipeline, complete or not, is refused (`invalid_input`,
naming the limit): a lens that fails every time would otherwise make each rerun pay for the
others again, so the owner waives the findings or the lane pushes a commit that changes the
tree. Else it asks `--version` of each CLI a session on this diff uses (`claude`,
`codex`; a lens skipped for size, below, uses none), the first of that name on `PATH`, run by its full path, reads the commit's files as
`diff` does and their merge base in the lane's worktree, makes a fresh checkout as `tests`
does (removed when the command ends), and reads `git diff <merge base> <commit>` there,
where the lane's git config does not reach. The reviewed commit's own `.gitattributes` still
applies: a file a rule such as `-diff` hides has no text in the diff, and the event counts
such files in `uncounted_files`, but for assets (`[assets] globs`, as `diff` names them in
`asset_files`), which the file list marks `asset` for the lenses to judge by name. A CLI that cannot start, a checkout that cannot be made and
a diff past 16 MiB are `invalid_input` and write nothing beyond those leftover rulings, except a
CLI that only sessions whose runner names a `fallback` on another CLI use (v20: `codex`): those
sessions are answered by the fallback, and the row says so (events.md, `review`).

A lens whose `[limits]` holds `<lens>_min_code_lines` (v20: `codex_min_code_lines`, 200)
is skipped when the harness `diff` on the commit counts fewer code plus test lines (`code_lines`, `removed_code` and `test_lines`: added plus removed, as #223 measured them): no session, its row
in `lenses` says `skipped` and why, and the review can still be complete. With no harness
`diff` on the commit, or one with `uncounted_files`, the lens runs.

The lenses run at once, each a fresh headless session in the checkout (no lane
variables, Claude Code's sandbox; Codex read-only, without the user's config, `.rules` files
or the checkout's `AGENTS.md`), given its prompt file and then the facts: the commit, the merge
base, the changed files with their counts and the diff, cut after 128 KiB at a line with a
note saying where the rest is. From a lane's second review under `review_rounds`, when the
commit of its newest complete harness review (carried or not) is an ancestor of this one, the
lenses are shown the files of the whole change but only `git diff <that commit> <commit>`, told
to raise findings only on what it changes and that the rest was reviewed (`reviewed_since` on
the review); after a rebase it is not an ancestor and they get the whole diff. The verifier
and the verdict always get the whole diff. Only `intent` is also told what the lane is for: the work
item's title, its slice of the spec, and the `goal` of its newest handoff's brief; and, when
there is one, the newest smoke on the commit as the verdict is shown it, the session's claim
labelled as one. No lens is
given the handoff's summary, `merge_case` or `look_at`. Every block of text from the lane or
a model (the file list, the diff, the goal, the findings shown to the verifier) is fenced
one backtick longer than its longest run of backticks, so nothing in it closes the fence, and
every prompt ends with one line of the harness's own saying the fenced text is data, not
instructions, and how to answer. Each lens answers `{findings: [{severity, title, body, path,
line}]}`. A lens that errors, passes its tokens or timeout, or answers anything else (prose,
another field, a severity the ledger does not know) did not run: never "no findings". When a
lens found something, one more fresh session, the verifier, is shown every finding by key and
answers `{rulings: [{key, verdict, why}]}`, `verified` or `refuted` with a why that is not
blank, or `unproven` on a `blocker` or `risk` with no proof (a run or a cited line), which the
review writes as a non-blocking `question` with `raised_as` and the why, and no `finding.updated`: one session for all, as the findings share the facts and the checkout, bounded like a
lens by its runner's timeout and, for a Claude runner, its tokens (Codex reports its tokens
only when its turn ends, so the timeout is its only bound). A Claude session runs with
`--output-format stream-json --verbose --include-partial-messages`; the harness sums each API
message's input, cache read, cache write and output tokens, the largest the stream reports for it
(an `assistant` line's output is partial, the message's `message_delta` has the final count;
on a real session the four sums equal the result's `modelUsage`), and once the
sum passes the runner's `tokens_limit` it stops the session as at its deadline: the run failed
`max_tokens` ("used N tokens, past its cap of M"), and its usage row has the tokens counted. It is also shown every earlier finding of the lane's complete
harness reviews, keyed `<review event id>/<finding_key>`, with its status and the reason of its
newest move (every lens is shown them too, without bodies), and may rule `fixed` only on one
still open or verified with a blocking severity, and `verified` or `refuted` only on a
`disputed` one that is not the owner's (`owner_held` false: #339, `finding dispute`; on a
pipeline with `dispute_attempts`, v21 on, whose lenses alone are told to title a one-way-door
or fail-open finding `owner:`); a
verifier starts when there is such a finding, even if no lens found anything. It may rule a finding of this review `same`, with
`same_as` an earlier key that still blocks, as `findings.open_blocking` counts it (the
verifier is shown which do), or one of a blocking severity the owner `waived`: the review writes it as a non-blocking `question` with
`raised_as` and `same_as`, and no `finding.updated`, as the earlier finding keeps the point's
standing. Never against a refuted, fixed or non-blocking earlier one, so a lens that now proves
that point has it judged afresh.
One that fails, or rules on a key it was not shown, with a verdict that key does not take or
twice, or gives a blank why, rules on nothing, and every finding stays open (still blocking). Each
session's prompt and whole answer are stored first, as two evidence files.

It then writes, in this order: one `usage.reported` per session that left a session id
(events.md, Usage), one `check.finished` (check `review`: the lens table, the verifier,
`complete`, the findings), and one `finding.updated` per ruling on a finding still open (for
`fixed`, open or verified, with this commit as `head_sha`; for a dispute, still disputed, with
the verifier's why as `reason`; an earlier finding moved after the review's event is left for a
new ruling), carrying the review's harness producer, so the verifier rules whichever session started the
run (finding statuses). It is the second command that appends more than one event, after
`usage report`, each event in its own transaction. A crash leaves whole events: usage rows
without a review are what those sessions cost, and stay; a review whose rulings are missing
is finished by the next `check run review` on the lane before any review is replayed, carried
or run (only the merge base and `patch_id` are read first), with no session started. A review that is not `complete` runs again on the same tree, up to
`review_attempts`. It prints the `check.finished` as `check record` does.

`check run verdict` takes the same lock and reads the commit the same way. The lane's stamped
pipeline must name a `verdict` check with a runner and a prompt file (a Claude runner naming a
model); v19 does not, so a lane on it is refused (`invalid_input`, nothing written) and
keeps recording verdicts with `check record`. `[limits]` must hold `verdict_attempts`, 1
or more. The verdict runs only on a clean commit, so a NO can be neither bought nor caused by
running it early: the commit must have a harness `diff`, a newest harness `tests` that passed
(only on a pipeline with a `tests_passed` gate, v20; from v21 the repo's CI runs tests, no local
run is needed and the verdict is shown `tests: null`), a newest harness `review` that is `complete` (carried counts; a newer incomplete one uncovers an
older complete one) under the key `check run review` derives from the base profile now (the
pipeline's hash folded with the required lenses the change touches), so a review run while the
clone's base ref named another profile, the ref since restored, is not one (a ref left forged
through the verdict is #262), and, when the base profile declares
`[smoke]` (read as `tests` reads `[tests]`), a newest harness `smoke` that passed. Otherwise it
is refused, naming what is missing or failed, and nothing is written. Blocking findings do not
stop it: they are shown, and the verdict may say no. A verdict that ran, yes or no, stands for
(lane, tree, pipeline): on this commit it replays, on another commit with this tree it is
carried with this commit's fact ids (events.md, the key), and nothing runs. It also stands for
its diff's `patch_id` (read in the worktree as `check run review` reads it, refused the same
way): on a commit of another tree whose diff has it (a rebase onto a newer base that kept the
lane's patch), it is carried the same way, `carried_by` `patch_id`, when the commit's complete
review is the one the verdict read or a carry of it; a fresh review there means a fresh verdict. A NO is reopened
only by a waive or a refutation recorded after it, or by changing the code; nothing else the
harness shows reopens it, as the base and its profile come from the lane's own clone. Else a
tree whose verdict sessions gave no answer `verdict_attempts` times by this pipeline is refused
(`invalid_input`, naming the limit): the owner decides. Else it asks `--version`
of the runner's CLI, reads the change and makes a checkout as `review` does, and starts one
fresh session there, given its prompt file, what the lane is for (as the intent lens is
told), the change (as every lens is shown it) and the harness's results as JSON: the key fields
of the newest `diff`, `tests` and complete `review` on the commit, the findings on the lane
that still block, from every review (labelled so, with a disputed one read `open`: the dispute
is the lane's word), and the newest smoke on the commit: whether the profile declares one, the
harness's `check run smoke` when it does and, labelled a claim the harness did not check, a session's `check record --check smoke` on the commit (or a
short form of it) with its `run` and the last 4 KiB of each evidence file; each fenced, then the harness's
closing line. It is told nothing else the lane
wrote: no summary, `merge_case` or `look_at`. It answers `{decision, why, proven,
not_proven}`, `decision` `yes` or `no` and a why that is not blank. It writes a
`usage.reported` for the session, then one `merge_verdict` with the harness producer: `ran`
with the decision, or `skipped` with `reason` `no_answer` when the session errored, stopped or
answered anything else, never a default yes or no. A verdict stands for its tree: the same
tree and pipeline replay it, so a NO is reopened only by a waive or a refutation recorded
after it, or by changing the code, which is deliberate. It prints the `check.finished` as `check record` does.

`check run plan --file <plan>` takes the same lock and reads the commit the same way. `--file` is
required here and refused on every other check. The lane's stamped pipeline must name a `plan`
check with a runner and a prompt, and `[limits]` must hold `plan_attempts`, 1 or more (none
before v20, so a lane on v19 is refused and keeps `check record`). The commit must change
no code file against its base, as `diff` counts code (any file a `[diff] code` glob of the base
branch's profile names, or not under `docs/` or ending in `.md`, with or without counted
lines): the plan is attacked before code exists, so a commit with code is refused
(`invalid_input`, giving the count of code files and lines, the first three paths and the
owner's waiver), and a base profile `diff` refuses is refused here too. The plan file must be a
regular file (a link to one is followed; the type is checked before it is opened, so a FIFO is
refused, against accidents rather than a lane swapping the file in between) of at most 64 KiB of
UTF-8 text that is not blank; it may sit outside the worktree, where agents keep their plans.
All of these write nothing. The plan is named as the evidence store keeps it, redacted, and
that text is what the session is sent; a plan check that ran on the same tree, pipeline and
plan replays while it is the lane's newest, else is carried as a new `ran` naming it
(`carried_from`, events.md), and one whose sessions gave no answer `plan_attempts` times there is refused (the
owner can waive the check). Neither stores the plan, which is written to the store only when a
session runs on it. Else one fresh session in the harness's checkout of the commit (no diff is
read) is given its prompt, what the lane is for (as the intent lens), the plan, fenced, and the
commit and merge base, and answers `{findings: [...]}`
as a review lens does. It writes a `usage.reported` for the session, then one `plan_check` with
the harness producer (events.md): `ran` with the findings, or `skipped` with `reason`
`no_answer`, never "no findings". It prints the `check.finished` as `check record` does.

`check waive plan` is the way past the plan check for a lane that coded before its plan
was attacked: the owner's, or the lane's own (#340) when the check cannot run now. The lane's stamped pipeline must name a `plan` check (else `invalid_input`: a lane
on v19 records a skipped plan check with `check record`, and a waiver there would fail its
gate). It reads the commit the lane is on as `check run` does and appends one `plan_check`
`skipped` with `reason` `waived` and `note` the `--reason` (one non-blank line of at most 500
characters, redacted at write), producer `session`. From a session that has held the lane it is
refused unless the head's harness `diff` changes code (`commit.changes_code`; with no diff on the
head, refused: run `check run diff`) or the head's newest plan has had `plan_attempts` `skipped`
`no_answer` plan checks, and refused once the owner has overturned a plan waiver (`check overturn plan`).
The same waiver again replays (keyed as `check
record`, on the payload as stored). It waives the plan check only, and gates nothing by itself:
a gate reads it as `plan_check.reason` `waived`. It prints what `check record` prints.

`check waive size --lines <n>` is the owner's escape from the size cap for a lane whose change
only makes sense whole. It reads the lane's commit the same way and appends one `size_waiver`
`ran` with `lines` `<n>` (above 0) and `note` the `--reason`, producer `session`; the same waiver
again replays. From a session that has held the lane `--lines` is refused: the lane's own `check
waive size --reason <r>` (#340) records why its change is past the cap, a `size_waiver` with no
`lines` that lifts no gate, which `pr open` shows; from anyone else `--lines` is required. A lane
may not record one once the owner has overturned its size waiver (`check overturn size`).
`--lines` is refused with `plan` (`invalid_input`). It raises that lane's limit alone, for every commit:
`code_lines_at_most` and `pr open` read it as `commit.size_waived`. Uncounted files still refuse.
A pipeline with no `code_lines_at_most` gate (v21 on) needs none: past the cap its review asks
what to cut (`over_cap` on the review, docs/events.md).

`check overturn plan|size --work <id> --reason <r>` is new (#340): the owner's answer to a plan
waiver or size reason the lane wrote itself. It appends one `plan_check` or `size_waiver`
`skipped` with `reason` `overturned` and `note` the `--reason`, producer `session`, refused from
any session that has held the lane, and refused when the lane's newest `plan_check` or `size_waiver` is no waiver (nothing to overturn). The lane's own plan waiver counts the `no_answer` plan sessions of the head's newest plan (its `plan_ref`), as the plan run's limit is per tree and plan. A plan overturn fails `plan_or_no_code` again
(`plan_check.reason` is `overturned`); a size overturn drops an earlier `--lines` raise, so
v20's `code_lines_at_most` applies again. After either the lane may not waive that check again.
It prints what `check record` prints.

`check waive rounds --rounds <n>` is the owner's raise of the cap on fresh reviews
(`review_rounds`, v21: 3) for one lane, after reconsidering its approach. It appends one
`rounds_waiver` the same way (`rounds` `<n>`, above 0), refused from any session that has held
the lane; the newest one, the owner's or the harness's extra round, is that lane's limit, in place of the pipeline's. `--rounds` is
required with `rounds` and refused with `plan` and `size`, as `--lines` is refused with `rounds`.

On v21 `done_verdict` is also met by `pr_merged`: the lane's current PR has a `pr.merged`
(`pr.merged`), so after an owner merge with no verdict on the head the lane still hands off done.
That `phase.entered` carries `merged_without_verdict: true` (events.md, Engine).

`verify --work <id>` is what a lane runs in the background: `check run diff`, `tests`, `review`
and `verdict` on the lane's commit, in that order, each exactly as that command runs it (the
same events and keys, so a step on the same tree replays and starts no session). It takes the
lane's run lock once for all four, so a second `verify` or `check run` on the lane meanwhile is
`conflict` (exit 4) and writes nothing. It stops at the first step that is refused
`invalid_input` (nothing more written), whose `tests` did not pass, whose `review` is not
complete or leaves blocking findings as the gate counts them (`findings.open_blocking`, complete
harness reviews only, so a plan check's finding does not stop it; the verdict does not run), or
whose verdict is not yes. Before each step after `diff` it reads the commit the lane is on again,
and stops if the lane moved off the commit `diff` measured. Any other refusal (a lane that does
not exist is `not_found`) or error is the command's, with its exit code, as for `check run`.
Plan and smoke are not steps: the plan check runs before code, and the smoke is the lane's claim
or the profile's `check run smoke`. Like `usage report` it appends more than one event, each
step's in its own transactions, and none of its own. It prints `{work_id, steps, stopped}`:
`steps` is `{check, head, event_id, replayed, passed}` per step that wrote or replayed (`head`
the commit it ran on), plus `carried_from` and `carried_by` when the step's event carried
another commit's result (`carried_by` `patch_id` for a rebase that kept the patch; absent for
an amend that kept the tree), which say why no session ran, and `stopped` is null when all four passed, else `{check, why}`: the
refusal's message, why the step did not pass, or `the lane moved from <sha> to <sha>`.

`pr open --work <id>` opens the lane's PR under the lane's run lock (a second run is
`conflict`). It refuses (`invalid_input`, nothing pushed, gh not called) a worktree without the
lane's branch, with any change or untracked file, or not at that branch's tip; a lane with no PR
whose `work update` the ledger would refuse (a dry run); a lane that moves while it runs; a
`check run diff` past the pipeline's `[limits] code_lines` (the number `code_lines_at_most`
reads; added code lines) unless the owner's size waiver covers it (`commit.size_waived`) or the
pipeline has no `code_lines_at_most` gate (v21 on, whose review asks what to cut instead), with an uncounted file (an asset is none) or missing a count (no limit, no refusal); a base that is no
`origin/` branch; and `check run tests` that did not pass. Diff and tests run as `check run`
runs them, so a pass on the same tree replays. A lane with no PR first asks `gh pr list --head
<branch>` for an open one a run opened and never recorded. Then `git push origin
<head>:refs/heads/<branch>`, and `gh` (found on `PATH`, run in the worktree, 120 s per call,
the body on stdin): `gh pr create --base <base> --head <branch> --title <t> --body-file -
[--draft]` (the title defaults to the item's), or for a held or listed PR `gh pr edit <n> --repo
<owner/repo> [--title <t>] --body-file -`. The body is ledger facts only: the newest handoff's
goal or the title, the tracker key, for a GitHub lane (`source_id` `owner/repo#N`) `Closes
#N` when the PR is in the issue's repo, else (another repo, or no PR yet) `Closes
owner/repo#N`, the diff's code lines and files (and how many are assets, when any), the test commands that
passed on the head, the plan check's outcome, the head's smoke (the harness's result, a
session's claim and its evidence file count), the lane's tokens as `stats` totals them (fresh
input, cache write, cache read and output, `unknown` for a kind no CLI reported, and the session
count; `none recorded` with no usage), and the findings of the complete review covering
the head (`review.event_id`) that are open, verified or disputed and do not block, as open (severity, title, path
and line). A PR new to the lane is `pr.opened` with
`head_sha` the head, as `work update --pr` records it. A gh failure is `internal`. It prints
`{work_id, pr, head, created, steps}`, `steps` as `verify`'s.

A repo's PR rules are the `[pr]` table of the base profile (read as `check run tests` reads it, never the
worktree): `title` (a template), `types`, `head`, `assignee` and `reviewers`, beside `flow`,
`create`, `address`, `merge` and `stack`, which are accepted and not read. Another key, a `title`
without exactly one `{text}`, with a second `{type}` or `{key}`, any other brace, or naming
`{type}` with no `types`, is `invalid_input`. A title fits the template when `{key}` is the
item's `source_key` exactly, `{type}` one of `types`, `{text}` non-empty with no space at either
end, and the rest literal. With `title` or `head` set, a lane with no `source_key` is refused;
with `title` set, a `--title` that does not fit is refused, and so is no `--title` while the lane
has no PR recorded (a type is never guessed), all right after the clean-worktree check, before
any gh call or push. A lane holding a PR and passing no `--title` reads it with `gh pr view <n>
--repo <owner/repo> --json title --jq .title` and is refused unless it fits, else it is kept.
The `head` lines, `{key}` filled, then a blank line, lead the body on every rewrite. When the
lane has no PR recorded, the create gains `--assignee <assignee>` and the edit of a listed PR
`--add-assignee`, when set; an edit of a PR the lane holds sets none. `pr open` never requests
`reviewers`: requesting a review notifies, so the ready handoff does (`work handoff`). Its
`git status` gets 120 s, the push's limit (a large repo under load passed 10 s). With none of
these keys, the gh calls and the body are as above.

`merge --work <id> [--head <sha>]` is the owner's merge of the lane's PR (its `pr_ref`). `--head` is the head the owner confirms; it defaults to the newest
handoff's commit. It refuses (`invalid_input`, no `gh pr merge`) a session or an agent that has
claimed or taken over the lane, a lane with no PR, a verdict that is not YES or not on the handoff
head, a handoff head that is not `--head`, a lane that is not ready (unless its merge is
recorded), and a PR that GitHub (`gh pr view`) has at another head, closed, or open though
recorded as merged. Then `gh pr merge <n> -R <owner/repo> --squash --delete-branch
--match-head-commit <head>` (found on `PATH`, run in the temp dir, 30 s per call, never `--admin`
or `--auto`), and `pr.merged` as `work merged` records it. A PR GitHub has merged at that head is
only recorded, so a second run merges nothing. When the lane's lease is not live it then hands
off `done` itself. A gh failure, or a done handoff the ledger refuses after the merge is
recorded, is `internal`. When the lease is live it instead tells the lane's recorded agent to
hand off done: `herdr agent prompt <agent> <text>` (found on
`PATH`, 30 s), the text naming the PR, the merge commit and the item's `source_key` (else
`work <id>`). Best effort: a prompt that fails is reported, never an error after the merge. It
prints `{work_id, pr, merge_sha, done_by, told, tell_error}`: `done_by` is `merge` (this run
handed off done), `already` (the lane was done) or `lane` (its lease is live, so the lane hands
off done itself, and a run after the lease lapses finishes it); `told` is true when the prompt
reached the agent, and `tell_error` says why it did not (else null).

`install` writes the Claude Code plugin embedded in the binary (every file git tracks under the
repo's `plugin/`, listed by `build.rs`, executable ones mode 0755) as a one-plugin marketplace named `chaperone` in
`<data dir>/chaperone/marketplace/`: `.claude-plugin/marketplace.json` and the plugin under
`chaperone/`, whose `plugin.json` gets the version `<binary version>+<7-char sha>`. It writes
a fresh tree beside the directory and renames it into place, so a file the binary no longer
embeds goes and a session never reads a half-written plugin. Then `claude plugin
marketplace add <dir>` and `claude plugin install chaperone@chaperone --scope user` (`claude`
found on `PATH`, 120 s per call); a non-zero exit is `internal`. Both pass when already done,
and Claude Code loads a plugin from a local marketplace in place, so a second run is the
update. `uninstall` runs `claude plugin uninstall chaperone@chaperone --scope user` and
`claude plugin marketplace remove chaperone`, whose exit is reported and never an error (they
exit 1 when nothing is installed), then deletes the directory. Neither reads the ledger or
Claude Code's settings. They print `{dir, plugin, version, ran}` (`uninstall` without
`version`), `ran` one `{args, exit, output}` per call, `output` its stderr or else its stdout.

`hook <event>` is the plugin's Claude Code hook (`plugin/hooks/hooks.json`, run through
`chaperone-hook.sh`, which is silent without `$CHAPERONE_WORK`, without `chaperone` on `PATH` and
when the binary fails, so it is what keeps a hook silent). The lane is `$CHAPERONE_WORK`; when it
is not a number, or the lane is unknown, done, cancelled or on a pipeline before schema 2 (v20),
it prints nothing and exits 0, as on most errors. It prints the hook's own JSON, not an envelope,
and opens the ledger read-only for every event but `stop-failure` and `prompt` (and `stop`'s usage write, below). `session-start` prints `additionalContext`: the lane's id, title,
status, pipeline, branch, head, PR, the ready gates not passing yet (as the lane page reads them)
and the lane's commands. `pre-bash` splits a Bash `tool_input.command` at `&&`, `||`, `;`, `|`, newlines,
`$(`, parentheses and backticks, drops each part's leading `VAR=value` words, and denies a part
that starts `gh pr merge` or `chaperone work ready` with `--yes` later in it, and on a lane not on
v21 one that starts `gh pr create` or `gh pr edit`, naming the command to run instead. A v21 lane may
open or edit its PR with gh when `pr open` will not; it records the PR in its ready handoff's
`pull_request`. `stop` blocks once (never with `stop_hook_active`) when the newest handoff is ready and
the branch tip, read as a ready handoff reads it, is not its commit. After printing its answer, `stop` records the lane session's tokens, every turn, so the lane never has to: `usage report --work <lane>`'s write, on its own writable open, reading only the folder of the hook's `transcript_path` (that session's project folder, its sessions matched by first `cwd` as always, and their subagents), since reading every transcript under `~/.claude` can pass the hook's 10 s timeout. It waits at most 5 s for it (a busy ledger is left, with whole events written, and the next turn adds the rest). Without a `transcript_path` it records nothing, any error is ignored, and with `DAISY_CLI_READONLY=1` it skips this write and stays a read. `stop-failure` records
`attention.recorded` type `stopped`, by `orchestrator`, its note the hook's `error`, as
`$DAISY_AGENT` and `$DAISY_SESSION`; a repeat with nothing new on the lane replays, as `attention
record` without a key does. `prompt` (Claude Code's `UserPromptSubmit`) records a prompt typed
into the lane, by herdr or by hand, as `attention.recorded` type `prompted`, its note the
prompt's first line, control characters (a pasted tab) as spaces, cut to 500 characters, by `orchestrator` when the prompt starts with
`Orchestrator` (the orchestrate skill's prefix; a best guess), else `owner`. It skips a prompt
that is a slash command (a first word starting with `/` and holding no other `/`: the spawner's
`/chaperone:lane`, not a pasted path) or starts with `Owner (` or `Owner check (` (an owner's
answer or send-back, which records its own attention, and `merge`'s notice) or `<task-notification>` (Claude Code's notice that a background task ended). It is a raw count of prompts: an orchestrator prompt the orchestrate skill also records
(`nudged`, `sent_back`, `answered`, `revived`) is counted under both types, by design. Each prompt is its own record (keyed by the lane and the time it was written), so the same text typed twice counts twice.
Both are writes, so `DAISY_CLI_READONLY=1` refuses them as any other.

`reviews record` reads `daisy-cli reviews get <job>`. That is the CLI's only call into
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
Findings come from the stored payload, and each one's `outcome` is its row's status, null
while `open`. `reviews record` prints the `reviews get` answer.

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

## `review eval`

`review eval` scores the reviewer on a checked-in set of past changes (`evals/review.toml`,
or `--set`): each `[[entry]]` names `repo`, `base` (its merge base), `head`, `goal` and,
when it has a bug the review must find, `bug = {paths, words, what}`, and optionally `known`, a
list of the same shape for real leftovers a review may find (an older binary refuses a set that
has it). For each entry the
newest embedded pipeline's lenses, and the `[[review.required]]` lenses the clone's base profile requires of the change, run in a fresh checkout of `head` fetched from
`<--repos>/<repo name>` (`checkouts/eval-<pid>-<name>` beside the ledger, removed after),
with the entry's goal for the intent lens, no smoke, plan or earlier findings, and lenses
skipped for size as a lane's are; then the verifier on what they found. A finding stands
when its severity blocks and the verifier did not rule it `refuted` or `unproven`. A
standing finding one of the entry's `known` matches (as `bug` matches) is listed under
`known` with that one's `what` and not scored. The rest decide the run: `hit` (a standing
finding on one of `paths` whose title or body has one of `words`, any case), `miss`,
`false_alarm` (a clean entry with a standing finding), `convention` (a clean entry whose
standing findings are all the `design_tests` lens's), `clean`, `skipped` (no clone, or a
commit the clone lacks) or `error` (a session did not run). Each entry is reviewed `--runs`
times (default 1) in one checkout. `data` is `{pipeline: {id, hash}, runs, entries: [{name,
run, outcome, why?, findings, known, bug, fallbacks, tokens}], per_entry: {name: {outcome: k}},
totals, tokens, score, mean, unused_known}`: a row per entry per run (`run` from 1, `bug` the entry's `what`, null
for a clean one, `fallbacks` the sessions another engine answered, a lane review's
`fallback_from`, `tokens` what that run's sessions used: `uncached_input`, `cache_write`,
`cache_read` and `output` summed over its lens, fallback and verifier sessions, null when none
reported it (Codex reports no `cache_write`), and `sessions`; a `skipped` row has none),
top-level `tokens` the same summed over every row, `per_entry` each entry's outcomes as k of `runs` and its `fallbacks` when
any, `totals` over every row with `fallbacks` summed, `score`
hit plus clean over the rows scored (`convention`, `skipped` and `error` are not), and
`mean` the score of one run, (hit + clean) / rows scored x entries not skipped, to two
decimals; `unused_known` lists `{name, what}` for each `known` that matched in no run of an
entry with a run that was reviewed (not `error`), so a matcher worded out of date shows. It never opens the ledger; prompts and
answers go to the evidence store beside it. `--only` with no such entry is refused
(`invalid_input`), as is a set that does not parse, a `bug` or `known` with no `paths`, two entries with one name or two `known` of an entry
with one `what`, or a CLI the pipeline uses that cannot
start.

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
| `finding dispute`, `finding waive`, `finding accept`, `finding answer`, `finding overturn` | ungated | `finding.updated` |
| `work heartbeat` | none | none |
| `migrate` | none | none (schema only) |

A gated command on a lane with no stamp (never claimed) is denied with gate `stamp_ok` and fact path `lane.pipeline_hash`. It is never ungated and never falls back to the latest pipeline. A gated command whose route has no matching transition is denied the same way, never passed.

`handoff` is the M1 gated transition. Its requires list is per route in
`pipelines/v19.toml` (#105). A handoff with `ready: true` on
`code_pr` or `direct` needs a `plan_check`, a `quality_gate`, a `merge_verdict` with
decision `yes` on the handed-off sha, and from v19: on a `code` lane a `smoke` and a
`kept_tests` check recorded (ran, or skipped with an allowed reason; denied as
`smoke_ok`, `kept_tests_ok`), and at most 3 review rounds, `local_review`s that ran plus
`review_round`s (denied as `rounds_at_most`). These v19 gates are on ready only: the
done handoff below is unchanged, so they do not bind a lane that goes to done, nor a
`no_pr` code lane, which has no ready row. No gate reads CI. The handed-off sha is
`handoff.commit`, or for `update --status done` the commit of the lane's latest
handoff. The two shas match when the shorter, at least 7 hex characters, is a prefix of
the longer, whichever side it is on. A
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

**Pipeline v20** (`pipelines/v20.toml`, for lanes already stamped with it) gates the same two `handoff` rows on harness facts only (events.md,
facts for pipeline v20), each about the handed-off commit. It has no other transition:
the write runs only `handoff`, so v19's other 20 rows could never run. The ready row's
gates, in the order a denial names the first that fails:

| Gate | Holds when | Facts a denial lists |
| --- | --- | --- |
| `stamp_ok` | the lane is stamped `v20` | `pipeline.id` |
| `no_uncounted_files` | a harness `diff` on the commit has no file git counts no lines of, but assets (`asset_files`) | `commit.uncounted_files` |
| `code_lines_at_most` | to v20 only: that diff adds at most 400 code lines (removed lines and test lines, `[size] tests`, not counted), or at most the owner's `check waive size --lines` for the lane | `commit.code_lines`, `commit.size_waived` |
| `tests_passed` | to v20 only: the newest harness `tests` on the commit passed. v21 leaves the repo's tests to its CI, so `verify` and `pr open` run none | `tests.passed` |
| `review_complete` | a complete harness review covers the commit (one carried to it from a commit with the same tree or `patch_id` counts) | `review.complete` |
| `no_blocking_findings` | no complete harness review on the lane, of any tree, has a finding open, verified or disputed of severity `blocker`, `risk` or another blocking one; refuted, waived, fixed and answered findings, incomplete and session reviews' and the plan check's never count | `findings.open_blocking` |
| `plan_or_no_code` | the diff changes no code, or the plan check ran (`check run plan`, before code) or the owner waived it (`check waive plan`) | `commit.changes_code`, `plan_check.outcome`, `plan_check.reason` |
| `smoke_or_no_code` | the diff changes no code, or the newest harness smoke on the commit passed, or the harness ran none there and a session's smoke claim on it ran with evidence (and, when it carries a `run`, exited 0) | `commit.changes_code`, `smoke.passed`, `smoke.claimed` |
| `verdict_is_yes` | the newest harness verdict says `yes` | `verdict.decision` |
| `verdict_on_head` | that verdict is on the commit (a carried verdict is on the amend it was carried to) | `verdict.sha`, `commit.sha` |

The done row (every route) needs `stamp_ok`, `plan_or_no_code` and `done_verdict`: the
diff changes no code, or a harness `yes` on the commit (`commit.changes_code`,
`verdict.decision`, `verdict.sha`, `commit.sha`). Whether a lane changes code is the
harness diff's word, not its declared kind, so every v20 lane needs a harness `diff` on
the commit to be ready or done, and a docs lane whose diff touches code climbs the full
row. Ready is derived on the board as for v19: a later fact that breaks a gate (a newer
`no` verdict, a newer failing run) takes ready back with no write.

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
(`parent`, `subagent`, and `harness` for the sessions `check run review` starts), and `sources`
has one row per transcript or harness session (`source`, `cli`, `session_id`, `agent_id`,
`agent_type`, `description`, `models`). Both carry `messages`, `peak_input_total` and the
sums of `uncached_input` (fresh input: `input_total` less `cache_read` where the input includes
the cache, as Codex's does, so a Codex event recorded before #217 counts right), `cache_read`,
`cache_write`, `output`, `reasoning`, `input_total` and `output_total`. No dollar figure: an old
event's `cost_usd` is not read. A sum skips the messages where that field is null, and is
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
  is the command's). `gate.denied`, `check.*`, `finding.updated`, `usage.reported`, `pr.merged` and `attention.recorded` are not lane actions
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
