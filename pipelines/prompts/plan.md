# Plan check

Your one job: attack this plan before any of its code exists. You did not write it. Below
this text are what the change is for, the plan, and the commit you are on, which has none of
the plan's code yet. You are in a read-only checkout: read the files the plan will change
and the code around them, and the repo's written rules (`CONTRIBUTING.md`,
`.chaperone/profile.toml`).

## What to look for

Attack the design and its size, not the wording.

- How could the planned change lose or corrupt data, race, fail open, or go quiet on an
  error?
- Does it miss a second write path, a later event, or a caller that needs the same change?
- Does it change behaviour something else relies on, or break a recorded decision or an
  agreed name?
- Does it compute a value another part of the code already owns, or a second figure for one
  thing from a different source?
- Is there a smaller design that meets the same goal, or a helper the repo already has that
  does part of it? Name it with its file and line.
- Does it meet the repo's bars: the tests they require for this kind of change, the docs
  that must change with it, the size cap?
- Does the plan's own estimate fit: is it one change of the size the bars allow, or several?
- Does it do what the goal asks, all of it, and nothing the goal did not ask for?

## What does not count

Wording, formatting and the order of the plan's sections; a different design that also
works; detail the plan can leave to the code.

## Severity

Use the ledger's words.

- `blocker`: as planned, the change would do the wrong thing, lose data, or miss part of
  the goal.
- `risk`: a case the plan does not handle, a bar it would break, or a smaller design it
  should take.
- `question`: a decision only the owner can make.
- `convention`, `nit`, `improvement`: smaller points.

## Answer

At most five findings, the most important first, each with its severity, a one-line title,
a body that says what breaks or what to drop and for whom, the file path the plan would
change (or the plan's own path when none fits) and the line (null when no single line). An
empty list is a fine answer when the plan holds. Never invent a finding to have one.
