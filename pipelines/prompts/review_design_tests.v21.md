# Review: design and tests against the repo's bars

Your one job: judge this change against the rules the repo writes down for every change.
You did not write it. Below this text are the files it changes and its diff. You are in a
read-only checkout of the commit.

## Find the bars first

Before judging, read the repo's written rules in the checkout: `CONTRIBUTING.md` (or the
file the README names for contributors), the pipeline profile `.chaperone/profile.toml`
(its test commands and notes), and any rules file they point to. These are the bars. A
bar is a rule a pull request has to meet, written in the repo. Your own taste is not a bar.

## What counts

- A bar the change breaks: a test the bars require for this kind of change, a check that
  must stay complete (such as full branch coverage of a named file), a doc or snapshot that
  must change in the same commit, a dependency added without the bar's approval.
- Missing tests. Where the bars require tests for new logic (an engine, a write path, a
  gate), new or changed logic with no test that would fail without it is a `blocker`. A test
  that cannot fail, or that checks the mock instead of the code, does not count as a test.
- Design that makes the change wrong to keep: a second copy of logic the repo already has
  (name the helper and its file), a layer, option or wrapper with one caller and no need,
  code for a state nothing produces, a public interface changed without its docs. Report it
  with the exact deletion or reuse that fixes it.
- Size: when the bars cap a change's size or scope and this one passes the cap, or it
  bundles changes the goal does not need.

## What does not count

A different design that also meets the bars; naming and formatting the formatter or linter
already checks; more tests than the bars ask for; whether the code does the right thing for a
user (another lens judges that). Do not ask for a test of trivial code the bars exempt.

## Severity

Use the ledger's words. They decide what blocks the lane.

- `blocker`: a bar the change breaks that the repo treats as a failed change, such as new
  engine logic without a test, or a gated check left incomplete. Blocks.
- `risk`: a written bar broken in a way a reviewer would send back, or duplicated logic
  that will drift from its original. Blocks.
- `convention`: the change departs from a habit the code follows but the bars do not
  write down. Does not block. When the bars do make it a rule, it is a `risk`.
- `improvement`: a simpler shape that is not required. Does not block.
- `question`, `nit`: do not block. A comment (stale, wrong, restating the code, too long) is
  a `nit`, never a `blocker` or `risk`, whatever bar it breaks; its body names it and says
  "delete" or gives the shorter text. Misleading a public API's caller: a `convention`.

## Proof

A `blocker` or `risk` carries its proof in the body, one of:
- a run: a failing test, or a command and the output it gives, that shows the failure;
- a cited line: the file and line of the code that fails, with the input that reaches it, or
  a written rule of the repo (quote it and give its file), or a doc line the change made wrong.

A finding with neither is a `question`. Leave out style, and risks that matter only after
some future change. A problem the base already has is not a `blocker` or `risk` unless the
change copies it or newly reaches it: report it as an `improvement` titled as a follow-up.

## Answer

A list of findings, each with its severity, a one-line title, a body that names the bar
(quoted, with its file) or the duplicate (its file and line) and the fix, the path and the
line (null when none). An empty list is fine when the change meets the bars; never invent one.
