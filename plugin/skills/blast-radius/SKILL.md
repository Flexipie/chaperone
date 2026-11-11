---
name: blast-radius
description: Before a PR, find everything a branch's change reaches that the diff doesn't show - callers, stale comments and docs naming an old value, wire formats, stored data - and the one fact that makes the change safe, proved by a run. Read-only; returns a short list to fix
allowed-tools: Read, Grep, Glob, Bash
context: fork
agent: general-purpose
background: false
---

# Blast radius

Read-only. Change nothing; report what the change reaches and whether it's safe.

## Input

$ARGUMENTS

The worktree and base (default `origin/main`). Diff: `git diff <base>...HEAD`.

## Steps

1. **Read the whole change**, including what it changes implicitly: a default, an order,
   a removed or skipped call, a renamed field, a changed type or unit.
2. **Old names and values.** For every identifier, constant, string, default or decision
   the diff changed or removed, `git grep` the old one across the repo. List each comment,
   doc, test name or caller that still names the old one.
3. **Where grep stops.** Check what text search misses: wire formats and API payloads,
   stored data (DB columns, saved files, caches, local storage) written by the old code,
   feature flags, config, other languages or packages, generated code.
4. **Safe because.** Name the one fact the change is safe because of (for example: "no
   stored row has the old shape", "every caller goes through `foo()`"), and prove it with
   a run or a `file:line` trace. Say which: run, trace, or unproven.

## Report (under 250 words)

```
stale: <file:line> names <old> (fix in this slice | outside it)
reaches: <caller or data the diff doesn't show> -> <ok, why | risk, what breaks for whom>
safe because: <the fact> (<run: command and result | trace: file:line | unproven>)
```

`NO FINDINGS` with the "safe because" line is a complete answer.
