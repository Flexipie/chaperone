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
| `pipeline_unknown` | 1 | `pipeline_id`, `pipeline_hash` | Claim or transition on a hash this binary cannot load. Writes nothing. |
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

## Commands in M1

Every write except `migrate` also takes `--agent` and `--session`, and every write but `migrate`, `check record`, `check run`, `check waive`, `verify`, `pr open`, `merge`, `reviews record`, `usage report`, `work merged`, `attention record` and `dossier note` takes `--dry-run`. `[x]` is optional.

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
| `overlap` | `[--repo <owner/repo>] [--uncommitted]` | read |
| `stats` | `--merged-since <date> \| --work <id>...` | read |
| `impact` | `<files>... [--cache <file>] [--entry-points [--profile <file>] [--declared <area>,...]]` | read |
| `pipeline current` | | read, no ledger |
| `pipeline history` | `[--dotfiles <dir>] [--repo <owner/repo>]` | read |
| `doctor` | `--skills <dir\|file>... [--repo <dir> [--profile <file>]]` | read, no ledger |
| `spec create` | `--parent <key> --file <path\|-> [--title]` | write |
| `spec get` | `[<spec>] [--key <slice key>] [--version <n>]` | read |
| `spec list` | | read |
| `spec update` | `<spec> --file <path\|-> --reason <r> [--expected-version <n>]` | write |
| `spec approve` | `<spec>` | write |
| `spec decide` | `<spec> [--slice <n>] [--key <k>] --question <q> --answer <a> [--where <w>]` | write |
| `check record` | `--work <id> --check plan_check\|quality_gate\|merge_verdict\|smoke\|kept_tests\|review_round --head <sha> --outcome ran\|skipped [--reason <r>] [--decision yes\|no] [--findings <n>] [--evidence <file>]...` | write |
| `check run` | `diff\|tests\|review\|verdict\|plan\|smoke --work <id> [--file <plan>]` | write |
| `check waive` | `plan --work <id> --reason <r>` | write |
| `check prune` | | read, removes directories |
| `verify` | `--work <id>` | write |
| `pr open` | `--work <id> [--title <t>] [--draft]` | write, pushes, calls gh |
| `merge` | `--work <id> [--head <sha>]` | write, calls gh |
| `reviews record` | `--work <id> --job <daisy job id>` | write |
| `reviews get` | `<job id> [--wait] [--timeout-secs <n>]` (accepted for the path swap; a recorded job answers at once) | read |
| `jobs get` | `<job id>` | read |
| `usage report` | `--work <id> [--claude-dir <dir>] [--codex-dir <dir>]` | write |
| `usage get` | `<id>` | read |
| `attention record` | `--work <id> --type asked\|answered\|nudged\|sent_back\|revived --by orchestrator\|owner [--reason <R12 id>] [--round <k>] [--answers <event id>] [--note <one line>] [--idempotency-key]` | write |
| `finding dispute` | `<event> <key> --reason <r>` | write |
| `finding waive` | `<event> <key> --reason <r>` | write |
| `dossier note` | `<key> --note <text> [--link <kind>:<ref>]... [--supersedes <event id>]` | write |
| `dossier get` | `<key>` | read |
| `migrate` | (uses `--db`; no actor flags) | write, no event |

`work create --kind` and `--pr-flow` are Chaperone's two additions to daisy's argv (#7). Both are optional and stored on `lane.created`. `--kind` defaults to `code` and `--pr-flow` to `pr`, so a skill that passes neither gets the strictest route. #14 reads them at the first claim, when the route is stamped.

`chaperone migrate` is the only command that creates the ledger or migrates it (`Ledger::open_and_migrate`, with a `VACUUM INTO` backup before an upgrade, #4). It creates a missing or empty file. It refuses a non-empty file whose `application_id` is not Chaperone's, a file that is not SQLite, and a version-0 file that already has tables, so it never applies 0001 over someone else's database. Every other command opens with `Ledger::open`, which never creates or migrates and refuses a mismatch with exit 3. Migration 0001 sets `PRAGMA application_id`, and open checks it, so a foreign SQLite file at version 1 is refused (#7).

`work handoff` reads the JSON from `<file>`, or from stdin when the file is omitted or `-`. Its optional `areas` (#46) is what the lane says it changes: names from the profile's `[areas]`, at most 20 of at most 100 characters each, not checked against the profile (the ledger reads none). An empty list is refused, so a handoff either declares areas or leaves the field out (not declared); the newest handoff is the lane's declaration, so a later one that leaves it out declares nothing. A handoff with `ready: true` or to `done` runs the `handoff` gate (commands and transitions below). Before a handoff that enters ready (`ready: true` and a `status` other than `done`) is written, the CLI reads, in the lane's `worktree_path`, the tip of the lane's `branch` (stack layers share one worktree, so `HEAD` may be a later layer's), or `HEAD` when the item names no branch or the worktree does not have it. The write refuses (`invalid_input`, nothing written, before the gate) a `commit` that is missing or is not that commit or a 7 to 64 character prefix of it, so the checks the gate reads are checks on what the branch holds. A handoff to `done` is not checked (after a merge the branch may have moved on), nor is a lane with no worktree, a relative `worktree_path`, or a directory that is gone; a worktree that is there and cannot be read refuses. The ledger makes no git call: other callers of the write pass the head they observed, or none. The write itself refuses a `lane.handed_off` that says ready and any event but `phase.entered` that moves a lane to `done`, so no path skips the gate.

`work merged` is new (#59). `/merge` runs it after the merge, from any session (it skips the
holder rule and leaves `updated_at` alone, as `check record` does). It appends one
`pr.merged` (events.md, PR). It refuses (`invalid_input`, nothing written) a `--pr` that is not
`owner/repo#N` or that the item has never held, a PR already merged on another item, a sha that
is not 7 to 40 lowercase hex digits, and a `--merged-at` that is not RFC 3339 UTC with
milliseconds (GitHub's `mergedAt`, `2026-11-05T10:00:00Z`, is passed as
`2026-11-05T10:00:00.000Z`). The same PR again replays. It prints `{event_id, work_id, kind, replayed, at,
payload}`.

`attention record` is new (#59). `/orchestrate` and the owner run it for each intervention on
a lane, from any session (it skips the holder rule and leaves `updated_at` alone). It appends
one `attention.recorded` (events.md, Attention). It refuses (`invalid_input`, nothing written)
an `asked` without `--reason`, a `--reason` on another type, a `sent_back` without `--round` of
1 or more, an `answered` without `--answers` and `--note` or whose `--answers` is not a question
asked on that item, `--answers` or `--round` on another type, and a note that is blank, has a line break or is
over 500 characters. `--idempotency-key` is trimmed (blank is no key, over 200 characters is refused, as for
`work create`) and scoped to the item and type, so the same key on another item, or on a
question and its answer, is a separate record. Without `--idempotency-key`, a record repeated with nothing new on the lane
replays; a real repeat (a second nudge on a stalled lane) passes its own key. It prints
`{event_id, work_id, kind, replayed, at, payload}`.

`work update` keeps daisy's owner rule (a live lease held by another pair refuses everyone
else) with one exception, the owner's pause from chaperone-ui (#119): agent `owner-ui` may move
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
rules on it. `waive` moves an `open`, `verified` or `disputed` finding to `waived` and is the
owner's: refused from any session that has held the lane, as `work ready` is. `--reason` is
required, one non-blank line of at most 500 characters (as `attention record --note`), and
redacted. A move the table does not allow, the wrong session or a bad reason is
`invalid_input`, and an unknown finding is `not_found` with `details.reference`
`<event>/<key>`; nothing is written. The same move again (a retry after a lost answer) is
refused as `finding <event>/<key> is already <status>`, not replayed. Both print
`{event_id, work_id, kind, replayed, at, payload}`.

`dossier note` and `dossier get` are new (#83, [ADR 0007](decisions/0007-issue-dossier.md)).
`<key>` is `owner/repo#N` or `TEAM-123` (`normalize_key`); a bare `#N` is `invalid_input`.
`dossier note` appends one `dossier.noted` (events.md, Dossier) on the key, from any session; it
names no lane, so it never changes a lane's `updated_at`. It refuses (`invalid_input`, nothing
written) a blank note or one over 600 characters, more than 5 `--link`, a link whose kind is not
`issue`, `pr`, `adr`, `doc` or `commit` or whose ref is not an identifier of that kind (a tracker
key, `owner/repo#N`, a repo-relative path with no `..`, 7 to 64 hex digits; at most 300
characters), and a `--supersedes` that is not a note on the same key. The note is redacted at
write; a secret in a link is refused. The same note again from the same session replays, unless
another note on the key came in between that says something else. It prints `{event_id, kind,
replayed, at, payload}`.

`dossier get` reads in one transaction and prints `{key, slice, lanes, handoffs, files, notes}`.
`slice` is `{in_spec, parent_of, omitted}`: the slice whose heading names the key (its spec, title
and the decisions `spec get --key` shows, each as its line) and the spec whose parent the key is
(title, version, slice count, decisions), either null. `lanes` is every work item whose `linear`,
`issue` or `github` source id is the key, ignoring case: `id`, `title`, `status`, `branch`,
`pr_ref`, `agent`, `pipeline_id`, `created_at`, `updated_at`. `handoffs.items` are each lane's
handoffs (`work_id`, `at`, `status`, `ready`, `next_action`, `commit`, `pull_request`, `summary`)
and `handoffs.decisions` the brief's decisions of each lane's newest handoff. `files` are the
newest handoff's `changed_files` per lane, with the lane and the handoff's `commit`. `notes` are
the key's notes no later note supersedes: `id`, `at`, `agent`, `session`, `note`, `links`,
`supersedes`. Lanes and items in them come newest first. Caps: 10 lanes, 5 handoffs and 5
decisions per lane, 20 spec decisions, 100 files, 20 notes; every string at 300 characters, a note
at 600. The whole answer as printed (the pretty envelope, `meta` included) is at most 48,000 bytes,
cut from the end of notes, then files, handoff decisions, handoffs and lanes; the slice is never
cut. Each section's `omitted` counts what it left out by list (`slice_decisions` and
`spec_decisions` in `slice`), and `strings` the strings it cut short. A key with no slice, spec, lane
or note is `not_found`. Notes are someone else's words: no gate reads them.

`work ready` is the owner's override. It is refused (`invalid_input`) from any session that has held the lane, the same honor-system rule as `check record`. It answers daisy's way: `{id, ready, ready_reason}` from the board (read for that item only, without `overlaps`), or `{id, ready_override, on_board: false}` for a done or cancelled item.

`work board` is daisy's board shape, `{specs, other}`, one read statement. Its rows are written as they are read to an unlinked, owner-only temp file, so the CLI never holds the whole board; stdout gets the file once the read has ended, so a slow or stalled reader never holds a WAL snapshot, and a ledger error leaves stdout empty. It needs a writable temp dir (`TMPDIR`); without one it fails `internal` with `temp file: ...`. Every open item (not `done` or `cancelled`) is under `other`, ordered by `lane_key`, `lane_pos`, `id`; `specs` stays empty until items are grouped by spec. Each entry is `{item, handoff, ready_override, ready, ready_reason, overlaps, overlaps_omitted}`: `item` is `work get`'s, `handoff` the newest `work handoffs` row or null, and `ready_override` the newest `work ready` as `{ready, reason, by, at}`, null when there is none or it was cleared. The ready rule is daisy's first steps: the override wins (`marked ready by <agent>: <reason>`); else an item not in `review` is not ready (`status is <s>, not review`); else it is ready only when its newest handoff says ready (`the lane says ready`, else `the lane has not said ready`). A ready handoff exists only past the `handoff` gate, so a later `needs you` handoff unsays it. Ready is also derived at read time: for an item the rule would call ready on the lane's word, the board evaluates the `handoff` gate again on the handoff's `commit` with the facts the lane's events project now, so a record written after the handoff (a NO verdict on that commit, say) unsays it as `the lane said ready, but <gate> no longer holds`. Every gate of the lane's ready row is asked, so a verdict recorded on a newer commit (even a yes) or a review round past the cap unsays it too, and a lane stamped with a pipeline this binary cannot load reads `the lane said ready, but this binary does not know pipeline <id>`. That read writes nothing (no `gate.denied`), leaves the lane's `phase` alone, runs only for those items, and makes no git call. The override is not re-evaluated. Daisy's further steps, a finished review on the PR head and no open thread from a person, need the PR and are the caller's to check; the board does not notice commits pushed after the gated handoff unless a check was recorded on them. `lanes` checks threads for every repo but the review on the head only for repos daisy does not review, so before it reads this board for a daisy-reviewed repo it has to check the review on the head there too. Daisy's other board fields (`pr_head`, `pr_state`, `pr_review`, `local_review`, `linear_outbox`, `threads`, `threads_checked`, `needs_you`) are not emitted.

`overlaps` (#42) flags two open lanes touching the same file: each path this lane's newest handoff lists in `changed_files` that another open lane's newest handoff in the same repo (`repo_slug`, any case) also lists, as `{path, work_ids, work_ids_omitted}`: the other lanes' lowest 10 ids and how many more touch it, sorted by path, the first 50 paths, the rest counted in `overlaps_omitted`. The read groups lanes by path, so lanes sharing a path cost it linear work, not a row per pair. Paths are compared as given, less a leading `./` or the lane's `worktree_path`. A lane with no `repo_slug` is never compared. It reads handoffs only, in the same statement, never git, and lanes usually hand off late, so an empty list does not mean no overlap: `overlap` adds what each worktree holds now.

`overlap` lists, per file, the open lanes touching it, so an agent or the orchestrator can call it before starting work (and `/ship` before ready). It reads the board (one snapshot) and only after that read has ended runs `git` in each lane's `worktree_path`, so a slow git never holds the ledger. Sources per lane: its newest handoff's `changed_files` (`source: handoff`, `commit` the handoff's), and its worktree (`source: worktree`, `commit` its `HEAD`): the paths its own commits touch since the merge-base with the first of `origin/HEAD`, `origin/main`, `origin/master` (as remote-tracking refs, never a local branch of that name; else the claimed `base_sha`), one `git log --no-renames` per lane (one more for each base that does not resolve, up to 4) that reads commits only, so both sides of a rename, and not what the lane merged from the base. `--uncommitted` adds one `git status` per lane for uncommitted and untracked (not ignored) paths; it stats every tracked file, so it costs about 4x at 50 lanes on a 400-file repo and grows with the repo. Git runs as the binary its `--exec-path` names (macOS's `/usr/bin/git` shim adds about 9 ms a call), with `GIT_OPTIONAL_LOCKS=0`, so it never takes the lane's index lock, `LC_ALL=C`, and without the variables that point git at another repository (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and the like, which git sets in hooks), so a caller inside a hook still reads each lane's own worktree. Bounds: one `git --exec-path` per run, at most 8 lanes at a time, every git call of the run within one 5 s deadline, 256 KiB of output per git call and 500 paths per lane. `data` is `{files, files_omitted, lanes}`. `files` is `{repo, path, shared, lanes: [{id, source, commit}]}`, `shared` true when two or more lanes touch it; shared rows come first, then by repo and path, the first 2,000, the rest counted in `files_omitted`. `lanes` has one row per open lane, `{id, repo, worktree, paths, cut, error?}`: `worktree` is `ok`, `none` (no worktree recorded), `missing` (not a directory), `no_base` (no base resolved; a repo git cannot read at all, such as a missing HEAD object, also lands here), `timed_out` (git did not finish, or never started, by the deadline), `error` (git failed for another reason than a missing base, such as a directory that is not a git worktree; `error` has git's last line, the fatal one) or `no_repo` (no `repo_slug`: listed, never compared); `paths` is how many worktree paths were kept and `cut` a count of what was dropped, not exact: output cut at 256 KiB counts as one, and a dropped path several commits touch counts once per commit. A lane that is not `ok` has only its handoff's files in `files`, so read `lanes` before taking a missing row as no overlap. `--repo` keeps one repo's lanes, any case. Worktree paths are not printed, except where git's own `error` line names one. The answer is written to stdout as it is serialized, after the read, with no copy held.

`stats` is new (#89). It prints [R11](research/r11-observability-cost-metrics.md)'s numbers for a cohort of lanes, read from the log in one statement (the cohort's events in lane order, each lane folded once); nothing is stored, so every total is a sum over events. The cohort is the lanes with a `pr.merged` whose `merged_at` is at or after `--merged-since` (`YYYY-MM-DD`, read as 00:00 UTC, or RFC 3339 UTC with milliseconds; anything else is `invalid_input`), or the `--work` ids (repeat the flag; a repeated id counts once, an unknown one is `not_found`). An empty cohort is `lanes: 0` with null medians. `data` is `{cohort, lanes}`.

Each lane is `{id, title, kind, pipeline_id, claimed_at, merged_at, claim_to_merge_h, with_pr, merged, tokens, peak_context, attention, send_back_rounds, plan_check, quality_gate, merge_verdict, verdict_on_head, head_reviewed, flat_rounds, reraised_findings, verified_findings, verified_findings_after_handoff, commits_after_handoff}`:

- `claimed_at` is the first `lane.claimed` (a reopened lane keeps it, so its wait counts), `merged_at` the latest `pr.merged.merged_at`, and `claim_to_merge_h` the hours between them, only when every PR the lane held (`pr.opened` or a `changed.pr_ref.to`) has merged (`merged`).
- `tokens` is keyed by `source` (`parent`, `subagent`, `harness`) with the sums of `input_total`, `cache_read`, `cache_write` and `output_total`, skipping nulls as `usage get` does; null when the lane has no usage event. `peak_context` is the largest parent `input_total`.
- `attention` counts `attention.recorded` by `type`; `send_back_rounds` is the highest `sent_back` round.
- `plan_check` and `quality_gate` are the latest `check.finished` of that check: `{outcome, head_sha, findings}` when it ran, `{outcome, head_sha, reason}` when skipped, null when absent.
- `merge_verdict` is the latest `merge_verdict` that ran, `{decision, head_sha}`, null when none ran, so a missing verdict reads apart from a NO or a YES on an older head.
- `verdict_on_head` is true when, for every merged PR, the latest `merge_verdict` that ran on its merged head (the gate's rule: the shorter sha, at least 7 hex, is a prefix of the longer) said yes; null until every PR the lane held has merged (a stack lane with a layer still open is null, not true).
- The review numbers read `check.finished` `local_review` and `pr_review` that ran (R11's `review.finished` and `finding.recorded`, events.md "Replaced names"). `head_reviewed`: every merged head has a local review on it (null until every PR has merged, as `verdict_on_head`). `flat_rounds`: adjacent local scores, recorded after the lane's first PR and ordered by daisy's job id, where the later is not higher. `reraised_findings`: verified findings grouped by trimmed lowercased title, the distinct jobs in each group minus one, summed. `verified_findings`: findings (by `finding_key`, once each) whose latest record in job order is verified. `verified_findings_after_handoff`: those whose latest record is on a review recorded after the first real handoff (one whose `next_action` does not start with `reviewing`, else the earliest; all of them when there is no handoff). The time is when Chaperone recorded the review, since daisy's finish time is not stored, so a review recorded late counts as after.
- `commits_after_handoff` is always null: it is a git count (events.md, PR), not a ledger read.

`cohort` has `lanes`, `with_pr`, `merged`, `too_few_lanes` (under 25: flagged, not scored), `claim_to_merge_h_median`, `tokens` (summed per source; a message recorded on two lanes of the cohort, which usage's worktree match allows, counts on each), `attention` (summed per type), R11's first five (`lanes_with_usage` (lanes with a parent `input_total`), `peak_context_median`, `context_tokens_per_merged_pr_median` (parent `input_total` over the PR count, merged lanes with usage), `lanes_peak_over_500k`; `lanes_reviewed` (lanes with a recorded review: the coverage of row 2's sums), `flat_rounds`, `reraised_findings`; `lanes_head_reviewed`; `lanes_with_commits_after_handoff`, `verified_findings`, `verified_findings_after_handoff`; `lanes_plan_checked`, `plan_check_skipped` and `quality_gate_skipped` (counts by reason), `lanes_quality_gated`, `lanes_verdict_on_head`, the last two over lanes with a PR as R11 defines), and `missing`. A median is over the non-null lane values, rounded to two decimals, and null when there are none. `missing` maps each cohort number printed as null because nothing records its input (by its cohort name; the lane fields of the same rows are null for the same reason) to what it needs: `lanes_with_commits_after_handoff` always, the three token numbers of row 1 when no lane in the cohort has a parent `input_total` (`lanes_peak_over_500k` is then null, not 0), and every review number when no lane in the cohort has a recorded review (with one, a lane without a review counts as unreviewed, not unknown).

`impact` (#43) lists the files that depend on the given files, nearest first, so an agent or a reviewer sees what a change might reach beyond its diff. It never opens the ledger. It runs in the current directory's git worktree: one `git rev-parse`, one `git config --get remote.origin.url`, and two `git ls-files` (the tracked files with their blob ids; the changed, deleted and untracked ones, which stats every tracked file), all four at once, then `git hash-object` (per 1,000 paths) for clean files the cache lacks, all under the same git rules as `overlap` and one 5 s deadline; output past 64 MiB from any of them fails `internal` rather than answer from a cut list. Targets are paths from the repo root, or from the current directory, or absolute. It reads Rust files and `Cargo.toml` manifests: a clean tracked file by its blob id (when its bytes still hash to it; else by their sha256), a changed or untracked one from the worktree, so the answer is for the worktree as it is now. Facts per file (its `mod` declarations, paths, `use` names, `impl` self types, each with its line) are parsed with tree-sitter and cached by content in one SQLite file per origin, shared by every worktree and clone ([ADR 0043](decisions/0043-code-graph-cache.md)); `--cache` names another file. Module nodes and import edges are rebuilt in memory on each call ([R43](research/r43-change-graph.md)). `data` is `{bound, cache, depths, files_omitted, stale, targets, unsure, unsure_omitted}`. `targets` is `{path, known, lang, module}` per target: `known` false for a path that is not a Rust file or `Cargo.toml` in the tree, `lang` `rust` or null (no edges are read for other files yet). `depths` is `{depth, bound, files: [{path, module}], files_omitted}`: depth 1 names a target directly, depth 2 names a depth 1 file, to the full closure; at most 500 files are listed over all depths, the rest counted per depth and in `files_omitted`. `bound` is `exact`, or `lower_bound` when some Rust file's references are not all known, or a target is not a Rust file in the tree (a `Cargo.toml`, a deleted file: its dependents are not read), so any depth, and an empty answer, may miss files: `unsure` lists those files with `why` (`unparsed`, `parse_error`, `path_attr`, `include`, `super_past_root`, `too_deep` for nesting past 256 levels, `orphan` for a file no crate root reaches), the first 20, the rest in `unsure_omitted`. `stale` is true when some Rust file of the tree could not be read (over 256 KiB, or unreadable). `cache` is `{parsed, reused, skipped?}`: `skipped` says why the cache was not used (the answer is then parsed in full); a SQLite file SQLite calls corrupt is renamed to `.bad` with its `-wal` and `-shm`, never deleted; any other file, an empty file passed as `--cache`, or a SQLite file with a `user_version` or tables of its own (the ledger) is never written to, its WAL included (the check runs under the write lock, so a ledger mid-`migrate` is waited for, and a refusal rolls back and closes without a checkpoint unless its WAL has no frames, which the close then removes as any last reader's does); a symlink is followed to its file, and a `--cache` file with more than one hard link is refused (`skipped: cache file has more than one hard link`; SQLite would name its WAL after the link); an empty file at the default path is a cache a first run left before writing, and is used. Not in a git worktree: `invalid_input` with git's line.

`impact --entry-points` (#44) also lists the entry points (commands, routes, screens) the targets reach, from the rules in the repo's pipeline profile. That is the file `--profile` names; else the repo's own `.chaperone/profile.toml` (plain TOML) as the base branch holds it: the first of the remote refs `origin/HEAD`, `origin/main` and `origin/master` this clone has (`git for-each-ref`, then `git ls-tree` and `git cat-file blob`, up to three more calls), never the worktree, the index or a local branch, so a lane cannot change the rules it is checked by in its own worktree or branch (the remote-tracking ref lives in the lane's own clone, which it can move: under the same-user trust model this is a speed bump, not a guarantee) (one that is there and does not parse, or is a link or a directory, is `invalid_input` naming it, not a fall back); else, for a repo that does not carry one yet, the TOML front matter of `~/dotfiles/claude/pipeline/profiles/<owner>-<repo>.md`, found by the GitHub origin URL read from git config (`remote.origin.url`; an `insteadOf` alias is not expanded, so pass `--profile` then; its `slug` must match the origin). A rule is `[[entry_points]] {kind, path, name?}`: `path` a glob from the repo root (`*` within a segment, a `**` segment any number of them), `name` the file's path when left out; `[areas]` maps an area to its globs. Before answering, every file of the tree and every target a rule names must be in exactly one area; else `invalid_input` listing the first 10, so an unmapped entry point is an error, never "other". A rule that names none of them (a typo, or a file renamed or deleted since the profile was written) does not refuse the call: it is listed in `unmatched`. No profile, no origin to find it by, no `[[entry_points]]`, an unknown key in a rule, or no commit yet (`git rev-parse HEAD` and `git diff-index --cached --name-only -z HEAD --`, two more calls at once with the others; `git config` also runs with `--cache` then, unless `--profile` is given) is `invalid_input` too, never an empty answer. `data.entry_points` is then `{bound, head, omitted, reached, total, unmatched}`: `bound` as the top-level one, and `lower_bound` too when `unmatched` lists a rule (its file may be one that is reached), `head` the commit the lines are read at, `reached` nearest first, at most 500 (the rest in `omitted`), each `{path, kind, name, area, depth, evidence}`; `total` the entry points the tree and the targets have, reached or not, one per file a rule names (a file two rules name counts twice, as `reached` lists it), so `reached` plus `omitted` is at most `total`; `unmatched` each `{kind, name, path}`, the rule's kind, name (null when it has none) and glob. A refusal lists its errors sorted. An entry point is reached when its file is a target (depth 0) or depends on one through the whole closure, past the 500 files `depths` lists. `evidence` runs from the entry point's file to a target, one `{path, at, dirty}` per hop: `at` the `file:line` of a reference that makes this hop depend on the next (the first one found; a `use` at the line of its `use`; for an `impl` edge, the line in the `impl` file; null for the target and for the Cargo edge from an integration test to a binary), `dirty` true when the file `at` names is read from the worktree or the index, not `head` (changed, untracked, staged, or not matching its blob id); false when `at` is null. A target that is not a Rust file is reached only by a rule on its own path. Without the flag, `entry_points` is absent and the answer is as before. `--declared a,b` (needs `--entry-points`) compares those areas, as a handoff's `areas` declares them, with the targets' and adds `data.areas`, every list sorted: `changed` the areas a target is in (by the `[areas]` globs; a target may be in several), `reached` the areas of every entry point reached (before the 500 cut), `undeclared` each changed area not declared as `{area, files, files_omitted}` (at most 50 files), `reached_undeclared` reached and neither declared nor changed (what a shared file reaches), `unused` declared and neither changed nor reached, `unknown` declared names the profile does not have, `unmapped` targets in no area (at most 50, the rest in `unmapped_omitted`: a change no area names, so an empty `undeclared` alone is not in scope), and `bound`, `entry_points.bound`: when `lower_bound`, `reached` may miss areas. It reads no ledger: the caller passes the declared areas. `chaperone-ui` shows it per lane whose newest handoff declares areas, on the files its worktree changes against its base, committed, uncommitted and untracked.

`pipeline current` (#106) prints `{id, hash, newer}`: the pipeline a new claim stamps by default, and in `newer` the `{id, hash}` of each embedded document newer than it that a first claim may opt into with `work claim --pipeline` (`v20`). Skills read the version from here rather than naming one, so a lane's stamp and its skill cannot disagree.

`pipeline history` (#112) prints each version of the pipeline changelog with the lanes that ran on it, so the pipeline's own performance can be shown (the UI's Pipeline page, `lane-stats`). It reads `<dotfiles>/claude/pipeline-changelog.md` (`--dotfiles`, default `~/dotfiles`; `~/.claude/pipeline-changelog.md` links to it): every heading that starts `## v` and a digit must be `## v<N> <YYYY-MM-DDTHH:MMZ>` (UTC), N and the time rising in file order, else `invalid_input` naming the line, as is a missing file; a time that has that shape but is not a real one (`2026-13-40T25:61Z`) is `invalid_input` naming its version; a version's `entry` is the text under its heading, up to the next `## ` heading. Lanes are every lane not cancelled (`--repo` keeps one repo's, any case), read from the log in one statement as `stats` does, after one query per version that checks its time is real, all in one read transaction; each lane's review rounds are then projected from its events (one projection per lane). A lane's version is the first `pipeline vN` (as words; `pipeline v17.1` names v17) a handoff summary names, in log order, else the version whose time is at or before the lane's first `lane.claimed` (`manual` before the first version), as `lane-stats` assigns them; so an open lane can move at its first handoff. A lane never claimed and never stamped ran no pipeline and is left out. `data` is `{versions}`, one row per changelog version (newest first, a version with no lanes too), then any stamped version the changelog lacks, then `manual` when it has lanes: `{version, started_at, entry, engine_ids, lane_ids, version_from: {stamp, claim}, lanes_review_rounds, review_rounds_median, cohort}`. `started_at` and `entry` are null for a version not in the changelog. `engine_ids` are the distinct engine documents its lanes were claimed under (`work get`'s `pipeline_id`, such as v17.1 next to changelog v17). `cohort` is `stats --work <lane_ids>`'s `cohort`, so its `claim_to_merge_h_median` is the per-version median and its `attention.sent_back` the version's send-backs. `review_rounds_median` is the median of the engine's `review.local_rounds` (the count the gates read) over the `lanes_review_rounds` lanes with at least one local round (a local review that ran or a `review_round` check; a daisy PR review alone is not one), null when none had one; a lane never reviewed is left out rather than counted as 0.

`doctor` (#106) checks skill and script files against this binary, so a skill that drifts from the CLI fails before a lane runs it. It never opens the ledger. `--skills` takes directories (read recursively, hidden entries skipped) or files: `.md`, `.sh`, `.py`, and extensionless scripts with a shell or python shebang (only their first 128 bytes are read to tell). A call is the binary's argv after `$L`, `"$L"`, `$chaperone`, `"$chaperone"`, or the word `chaperone` followed by a command (after a space, `/` or `(`); in Markdown only in fenced blocks and code spans (which may wrap within a paragraph), never prose; shell lines join `\` continuations and skip comments; a word stops at `|`, `;`, `&`, `)`, a backtick or a redirect. In Python, a call is a run of string literals whose first is a chaperone command, on a line that names `ledger` or `chaperone` (`[ledger, "work", "get", id]`, `cli(ledger, ["work", ...])`, `daisy("work", "list", cli=ledger)`). A placeholder (`<id>`, `"$PR"`, `$(...)`, an f-string, any other expression) stands for any value. Each call goes through this binary's own parser; an unknown command, flag or value is a finding, and so, in a fenced block or a shell file (whole commands), is a missing required argument or a value that does not parse (`--limit lots`); a missing argument or bad number in a code span or a Python list (either may be a fragment), or in a call with a word that may expand to several (`"$@"`, `$*`, `"${a[@]}"`, an unquoted `$x`), and a placeholder's refusal are not. `--flag=<placeholder>` keeps its flag, so the rest of the call is still checked. `--status` of `work update` and `work list`, and `--reason` of `check record` for its `--check`, are checked against the ledger's lists, as `--flag value` or `--flag=value`. A fenced `json` block with a `changed_files` key is a handoff: each key, nested ones too, must be a field the ledger keeps (it drops the rest). In `.md` files, every `pipeline vX` and `Pipeline version: **vX**` must be `pipeline current`'s id. A path with no such file, no call in any file, or an unreadable file is a finding too, so a wrong path never passes. With `--repo <checkout>` it also reads that repo's profile (`--profile`, else the repo's own on its base branch, else the origin's, as `impact --entry-points` finds it) and its `git ls-files`: every tracked file must be in exactly one area (the first 20 per kind listed, the rest counted), and an entry point rule that names no file is a finding; and it compares the commit this binary was built from (`build`, embedded at build time, empty outside a git checkout) with the repo's `origin/main` on `crates`, `pipelines`, `Cargo.toml` and `Cargo.lock`, so a docs-only merge does not flag it; it does not fetch. No findings: `data` is `{pipeline: {id, hash}, build, invocations, files: [{path, invocations}], origin_main?}`. Any finding: `drift` (exit 1).

Lease: `--lease-secs` defaults to 900, minimum 30, maximum 86400. Claiming a `done` or
`cancelled` item is `invalid_input`.

`--pipeline <id>` opts a lane into an embedded pipeline newer than the default at its first
claim (`v20` while the default is `v19`, for a pilot lane before the M3 cutover): the claim
stamps it in place of the default, recorded on `lane.claimed` as the default stamp is. An id
that is not newer than the default (`v19`, `v17`) or not embedded is `invalid_input`, and so
is `--pipeline` on a renewal, a takeover, or a claim of a lane already stamped. On a lane
stamped `v20`, `check record` refuses `plan_check` and `merge_verdict` (events.md, producer):
the harness runs them (`check run plan`, `check run verdict`); `check waive plan` and a smoke
claim stay.

`check record` is new (#16, next to `reviews record`). The recorder passes its own
`--session`. The write is refused when that session is any session that has held the
lane (events.md, producer). `--head` is 7 to
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
`[diff]` does not load are `invalid_input` and write nothing. The per-file list goes to the
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
directory, never a symlink) are removed once the lane is done or cancelled, or for an id the ledger
does not have, under that lane's run lock, skipping a lane whose lock is held: at the start of every
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

`check run smoke` is `check run tests` on the base profile's `[smoke]` table, which has the
same keys and rules as `[tests]`, recorded as check `smoke` with the harness producer (events.md).
A profile with no `[smoke]` is refused (`invalid_input`, nothing written), never a pass: whether
a lane needs a smoke is a gate's to say, not this command's. The lane's stamped pipeline must
define the `verdict` check (v20 on); a lane on v17 to v19 is refused (`invalid_input`, nothing
written, the message says the lane records smoke with `check record`), as a harness `smoke`
would meet v19's `smoke_recorded` from the lane's own session. The lane's own account of its smoke stays a session's `check record
--check smoke`, with its evidence files.

`check run review` takes the same lock and reads the commit the same way. The lane's stamped
pipeline must name the review's checks, each with a runner (`[runners]`: the CLI, model,
tools, budget and timeout) and a prompt file: `review_correctness`, `review_design_tests`,
`review_intent` and `review_codex` (the lenses) and `review_verifier`; a Claude runner must
name a model; and `[limits]` must hold `review_attempts`, 1 or more. v17 to v19 name none, so
a lane on them is refused (`invalid_input`, nothing written) and keeps recording reviews with
`check record` and `reviews record`. Then it writes the rulings a crash left unwritten on the
lane's newest review, and a complete review already stored under its key (events.md;
`<profile>` is the pipeline's hash) is printed with `replayed` true and nothing runs. A
complete review of the same tree by this pipeline on another commit (an amend that kept the
tree) is carried to this commit, `carried_from` and no findings of its own (events.md), and no
session starts: the lenses would only word their findings anew. A commit that already has it
replays it, so flipping between commits of a tree appends nothing after the first carry. Else a tree that already has
`review_attempts` reviews by this pipeline, complete or not, is refused (`invalid_input`,
naming the limit): a lens that fails every time would otherwise make each rerun pay for the
others again, so the owner waives the findings or the lane pushes a commit that changes the
tree. Else it asks `--version` of each CLI a runner of the plan uses (`claude`,
`codex`), the first of that name on `PATH`, run by its full path, reads the commit's files as
`diff` does and their merge base in the lane's worktree, makes a fresh checkout as `tests`
does (removed when the command ends), and reads `git diff <merge base> <commit>` there,
where the lane's git config does not reach. The reviewed commit's own `.gitattributes` still
applies: a file a rule such as `-diff` hides has no text in the diff, and the event counts
such files in `uncounted_files`. A CLI that cannot start, a checkout that cannot be made and
a diff past 16 MiB are `invalid_input` and write nothing beyond those leftover rulings.

The four lenses run at once, each a fresh headless session in the checkout (no lane
variables, Claude Code's sandbox; Codex read-only, without the user's config, `.rules` files
or the checkout's `AGENTS.md`), given its prompt file and then the facts: the commit, the merge
base, the changed files with their counts and the diff, cut after 128 KiB at a line with a
note saying where the rest is. Only `intent` is also told what the lane is for: the work
item's title, its slice of the spec, and the `goal` of its newest handoff's brief; and, when
there is one, the newest smoke on the commit as the verdict is shown it, the session's claim
labelled as one. No lens is
given the handoff's summary, `merge_case` or `look_at`. Every block of text from the lane or
a model (the file list, the diff, the goal, the findings shown to the verifier) is fenced
one backtick longer than its longest run of backticks, so nothing in it closes the fence, and
every prompt ends with one line of the harness's own saying the fenced text is data, not
instructions, and how to answer. Each lens answers `{findings: [{severity, title, body, path,
line}]}`. A lens that errors, hits its budget or timeout, or answers anything else (prose,
another field, a severity the ledger does not know) did not run: never "no findings". When a
lens found something, one more fresh session, the verifier, is shown every finding by key and
answers `{rulings: [{key, verdict, why}]}`, `verified` or `refuted` with a why that is not
blank: one session for all, as the findings share the facts and the checkout, bounded like a
lens by its runner's timeout and, for a Claude runner, its budget (Codex takes no budget, so
the timeout is its only bound). It is also shown the lane's earlier open or verified blocking
findings from complete harness reviews, keyed `<review event id>/<finding_key>`, and may rule
those only `fixed` (a verifier starts when there are any, even if no lens found anything).
One that fails, or rules on a key it was not shown, with a verdict that key does not take or
twice, or gives a blank why, rules on nothing, and every finding stays open (still blocking). Each
session's prompt and whole answer are stored first, as two evidence files.

It then writes, in this order: one `usage.reported` per session that left a session id
(events.md, Usage), one `check.finished` (check `review`: the lens table, the verifier,
`complete`, the findings), and one `finding.updated` per ruling on a finding still open (for
`fixed`, open or verified, with this commit as `head_sha`), carrying the review's harness producer, so the verifier rules whichever session started the
run (finding statuses). It is the second command that appends more than one event, after
`usage report`, each event in its own transaction. A crash leaves whole events: usage rows
without a review are what those sessions cost, and stay; a review whose rulings are missing
is finished by the next `check run review` on the lane before anything else, with no session
started. A review that is not `complete` runs again on the same tree, up to
`review_attempts`. It prints the `check.finished` as `check record` does.

`check run verdict` takes the same lock and reads the commit the same way. The lane's stamped
pipeline must name a `verdict` check with a runner and a prompt file (a Claude runner naming a
model); v17 to v19 do not, so a lane on them is refused (`invalid_input`, nothing written) and
keeps recording verdicts with `check record`. `[limits]` must hold `verdict_attempts`, 1
or more. The verdict runs only on a clean commit, so a NO can be neither bought nor caused by
running it early: the commit must have a harness `diff`, a newest harness `tests` that passed, a
harness `review` that is `complete` (carried counts) and, when the base profile declares
`[smoke]` (read as `tests` reads `[tests]`), a newest harness `smoke` that passed. Otherwise it
is refused, naming what is missing or failed, and nothing is written. Blocking findings do not
stop it: they are shown, and the verdict may say no. A verdict that ran, yes or no, stands for
(lane, tree, pipeline): on this commit it replays, on another commit with this tree it is
carried with this commit's fact ids (events.md, the key), and nothing runs. A NO is reopened
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
short form of it) with the last 4 KiB of each evidence file; each fenced, then the harness's
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
before v20, so a lane on v17 to v19 is refused and keeps `check record`). The commit must change
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
plan replays, and one whose sessions gave no answer `plan_attempts` times there is refused (the
owner can waive the check). Neither stores the plan, which is written to the store only when a
session runs on it. Else one fresh session in the harness's checkout of the commit (no diff is
read) is given its prompt, what the lane is for (as the intent lens), the plan, fenced, and the
commit and merge base, and answers `{findings: [...]}`
as a review lens does. It writes a `usage.reported` for the session, then one `plan_check` with
the harness producer (events.md): `ran` with the findings, or `skipped` with `reason`
`no_answer`, never "no findings". It prints the `check.finished` as `check record` does.

`check waive plan` is the owner's way past the plan check for a lane that coded before its plan
was attacked. The lane's stamped pipeline must name a `plan` check (else `invalid_input`: a lane
on v17 to v19 records a skipped plan check with `check record`, and a waiver there would fail its
gate). It reads the commit the lane is on as `check run` does and appends one `plan_check`
`skipped` with `reason` `waived` and `note` the `--reason` (one non-blank line of at most 500
characters, redacted at write), producer `session`: refused from any session that has held the
lane, as `work ready` and `finding waive` are. The same waiver again replays (keyed as `check
record`, on the payload as stored). It waives the plan check only, and gates nothing by itself:
a gate reads it as `plan_check.reason` `waived`. It prints what `check record` prints.

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
the commit it ran on), and `stopped` is null when all four passed, else `{check, why}`: the
refusal's message, why the step did not pass, or `the lane moved from <sha> to <sha>`.

`pr open --work <id>` opens the lane's PR under the lane's run lock (a second run is
`conflict`). It refuses (`invalid_input`, nothing pushed, gh not called) a worktree without the
lane's branch, with any change or untracked file, or not at that branch's tip; a lane with no PR
whose `work update` the ledger would refuse (a dry run); a lane that moves while it runs; a
`check run diff` past the pipeline's `[limits] code_lines` (the number `code_lines_at_most`
reads), with an uncounted file or missing a count (no limit, no refusal); a base that is no
`origin/` branch; and `check run tests` that did not pass. Diff and tests run as `check run`
runs them, so a pass on the same tree replays. A lane with no PR first asks `gh pr list --head
<branch>` for an open one a run opened and never recorded. Then `git push origin
<head>:refs/heads/<branch>`, and `gh` (found on `PATH`, run in the worktree, 120 s per call,
the body on stdin): `gh pr create --base <base> --head <branch> --title <t> --body-file -
[--draft]` (the title defaults to the item's), or for a held or listed PR `gh pr edit <n> --repo
<owner/repo> [--title <t>] --body-file -`. The body is ledger facts only: the newest handoff's
goal or the title, the tracker key, the diff's code lines and files, the test commands that
passed on the head, the plan check's outcome and the head's smoke (the harness's result, a
session's claim and its evidence file count). A PR new to the lane is `pr.opened` with
`head_sha` the head, as `work update --pr` records it. A gh failure is `internal`. It prints
`{work_id, pr, head, created, steps}`, `steps` as `verify`'s.

`merge --work <id> [--head <sha>]` is the owner's merge of the lane's PR (its `pr_ref`), as
chaperone-ui's Merge does it. `--head` is the head the owner confirms; it defaults to the newest
handoff's commit. It refuses (`invalid_input`, no `gh pr merge`) a session or an agent that has
claimed or taken over the lane, a lane with no PR, a verdict that is not YES or not on the handoff
head, a handoff head that is not `--head`, a lane that is not ready (unless its merge is
recorded), and a PR that GitHub (`gh pr view`) has at another head, closed, or open though
recorded as merged. Then `gh pr merge <n> -R <owner/repo> --squash --delete-branch
--match-head-commit <head>` (found on `PATH`, run in the temp dir, 30 s per call, never `--admin`
or `--auto`), and `pr.merged` as `work merged` records it. A PR GitHub has merged at that head is
only recorded, so a second run merges nothing. When the lane's lease is not live it then hands
off `done` itself. A gh failure, or a done handoff the ledger refuses after the merge is
recorded, is `internal`. It prints `{work_id, pr, merge_sha, done_by}`: `done_by` is `merge`
(this run handed off done), `already` (the lane was done) or `lane` (its lease is live, so the
lane hands off done itself, and a run after the lease lapses finishes it).

`reviews record` reads `daisy-cli reviews get <job>`. That is the CLI's only call into
daisy (chaperone-ui's Backlog also reads daisy's GitHub mirror, ADR 0003). A job that is not done (`data.done` false, status `queued` or `running`) appends
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
runs `reviews record` can point it at any program. chaperone-ui's Backlog reads daisy at the
same path and override, also within 30 s (ADR 0003).

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
| `finding dispute`, `finding waive` | ungated | `finding.updated` |
| `work heartbeat` | none | none |
| `migrate` | none | none (schema only) |

A gated command on a lane with no stamp (never claimed) is denied with gate `stamp_ok` and fact path `lane.pipeline_hash`. It is never ungated and never falls back to the latest pipeline. A gated command whose route has no matching transition is denied the same way, never passed.

`handoff` is the M1 gated transition. Its requires list is per route in
`pipelines/v19.toml` (#105; lanes stamped `v17.1` keep `pipelines/v17.1.toml`, #34, and
lanes stamped `v17` keep `pipelines/v17.toml`, #12). A handoff with `ready: true` on
`code_pr` or `direct` needs a `plan_check`, a `quality_gate`, a `merge_verdict` with
decision `yes` on the handed-off sha, and from v19: on a `code` lane a `smoke` and a
`kept_tests` check recorded (ran, or skipped with an allowed reason; denied as
`smoke_ok`, `kept_tests_ok`), and at most 3 review rounds, `local_review`s that ran plus
`review_round`s (denied as `rounds_at_most`). These v19 gates are on ready only: the
done handoff below is unchanged, so they do not bind a lane that goes to done, nor a
`no_pr` code lane, which has no ready row. No gate reads CI. The handed-off sha is
`handoff.commit`, or for `update --status done` the commit of the lane's latest
handoff. The two shas match when the shorter, at least 7 hex characters, is a prefix of
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

**Pipeline v20** (`pipelines/v20.toml`, a lane opts in with `work claim --pipeline v20`
until the cutover) gates the same two `handoff` rows on harness facts only (events.md,
facts for pipeline v20), each about the handed-off commit. It has no other transition:
the write runs only `handoff`, so v19's other 20 rows could never run. The ready row's
gates, in the order a denial names the first that fails:

| Gate | Holds when | Facts a denial lists |
| --- | --- | --- |
| `stamp_ok` | the lane is stamped `v20` | `pipeline.id` |
| `no_uncounted_files` | a harness `diff` on the commit has no file git counts no lines of | `commit.uncounted_files` |
| `code_lines_at_most` | that diff counts at most 400 code lines | `commit.code_lines` |
| `tests_passed` | the newest harness `tests` on the commit passed | `tests.passed` |
| `review_complete` | a complete harness review covers the commit (one carried to it from a commit with the same tree counts) | `review.complete` |
| `no_blocking_findings` | no complete harness review on the lane, of any tree, has a finding open, verified or disputed of severity `blocker`, `risk` or another blocking one; refuted, waived, fixed and answered findings, incomplete and session reviews' and the plan check's never count | `findings.open_blocking` |
| `plan_or_no_code` | the diff changes no code, or the plan check ran (`check run plan`, before code) or the owner waived it (`check waive plan`) | `commit.changes_code`, `plan_check.outcome`, `plan_check.reason` |
| `smoke_or_no_code` | the diff changes no code, or the newest harness smoke on the commit passed, or the harness ran none there and a session's smoke claim on it ran with evidence | `commit.changes_code`, `smoke.passed`, `smoke.claimed` |
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
sums of `cache_read`, `cache_write`, `output`, `reasoning`, `input_total`, `output_total` and
`cost_usd` (the CLI's own figure: only harness rows of a Claude session carry one, so a
transcript source's is null). Only the transcript rows carry `uncached_input`, since its meaning differs
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
