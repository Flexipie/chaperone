---
name: diagnose
description: Find the root cause of a bug, a failing test, smoke or check before anyone changes code, then make the smallest complete fix. Use instead of guessing at a fix when the cause isn't obvious from the output. Runs in a fresh agent and returns the root cause as file:line with proof
allowed-tools: Read, Grep, Glob, Edit, Write, Bash
context: fork
agent: general-purpose
background: false
---

# Diagnose

Find where the system first went wrong, prove it, and fix it there with the least code.

## Input

$ARGUMENTS

The symptom (error, failing command, ticket text), the worktree, and whether you may
fix (`--fix`) or only report. No `--fix`: report only, change nothing.

## 0. Cheap first

One read-only pass: read the error, the stack and the code it names. Check whether the
same check fails on the base branch, in a separate worktree; never switch this one. If this pass names the cause and a narrow fix with
evidence, skip to step 4.

## 1. A loop that shows the bug

Build one command you have already run at least once that shows the bug red: the
smoke, the failing test, or a CLI or HTTP call against the local setup. It must be
deterministic, fast and runnable by an agent. No loop, no hypotheses: if you can't make
one after a real attempt, report what you tried and stop.

## 2. Shrink it

Cut inputs, steps and data until removing anything more makes the bug go away. The
smallest failing case usually points at the cause.

## 3. Hypotheses

Write 3 to 5 ranked hypotheses, each with the result that would disprove it. Test the
cheapest first, one at a time, with tagged debug output (`[diag]`). Ask "why" until the
answer is a wrong decision in the code, not a consequence of an earlier one.

- A hypothesis the result disproved: revert anything it motivated.
- Two fixes on the same premise failed: write the premise down and test it before a third.
- Three hypotheses disproved, or the fix would touch more than 5 files: stop and report.

## 4. Fix (only with `--fix`)

Fix at the root cause, in the shared function every caller goes through rather than a
guard in each caller. Look for the same fault next to it; fix instances inside the task,
list the rest. Keep one test that fails without the fix, if the repo's test bar allows a
bug-fix test; show it red on the unfixed code. Re-run the loop from step 1 and show it
green. Remove all `[diag]` output. Commit with a short plain message naming the root cause.

## Report (under 250 words)

```
root cause: <file:line> <the wrong decision, one line>
evidence: <the loop command> red before, green after (or: not fixed)
fix: <commit sha, what changed> | not fixed: <why>
same fault elsewhere: <file:line list, or none>
rejected: <one bigger fix you didn't make, and why>
open: <anything that needs a product call>
```
