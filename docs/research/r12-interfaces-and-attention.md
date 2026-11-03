# R12: Interfaces and attention

Status: recommendation, 2026-11-02. The screens, the "needs you" rules, and the first interface. Evidence for a decision record. Builds on [VISION.md](../VISION.md) bet 5, [r1-landscape.md](r1-landscape.md) bet 5, and [r2-own-evidence.md](r2-own-evidence.md).

## Pick

**The first interface is a CLI query, `seamlane attention`.** It prints four queues from the ledger: needs you, ready, stuck, expensive. A later TUI, a later local web view, and daisy all render that same JSON. None of them classifies a lane on its own.

Notifications are not a fifth interface. One subscriber reads the same rows and may call `herdr notification show`. The agent process does not notify.

Bet 5 is a query once gates have evidence ([r1-landscape.md](r1-landscape.md)). A dashboard before that query is the surface R1 already set aside. Milestone 7 in the vision is when a resident view is worth building. Milestone 1 still speaks the CLI the skills use.

## What the queues are

A row is a fold of the ledger, the same shape as an R7 fact bundle. `next_action` is the one-line body. It does not choose the queue. [R11](r11-observability-cost-metrics.md) still counts `needs_you` as "the latest `next_action` starts with `needs you`, or status is `blocked`". That metric stays. This screen is narrower on purpose: a blocked lane is stuck, and a handoff that says `ready for you` while the phase is not `ready` is working.

| Queue | Holds when | What I do with it |
| --- | --- | --- |
| **Needs you** | Phase is `needs_you` and the reason is in the closed set below. | Answer a question the spec, the code, and the other lanes cannot close. |
| **Ready** | Phase is `ready`. `to_ready` has already required `ready_evidence` ([r7-process-engine.md](r7-process-engine.md)). | Merge, or say no. The score is on the row. Under 8 does not move the row. |
| **Stuck** | Overlay `blocked`, or the agent has exited, or the same `gate.denied` has fired twice on one sha, or a host the review depends on is down. | Usually nothing, until recovery is spent. Then one look. |
| **Expensive** | A working, stuck, or ready lane has crossed a cost line from R2. A flag, not a phase. | See it. Only the 500k peak interrupts. |

Working, queued, and done are counts on the board. They have no queue of their own and they do not notify. `in review` from today's `lanes` script (CI running, a review pending, threads the lane can answer) is working.

### Needs you, the closed set

New reasons are a pipeline change. A free-text "needs you" from the agent is not a member. The engine writes the reason when it enters `needs_you`, or when it accepts `question.opened` from a runner that is not the lane session (the same independence rule as [R10](r10-verification-review-layer.md)).

| Reason | Entered by | What I am asked |
| --- | --- | --- |
| `question.done_ambiguous` | `/issue` stop: "done" has two readings that change the code. | Which reading. |
| `question.scope` | Real work outside the slice's "done when". | Redraw the slice, or cut it. |
| `question.one_way` | Money, a migration or backfill, prod data, a user's files, a delete, credentials, or anyone outside the lane. The profile's Domain brief names the repo's list. | Whether to do it. |
| `question.recorded_decision` | The plan leaves a decision in the ticket, the spec, or the comments. Asked before code, which is the CORE-3897 miss (asked after 1,600 lines). | Keep the decision, or replace it. |
| `question.size` | `oversize_stop` when `diff.code_lines` is over the pipeline limit (750 in the v17 document), or the plan estimate is over 1.5× the slice (or past about 900 code lines; about 600 on a lone follow-up). | Admit the size, or take the split proposal. |
| `question.investigation` | `investigation_needs_you`: the decision list is present, one local review, plan check recorded. | File the children, or drop them. |
| `question.verdict_no` | `verdict_no_at_cap`: verdict NO and the local-round cap is spent. | Settle the reason, or stop the lane. |
| `question.product` | A plan-check finding the runner marked as a product call. | The call. |
| `question.seam_reject` | The reader called `seam.reject` ([r9-coordination-between-lanes.md](r9-coordination-between-lanes.md) case 10). | The new shape changes the other lane's spec. |
| `question.overlap` | One `overlap.needs_human` because the spec lists two owners for one path (case 2). | Fix the spec. Neither lane is granted the path. |

`question.overlap` is one row for the path set, shown on both lanes, one interrupt. `question.seam_reject` is the rejecting lane only. The owner is not notified, which is the R9 rule. A normal path deny (the other lane holds it, cases 1 and 5) is stuck, with an empty inbox for the holder. `seam.revised` (case 8) stays in building until the reader acks. It is not a question.

These are the asks `/orchestrate` already keeps for a person (one-way door, product choice, scope, a seam that changes another spec, someone outside the lane). The difference is the engine writes the reason. The watcher prompt is no longer the classifier.

A missing gate is not a reason. R2's criterion 10 is at most one needs-you per 25 lanes, and none of them caused by a missing gate. v16 was 1 of 39: `CORE-4370`, an admission on a 2,188-line lane, which is `question.size`. A lane that skipped plan check, the quality gate, or the verdict stays in its phase on `gate.denied`. It does not become needs you so that a person will supply the stamp.

### Ready

Ready means the checks passed. Today's `/ship` notification fires on the handoff text `ready for you`. v16 handed off 22 lanes that way, and 11 of those were not reviewed on the head. The board shows a lane here only after `phase.entered` / `ready`.

The row carries the merge case, the local score, the verdict sha, and the PR. `ready_under_8` (5 lanes in v16) stays in this queue. The score is printed. A second queue for "ready but the score is low" would be a feeling about a gate the engine already evaluated.

`human.merge_yes` is the answer to this queue. It is not a new question.

### Stuck

Stuck is a lane that cannot move, where the next move is mechanical or belongs to another lane.

| Blocker | Examples | Recovery before I hear about it |
| --- | --- | --- |
| `blocked` on a path, a seam pin, a lease, or a foreign PR | R9 cases 1, 4, 5, 16. Resume phase stored. | Clears when the holder finishes or the lease expires. No mail to the holder. |
| Agent exited or stalled | R8: one `resume-once` per six hours, then blocked. Today's `/orchestrate` nudge is once per state. | Nudge once, revive once per six hours. |
| Same `gate.denied` twice on one transition and one sha | The runner was sent back and the gate still fails. This is today's "ready, not green" after two send-backs. | Two send-backs, then the row. |
| CI failing, conflicts, a second known-flake failure | The lane can rebase, fix, or cite the flake. | The lane handles it. A second identical failure after that is the gate-denied row above. |
| Host down | Daisy closed while `review.daisy` is required. [R10](r10-verification-review-layer.md): closed is a blocker, not "no review needed". | One host row, not one row per lane. |

`threads.human_open` keeps the lane out of `ask_verdict`. The lane answers the thread. A thread becomes needs you only when the runner files one of the question reasons above. A review comment is not automatically my question.

### Expensive

R2's largest cost never asked for a person. 21 lanes peaked over 400k and hold 1.88B of 4.99B parent context (38%). The context hook warns the agent at 300k, 500k, and 700k, and stops nothing. v16's only needs-you was also the lane that peaked at 525k (`CORE-4370`). `CORE-4264` peaked at 442k and never asked.

The flag is computed from R11 fields. It does not add a gate. R11 leaves the stop threshold to R7.

| Flag | Line | On the board | Interrupt |
| --- | --- | --- | --- |
| `peak_context` | over 400,000 | Yes | No. Twenty-one historical lanes would have sounded. |
| `peak_context` | over 500,000 | Yes | Once per lane, when the sample first crosses. R2 criterion 8 is zero lanes over this line. |
| `code_additions` | over 600 | Yes, while the lane is still under the pipeline size stop | No. Crossing the pipeline limit is `question.size`, which already interrupts. |
| Flat rounds | 2 or more on this lane | Yes | No. |
| Re-raised findings | 1 or more | Yes | No. |

A lane in needs you or ready keeps that queue. The flag is a column on the same row. Crossing 500k and later entering needs you are two reasons, so they are two interrupts. `CORE-4370` would have sounded at 500k, and again on the size question. A ready lane that is also expensive sounds once, for ready, and the peak is on the row.

## What may interrupt

An interrupt is one `herdr notification show`, with a sound, for one identity. The identity is `(audience, reason, subject)`. The same identity does not fire again. A new reason, a new head sha, or a new blocker id is a new identity. Clearing the row (phase leaves `needs_you` or `ready`, the blocker clears, a decision is recorded) retires it. A peak that falls under 500k and later crosses again may fire again.

| Event | Sound | Title |
| --- | --- | --- |
| Needs you, any reason in the closed set | `request` | `<KEY>: needs you` |
| Phase enters `ready` | `done` | `<KEY>: ready` |
| Stuck, and the recovery in the table above is spent | `request` | `<KEY>: stuck` |
| Setup failed before the agent started | `request` | `<KEY>: stuck` |
| Daisy closed, at least one review waiting | `request` | `daisy closed` |
| `peak_context` crosses 500,000 | `request` | `<KEY>: expensive` |

The body is one line: the question, the merge case, the blocker, or the peak. Overlap uses one title for the path set (`<KEY-a>+<KEY-b>: needs you`). The seam owner gets nothing on `seam.reject`.

Everything else waits on the board:

- CI running, a review running, an idle agent before the nudge, a lease that has not expired.
- A sibling blocked on a path I do not hold. `seam.revised` waiting on an ack.
- A peak between 400k and 500k, flat rounds, re-raised findings, code over 600 while the size gate has not fired.
- A handoff that claims `ready for you` while the phase is not `ready`.
- A score under 8 on a lane that is actually ready.
- A lane reaching `done`, including a no-PR measure. Done is a count. A result I must act on is `question.investigation` or another needs-you reason, not a done sound.
- The final "all lanes done" line. It is a log row. I asked to be told when something needs me, not when the board is empty.

Today three places call `herdr notification show` themselves: `/issue` on every block, `/ship` on every `ready for you` and `needs you` (sound `done` for both), and `/orchestrate` after its own re-check. Those calls retire when the subscriber exists. Until then, `/orchestrate` is the only one that may notify, and only by reading this query. `/issue` and `/ship` record the event and stay quiet. A lane that notifies because the skill text said so is the prose path bet 1 is leaving.

Delivery stays local. herdr is the notifier while herdr is the viewer ([r8-process-terminal-runtime.md](r8-process-terminal-runtime.md)). If `herdr` is not on `PATH`, the row still sits on the board. Slack, a phone push, and a second sound from the agent session are out. Cursor and Devin already reach Slack. That is a status ping, which is the half of bet 5 R1 called already served.

## Screens

The CLI is the screen. `--json` prints the same object the later clients render. Sort is needs you, ready, stuck (recovery spent first), expensive by peak descending. Working, queued, and done are one count line unless `--all`.

### Board: `seamlane attention`

```
needs you  1
  CORE-4370   question.size        2188 code   admit, or split as proposed
ready  1
  CORE-4394   #3140  local 9   YES abc1234     merge
stuck  1
  CORE-4383   agent.exited         resume spent, last: reviewing
expensive  1
  CORE-4264   peak 442k            working     silent until 500k
working 4   queued 2   done today 3
```

A row that is ready and expensive does not appear twice. The peak is on the ready line, and the expensive count still includes it.

### One lane: `seamlane attention show CORE-4370`

```
CORE-4370   needs you   question.size
2188 code lines, pipeline limit 750. Peak 525k. Handoff score 6.
6 flat rounds, 13 re-raised, 6 commits after the handoff.

Question
  Admit the size, or take the split proposal in open_questions.

Evidence
  open_questions: <the split, one line each>
  look_at: the brief's three entries, path:line
  verdict: none. This lane is not ready.

Answer
  seamlane attention answer CORE-4370 --decision "<one line>"
```

The answer command appends the decision and leaves `needs_you`. It does not paste into the PTY. Until milestone 2's subscription, it prints the one line a person can pass to the lane. The transcript stays in the agent's terminal. This screen is the question, the few evidence fields the projection already stores, and the command that closes it.

`show` on a ready lane prints the merge case, the smoke line, the scores, the verdict sha, and `human.merge_yes` as the next command. It does not print a diff.

### What waited: `seamlane attention log`

```
13:10  CORE-4264   peak 442k          silent
13:22  CORE-4398   gate.denied        verdict_on_head, phase stays verdict
13:40  CORE-4387   seam.revised       reader still building, owner not told
14:01  host        daisy closed       2 reviews waiting
```

This is how a quiet afternoon is explained. Rows here are the events that did not meet the interrupt table.

## CLI, TUI, local web, daisy

| | First | What it is here | Why it waits |
| --- | --- | --- | --- |
| **CLI** | Yes | `seamlane attention` and `show`. Read-only except `answer`. | It is the query. Warm start of a Rust CLI is already 3.9 ms ([r6-language-and-runtime.md](r6-language-and-runtime.md)). `lanes` is the glance I run now. Skills and `/orchestrate` already speak CLI. |
| **TUI** | No | The same four sections in a pane, milestone 7. A client of the ledger, beside `seamlane attach`. | herdr is already the terminal, with a sidebar lane badge. R8 leaves the needs-you screen at milestone 7. A second full-screen UI before the query exists copies `lanes`. The TUI does not embed the agent session. Attach stays the conversation. |
| **Local web** | No | The same JSON, on the supervisor's local socket, milestone 7. No account, no remote. | R1: a fleet dashboard that pulls you in when an agent stops is the served half of bet 5, and "ready" there means the agent stopped. Superset, Conductor, and Vibe Kanban already occupy that surface. A page also wants a listener, which bet 7 is trying not to multiply. |
| **Daisy client** | No | Daisy opens the ledger read-only ([r5-state-and-storage.md](r5-state-and-storage.md)) and may render these four queues later. | The ledger is leaving daisy. Daisy closed is exit 5, and today's `lanes` stops when that happens. The Work page ready rule is one of the ways a lane looked ready before the head was reviewed. The personal hub (notes, inbox, mirrors) stays a non-goal. |

The TUI and the web are views. If their buckets ever differ from `seamlane attention --json`, the view is wrong.

## What I can check

The board and the interrupt list are a pure function of a fixture of events. One test, the same shape as R7's `to_ready` denial:

1. A `phase.entered` / `needs_you` with `question.size` produces one needs-you row and one interrupt.
2. A handoff whose `next_action` starts `ready for you`, with no `phase.entered` / `ready`, produces a working row and no interrupt.
3. Two `gate.denied` on the same sha produce one stuck interrupt. The first denial produces none.
4. `overlap.needs_human` produces one interrupt for the pair. A path deny against a live holder produces a stuck row and no interrupt.
5. `peak_context` 442k is an expensive row and no interrupt. 525k is one interrupt. A later sample still over 500k is not a second one.
6. Status `blocked` increments R11's `needs_you` metric and does not enter this needs-you queue.

## What this does not decide

- The dollar or peak gate that stops a lane. R11 owns the fact (`peak_context`). The threshold, if it becomes a refusal, is an R7 pipeline change. This note only says when I hear about it.
- Replacing herdr's notifier. The subscriber calls `herdr notification show` until a harness notifier exists.
- The TUI layout, the web stack, and daisy's Work page. They consume the JSON when milestone 7, or a daisy change, is actually picked up.
- How an answer reaches the agent process. Milestone 2's subscription does that. `answer` records the decision.
- Quiet hours, a phone, Slack. Not on this machine's evidence. 26 needs-you lanes across the whole history, and 1 in v16, is a small interrupt budget. Widening the channel is how CI failures get back into the queue.
- Redefining R11's `needs_you` field. The screen and the metric disagree on `blocked`, and the metric stays the one R2's criterion 10 is scored on: a real ask, at most one per 25 lanes.

## Sources

Read 2026-11-02.

- [VISION.md](../VISION.md) bet 5, the chat-UI non-goal, milestones 1 and 7.
- [RESEARCH.md](../RESEARCH.md) R12. [r1-landscape.md](r1-landscape.md) bet 5 (status surfaces served, the interrupt rule unserved). [r2-own-evidence.md](r2-own-evidence.md) needs-you counts, `CORE-4370`, the 400k band, criterion 10. [r7-process-engine.md](r7-process-engine.md) `needs_you` transitions, `to_ready`, `blocked`. [r8-process-terminal-runtime.md](r8-process-terminal-runtime.md) attach versus the needs-you screen. [r9-coordination-between-lanes.md](r9-coordination-between-lanes.md) cases 1, 2, 5, 8, 10, and the open R12 question. [r10-verification-review-layer.md](r10-verification-review-layer.md) daisy closed is a blocker. [r11-observability-cost-metrics.md](r11-observability-cost-metrics.md) `needs_you`, `ready_under_8`, `peak_context`.
- `~/.claude/skills/issue/SKILL.md` Modes and the stop-blocked notification. `ship/SKILL.md` size stop, verdict NO, sibling re-check, the handoff notification. `orchestrate/SKILL.md` sections 4a, 4e, and 5. `lanes/SKILL.md`.
- `~/dotfiles/herdr/local-plugins/next-ticket/lanes.sh` the classifier that puts CI, conflicts, a missing agent, and an idle agent into `needs you`.
- `~/dotfiles/claude/pipeline-changelog.md` v14 (what `/orchestrate` was expected to move) and v17.
