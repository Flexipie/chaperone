# R1: Landscape

Status: draft, 2026-11-02. Survey for the seven bets in [VISION.md](../VISION.md).
Star counts and push times are from the GitHub API (`gh`) on this date. Vendor
products with no public repo are dated from their current docs.

## Scale

- **Done well:** a product a stranger can install does this bet's distinctive claim.
- **Partly:** a product does a real slice, so rebuilding that slice would be wasted.
- **Unserved:** nothing shipping does the claim. Near misses are named so a later
  design does not rediscover them.

The distinctive claim is the whole bet. A prompt that says "mandatory", or a
script the agent is asked to run, does not by itself serve "the harness refuses
the transition".

## Matrix

Cells are the slice that matters for the bets. Evidence is in the notes and the source list.

| Product | Orchestration | State | Process enforcement | CLI integration | Measurement | License | Activity |
|---|---|---|---|---|---|---|---|
| [Superset](https://superset.sh) | One git worktree, branch, and terminal per task. A coordinator skill fans workers out through the CLI. | Workspaces, tasks, terminals, hosts. A terminal daemon keeps sessions across app restarts. Cloud API at `api.superset.sh`. | Status flips (working, done, blocked). The coordinator is prompted to check a worker's report. | Launches almost any local CLI in a terminal, including Claude Code, Codex, Cursor, Droid, Devin, Amp. MCP, CLI, and TypeScript SDK drive Superset itself. | Fleet status. No process-version metrics found. | Elastic License 2.0. Desktop use is free. Offering Superset itself as a hosted service is forbidden. | 15,838 stars. Pushed 2026-11-02. |
| [Conductor](https://www.conductor.build/) | Mac app. One workspace per independent task for Claude Code, Codex, Cursor, OpenCode. Several agents can share one workspace on purpose. Paid cloud workspaces. | Workspace holds the branch, worktree, chats, diff, PR, and gitignored `.context` notes. | UI path is review, PR, merge, archive. The agent decides when work is ready. | Spawns those four CLIs. Cloud API creates workspaces, sends prompts, reads transcripts. | No public process or cost-per-version metrics. The API guide tells clients to poll session status until idle. | Proprietary (Melty). Free local tier. Pro is $50/month and adds cloud, the API, and mobile. | Docs and pricing current in 2026. No public source repo. |
| [Claude Squad](https://github.com/smtg-ai/claude-squad) | One tmux session and git worktree per task. Any program via `-p`. | Local instance list plus tmux and worktrees. | Commit, push, pause, resume. No gates. | The configured shell command. Profiles for Claude, Codex, Aider, and others. No hook or event contract. | None. | AGPL-3.0. | 8,719 stars. Last push 2026-08-20. Maintainers said in March 2026 they work on it part time. |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | Kanban issue, then a workspace (worktree, branch, terminal, preview) running a coding agent. | Local records for workspace, session, and execution process. Hosted issues, comments, and orgs were turned off after the company shutdown. | A human moves the card and reviews the diff. The agent is not blocked by a missing check. | Ten-plus executors: Claude Code, Codex, Gemini CLI, Copilot, Amp, Cursor, OpenCode, Droid, and others. | Streaming execution logs. No outcome experiments. | Apache-2.0. | Company shut down 2026-04-10. Repo still moves: 28,561 stars, commits through 2026-10-28. |
| [OpenHands](https://github.com/OpenHands/OpenHands) | Agent Canvas over an agent server. Own agent, or an ACP agent, on the laptop, Docker, a VM, or OpenHands cloud. Automations on a schedule or webhook. | Conversation event stream (messages, tool calls, usage). Automation run history. | A conversation and an automation. No gate that refuses a "ready" transition. | Agent Client Protocol over stdio JSON-RPC for Claude, Codex, Gemini, Pi, OpenCode. Also its own agent loop. | Per-conversation token cost in the SDK. Enterprise adds budget limits and traces. | MIT for the core. `enterprise/` is PolyForm Free Trial (30 days per calendar year, no redistribution). | 92,491 stars. Pushed 2026-11-02. |
| [Devin](https://docs.devin.ai/get-started/devin-intro) | Cognition's own cloud agent. Can spawn managed Devin sessions in parallel VMs. Dynamic workflows are recorded, resumable scripts the agent writes. | Cloud sessions, skills, knowledge, PRs. | Skills are `SKILL.md` procedures. Playbooks became skills; new accounts from 2026-10-24 have skills only. The coordinator is Devin. | Devin is the agent. A terminal Devin can sit inside Superset. It does not host Claude, Codex, or Cursor. | ACU (agent compute units), session size, user-message count, Session Insights (prompt rewrite, knowledge that helped or misled). | Proprietary. | Product docs current through late October 2026. |
| [Factory](https://docs.factory.com/missions/overview) | Droid sessions. Missions: a plan of features and milestones, then workers plus validator agents under Mission Control. | Sessions, mission plan, links to issues and PRs. | The orchestrator is an agent you can pause and redirect. Validators are another model, not a harness refusal. | Droid CLI (`droid`, `droid exec`), hooks, MCP, skills, plugins. Not a host for other vendors' CLIs. | Analytics API: Factory credits per session, PR, ticket, and project; skill and tool counts; autonomy ratios. Enterprise effectiveness dashboard (cycle time, intent, attribution). | Proprietary. | Docs current in 2026. |
| [Amp](https://ampcode.com/docs) | A thread is the unit of work. It runs on the local CLI, an Amp-hosted orb, or your runner. Agents start other threads and pass files back. | The thread: prompts, replies, tool calls, files changed. One URL, open from web, CLI, or phone. | None found. A thread is finished when the agent ends the turn. | Amp is the agent, with bring-your-own model keys. It does not host other CLIs. | Dollar cost on the thread. Orb time by size. `amp usage`. Enterprise spend limits. | Proprietary. Hobby is free with your own keys. Amp-hosted orbs are billed by the minute (about $0.08 to $2.13 per hour by size, docs current 2026). | Pricing change announced 2026-09-13. Docs current. |
| [Codex cloud](https://help.openai.com/en/articles/20001545-using-codex-cloud) | One isolated cloud task per prompt, from a published environment (repos, tools, secrets). Local Codex CLI is a separate surface. | The cloud task, the environment, the diff and PR. | The task ends when the agent stops. A person reviews the result. | Codex only. | Plan usage limits. No public query of cost by process version. | Proprietary (OpenAI). | Help Center article current in 2026. |
| [Cursor cloud agents](https://cursor.com/docs/cloud-agent) | Parallel cloud VMs. Self-hosted machines run tool calls locally; the agent loop stays in Cursor's cloud. Automations are a trigger plus a prompt. Best-of-N runs several candidates. | The run, the environment snapshot, the PR. Automations live in Cursor. | The prompt is the process. Bugbot and security agents comment on PRs. Nothing refuses a lane state. | Cursor's agent only. | Team dashboard: agents created, PRs, lines. Admin API: `chargedCents` filterable by `cloudAgentId` and `automationId`. | Proprietary. | Docs current in 2026. |
| [gstack](https://github.com/garrytan/gstack) | Not an orchestrator. A role-based skill team (plan, review, QA, ship) inside the host CLI. | The repo, plus gstack's state directory. Ship behavior comes from `.gstack/ship-policy.json` on the base branch. | `/ship` calls binaries that exit 3 on a refused step, and can require a gate log bound to the merging tree. The credential pre-push hook is documented as bypassable (`--no-verify`, `GSTACK_REDACT_PREPUSH=skip`). The agent can merge without invoking `/ship`. | Claude Code first. Setup also targets Codex, Cursor, and other hosts. | gstack runs paid model evals of its own skills. No per-lane experiment of a process version (rounds, escapes, tokens, attention). | MIT. | 138,637 stars. Pushed 2026-11-02. |
| [pstack](https://github.com/backnotprop/pstack) | Not an orchestrator. `poteto-mode` picks a playbook (bugfix, perf, refactor, autopilot, shipping). | Skill files. The shipping playbook tells the agent to post a verdict on the PR. | The playbook says a restack invalidates verdicts and the agent must re-check `git patch-id` before trusting one. That check is an instruction. Upstream skills are mostly manual. | Upstream is the Cursor plugin `cursor/plugins/pstack` (Poteto). The `backnotprop/pstack` mirror installs the same skills into Claude Code, Codex, Pi, OpenCode, and Cursor via `npx skills`. | None beyond unit tests of helper scripts. | MIT. | Mirror: 3,386 stars, pushed 2026-10-28, tracking upstream 0.15.x. |
| Skill ecosystems | Process shipped as `SKILL.md`. [Superpowers](https://github.com/obra/superpowers) is the end-to-end methodology: brainstorm, worktree, plan, subagents, test-first, review, finish. | Files the host CLI loads from its skill directories. | Superpowers describes its workflows as mandatory and injects text at session start. Following them is up to the model. Hook support differs by CLI. | [Agent Skills](https://agentskills.io/specification) (`SKILL.md` plus optional scripts). Distributed by `npx skills` and by Claude and Codex plugin marketplaces. | No public comparison of process versions. A 2026-10-14 survey of ten libraries found no controlled study showing a public skill pool beats no skills. | Superpowers is MIT. The spec is open. Other large libraries are mostly MIT. | Superpowers: 310,965 stars, pushed 2026-10-29. |

Adjacent, not a row: Claude Code agent teams (experimental, Claude-only) give a lead session a shared task list and let teammates message each other. Task states are pending, in progress, and completed. The lead is prompted to use them. They are off unless `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, and they do not spawn in non-interactive `-p` sessions.

## Notes that change the verdicts

**Worktree UIs are a crowded category.** Superset, Conductor, Claude Squad, and Vibe Kanban all isolate a task as a branch plus a worktree plus a terminal, then show the diff. Superset's own orchestration page describes this as the job: where agents run, how they stay alive, who watches, how output is reviewed. Conductor documents the same split, and says workspace isolation is development isolation, not a security boundary.

**Vibe Kanban's company died and the repo did not.** On 2026-04-10 Bloop shut down, citing free users and no business model they wanted, and left the project open source. Hosted kanban data was scheduled to go away after 30 days. Local workspaces remained. Commits in late October 2026 are bugfixes (Copilot approvals, a local CI script). A kanban over agents is something the market already tried at 28k stars.

**gstack is the nearest miss on gates.** By late October 2026, `/ship` reads a JSON policy committed on the base branch, so a PR cannot swap its own gate. Tools share an exit table (`0` ok, `1` fail, `2` usage, `3` refused). With `full_e2e: true`, queue `ready` refuses unless a passing gate result is bound to the tree being merged. The same repo's credential hook says, in the script header, that it is a guardrail and not enforcement. Both facts belong in the design: machine-checked evidence on a commit is possible in a skill bundle, and it still depends on the agent choosing the skill. gstack also stamps its own version onto the repo it develops. That stamp is a release version, not an experiment over rounds, escapes, tokens, and attention.

**pstack rediscovered stale reviews as prose.** The shipping playbook tells the agent that a restack rewrites SHAs and silently invalidates verdicts, and to compare `git patch-id` before trusting an old verdict. That is the failure mode in VISION (reviews that are not on the head), written as a reminder.

**OpenHands is the nearest miss on a cross-CLI event stream.** `ACPAgent` spawns an Agent Client Protocol server and turns its session updates into conversation events, including tool calls and `UsageUpdate` token and cost figures. The stream is the agent's trajectory. VISION's ledger is claims, plans, reviews, findings, gates, verdicts, messages, and merges, queried without reconstructing transcripts.

**Factory and Devin already sell session cost.** Factory's analytics API returns credits for a session, a PR, a ticket, or a project, plus skill invocations and autonomy ratios. Devin reports ACU, classifies session size, and writes a post-hoc analysis with a rewritten prompt. Amp shows a dollar figure on the thread. Cursor's admin API returns `chargedCents` per cloud-agent run and per automation. A cost dashboard for "how much did this agent session cost" is a solved vendor feature. A cost figure joined to a process version is not in any of them.

**Conductor still polls.** The cloud API guide says to poll session status until it has been `working` and then `idle`, because a fast turn can finish between polls. That is the waiting pattern VISION is trying to leave.

**Skill libraries won distribution and stayed prompts.** Superpowers (311k stars) and gstack (139k stars) are larger than the orchestrator repos in this survey. Superpowers' README calls the workflows mandatory. A third-party read of the libraries on 2026-10-14 found session-start injections and "ABSOLUTELY MUST" wording, and no controlled evaluation of the libraries themselves. Portability of `SKILL.md` across Claude, Codex, Cursor, Devin, and Factory means the same prose process now skips the same way on every CLI.

## Verdicts

### 1. Process as code, versioned and measured. Unserved

What exists is prose that describes a pipeline, plus one opt-in ship tool.

Superpowers, pstack, Devin skills, and Factory Missions all put the steps in text the agent is asked to follow. Factory adds validator agents the orchestrator can ignore or be talked out of. Devin's dynamic workflows are scripts the agent records so it can resume. gstack goes further for the ship step only: a reviewed JSON policy and binaries that refuse to print "ready" without a gate log on that tree. The agent can still ship by some other path, the policy is not a lane state machine (plan check, quality gate, merge verdict), and nothing stamps a process version onto a lane and compares rounds, escapes, tokens, and attention across versions.

The bet is worth making. The design should assume gstack's ship policy exists, and should not stop at "a JSON file the skill remembers to run".

### 2. Bring your own agent. Partly

Launching a foreign CLI in an isolated worktree is done well. Superset lists Claude Code, Codex, Cursor, Amp, Droid, Devin, Gemini, OpenCode, and "any other CLI" with no adapter. Vibe Kanban, Conductor, and Claude Squad do the same for a shorter list. OpenHands ACP is the real protocol: a subprocess, JSON-RPC, streamed tool and usage events.

The contract this project needs is narrower and none of them ship it: context in, gate and tool calls out, lifecycle events into a process engine, with the model still chosen per lane. ACP events land in a conversation. Superset's integration is "start this binary in this terminal". Devin, Factory, Amp, Codex, and Cursor cloud are the agent, not a host for the others.

Build adapters for Claude Code, Codex, and Cursor. Do not build another "spawn any binary in a worktree" layer as the product.

### 3. An event-sourced ledger. Unserved

Trajectories are common and they are the wrong object. OpenHands stores a conversation event stream. Amp stores the thread. Factory stores session rows and attributes credits to a PR. Conductor stores chats on the workspace. Vibe Kanban stores execution processes. All of those support "what did the agent say and which files changed". None of them is an append-only log of claims, plans, reviews, findings, gates, verdicts, messages, and merges from which the metrics are queries.

This bet is worth making. It is also the one that makes bet 1 measurable. A process version with no ledger is another feeling.

### 4. Lanes that know about each other. Partly

Fan-out through a parent agent is a product category. Superset's `superset:orchestrate` skill creates workspaces, launches workers, and reads their terminals. Factory Mission Control assigns features to workers and validators. Devin's coordinator session scopes work, watches managed sessions, and is described as resolving conflicts. Amp starts side threads and transfers files explicitly, and tells you to keep same-file edits in one thread. Claude agent teams add a shared task list and direct messages, inside one vendor, behind an experimental flag.

File and seam ownership, an early warning when two lanes touch the same code, and a typed channel ("this signature changed", "rebase on mine") are not in any of them. The messages that exist are natural language in a terminal, or a file copy. Rebuilding a coordinator prompt would duplicate Superset, Factory, and Devin. The ownership record is still open, and it wants the ledger from bet 3.

### 5. Attention as the scarce resource. Partly

Status surfaces are done well enough to copy badly. Superset's pitch is a fleet view that pulls you in when an agent is waiting or finished. Conductor's is a glance across workspaces, then review and merge. Amp notifies you when a thread wants a message. Devin and Cursor reach Slack, and Cursor automations fire on Linear, GitHub, and webhooks.

"Ready" in these products means the agent stopped or a diff exists. The rule in the bet, that a lane may interrupt a person only for a question the spec, the code, or another lane cannot answer, and that ready means the checks passed, is unserved. pstack and gstack try to define it in the playbook the agent may skip. This bet is mostly a consequence of bet 1: once the harness knows which gates have evidence, the attention rule is a query. A new dashboard before that query exists would match the category that just lost Vibe Kanban's company.

### 6. Cost as a first-class number. Partly

Per-session money is done well. Factory, Devin, Amp, and Cursor each expose a number for a session, a thread, a cloud run, or an automation, and Factory and Cursor can group it by PR or project. OpenHands Enterprise adds budgets. That slice should be consumed (read the CLI's usage events into the ledger), not reinvented.

Two pieces are unserved. Cost is not joined to a process version, so a process change still cannot be scored on tokens. Waiting is still pull-based: Conductor documents polling, and Superset's coordinator reads terminal screens. Vendor plan limits and org spend caps are not a per-lane budget that stops work before it burns a day.

### 7. Fast and boring to run. Partly

Claude Squad is the boring shape: one local binary, tmux, git worktrees, no account, works offline. It is a session switcher, last pushed 2026-08-20, AGPL, maintained part time. It does not hold gates or a ledger.

Everything that orchestrates real work has accreted a surface. Superset is a desktop app plus a terminal daemon plus a cloud API (source-available, and the license blocks reselling the app as a service). Conductor is a Mac app with a cloud tier. OpenHands is an agent server plus an automation server plus a UI, often with Docker. Devin, Factory, Amp orbs, Codex cloud, and Cursor cloud agents need the vendor's account; Cursor's self-hosted option still runs the agent loop in Cursor's cloud. Vibe Kanban is local again only because the hosted half was shut off.

The bet is worth keeping as a constraint on whatever serves bets 1 and 3. It is not a reason to build another tmux multiplexer.

## Why this project should exist

Worktree orchestrators and skill libraries already run many coding agents and already describe the workflow in prose; none of them can refuse a lane that skipped its gates and then show, from a ledger, whether a process change helped.

## What that sentence rules out

- Another kanban or desktop IDE over worktrees. Superset (active, ELv2), Conductor (funded Mac app), and Vibe Kanban (Apache, company dead, repo alive) already occupy it.
- Another skill pack. Superpowers and gstack are the distribution winners, and they are the failure mode in VISION.
- Another session cost dashboard. Factory, Devin, Amp, and Cursor already have one.
- An in-house agent loop. OpenHands, Devin, Factory, Amp, Codex, and Cursor are that product, and VISION already lists it as a non-goal.

The open work is the refused transition, the version stamp, and the ledger those two write.

## Sources

Primary, read 2026-11-02 unless noted:

- Superset repo and license: <https://github.com/superset-sh/superset>, <https://github.com/superset-sh/superset/blob/HEAD/LICENSE.md>
- Superset orchestration, agents, MCP: <https://superset.sh/agent-orchestration>, <https://docs.superset.sh/orchestration>, <https://docs.superset.sh/mcp-server>, <https://github.com/superset-sh/superset/blob/main/apps/docs/content/docs/agent-integration.mdx>
- Conductor: <https://www.conductor.build/>, <https://www.conductor.build/docs>, <https://www.conductor.build/docs/concepts/parallel-agents>, <https://www.conductor.build/docs/concepts/git-worktrees>, <https://www.conductor.build/docs/api>, <https://www.conductor.build/pricing>, <https://www.conductor.build/terms>
- Claude Squad: <https://github.com/smtg-ai/claude-squad>, <https://github.com/smtg-ai/claude-squad/blob/main/LICENSE.md>, releases through v1.0.20 (2026-08-20), maintenance comment on issue 250 (2026-03-01)
- Vibe Kanban: <https://github.com/BloopAI/vibe-kanban>, <https://www.vibekanban.com/blog/shutdown> (2026-04-10), commits through 2026-10-28
- OpenHands: <https://github.com/OpenHands/OpenHands>, <https://github.com/OpenHands/OpenHands/blob/main/enterprise/LICENSE>, ACP guide <https://docs.openhands.dev/sdk/guides/agent-acp>
- Devin: <https://docs.devin.ai/get-started/devin-intro>, <https://docs.devin.ai/product-guides/skills>, <https://docs.devin.ai/product-guides/using-playbooks>, <https://docs.devin.ai/work-with-devin/advanced-capabilities>, <https://docs.devin.ai/product-guides/session-insights>
- Factory: <https://docs.factory.com/missions/overview>, <https://docs.factory.com/missions/running-cli>, <https://docs.factory.com/droid-cli/overview>, <https://docs.factory.com/droid-exec/overview>, <https://docs.factory.com/api-reference/analytics>, <https://docs.factory.ai/agent-effectiveness/dashboard>
- Amp: <https://ampcode.com/docs>, <https://ampcode.com/docs/threads>, <https://ampcode.com/docs/orbs/agent-to-agent>, <https://ampcode.com/docs/orbs/sizes-and-costs>, <https://ampcode.com/pricing>, <https://ampcode.com/news/free-agent> (2026-09-13)
- Codex cloud: <https://help.openai.com/en/articles/20001545-using-codex-cloud>
- Cursor: <https://cursor.com/docs/cloud-agent>, <https://cursor.com/docs/cloud-agent/choose-runtime>, <https://cursor.com/docs/cloud-agent/automations>, <https://cursor.com/docs/account/teams/analytics>, <https://cursor.com/docs/account/teams/admin-api>
- gstack: <https://github.com/garrytan/gstack>, <https://github.com/garrytan/gstack/blob/main/docs/ship-policy.md>, <https://github.com/garrytan/gstack/blob/main/ship/SKILL.md>, <https://github.com/garrytan/gstack/blob/main/bin/gstack-redact-prepush>
- pstack: <https://github.com/backnotprop/pstack> (mirror of <https://github.com/cursor/plugins/tree/main/pstack>)
- Skills: <https://agentskills.io/specification>, <https://github.com/obra/superpowers>, Claude agent teams <https://code.claude.com/docs/en/agent-teams>
- GitHub API via `gh` on 2026-11-02 for stars, license SPDX, and `pushed_at` of `superset-sh/superset`, `smtg-ai/claude-squad`, `BloopAI/vibe-kanban`, `OpenHands/OpenHands`, `garrytan/gstack`, `backnotprop/pstack`, `obra/superpowers`. Superset's SPDX id is `NOASSERTION`; the repo's `LICENSE.md` is Elastic License 2.0.

Secondary, used only where the primary page did not answer the question:

- Dbggr, "Agent skill libraries compared" (2026-10-14): <https://dbggr.dev/articles/agent-skill-libraries-compared/>. Skill counts, session-start cost, and the "no controlled study" claim. Star counts in that article are older than the `gh` figures above; the `gh` figures win on conflict.

## Gaps

- Conductor, Devin, Factory, Amp, Codex, and Cursor have no public server code. "No gate" means no gate in the docs and the public behavior, not a line-by-line reading of their backends.
- Factory's effectiveness dashboard and OpenHands Enterprise budgets were read from docs, not from a logged-in account.
- Claude Squad's March 2026 "part time" comment may be stale. The last push, 2026-08-20, is the stronger activity signal.
- This survey did not score the long tail of GitHub orchestrators (task ledgers, graph runners, other tmux multiplexers). A later pass is warranted only if one of them claims refused lane transitions plus a queryable work ledger. The named set already covers the products a user would actually adopt instead of building this.
