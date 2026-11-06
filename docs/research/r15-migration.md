# R15: Migration from today's setup

> Superseded in part by [ADR 0003](../decisions/0003-chaperone-replaces-daisy.md): the router, marker, id floor, merged reads and daisy reading ledger views are out. Chaperone replaces daisy.

Status: draft, 2026-11-02. The milestone 1 plan. It sequences [R5](r5-state-and-storage.md), [R7](r7-process-engine.md), [R8](r8-process-terminal-runtime.md), and [R10](r10-verification-review-layer.md) so a lane that is already claimed keeps its process, its work-item id, and its herdr pane.

## Question

What moves from daisy (`work`, `spec`, `review`), from dotfiles (skills, `ship-handoff`, `lane-stats`, `pipeline-profile`), and from herdr, in what order, while the CLI the skills already call keeps working?

## Recommendation

**One owner per id. New creates go to the Chaperone ledger. A live id stays in daisy until that lane is `done` or `cancelled`. The binary path the skills call does not change, and it never writes both databases.**

Milestone 1 is phases 0 through 6. Phase 6 is the ship. Phase 7 backfills finished history and is not required for that ship. Herdr stays the parent of every agent. The review worker stays in the daisy app. Skills, `ship-handoff`, `lane-stats`, and `pipeline-profile` stay in dotfiles and keep today's argv.

[VISION.md](../VISION.md) milestone 1 is the work and spec ledger out of daisy, with enforced lane states, and the CLI contract left intact. [R5](r5-state-and-storage.md) already forbids a second writer of the ledger file during the overlap, and forbids daisy from migrating that file. [R8](r8-process-terminal-runtime.md) leaves `spawn` in place until `chaperone supervise` can create a worktree, wait on setup, and attach from a herdr pane. That swap is the runtime milestone, not this one. The ledger schema still has room for pid, exit, the CLI session id, the port block, and a viewer hint, so the later swap does not rewrite history.

## Ownership

A work-item id and a spec key have one store for their whole life.

- Lookup for a write is exact. `work get`, `work update`, `work claim`, `work handoff`, and `spec update` on an id that exists in `daisy.db` run today's daisy code. The same commands on an id that exists only in the ledger exec `chaperone`. Neither side opens the other file for write.
- `work create` and `spec create`, after the cutover marker, still look up the natural key in daisy first (`source` + `source_id` for a work item, the parent key for a spec). A hit returns the daisy row and does not insert a ledger row. A miss inserts in the ledger. A spawn retry after the marker flips cannot fork a lane that was created a minute earlier.
- Copying a claimed row across is out of the plan. So is restamping a daisy lane with a pipeline hash, and so is asking its pane to re-claim.
- Integer ids would collide if both files start at 1. At the moment the marker flips, the ledger's next work id and next job id are `max(daisy id) + 1000000`. Spec keys are already external strings (`CORE-123`, a GitHub parent). A key lives in one store.
- `work list`, `spec get`, and `jobs list` merge reads. The cursor is opaque and belongs to the router. A page can contain rows from both stores. The merge is not a transaction across files, and it does not take the ledger write lock.
- Removing the marker sends later creates back to daisy. Rows already in the ledger stay there.

The marker is a file beside the ledger, read on each CLI invocation. Running panes do not need a new environment and do not need a restart. The next `daisy-cli` call sees it.

Closing the daisy app does not expire a harness lease and does not kill the herdr child. `/orchestrate` as written still runs `$D doctor` and stops on exit 5. That skill stays until no orchestrator pane is mid-run. The lane survives. The watcher skill, on the next start, still asks for the app, because the review worker and the old lanes still use it.

## The CLI contract

Skills, `spawn`, `ship-handoff`, `lane-stats`, `lane-brief`, and `lanes-watch` call `/Applications/daisy.app/Contents/MacOS/daisy-cli` by absolute path. Checked 2026-11-02: contract version 1, CLI 0.2.0, daisy schema 40.

The envelope stays `{ok, data, meta}` on success and `{ok: false, error: {code, message, retryable, details}}` on failure. Exit codes stay 0 ok, 1 error, 2 usage, 3 schema mismatch, 4 conflict, 5 runtime unavailable. Exit 4 on a lease still carries `by` and `until`. Pagination stays `next_cursor` plus the limit flags these scripts already pass. `--format jsonl` stays.

`schema_version` on this envelope is daisy's. The ledger has its own `user_version` and its own `reader_contract.version`. An agent never migrates either file. Exit 3 on a daisy-owned id means the daisy schema, with today's hint. Exit 3 on a harness-owned id names the ledger. The two files are not compared to each other.

Status values on the wire stay `queued`, `claimed`, `active`, `blocked`, `review`, `done`, `cancelled`. `ready for you` stays a `next_action` while status stays `review` ([R7](r7-process-engine.md)). The engine's phases are finer and project onto that set. `work events` keeps the fields `lane-stats` reads (`action`, `at`), and the router maps them from ledger kinds.

Commands the live path calls, and the router must answer for both stores:

| Surface | Argv the callers use | Who calls it |
| --- | --- | --- |
| Work | `create --source --source-id --title [--spec --lane --lane-pos]`, idempotent on the source pair | `spawn` |
| Work | `update <id> --status --expected-updated-at [--blocker] [--pr] [--spec --lane --lane-pos] [--reason]` | `issue`, `ship`, `spawn` |
| Work | `get`, `list --all\|--mine [--limit]`, `claim --agent --session --worktree --branch --lease-secs`, `handoff <id> <file>`, `handoffs`, `events` | skills, `spawn`, `ship-handoff`, `lane-stats`, `lane-brief` |
| Work | `board` | `/lanes` (the ready rule) |
| Spec | `get <parent>` and `get --key`, `create --linear <parent> --file`, `update --file --reason --expected-version`, `approve`, `decide --key --question --answer --where` | `dispatch`, `issue`, `ship`, `orchestrate`, `merge`, `lanes-watch` |
| Reviews | `local --path (--pr\|--base) --wait --timeout-secs`, `wait --pr --timeout-secs`, `get <job> [--wait]`, `stats` | `ship`, `daisy-review`, `lane-stats` |
| Jobs | `list --kind review --format jsonl [--limit]`, `get` | `ship`, `merge`, `orchestrate`, `lane-stats`, `daisy-context` |

`spawn` passes the herdr workspace id as `--session` and `--lease-secs 86400`. That pair stays the lease holder ([R5](r5-state-and-storage.md): agent and session, default 900, floor 30, ceiling 86400). The same string is copied to `viewer_hint`. It is the hint [R8](r8-process-terminal-runtime.md) allows while `spawn` is the parent. It is not the CLI resume id. `cli_session_id` stays null until an adapter records one. A viewer hint can change without a takeover event. A holder change is a takeover event.

`work heartbeat`, `work ready`, `work release`, `work cancel`, `reviews pr`, `reviews run`, `spec list`, and `jobs events` are on the daisy capabilities list and are not what these skills call. They ship in the same router so the surface does not fork. No skill starts calling them in this milestone.

These stay daisy commands, because they are the personal hub and the mirrors ([VISION.md](../VISION.md) non-goal): `context task`, `linear get`, `issues get`, `github list`, `github get`, `doctor`. `lane-stats` keeps using `github list` and `github get` for PR mirrors. `doctor` keeps meaning the app lock (`app.lock` beside `daisy.db`).

[R4](r4-agent-cli-integration.md) is the adapter contract (instruction file, one MCP server, `session.started`, `turn.finished`, `session.exited`). Milestone 1 does not point the skills at it. The skills keep the argv above. Adapters land with the runtime that can emit those events.

## Phases

### 0. Freeze the contract

No binary change. The table above is the contract. A later test replays recorded argv and stdout for those commands against a fixture `daisy.db`. Running lanes are untouched.

### 1. The ledger file, with nobody writing it from a lane

Create the SQLite file from [R5](r5-state-and-storage.md): WAL, `synchronous=FULL`, a 5 s busy limit retried every 1 ms, one write function, `BEGIN IMMEDIATE`, idempotency keys, `user_version` migrations, `VACUUM INTO` before an upgrade, reader views, `reader_contract`. Agents do not migrate. Daisy does not open the file yet.

Columns from [R8](r8-process-terminal-runtime.md) are in the first migration, nullable: pid, exit code, `cli_session_id`, `port_base`, `port_count`, `viewer_hint`. `spawn` leaves the first five empty. Filling them later is an update on the lane row, not a new identity.

The evidence directory from [R10](r10-verification-review-layer.md) exists (`<ledger-dir>/evidence/sha256/...`). `gate_results.evidence_ref` is nullable. Milestone 1 gate events store the fields the projection reads. Screenshot bytes can still be described in the handoff until a producer writes a manifest. The row stays small either way.

Tests in this phase: rebuild the tables from the log, the R5 kill cases against the real write function, and a refused transition that writes nothing. No lane points at the file.

### 2. Router, marker off

`daisy-cli` at the absolute path above dispatches. Daisy-owned ids run today's code. Harness-owned ids exec `chaperone` and return its stdout, stderr, and exit code unchanged. The marker is absent, so every create still goes to daisy. There are no harness ids yet.

Replacing that binary does not kill a pane. Agents spawn the CLI per call. The daisy path for an existing id has to match the fixture from phase 0, including exit 4 and exit 5.

`chaperone` is the harness binary ([ADR 0005](../decisions/0005-chaperone-name.md)). Skills do not learn that name in this milestone.

### 3. New work and spec rows

Flip the marker. The next `work create` or `spec create` whose natural key is absent from daisy inserts in the ledger. A parent spec that already exists stays in daisy, and every lane already on it stays with it, including a later slice filed onto that parent. A new parent is a ledger spec.

Harness `work` and `spec` commands do not consult `app.lock` and do not exit 5. Daisy-owned ids still do, where they do today (reviews that need the worker). Claim, renew, `expected-updated-at`, and the idempotent create match the daisy rules, including the lease ceiling `spawn` already sends (86400).

`ship-handoff record` still shells to `work handoff`. For a daisy id that is today's command. For a ledger id it is `chaperone`, and phase 4 is what makes that command refuse.

### 4. Gates on lanes claimed in the ledger

At claim, the engine stamps pipeline id and semantic hash ([R7](r7-process-engine.md)). The document it loads is v17. A daisy lane is not stamped.

The engine refuses the handoff transition, and the `done` transition, unless plan check, quality gate, and a YES verdict on the handed-off sha are recorded. That includes a `done:` handoff. `ship-handoff` only regex-checks those three lines when `next_action` is `ready for you` ([R2](r2-own-evidence.md), the `CORE-4383` hole). The engine is the check that closes it, and only for ledger lanes. A refused call writes no event and leaves the phase. `ship-handoff` still runs its shape checks (verification notes, smoke line, kept-test bar, brief length, `ready` agreeing with `next_action`) and still calls `work handoff`.

The prose parser that turns a `plan check:` line into `check.finished` is phase 7. It is not called from a predicate and not called from `ship-handoff` ([R10](r10-verification-review-layer.md)). A live ledger lane writes the structured records. The lane session is the runner for plan check, quality gate, and the verdict, and it still cannot be the author of those finishes. A finish whose actor is the lane session is a refused write.

### 5. Review records for new jobs

The worker stays in the daisy app (`src-tauri/src/jobs/review.rs`). `jobs.rs`, `review_outcomes.rs`, and `review_missed.rs` move as the ledger shape ([R6](r6-language-and-runtime.md)), on the harness migration series, not as a dependency on the whole `daisy-core` crate.

A review for a ledger lane appends `check.requested`, then the same `reviews local` or `reviews wait` argv. The app runs the job it already runs. The finish is a ledger event. Findings for that job are ledger rows. Local reviews still do not post. PR reviews still post one GitHub review.

A job id that already exists in daisy finishes in daisy. It is not moved mid-run. `reviews get` on that id reads daisy. New job ids come from the ledger range in the ownership rule.

When the profile has `review.daisy` and the app is closed, the projected state is `running`. The lane records the blocker. It does not record `absent`, and it does not get a fresh process ([R10](r10-verification-review-layer.md)). Exit 5 remains the reviews response when the worker is required and the app is down. The herdr child keeps running.

Plan check, the quality gate, and the merge verdict stay fresh agent sessions. They do not join this worker.

### 6. Readers, then ship

Daisy opens the ledger with the [R5](r5-state-and-storage.md) read path: read-only, `query_only=ON`, short transactions, `reader_contract.version` checked, views only. The Work page can show ledger lanes. Daisy does not migrate the file and does not write it.

`lane-stats` and `lane-brief` keep calling `work list`, `work handoffs`, `work events`, `jobs list`, `reviews get`, and `reviews stats`. The router's merged list is how new lanes become visible. No script learns a second path in order for milestone 1 to count.

Ship criterion: the marker is on, a newly spawned lane's work item is in the ledger, its gates refuse a `done:` handoff that skips the three stamps, an already claimed daisy id still answers `work get` from `daisy.db`, and its herdr pane was not restarted. That is milestone 1.

### 7. Importer, after the ship

A batch job reads finished daisy lanes only (status `done` or `cancelled`, lease empty). It appends ledger events under idempotency keys derived from the daisy ids. It may synthesize `check.finished` and the [R11](r11-observability-cost-metrics.md) metric events from a handoff summary. It is the only prose parser. A query never parses that prose itself.

Re-running it is a no-op when the keys already committed. It does not open a claimed row. The next 25-lane score ([R2](r2-own-evidence.md)) does not wait on it. Token medians for that score still come from `lane-stats` reading transcripts, because `usage.reported` arrives with the R4 adapters, which this milestone does not start.

## Milestone 1 deliverables

1. A ledger file separate from `daisy.db`, with the R5 write function, reader views, and the R8 columns present and empty under `spawn`.
2. A router at the existing `daisy-cli` path. Same argv, envelope, exit codes, and lease conflict. One store per id. Creates look up daisy before inserting. Ledger ids start a million above daisy's max.
3. New work items and new parent specs written only to the ledger. A live daisy id stays there until `done` or `cancelled`, with its pane and its herdr workspace id left in place.
4. v17 stamped at claim on ledger lanes. Plan check, quality gate, and a YES verdict on the handed-off sha are refused into existence, including on `done:`. A daisy lane is under today's `ship-handoff` rules only.
5. Review jobs and findings for ledger lanes stored as check events. The worker remains the daisy app, invoked by the same `reviews` argv. In-flight daisy jobs finish in daisy. A closed app projects `running` and blocks the gate.
6. Daisy and the dotfiles readers (`lane-stats`, `lane-brief`, `lanes-watch`) see both stores through the merged CLI and the read-only views. No second writer.

## What stays in dotfiles for now

These stay on their current paths and keep their current argv through milestone 1. They move when the thing that replaces them is already what the running lanes call.

| Piece | Path | Why it stays | When it can leave |
| --- | --- | --- | --- |
| Pipeline skills | `~/dotfiles` `claude/` tree, loaded from `~/.claude/skills` (`dispatch`, `issue`, `ship`, `merge`, `orchestrate`, `lanes`, `quality-gate`, `diagnose`, `daisy-review`, `daisy-context`, `lane-stats`) | A pane that already loaded a skill still types `$D` and the argv above. An edit mid-flight changes the next slash command. | After no lane is mid-flight on the old text, and the engine plus adapters are what a new skill would call. |
| `ship-handoff` | `~/dotfiles/bin/ship-handoff` (on PATH as `~/.local/bin/ship-handoff`) | `/ship` records through it. It keeps the shape checks. The engine inside `work handoff` is the gate for ledger lanes. | When those shape checks are engine predicates and no live lane still uses the script as the only recorder. |
| `lane-stats` | `~/dotfiles/bin/lane-stats` | It is the scorer, including transcript tokens. Criterion 8 has no `usage.reported` yet. The router merge makes new lanes visible without a rewrite. | When the R11 queries cover every number it prints, including tokens. |
| `pipeline-profile` and profiles | `~/dotfiles/bin/pipeline-profile`, `~/dotfiles/claude/pipeline/profiles/` | Per-repo inputs: tracker, `git.base`, `pr.flow`, `review.daisy`, smoke, known flakes, test bar. [R7](r7-process-engine.md) treats them as inputs to route choice, not as a second pipeline language. The v17 TOML lives with the harness. | When a harness command returns the same JSON and the same `--section` markdown, and every repo profile has moved. |
| Pipeline changelog | `~/.claude/pipeline-changelog.md`, diffed from dotfiles | Cohort headings for lanes whose stamp is a summary line. New lanes also carry id plus hash. | When the importer has given old lanes a hash and `lane-stats` groups on that hash. |
| `spawn`, `lanes-watch`, `lane-brief` | `~/.local/bin/spawn`, `~/dotfiles/bin/lanes-watch`, `~/dotfiles/bin/lane-brief` | [R8](r8-process-terminal-runtime.md): `spawn` plus herdr is the parent, `/orchestrate` plus `lanes-watch` is the watcher, until `supervise` can create a worktree, wait on setup, and attach. | The runtime milestone. The ledger must already have pid, exit, `cli_session_id`, the port block, and `viewer_hint`. |

`herdr` stays the process parent and the place notifications are sent. Nothing in herdr moves in this milestone. A workspace id stored on a lane is a viewer hint and the lease's session string, not the lane's primary key.

Daisy keeps notes, inbox, the Linear, GitHub, and Slack mirrors, `context task`, and the review worker. Old lanes keep depending on the app being open. Ledger lanes do not, except for a `review.daisy` job, which waits.

## What this leaves open

- The router is a change in the daisy repo (`daisy-cli` execs `chaperone`). This plan assumes that binary may grow a dispatch. A new path would mean a skill edit, which waits until no pane is mid-flight.
- The id gap is `max + 1000000` at flip time. A daisy create after the flip, which the lookup rule still allows for keys that already exist, does not allocate from the ledger range. A daisy id past the gap would collide. The router refuses a create that would land in the reserved range, on either side.
- `work list` across two files is not one snapshot. Two pages can disagree if a row commits between them. Acceptable for the board. A gate decision reads one store, inside one transaction.
- `/orchestrate` still exits when `doctor` exits 5. Editing that skill is a later change, done while no orchestrator pane is running.
- Who writes the first structured plan-check and verdict records is the lane's runner plus a fresh session ([R10](r10-verification-review-layer.md)). The skill text that tells the runner to do that is the current `/issue` and `/ship`. This plan does not rewrite those skills. A ledger lane whose skill never emits the structured record fails the gate and stops, which is the milestone 1 behavior. If that turns out to be too blunt on the first spawned lane, the follow-up is a skill edit scheduled between lanes, not a prose parser on the live path.
- Port allocation, PTY ownership, and `session.exited` from `wait` are [R8](r8-process-terminal-runtime.md). The columns are reserved here. `spawn` keeps assigning no port.
- Token queries ([R11](r11-observability-cost-metrics.md) family 1) wait on adapters. Families 2 through 5 can be answered from phase 4 and 5 events for ledger lanes. The 25-lane score still uses `lane-stats` until that script reads those events on its own, which is a dotfiles change after the ship, not a blocker for it.

## Sources

Read 2026-11-02.

- [RESEARCH.md](../RESEARCH.md) R15. [VISION.md](../VISION.md) milestone 1 and the non-goal on daisy's personal hub.
- [r4-agent-cli-integration.md](r4-agent-cli-integration.md) (the adapter contract milestone 1 does not put in front of the skills).
- [r5-state-and-storage.md](r5-state-and-storage.md) (separate file, one writer, daisy reads views, no second writer during the overlap).
- [r6-language-and-runtime.md](r6-language-and-runtime.md) (`work.rs`, `spec.rs`, `jobs.rs`, `review_outcomes.rs`, `review_missed.rs` move; the hub stays).
- [r7-process-engine.md](r7-process-engine.md) (stamp at claim, status versus `next_action`, profiles as inputs).
- [r8-process-terminal-runtime.md](r8-process-terminal-runtime.md) (`spawn` remains the parent; schema for pid, exit, session id, port block, viewer hint).
- [r10-verification-review-layer.md](r10-verification-review-layer.md) (worker stays, prose parser off the live path, closed app is `running`).
- [r2-own-evidence.md](r2-own-evidence.md) (the 25-lane score, the `done:` hole). [r11-observability-cost-metrics.md](r11-observability-cost-metrics.md) (importer may synthesize, queries do not parse prose).
- [ADR 0005](../decisions/0005-chaperone-name.md): the binary name `chaperone`.
- daisy CLI capabilities, 2026-11-02: contract version 1, CLI 0.2.0, schema 40, exit codes 0 through 5. Commands exercised by the skills and the dotfiles scripts, listed above.
- `daisy-core` `db.rs`: `open_existing` refuses to migrate, `APP_LOCK_FILE` is how `doctor` and `reviews` know the app is up.
- `~/dotfiles/bin/ship-handoff`, `pipeline-profile`, `lane-stats`, `lanes-watch`, `lane-brief`. `~/.local/bin/spawn`. Skills under `~/.claude/skills` for `dispatch`, `issue`, `ship`, `merge`, `orchestrate`, `lanes`.

## Choices this plan makes

1. Live ids stay in daisy. The cutover is a create-time fork, with a daisy lookup first so retries do not duplicate.
2. The compatibility shim is the existing `daisy-cli` path. It execs `chaperone` and does not write the ledger itself.
3. Gates apply to lanes claimed after the marker flips. In-flight lanes keep today's `ship-handoff` behavior.
4. Skills, `ship-handoff`, `lane-stats`, `pipeline-profile`, `spawn`, `lanes-watch`, and `lane-brief` stay in dotfiles for this milestone.
5. Herdr stays the parent. The review worker stays in the daisy app.
6. The handoff-prose importer runs after the ship, on finished rows only, and is the only prose parser.
