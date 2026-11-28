---
name: lane
description: Take one Chaperone task from claim to a ready handoff on a v21 lane, then hand off done after the owner merges
argument-hint: "<task key> --work <N>"
disable-model-invocation: true
allowed-tools: Bash(chaperone pipeline limits *)
---

# Lane

Task `$0`, work item `$2`. Work in this worktree only. Every write acts as `$DAISY_AGENT` and
`$DAISY_SESSION`, the pair the spawner set; never pass others.

Limits (an error means the lane is not claimed yet: run this again after step 1):

!`chaperone pipeline limits --work $2 --format jsonl || true`

For `$G` below, write out the path `git rev-parse --absolute-git-dir` prints (a shell
variable does not last between calls). The plan, handoff JSON and `lane-state.md` live there.

## Never

- `gh pr merge`, `chaperone work ready --yes`, and, except on v21, `gh pr create` and `gh pr edit`:
  the hook denies them. `chaperone pr open` opens the PR; the owner merges.
- Push to the base branch, push the branch yourself (but step 7's v21 fallback), merge, `--no-verify`, amend or rebase
  after `pr open` (it pushes without force).
- `check record` anything but the smoke claim; `finding overturn`, `check overturn`,
  `check waive rounds`, `check waive size --lines` and every `finding waive` but
  `--kind deferred` are the owner's. Editing `.chaperone/profile.toml` changes nothing: checks read the base branch's.

## Lane state

Keep `$G/lane-state.md` current after every step: key, work id, branch, goal and done-when,
decisions, plan path, status (done, next), each blocking finding and what you did with it.
After a compaction, read it, then `chaperone work get $2`.

## 1. Claim

`chaperone work get $2`, then:
- `pipeline_id` null (never claimed):
  `chaperone work claim $2 --worktree "$(git rev-parse --show-toplevel)" --branch "$(git branch --show-current)" --lease-secs 86400`
- `pipeline_id` `v21` (or `v20`, a lane claimed before v21), `agent` and `session` yours, `worktree_path` and `branch` this
  worktree's: claimed already. If `lease_state` is not `active`, `chaperone work heartbeat $2`.
- Anything else (another pipeline, holder, worktree or branch): Needs you.

## 2. Understand

Read `${CLAUDE_SKILL_DIR}/judgement.md`, then the task (`gh issue view` for a GitHub key, the tracker's tool
otherwise), `chaperone spec get --key $0` (it may be `not_found`), then the code it
touches. Then hand off progress with the goal:
the plan check, intent lens, verdict and PR body read `brief.goal` from the newest handoff.

Every handoff is `chaperone work handoff $2 "$G/handoff.json"`. Each one replaces the last:
always write `summary`, `status`, `next_action` and the whole `brief`. Here: `status`
`active`, `next_action` `plan check`, `brief` `{goal, decisions}`.

## 3. Plan, before any code commit

Write `$G/plan.md`: goal and done-when, the files and the change in each, the smoke (exact
commands and the output that proves the done-when), the tests the repo's bar asks for, a
line `Estimated added code lines: N` (the harness reads it; past `code_lines` on v21 the plan
check asks split, naming the slices, or keep whole, as a finding you answer), and for a bug the root cause as `file:line`.

`chaperone check run plan --work $2 --file "$G/plan.md"`, in the background (a model session,
up to `plan_timeout` seconds). Then:
- `outcome` `ran`: the gate is met. Fix the plan for each finding that holds; a `question`
  or one that needs a product call is Needs you. Run again only after a real design change
  (each run is a paid session; the same plan replays). Then, before the diff check, answer
  every finding (`finding list --work $2` lists them, `check` `plan_check`):
  `chaperone finding accept <event> <key> --reason "<how the plan acts on it>"`, or
  `chaperone finding dispute <event> <key> --reason "<why it does not hold>"`, or for a
  `question` `chaperone finding answer <event> <key> --reason "<the answer>"`. Until each
  has an answer, `check run diff`, `check run review` and `verify` refuse. The review is
  shown the answers: an accepted one the diff does not address blocks.
- `skipped`, `no_answer`: run again. Refused at `plan_attempts`, or because the commit
  already changes code: the check cannot run now, so `chaperone check run diff --work $2`, then
  `chaperone check waive plan --work $2 --reason "<why it cannot run>"`. The PR body lists it
  and the owner can overturn it; refused after an overturn: Needs you.

A lane whose diff changes no code skips step 3, and step 5 unless the base profile declares
`[smoke]` (the verdict needs its pass). Code is every file but `.md` files and `docs/`, plus
what the base profile's `[diff] code` names.

## 4. Implement

The smallest complete change. A bug whose cause is not plain: invoke `chaperone:diagnose`
with the symptom, the failing command and `--fix`. Commit with short plain messages; leave
the tree clean. Write no comment the repo's rules do not keep, and cut those you touch. Then:
- `chaperone check run diff --work $2`: `uncounted_files` over 0 will not pass (a binary an
  `[assets]` glob of the base profile names is `asset_files` instead, and passes).
  `code_lines` counts added lines; removed lines (`removed_code`) and test files
  (`test_lines`) do not count. Over the limit, on v20 it will not pass; from v21 the review
  asks what to cut or split, and a `cut:` finding blocks like any other. Never get under by
  compressing code, removing checks or dropping behavior: cut only what the done-when does
  not need, split a piece that stands alone into its own PR, or, when the change only makes
  sense whole, `chaperone check waive size --work $2 --reason "<why it is needed whole>"` (no
  `--lines`), which the PR body shows. It lifts nothing: on v21 answer each `cut:` as any
  finding; on v20 the gate still needs the owner's `check waive size --lines`: Needs you.
  A refused companion rule names a doc the change must update: update it, or, when the
  change truly needs no edit there, commit a `No-companion: <rule> <why>` trailer.
- Invoke the `chaperone:blast-radius` skill once; fix what it finds inside the slice.

## 5. Smoke, on the commit you will hand off

Before every verify: the review and the verdict read the smoke on the commit, and a NO
stands for the tree even if a smoke comes later. Every new commit needs a new one.

`chaperone check run smoke --work $2`. `result` `fail`: read its `commands`, fix, commit, run
it again. Refused for no `[smoke]` in the base profile: put the plan's smoke commands in
`smoke-$2.sh` at the worktree root and record your claim; the CLI runs it with bash there and
keeps the script, its output and exit code:

```
chaperone check record --work $2 --check smoke --head "$(git rev-parse HEAD)" --outcome ran --run smoke-$2.sh
rm smoke-$2.sh
```

A run that exits non-zero is recorded and claims nothing: fix, commit, run it again. The
ledger keeps a copy; `pr open` refuses untracked files. A code change with no smoke that ran cannot be ready: after a real attempt, Needs you.

The smoke stops every server, watcher and background job it starts (`trap 'kill $pid' EXIT`)
and prints what it stopped, so the record shows it. Then stop any the lane started earlier.

## 6. Verify

Not while an open question is unanswered (Needs you).

`chaperone verify --work $2` with `run_in_background: true`, then end the turn (it can take
45 minutes). Never commit while it runs. Its `stopped`, by `check` and `why`:
- `null`: every step passed. Go to 7.
- `tests`: "tests failed". `chaperone check run tests --work $2` replays it: `commands` has
  each argv and its exit. Run the failing one locally, fix, commit, back to 5.
- `review`, "N finding(s) still block" or "review not complete"; `verdict`, "the verdict is
  not yes": read `${CLAUDE_SKILL_DIR}/findings.md`.
- "the lane moved": you committed meanwhile. Back to 5.
- A `why` that names `review_rounds` (the lane's fresh reviews are used up): Needs you, with
  the findings that still block; never a commit to try another round.
- Any other `why` is a refusal that says why: fix the cause or Needs you. Exit 4: another
  run holds the lane; wait for it, never start a second.

## 7. Open the PR, then watch it

`chaperone pr open --work $2` pushes and opens or updates the PR, its body from the ledger.
When the base profile's `[pr]` has a `title` (`git show origin/<base>:.chaperone/profile.toml`),
pass `--title` filled from it: `{key}` the task's key exactly, `{type}` one of `types` that fits
the change, `{text}` a short title. Its refusals name the fix; a gh failure (`internal`): retry
once, then Needs you. Note `pr`.

On v21, when `pr open` keeps refusing a finished change for a reason outside the lane, push the
branch and open or edit the PR with `gh pr create` / `gh pr edit` (the hook allows them there),
then name it in the ready handoff's `pull_request`, which records it as the lane's.

Then the PR's CI and review threads are yours; CI is no chaperone check. `gh pr checks <n>
--watch --fail-fast` with `run_in_background: true`, then end the turn; never a poll loop.
Read its output, not its exit (red and none both exit 1):
- "no checks reported" right after a push may be CI not queued yet: watch once more as
  `sleep 60; gh pr checks <n> --watch --fail-fast`. Still none, or checks that never start:
  say so in the ready handoff's `summary`.
- Red: a known flake (`pipeline-profile --section "Known flakes"`; none listed or no such
  command, every failure is real) gets one rerun (`gh run rerun <run> --failed`) and its citation in the `summary`.
  Else read `gh run view <run> --log-failed`, fix, commit, then 5, 6, `pr open` again (it
  pushes), and watch again.
- When the watch ends, threads and reviews from people or bots (`gh pr view <n> --json
  reviews,comments`, `gh api repos/<owner>/<repo>/pulls/<n>/comments`): one a user or
  production would meet is fixed the same way, or answered in its thread; list the
  non-blocking ones in the `summary`, unchased. One landing after step 8 is a step 9 prompt.
- Fix rounds since `pr open` share one count for checks and threads (a flake rerun is not
  one); keep it in `lane-state.md`. Two and still red or open: Needs you, with the check
  name and its log's tail. Else green: go to 8.

## 8. Ready handoff

On the commit verify passed (`git rev-parse HEAD` is its `head`):

```json
{"summary": "<what changed and what proves it>", "status": "review", "ready": true,
 "commit": "<full sha>", "pull_request": "<owner/repo#N>", "changed_files": ["<path>"],
 "next_action": "needs you: review and merge <owner/repo#N>",
 "brief": {"goal": "<goal, as in step 2>", "merge_case": "<why this is safe to merge>",
  "decisions": ["<decision>"], "look_at": [{"path": "<path>", "line": 1, "why": "<why>"}],
  "check_by_hand": ["<what only a person can check>"]}}
```

A `gate_denied` names the first gate that fails. `smoke_or_no_code`: step 5.
`plan_or_no_code`: Needs you. `code_lines_at_most` (v20 only): step 4. Any other: verify has not passed
on this commit (a missing diff reads as `no_uncounted_files`): back to 5.
A `reviewers` `error` in its output: hand off the same JSON again once, then Needs you.

## 9. Wait, then done

End the turn. The owner merges. Prompted after the ready handoff (the owner asks for changes,
a red check, a new thread, a merge queue ejection): hand off `active` before you commit (the
stop hook blocks once while the newest handoff is ready on another commit), then 5 to 8.

After the merge: `chaperone work get $2`. `status` `done` (the merge handed it off): stop.
Else, when the merge is not recorded (the owner merged outside `chaperone merge`), record it.
On v21: `chaperone work merged $2 --pr <owner/repo#N>`; it reads the merge from GitHub, and records only
the lane's PR; a lane with no PR takes one merged from its branch. Off v21 also pass `--merge-sha <sha> --head
<PR head> --merged-at <mergedAt, with .000Z>`. Then hand off `status` `done`, `ready` false, `commit` the ready handoff's commit,
`next_action` `done: merged as <sha>`. On v21 a recorded merge needs no verdict on that commit.

## Needs you

Stop and ask only when: you cannot tell what done means, or two readings lead to different
code; doing it properly needs work outside the done-when; a one-way door (schema, migration,
prod data, a user's files, money); the plan departs from a recorded decision; the size
passes `code_lines`; a check you cannot pass after a real attempt; a dispute that is the
owner's (`findings.md`); or a refusal only the owner can lift. A `next_action` that waits on
the owner names the exact command they run.

A question names the part of the done-when it blocks. While some part is not blocked, hand
off `active`: `open_questions` one line each, `<question> (blocks: <part>)`; `blockers` one
line per part, `blocked on the answer: <part>` or `parked: <part>, <why it waits>` for one you
set aside; `next_action` `needs you: <what to decide>; meanwhile: <what you build>`. Keep
building the rest, and carry the same lines in every handoff until the answer comes (each
handoff replaces the last).

When nothing left is unblocked: hand off `status` `blocked`, or `active` when the lane is in
`review` (review cannot move to blocked), with the same lines and `next_action` `needs you:
<what to decide>`; end the turn. Answered: add it to `brief.decisions`, hand off `active` with
`next_action` `resolved: <answer>; <next step>`, go on.
