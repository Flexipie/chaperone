# Findings

## What blocks

A finding of severity `blocker` or `risk`, status `open`, `verified` or `disputed`, from a
complete review of any commit of the lane. `question`, `convention`, `nit` and
`improvement` never block; plan check findings never block.

## Read them

- Review, only after verify stopped on "N finding(s) still block" (after "review not
  complete" it starts a new paid run): `chaperone check run review --work <N>` replays the
  head's review: `event_id`, and `payload.findings[]` with `finding_key`, `severity`,
  `title`, `body`, `path` and `line`.
- An earlier commit's review: no command lists it. Copy each blocking finding's `event_id`,
  key and title into `lane-state.md` when you first read it, and keep its status there.
- Verdict: `chaperone check run verdict --work <N>` replays it: `decision`, `why`,
  `not_proven`.

## Each blocking finding

Weigh it (`judgement.md`, "Weighing a finding"), then one of:
- It holds: fix it at the root and commit. Nothing to record: the next verify reviews the
  new tree, and its verifier rules an earlier finding `fixed` only when it can name the line
  that fixes it. One it cannot stays open and blocks.
- It is wrong: `chaperone finding dispute <event_id> <finding_key> --reason "<file:line that answers it>"`.
  A dispute still blocks until the owner waives it or an independent session refutes it,
  so then Needs you, naming the finding. Never argue in prose, the PR or the handoff.

Fix several findings at once, then back to step 5 (smoke) and verify once. A third verify
on this lane that stops at `review` or `verdict`: Needs you. A big fix round
goes to a fresh subagent given the worktree, `lane-state.md` and the findings, so this
session keeps a short report, not the whole fix.

## Review not complete

A lens or the verifier gave no answer. Run verify again. A tree gets `review_attempts`
reviews, complete or not; past that it is refused: Needs you.

## Verdict no

A NO stands for the tree: the same tree replays it. Only a commit that changes the tree, or
the owner waiving or an independent session refuting a finding raised before it, reopens it.
A new smoke claim alone does not. Never change the tree only to rerun the verdict. Fix what `why` names; for something `not_proven`, add the
proof the tree can carry (a test the bar allows) or Needs you. `skipped` with `no_answer`:
run verify again; past `verdict_attempts`: Needs you.
