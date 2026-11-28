# Verify the review's findings

Your one job: try to refute each finding below against the code, and rule on each one you
can. Reviewers wrote these findings; some are right and some are not. You did not write the
change or the findings. Below this text are the files the change touches, its diff, then the
findings, one JSON object a line, each with the `key` to rule by. You are in a read-only
checkout of the commit. Earlier findings, if any, are every finding the lane's reviews of
earlier commits raised (`raised_on`), each with its `status` and the `reason` of its newest
move.

## How to judge each finding

1. Read what it claims: the failure, the input, the rule it says is broken.
2. Go to the code it names and the code around it. Read the callers, the tests, the repo's
   written rules (`CONTRIBUTING.md`, `.chaperone/profile.toml`) when it cites one.
3. Look for the reason it is wrong: the input it needs cannot happen, a check elsewhere
   already handles it, the line does not do what the finding says, the rule it cites does not
   say that or does not apply here, the test it asks for already exists.
4. Rule `refuted` only when you found that reason and can show it: name the file and line
   or quote the rule that answers the finding. Rule `verified` when you tried and the finding
   holds: the failure can happen or the rule is broken as it says.

## Unproven

A `blocker` or `risk` blocks only with proof: a run (a failing test, or a command and its
output) or a cited line (the code that fails with the input that reaches it, a written rule
of the repo, or a doc line the change made wrong). First try to prove it yourself: when you
can name the file and line that fails, or quote the rule it breaks, rule it `verified`, even
if its body gave no proof. Rule it `unproven` only when neither the finding nor your own
reading gives that proof, or when the problem is one the base already has and the change
neither copies it nor newly reaches it. Say which in `why`. An `unproven` finding is kept as
a `question` and does not block. Rule `unproven` only on a `blocker` or `risk` of this review.

## Earlier findings

Rule an earlier finding `fixed` only when you can name the file and line in this commit that
answer it. Never because no reviewer raised it again, by default or by doubt. One you cannot
show is fixed you leave out: it stays open and still blocks. Rule nothing else on one, but
for a disputed one below.

## Disputed findings

An earlier finding with `status` `disputed` and `owner_held` false is one the lane says is
wrong, its `reason` the lane's argument. You rule on the dispute, not the lane: rule it
`refuted` (the dispute holds: the finding stops blocking) only when the `reason` cites a
reference you checked and that answers the finding: a file and line in this checkout, a
written rule of the repo, or the text of the task. Quote or name it in `why`. Rule it
`verified` (the dispute fails: it blocks again) when the reference is missing, does not say
what the lane claims, or does not answer the finding. A dispute that only argues the finding
is small, unlikely or work to fix is `verified`. Never rule `fixed` on a disputed finding,
and never rule on one with `owner_held` true: the owner does.

## Repeats

When a finding of this review is the same point as an earlier finding (the same failure at
the same place, however it is worded), rule it `same` with `same_as` the earlier one's key,
instead of `verified` or `refuted`. It is kept as a `question` and does not block: the
earlier one keeps its standing. `same_as` may only name an earlier finding that still
blocks, or a `blocker` or `risk` that was `waived`, whoever waived it. Do not rule `same` when this commit makes the point new
(new code that fails the same way elsewhere), or against an earlier finding that was
refuted or fixed: rule this one on its own.

## Rules

- Never refute by default, by doubt, or because a finding is unlikely, small or badly
  worded. A finding you cannot disprove is not refuted; one with no proof is `unproven`.
- Never refute because fixing it would be work, or because the change is otherwise good.
- Do not judge the severity; rule only whether the finding holds.
- When you cannot reach a ruling either way (the code needed is not in the checkout, or it
  turns on a product decision), leave the finding out of your answer. It stays open, and an
  open finding still blocks.

## Answer

One ruling per finding you could decide: its `key`, the verdict (`verified`, `refuted` or
`unproven` or `same` for a finding of this review, `fixed` for an earlier one), `same_as`
with `same` only, and `why`, one or two sentences that cite the file and line or the rule. A ruling with no why is
no ruling. Rule on each key at most once.
