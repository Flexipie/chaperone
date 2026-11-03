# Vision

Status: draft, 2026-11-02. Everything here is a hypothesis until the research in
[RESEARCH.md](RESEARCH.md) confirms or kills it.

## Where this comes from

Since October 2026 one person (me) has run dispatch "lanes": a parent issue split into
slices, one coding agent per slice in its own git worktree, each planning, building,
smoke testing, opening a PR, going through review rounds and handing off. About 150
lanes have run through 17 versions of that process. It works, and it has outgrown the
pieces it is built from:

- **The process lives in prose.** Skills (`/dispatch`, `/issue`, `/ship`, `/merge`,
  `/orchestrate`) tell agents what to do. The steps left to an agent's judgement get
  skipped: in v16 the plan check ran in 6 of 40 lanes, `diagnose` never ran, and 13 of
  28 PRs merged with their last review not on the head. The one step that never gets
  skipped is the one a script enforces (`ship-handoff` refusing a handoff).
- **State is scattered.** Work items, specs, handoffs and reviews in daisy (a personal
  desktop app), the process and its versions in dotfiles, the runtime in herdr, the
  evidence in agent transcripts and GitHub. Lanes stop when the desktop app is closed.
- **Every agent CLI needs its own adapter.** Claude Code, Codex and Cursor each load
  rules, hooks and subagents differently; Cursor only reads subagents from inside the
  repo, and drops skill front matter. Each new CLI is another week of glue.
- **Measuring is reconstruction.** Whether a process change helped is answered by a
  720-line script guessing from four sources after the fact.
- **Waiting and coordination are expensive.** Agents poll, re-run reviews before
  answering the last one, and learn about each other's changes late.

## The goal

A local-first harness that makes many agents working in parallel **trustworthy,
cheap and measurable**, without writing its own agent.

It doesn't replace Claude Code, Codex or Cursor. It runs them, gives them the state and
rules they need, holds the gates they must pass, connects them to each other, and
records everything as evidence.

## What would make it different (hypotheses to test)

There are many agent orchestrators. Most are a UI over worktrees and terminals, or a
new agent loop. These are the bets that could make this one worth building; the
landscape research has to show which are really unserved.

1. **Process as code, versioned and measured.** The pipeline (lane states, gates,
   evidence each gate needs) is a versioned, reviewable definition, not prompt text. A
   lane can't reach "ready" without a plan check, a quality gate and a merge verdict on
   its head commit, because the harness refuses the transition. Every lane is stamped
   with the process version it ran, so a process change is an experiment with a
   measured result (rounds, escapes, tokens, my attention), not a feeling.
2. **Bring your own agent.** One small integration contract (context in, tool and gate
   calls out, lifecycle events) that any agent CLI can meet through its own hooks, plus
   adapters for Claude Code, Codex and Cursor on day one. Model and vendor choice stays
   per lane.
3. **An event-sourced ledger.** Every claim, plan, review, finding, gate, verdict,
   message and merge is an append-only event. Metrics are queries, any lane can be
   replayed, and nothing is reconstructed from transcripts.
4. **Lanes that know about each other.** File and seam ownership, early warning when
   two lanes touch the same code, and a typed channel between lanes ("I changed this
   signature", "rebase on mine") instead of everything going through a human.
5. **Attention as the scarce resource.** The harness decides what reaches me and when:
   a lane only asks me what the spec, the code or another lane can't answer, and "ready"
   means checked, not claimed.
6. **Cost as a first-class number.** Waiting is event-driven instead of polling,
   context size and token spend are tracked per lane and per process version, and
   budgets can stop a lane before it burns a day.
7. **Fast and boring to run.** One local binary, instant startup, works offline, no
   cloud account, no daemon zoo. Performance and reliability good enough that it is
   never the reason a lane stalled.

## Non-goals (for now)

- Its own LLM agent loop or model hosting.
- A hosted SaaS or multi-tenant service.
- A chat UI. The agents' own terminals stay the place to talk to them.
- Replacing the issue tracker or GitHub. It reads and writes them; they stay the source
  of truth for issues and code.
- Reimplementing daisy's personal hub (notes, inbox, mirrors).

## Where it could go

Ordered roughly by when it becomes worth doing, each gated on the one before working:

1. **Ledger and gates.** The work and spec ledger out of daisy, with enforced lane
   states, keeping the CLI contract today's skills use so nothing breaks.
2. **Events, not polling.** Agents and the orchestrator subscribe to "review done",
   "CI green", "sibling merged".
3. **Lane-to-lane coordination.** Ownership, overlap warnings, typed messages.
4. **Process versions as experiments.** Built-in metrics per version, and side-by-side
   versions (two process variants on similar slices) to learn faster.
5. **Verification layer.** Independent reviewers and verdict agents as first-class
   steps, with smoke evidence (screenshots, logs, command output) stored and linked.
6. **Runtime.** Owning the terminal and worktree runtime (or integrating deeply with
   herdr), with ports, resources and sandboxes per lane.
7. **Interfaces.** A TUI and a local web view of all lanes: what needs me, what is
   ready, what is burning tokens.
8. **Open source.** If it holds up, released with the quality bar below.

## Quality bar

The bar is a serious open-source project, not a personal script collection:

- Every architectural choice has a written decision record with the alternatives and
  the evidence.
- Correctness first: the ledger and the gates are the product, so they get property
  tests, crash and concurrency tests, and a fake agent CLI to run whole lanes in tests
  deterministically.
- Performance has budgets (startup, event latency, memory per lane, events per second)
  and benchmarks in CI that fail on regression.
- Public interfaces (CLI, integration contract, event schema) are versioned and
  documented before they are implemented.
- Security is designed in: secrets never in the ledger, review text and tool output
  treated as untrusted data, least privilege per lane.
- It is dogfooded: from milestone 1 on, its own development runs in its own lanes.
