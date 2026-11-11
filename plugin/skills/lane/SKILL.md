---
name: lane
description: Take one Chaperone task from claim to a ready handoff on a v20 lane, then hand off done after the owner merges
argument-hint: "<task key> --work <N>"
disable-model-invocation: true
allowed-tools: Bash(chaperone pipeline limits *)
---

# Lane

Task `$0`, work item `$2`. Work in this worktree only. Every write acts as `$DAISY_AGENT` and
`$DAISY_SESSION`, the pair the spawner set; never pass others.

Limits (an error means the lane is not claimed yet: run this again after step 1):

!`chaperone pipeline limits --work $2 --format jsonl || true`

`$G` stands for the path `git rev-parse --absolute-git-dir` prints. The plan, the handoff
JSON and `lane-state.md` live there, never in a commit.

## Never

- `gh pr create`, `gh pr merge`, `chaperone work ready --yes`: the hook denies them.
  `chaperone pr open` opens the PR; the owner merges.
- Push to the base branch, push the branch yourself, merge, `--no-verify`, amend or rebase
  after `pr open` (it pushes without force).
- `check record` anything but the smoke claim; `finding waive` and `check waive` are the
  owner's. Editing `.chaperone/profile.toml` changes nothing: checks read the base branch's.

## Lane state

Keep `$G/lane-state.md` current after every step: key, work id, branch, goal and done-when,
decisions, plan path, status (done, next), each blocking finding and what you did with it.
After a compaction, read it, then `chaperone work get $2`.

## 1. Claim

`chaperone work get $2`, then:
- `pipeline_id` null (never claimed):
  `chaperone work claim $2 --pipeline v20 --worktree "$(git rev-parse --show-toplevel)" --branch "$(git branch --show-current)" --lease-secs 86400`
- `pipeline_id` `v20`, `agent` and `session` yours, `worktree_path` and `branch` this
  worktree's: claimed already. If `lease_state` is not `active`, `chaperone work heartbeat $2`.
- Anything else (another pipeline, holder, worktree or branch): Needs you. `--pipeline`
  works on the first claim only.

## 2. Understand

Read the task (`gh issue view` for a GitHub key, the tracker's tool otherwise),
`chaperone spec get --key $0` and `chaperone dossier get $0` (either may be `not_found`),
then the code it touches. How: `judgement.md`, "Goal". Then hand off progress with the goal:
the plan check, the intent lens, the verdict and the PR body read `brief.goal` from the
newest handoff.

Every handoff is `chaperone work handoff $2 "$G/handoff.json"`. Each one replaces the last:
always write `summary`, `status`, `next_action` and the whole `brief`. Here: `status`
`active`, `next_action` `plan check`, `brief` `{goal, decisions}`.

## 3. Plan, before any code commit

Write `$G/plan.md`: goal and done-when, the files and the change in each, the smoke (exact
commands and the output that proves the done-when), the tests the repo's bar asks for, a
code-line estimate against `code_lines`, and for a bug the root cause as `file:line`.

`chaperone check run plan --work $2 --file "$G/plan.md"`, in the background (a model session,
up to `plan_timeout` seconds). Then:
- `outcome` `ran`: the gate is met. Fix the plan for each finding that holds; a `question`
  or one that needs a product call is Needs you. Run again only after a real design change
  (each run is a paid session; the same plan replays).
- `skipped`, `no_answer`: run again. Refused at `plan_attempts`: Needs you.
- Refused because the commit already changes code: Needs you (the owner may waive).

A lane whose diff changes no code skips steps 3 and 5. Code is every file but `.md` files
and `docs/`, plus what the base profile's `[diff] code` names.

## 4. Implement

The smallest complete change (`judgement.md`). A bug whose cause is not plain: invoke the
`chaperone:diagnose` skill. Commit with short plain messages; leave the tree clean. Then:
- `chaperone check run diff --work $2`: `code_lines` over the limit or `uncounted_files`
  over 0 will not pass. Cut, or Needs you with a proposed split.
- Invoke the `chaperone:blast-radius` skill once; fix what it finds inside the slice.

## 5. Smoke, on the commit you will hand off

Before every verify: the review and the verdict read the smoke on the commit, and a NO
stands for the tree even if a smoke comes later. Every new commit needs a new one.

`chaperone check run smoke --work $2`. Refused for no `[smoke]` in the base profile: run the
plan's smoke yourself, put each command and its output in `smoke-$2.log` at the worktree
root, then record your claim:

```
chaperone check record --work $2 --check smoke --head "$(git rev-parse HEAD)" --outcome ran --evidence smoke-$2.log
rm smoke-$2.log
```

Record only what ran and what you saw. The ledger keeps a copy; `pr open` refuses untracked
files. A code change with no smoke that ran cannot be ready: after a real attempt, Needs you.

## 6. Verify

`chaperone verify --work $2` with `run_in_background: true`, then end the turn: it runs diff,
tests, the review (four lenses and a verifier) and the verdict, and can take 45 minutes.
Never commit while it runs. Its `stopped`:
- `null`: every step passed. Go to 7.
- `tests`: "tests failed". `chaperone check run tests --work $2` replays it: `commands` has
  each argv and its exit. Run the failing one locally, fix, commit, back to 5.
- `review` or `verdict`: read `findings.md`.
- "the lane moved": you committed meanwhile. Back to 5.
- Any other refusal: its message says why. Fix the cause or Needs you. Exit 4: another run
  holds the lane; wait for it, never start a second.

## 7. Open the PR

`chaperone pr open --work $2` pushes and opens or updates the PR, its body from the ledger.
Its refusals name the fix. Note the `pr` it prints.

## 8. Ready handoff

On the commit verify passed (`git rev-parse HEAD` is its `head`):

```json
{"summary": "<what changed and what proves it>", "status": "review", "ready": true,
 "commit": "<full sha>", "pull_request": "<owner/repo#N>", "changed_files": ["<path>"],
 "next_action": "needs you: review and merge <owner/repo#N>",
 "brief": {"goal": "", "merge_case": "", "decisions": [], "look_at": [{"path": "", "line": 1, "why": ""}], "check_by_hand": []}}
```

A `gate_denied` names the first gate that fails. `smoke_or_no_code`: step 5.
`plan_or_no_code`: Needs you. `code_lines_at_most`: step 4. Any other: verify has not passed
on this commit (a missing diff reads as `no_uncounted_files`): back to 5.

## 9. Wait, then done

End the turn. Do not commit after the ready handoff (the stop hook blocks once if you do).
The owner merges. If the owner asks for changes: hand off `active`, change, then 5 to 8.

After the merge: `chaperone work get $2`. `status` `done` (the merge handed it off): stop.
Else hand off `status` `done`, `ready` false, `commit` the ready handoff's commit,
`next_action` `done: merged as <sha>`.

## Needs you

Stop and ask only when: you cannot tell what done means, or two readings lead to different
code; doing it properly needs work outside the done-when; a one-way door (schema, migration,
prod data, a user's files, money); the plan departs from a recorded decision; the size
passes `code_lines`; a check you cannot pass after a real attempt; a finding you disputed;
or a refusal only the owner can lift.

Hand off `status` `blocked` (`active` from `review`, which cannot go to blocked), `blockers`
and `open_questions` one line each, `next_action` `needs you: <what to decide>`; end the
turn. Answered: add it to `brief.decisions`, hand off `active` with `next_action`
`resolved: <answer>; <next step>`, go on.
