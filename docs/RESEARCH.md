# Research before we build

Each item answers one question that changes what we build. Done means its output is
written in `docs/research/<id>.md` with sources, and any decision it forces is a
decision record in `docs/decisions/`. Order matters: R1 to R3 come first because they
decide whether the bets in [VISION.md](VISION.md) are worth making at all.

## Positioning

**R1. Landscape: what already exists, and what nobody does well.**
Survey agent orchestrators and harnesses (for example Superset, Conductor, Claude
Squad, Vibe Kanban, OpenHands, Devin, Factory, Amp, Codex and Cursor cloud and
background agents, gstack, pstack, and agent skill collections). For each: what it
orchestrates, how it holds state, whether it enforces process, how it integrates agent
CLIs, how it measures outcomes, license, activity. Output: a matrix, and which of the
seven bets in VISION are unserved, partly served or already done well.
Done when: every bet has a verdict with evidence, and we can say in one sentence why
this project should exist.
Output: [research/r1-landscape.md](research/r1-landscape.md).

**R2. Our own evidence: where time, tokens and quality go.**
Mine the ~150 lanes, their transcripts, reviews and lane-stats reports (v1 to v17).
Rank the problems by cost: rounds, re-raised findings, escapes, tokens, polling, my
interventions, time to merge. Output: a ranked list of the problems the first
milestone must move, each with its baseline number.
Done when: milestone 1's success criteria are numbers from this list.
Output: [research/r2-own-evidence.md](research/r2-own-evidence.md).

**R3. Name and open-source model.**
Name (no collisions on GitHub, crates/npm, domains), license, and whether this is
built to be open source from day one or opened later.
Done when: a name and a license are picked.
Output: [research/r3-name-and-license.md](research/r3-name-and-license.md),
decision [decisions/0001-name-license-oss-posture.md](decisions/0001-name-license-oss-posture.md).

## Architecture

**R4. Agent CLI integration surface.**
For Claude Code, Codex, Cursor, Gemini CLI, opencode and Amp: headless and interactive
modes, hooks and their payloads, context injection, subagent definitions and where
they load from, skills, permission modes, session resume, transcript formats, token
reporting. Plus the protocols that could make this generic: MCP, the Agent Client
Protocol, A2A. Output: a capability table and a draft of the smallest integration
contract every CLI can meet.
Done when: the contract is drafted and checked against at least three CLIs.
Output: [research/r4-agent-cli-integration.md](research/r4-agent-cli-integration.md).

**R5. State model and storage.**
Event sourcing vs a state table with an event log; SQLite (WAL, single writer) vs
alternatives; schema for lanes, specs, gates, findings, messages; migrations; leases and
concurrency with many agents writing at once; crash safety; how daisy reads it.
Output: the data model and storage decision, with a load test of the expected write
pattern.
Done when: a prototype survives 50 concurrent writers and a crash mid-write without
losing or duplicating an event.
Output: [research/r5-state-and-storage.md](research/r5-state-and-storage.md).

**R6. Language and runtime.**
Rust, Go, TypeScript (Bun) or Zig, judged on: single static binary, startup time,
memory, concurrency model, SQLite and PTY libraries, plugin and adapter story,
contributor pool, and how much of daisy's Rust (`daisy-core`: work, spec, review) we
can reuse. Output: a decision record with a small benchmark of the hot paths (event
append, subscribe and fan-out, CLI startup).
Done when: the language is picked on measured numbers, not taste.
Output: [research/r6-language-and-runtime.md](research/r6-language-and-runtime.md).

**R7. Process engine: gates and pipeline versions as code.**
How to express the pipeline (lane states, gates, evidence each gate needs): a state
machine in code, a config language (TOML, CUE, Starlark), or a policy engine (Rego).
How a version is pinned per lane, diffed and rolled back, and how two versions run
side by side as an experiment. Output: a spec of the pipeline format, with today's v17
written in it.
Done when: v17 is fully expressible and a gate violation is caught by the engine in a
test.
Output: [research/r7-process-engine.md](research/r7-process-engine.md).

**R8. Process and terminal runtime.**
Own PTY and worktree management, build on herdr, or integrate with tmux or zellij. Port
and resource isolation per lane, sandboxing options on macOS and Linux, what happens
when an agent hangs or dies. Output: a decision, and the boundary between harness and
runtime.
Done when: we know who starts, watches and restarts an agent process.
Output: [research/r8-process-terminal-runtime.md](research/r8-process-terminal-runtime.md).

**R9. Coordination between lanes.**
File and seam ownership, overlap detection before code is written, a typed message
channel between lanes, and how to keep it from turning into costly chatter. Prior art:
multi-agent systems, CRDT and lease designs, git merge-queue tools. Output: a design
and the cases it must handle (two lanes, one file; a seam changed mid-flight; a stack).
Done when: the design handles the coordination failures seen in R2.
Output: [research/r9-coordination-between-lanes.md](research/r9-coordination-between-lanes.md).

**R10. Verification and review layer.**
How independent checks (plan check, quality gate, review rounds, merge verdict, smoke
evidence) become first-class steps the engine runs and records. Whether daisy's
reviewer moves here or stays a service. How evidence (screenshots, logs, command
output) is stored and linked. Output: the design and the reviewer's new home.
Done when: every v17 gate has a defined producer, evidence format and check.
Output: [research/r10-verification-review-layer.md](research/r10-verification-review-layer.md).

## Operations and quality

**R11. Observability, cost and metrics.**
Token and cost accounting across CLIs, context size tracking, OpenTelemetry for agent
work, which metrics are computed from events, and how process versions are compared
fairly. Output: the metrics list (today's lane-stats numbers plus new ones) and how each
is computed.
Done when: every lane-stats number has an event-based definition.
Output: [research/r11-observability-cost-metrics.md](research/r11-observability-cost-metrics.md).

**R12. Interfaces and attention.**
What I need to see while multitasking: what needs me, what is ready, what is stuck or
expensive. CLI, TUI, local web or desktop; notifications. Output: the screens and the
attention rules (what may interrupt me, what waits).
Done when: the "needs you" rules are written and the first interface is chosen.
Output: [research/r12-interfaces-and-attention.md](research/r12-interfaces-and-attention.md).

**R13. Security and trust.**
Threat model: secrets in prompts, logs and the ledger; prompt injection from review
text, PR bodies and tool output; agent permissions and sandboxing; supply chain of
adapters and plugins. Output: the threat model and the rules the code must follow.
Done when: each threat has a mitigation or an accepted risk.
Output: [research/r13-security-and-trust.md](research/r13-security-and-trust.md).

**R14. Engineering standards.**
Repo layout, CI, test strategy (unit, property, crash, concurrency, a fake agent CLI
for whole-lane tests), performance budgets and benchmarks, versioning and release,
docs, contribution rules. Benchmarked against well-run projects we respect. Output:
`CONTRIBUTING.md` and the CI plan.
Done when: the first code PR has a bar to meet.
Output: [research/r14-engineering-standards.md](research/r14-engineering-standards.md),
draft [CONTRIBUTING.md](../CONTRIBUTING.md).

**R15. Migration from today's setup.**
What moves from daisy (work, spec, review), dotfiles (skills, `ship-handoff`,
`lane-stats`, profiles) and herdr, in what order, keeping the CLI contract the skills
use so lanes keep running throughout. Output: the migration plan for milestone 1.
Done when: milestone 1 can ship without stopping any running lane.
Output: [research/r15-migration.md](research/r15-migration.md).

Synthesis: [ROADMAP.md](ROADMAP.md) (locked plan; [PLAN.md](PLAN.md) superseded).
