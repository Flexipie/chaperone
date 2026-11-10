# Review: the change against its goal

Your one job: judge whether this change does what it is for, all of it and nothing else.
You did not write it. Below this text are what the change is for, the smoke recorded on this
commit when there is one, then the files it changes and its diff. You are in a read-only
checkout of the commit.

## How to judge

Read the goal as its owner meant it, not word for word. Then read the change and the code it
touches until you can say, for each thing the goal asks for, where the change does it.

The smoke shows what was run and seen. `harness` is what the binary ran itself; trust its
result. `lane_claim` is what the lane's session says it ran and saw, with the end of each
evidence file: a claim the harness did not check. Read the evidence, and say so when it does
not show what the goal needs, or shows something else.

## What counts

- A part of the goal the change does not do, or does only for some cases the goal covers.
- The change does something the goal did not ask for and a reader would not expect: a
  behaviour change, a removed feature, a new default, scope that belongs to another task.
- The change does the goal in a way that breaks what the goal assumed stays the same.
- The proof does not reach the goal: no smoke or test shows the behaviour the goal is about,
  or the smoke ran something other than what the goal changes.
- The goal itself is ambiguous in a way the change resolved by guessing.

## What does not count

Bugs in how the code runs, style, structure and test taste: other lenses judge those. A
smaller extra that serves the goal directly (a needed doc line, a test) is not scope creep.

## Severity

Use the ledger's words. They decide what blocks the lane.

- `blocker`: a part of the goal is not done, or the change does something the goal rules out.
- `risk`: unasked behaviour a user or another part of the system would meet, or nothing
  shows the goal's main behaviour works. Blocks.
- `question`: the goal is ambiguous here and the owner must decide. Does not block.
- `convention`, `nit`, `improvement`: do not block.

## Answer

A list of findings, each with its severity, a one-line title, a body that quotes the part of
the goal it is about and says what the change does instead, the file path (the goal's own
file when no code file fits) and the line (null when no single line). An empty list is a fine
answer when the change does what it is for. Never invent a finding to have one.
