# Findings

## The bar

Only a finding with `blocks` (a proven `blocker` or `risk`, not settled) or a check failing
on this head holds a lane; no other finding, gate or round does. One that does not block is
never fixed alone and never earns a verify: `pr open` lists it in the PR body as open. A
comment one you delete or shorten in a commit you make anyway, never disputing it.

## Read them

- Review: `chaperone finding list --work <N>` lists every finding of every complete review
  of the lane, any commit's: `check_event_id`, `finding_key`, `severity`, `title`, `body`,
  `path`, `line`, `head_sha` (the commit it was raised on), `status`, and `blocks`, true for
  exactly the ones verify's "N finding(s) still block" counts. Work the ones with `blocks`.
- Verdict: `chaperone check run verdict --work <N>` replays it: `decision`, `why`,
  `not_proven`.

## Each blocking finding

Weigh it (`judgement.md` beside this file, "Weighing a finding"), then one of:
- It holds: fix it at the root and commit. Nothing to record: the next verify reviews the
  new tree, and its verifier rules an earlier finding `fixed` only when it can name the line
  that fixes it. One it cannot stays open and blocks.
- It is wrong: `chaperone finding dispute <event_id> <finding_key> --reason "<the reference that answers it>"`,
  the reference a `file:line`, a repo rule or the task's text, which the verifier checks.
  No commit is needed: the next verify's verifier rules on it. `refuted`, the dispute holds:
  it stops blocking and the PR body lists it with the ruling, which the owner may overturn.
  `verified`, it failed: fix it, or, if you still contest it, dispute again, which is the
  owner's to rule, so Needs you. A finding titled `owner:` (a one-way door, a gate that fails
  open) and one the owner overturned are the owner's too: dispute it and Needs you, naming
  it and the command (`chaperone finding waive <event_id> <finding_key> --kind disproved
  --ref <ref> --reason <why>`). Never argue in prose, the PR or the handoff.

Fix every blocking finding at once, then back to step 5 (smoke) and verify once. A third verify
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
