# R4: Agent CLI integration surface

Status: draft, 2026-11-02. Sources checked that day. This drafts the contract for bet 2 in [VISION.md](../VISION.md). A decision record follows when the contract is accepted.

## Question

What is the smallest integration contract Claude Code, Codex, Cursor, Gemini CLI, opencode, and Amp can all meet, and do MCP, the Agent Client Protocol (ACP), or A2A make that contract generic?

## Verdict

The contract is three obligations on an adapter: write a native instruction file before the first turn, load one stdio MCP server, and emit three lifecycle events (`session.started`, `turn.finished`, `session.exited`) with a stable session id. Headless JSON streams are the common way to get those events. Hooks are an adapter detail.

ACP can drive all six later, through wrappers for Claude Code, Codex, and Amp. It is a second transport, and it pulls the harness into the role of chat client, which the vision leaves to each CLI's own terminal. A2A is agent-to-agent discovery over the network. It belongs with lane coordination (R9), if anywhere.

Day-one adapters are Claude Code, Codex, and Cursor, checked below. Gemini CLI, opencode, and Amp meet the same three obligations on a local run.

## What bet 2 asks for

[VISION.md](../VISION.md) bet 2: one small contract (context in, tool and gate calls out, lifecycle events) that any agent CLI meets through its own hooks, plus adapters for Claude Code, Codex, and Cursor on day one. Model choice stays per lane. The harness does not ship an agent loop, and the agent's own terminal stays the place to talk to it.

Two vision notes about Cursor are stale against current docs, and one live bug replaces them:

- Subagents are documented at `.cursor/agents/` and `~/.cursor/agents/`, with Claude and Codex compatibility paths. User-level skills are still absent from cloud agents and self-hosted workers unless synced or baked into the image ([Cursor subagents](https://cursor.com/docs/subagents), [Cursor skills](https://cursor.com/docs/skills)).
- Skill docs require YAML `name` and `description`, and say the agent uses `description` to decide relevance. That contradicts "Cursor drops skill front matter" ([Cursor skills](https://cursor.com/docs/skills)).
- `sessionStart.additional_context` is documented as initial system context and, in current Cursor, dropped because the hook runs before the composer handle exists. `env` from the same hook still applies ([Cursor forum](https://forum.cursor.com/t/sessionstart-hook-additional-context-is-never-injected-into-agents-initial-system-context/158452)). `sessionStart` is fire-and-forget and cannot block session creation ([Cursor hooks](https://cursor.com/docs/hooks)).

## Capability table

Cells are the native surface an adapter would call. "JSON stream" means a documented headless mode that prints one JSON object per line and includes a session id.

| | Claude Code | Codex | Cursor | Gemini CLI | opencode | Amp |
|---|---|---|---|---|---|---|
| Interactive | `claude` | `codex` | `agent` | `gemini` | `opencode` | `amp` |
| Headless | `claude -p` | `codex exec` | `agent -p` | `gemini -p` | `opencode run` | `amp -x` / `--execute` |
| JSON stream | `--output-format stream-json` (also `json`) | `codex exec --json` (`thread.started`, `turn.*`, `item.*`) | `--output-format stream-json` (also `json`) | `--output-format stream-json` (`init`, `tool_use`, `result`) | `opencode run --format json` | `--execute --stream-json` |
| Resume | `--resume <id\|name\|jsonl path>`, `--continue` | `codex exec resume <id>`, `--last` | `--resume [chatId]`, `--continue` | `--resume <uuid\|index\|latest>` | `run --session <id>`, `--continue` | `amp threads continue <threadId>` |
| Instruction file | `CLAUDE.md`, `.claude/rules` | `AGENTS.md` | `.cursor/rules`, `AGENTS.md` (also reads `CLAUDE.md` via skill compatibility paths) | `GEMINI.md` (`context.fileName` can add names) | `AGENTS.md`, falls back to `CLAUDE.md` unless disabled | `AGENTS.md`, else `AGENT.md` or `CLAUDE.md` |
| Extension | stdin JSON hooks in settings; command or HTTP | stdin JSON hooks in `hooks.json` / `config.toml`; `features.hooks` | stdin JSON `hooks.json`; camelCase events | `hooks.*` in `settings.json` (`BeforeTool`, `SessionStart`, `BeforeAgent`, …) | JS/TS plugins (`.opencode/plugins`) and server hooks | TS plugins, `amp.on(...)` |
| Can block a tool | `PreToolUse` exit 2 / `decision: block` | `PreToolUse`, `PermissionRequest` | `preToolUse`, `beforeShellExecution`, `beforeMCPExecution`. Crash fails open unless `failClosed: true` | `BeforeTool` | `tool.execute.before`, `permission` `deny` | `tool.call` → `reject-and-continue` |
| Subagent defs | `.claude/agents/*.md` and `~/.claude/agents/` (markdown + frontmatter) | `~/.codex/agents/*.toml` and `.codex/agents/` (`name`, `description`, `developer_instructions`) | `.cursor/agents/*.md` and `~/.cursor/agents/` (plus `.claude/agents`, `.codex/agents`) | `.gemini/agents/*.md` and `~/.gemini/agents/` | `opencode agent create` → `.opencode/agents` or global; `primary` vs `subagent` | Built-in Task and Read Thread only. Custom subagents are plugin tools |
| Skills | `.claude/skills/*/SKILL.md`, `~/.claude/skills` | `.agents/skills` from cwd up to repo root, `~/.agents/skills` | `.agents/skills`, `.cursor/skills`, `~/.agents/skills`, `~/.cursor/skills`; also `.claude/skills` and `.codex/skills` | `.gemini/skills` or `.agents/skills`, user and workspace | `.opencode/skills`, `~/.config/opencode/skills`; also `.claude` and `.agents` unless disabled | `.agents/skills`, `~/.config/agents/skills`; also reads `.claude/skills` |
| Unattended permissions | `--permission-mode acceptEdits\|bypassPermissions\|dontAsk\|plan`. `--permission-prompt-tool` names an MCP tool | `--sandbox workspace-write` plus `--ask-for-approval never` (or `on-request`) | `--force` / `--yolo`. `--sandbox enabled\|disabled`. Print mode has write access; `--force` applies edits | `--approval-mode auto_edit\|yolo\|plan` | `--auto` (still honors explicit `deny`) | `amp.dangerouslyAllowAll`, command allowlists. Ask-first MCP tools stall an unattended thread |
| Transcript | `~/.claude/projects/<cwd>/<session>.jsonl`. Hook field `transcript_path` can lag the live turn | Rollouts under `$CODEX_HOME` (default `~/.codex`). Docs say `transcript_path` is not a stable interface | Hook `transcript_path`. Headless can write Claude-compatible JSONL. Subagent files under the parent conversation | Session store, `--list-sessions`. Headless JSON is the stable machine output | `opencode session export <id>` (JSON, optional `--sanitize`) | Threads synced to ampcode.com. Plugin `thread.messages`. Stream JSON is the local machine output |
| Token fields on the machine stream | Result and assistant lines: `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`; `total_cost_usd` when pricing is known. Subagent spend counts toward `--max-budget-usd` | `turn.completed` usage: input, cached input, output, reasoning. Account totals also via `/usage` | Changelog and live CLI: `result.usage` with `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`. The output-format schema page still omits `usage`. No cost field. Official docs and the schema page disagree; treat `usage` as observed, not guaranteed | `--output-format json` has `stats`. `stream-json` `result` has per-model token totals | `opencode stats --json --cost` is the documented aggregate. Per-message tokens exist on the server/session API; the `run --format json` schema is not published as a stable usage contract | Assistant messages in `--stream-json`: `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`. Result message can also carry `usage` |
| MCP client | Yes. `--mcp-config`. `SessionStart` runs before MCP is up | Yes. `[mcp_servers]` in `config.toml` | Yes. `mcp.json`. `agent mcp enable`. `--approve-mcps` | Yes. `mcpServers` in settings. `--allowed-mcp-server-names` | Yes. `mcp` in `opencode.json`. `opencode mcp add` | Yes. `amp.mcpServers`, `--mcp-config`. Workspace servers need `amp mcp approve` |
| ACP | Wrapper: `@agentclientprotocol/claude-agent-acp` (Claude Agent SDK) | Wrapper: `@agentclientprotocol/codex-acp` (Codex app server) | Listed in the ACP registry as Cursor's agent | Native: `--experimental-acp` | Native: `opencode acp` | Wrapper in the ACP registry |

Sources for the table are listed at the end. Where an official schema and a changelog disagree (Cursor `usage`), both are cited.

## Protocols

| | Direction | What it standardizes | Fit for this harness |
|---|---|---|---|
| **MCP** | Host (the coding CLI) to a tool server | Tools, resources, prompts. Stdio or HTTP. JSON-RPC | The outbound half of the contract. Every CLI here is an MCP client. The harness is the server. MCP does not start the CLI, resume it, or report its tokens |
| **ACP** | Editor or orchestrator to an agent | `session/new`, `session/prompt`, `session/cancel`, streamed updates, permission requests, optional MCP server config on the session. JSON-RPC over stdio | A possible later driver. Registry entries exist for all six, but Claude, Codex, and Amp go through adapters, so native hooks, skill paths, and permission flags are whatever the wrapper maps. Driving ACP makes the harness the chat client |
| **A2A** | Agent to agent | Agent Cards, tasks, messages, artifacts. Discovery over HTTP | Out of this contract. Lanes that need to talk to each other are R9. A local harness already knows its lanes; it does not need a discovery card |

BeeAI's older "ACP" (Agent Communication Protocol) was folded into A2A. The ACP in this doc is Zed's Agent Client Protocol, now co-maintained with JetBrains ([agentclientprotocol.com](https://agentclientprotocol.com/get-started/registry), [A2A and MCP](https://a2a-protocol.org/latest/topics/a2a-and-mcp/)).

## Smallest contract (v0)

An adapter is a small program the harness ships per CLI. The CLI process stays the agent. The adapter translates.

1. **Spawn.** Start one CLI process with its cwd set to the lane worktree. Record pid, argv, and exit code. Interactive and headless are both allowed. The harness does not draw the UI.
2. **Session id.** On start, emit `session.started` with a `session_id` the same CLI accepts later for resume. The id is opaque. Adapters store whatever the CLI printed (`session_id`, thread id, chat id).
3. **Context in.** Before the first model turn, the lane brief is on disk as that CLI's instruction file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, or a `.cursor/rules` file). The brief is markdown the model sees as project guidance. Adapters pick the filename. The harness supplies the text.
4. **Calls out.** The adapter loads one harness MCP server over stdio for that process. v0 tools are `lane.read` (spec, gate state, sibling notes the harness chooses to show), `lane.message` (typed message to another lane or to the human queue), and `evidence.attach` (a path or digest the harness stores). Tool names live in the MCP server, so a CLI rename cannot fork them.
5. **Gates stay in the harness.** The agent cannot mark a gate passed. A gate tool would be advisory, and agents already skip unenforced steps. The harness changes lane state from events plus the checks in R7. MCP is how the agent asks and how it files evidence.
6. **Lifecycle.** The adapter emits, at minimum:
   - `session.started` (`session_id`, cwd, model if the CLI reports one)
   - `turn.finished` (`session_id`, stop reason if known)
   - `session.exited` (`session_id`, exit code) only when the turn is over
   Optional, same envelope, omitted when the CLI has no field: `tool.started`, `tool.finished`, `usage.reported`, `permission.denied`.
7. **Usage.** `usage.reported` carries `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, each nullable, plus `source: parent | subagent` and the CLI's raw object. Missing numbers stay null. Adapters do not estimate cost. Subagent calls emit their own usage events when the CLI exposes them. A parent total is never assumed to include children.
8. **Resume.** `resume(session_id, prompt)` continues that session. A new id is a new session.
9. **Permission profile.** The harness passes `interactive` or `unattended`. The adapter maps it onto the flags in the table. `unattended` means the process must not block on a human prompt. It does not mean the sandboxes are equivalent.
10. **Identity of the model** is an adapter input (`--model` or the CLI's equivalent), stored on `session.started`. The contract has no model list.

v0 leaves these to adapters and later versions: a shared subagent format, a shared skill tree, parsing native transcripts into the ledger, ACP as the required transport, A2A, and treating process exit as turn completion for a remote executor.

Hook installation is not part of v0. Codex skips a new command hook until a human trusts that hook's hash (`/hooks`). Cursor's `sessionStart` cannot block and currently drops `additional_context`. Amp's lifecycle is a TypeScript plugin (`session.start`, `agent.start`, `tool.call`, `agent.end`) with no `session.end`. Those three facts are why the bet's phrase "through its own hooks" does not survive contact with the day-one CLIs. Hooks remain available inside an adapter when they work.

## Checked against the day-one CLIs

| Obligation | Claude Code | Codex | Cursor |
|---|---|---|---|
| Spawn in a worktree | `claude -p` or a PTY `claude`, cwd = worktree | `codex exec` or TUI, cwd = worktree | `agent -p` or TUI. `--workspace`, or the harness's own worktree (Cursor also has `--worktree`, which the harness should not need if it already made one) |
| Session id | JSON `session_id`. `--resume` accepts it | `thread.started` id. `codex exec resume` | `stream-json` `session_id`. `--resume` |
| Context file | Write `CLAUDE.md` or a `.claude/rules` file in the worktree | Write `AGENTS.md` | Write `AGENTS.md` or `.cursor/rules`. Do not depend on `sessionStart.additional_context` |
| MCP stdio | `--mcp-config` pointing at the harness server. Connect before the first turn (`-p` waits, with a timeout) | `[mcp_servers.harness]` in a config layer for that run | `.cursor/mcp.json` or user mcp config, plus `--approve-mcps` so the headless run does not prompt |
| `turn.finished` without a trusted hook | `stream-json` result event, or process exit after `-p` | `turn.completed` / `turn.failed` on `--json` | terminal `result` event on `stream-json`. A failed run can exit non-zero with no result event; the adapter then emits `turn.finished` from the exit |
| `unattended` | `--permission-mode acceptEdits` or `bypassPermissions` | `--sandbox workspace-write --ask-for-approval never` | `--force` (and `--trust` on a fresh workspace) |
| Usage event | Map the result `usage` object. Children count toward the budget flag; still emit a child event when the stream shows one | Map `turn.completed` usage. Cached input → `cache_read_tokens`. No cache-write field → null | Map `result.usage` when present. Schema page omits it, so a missing object is null, not zero |

Gemini CLI, opencode, and Amp pass the same check on a local attached run:

- Gemini: `gemini -p --output-format stream-json --approval-mode auto_edit`, context in `GEMINI.md`, MCP in settings, `init.session` id with `--resume`.
- opencode: `opencode run --format json --auto`, context in `AGENTS.md`, MCP in `opencode.json`, `--session` to resume. Usage comes from session/stats APIs when the run stream has no token object.
- Amp: `amp --execute --stream-json`, context in `AGENTS.md`, `--mcp-config` for the harness server, `threads continue` to resume. `amp -ox` without `--stream-json` prints a URL and exits while the orb keeps working. That path fails obligation 6. The adapter either stays attached (`-ox --stream-json`) or emits a distinct detached state and withholds `session.exited` until a later poll. v0 only requires the attached form.

## Hardest gap

**A turn-complete usage record when work leaves the parent process.**

The three lifecycle events are available on every local attached CLI above. What is not portable:

- Amp's orb mode (`amp -ox`, and `--executor runner:<id>`) returns before the turn ends unless the adapter stays on `--stream-json`. Process exit is then a lie.
- Every CLI can spend tokens in a subagent with its own context window. Claude folds that spend into `--max-budget-usd`. Cursor documents subagent usage as independent. Codex warns that subagents cost extra and does not promise the parent `turn.completed` usage includes them. A single parent total will under-count on at least one day-one CLI.
- Codex will not run a newly installed hook until a person trusts its hash, so a hook cannot be the thing that makes the record trustworthy on an unattended first launch.
- Cursor can fail open when a blocking hook crashes, and its documented session-start context injection is currently dropped.
- Native transcripts are the wrong source of truth: Codex says `transcript_path` is unstable, Claude's file can lag the hook, Amp's full thread lives on ampcode.com.

So the ledger (R5, R11) should store adapter events, with nulls where a CLI is silent, and should refuse to treat "parent process exited" as "the lane's model spend is complete" once any adapter supports a remote executor. Closing the gap means each adapter follows child streams it can see, and the Amp adapter refuses detached orb runs until it can attach or poll a thread id. It does not mean a shared hook schema.

## What this changes in the bet

Keep bet 2's shape: context in, calls out, lifecycle events, per-lane model, no agent loop inside the harness.

Rewrite the mechanism: the CLI meets the contract through an adapter over its headless event stream and MCP, and hooks are optional inside that adapter. "One week of glue per CLI" shrinks to argv, one instruction filename, one MCP config, and a field map for session id and usage. It does not shrink to zero, and ACP wrappers do not remove it if the harness still wants native permission flags and the CLI's own terminal.

## Sources

Checked 2026-11-02.

- Claude Code: [CLI reference](https://code.claude.com/docs/en/cli-reference), [sessions](https://code.claude.com/docs/en/sessions), [hooks](https://code.claude.com/docs/en/hooks)
- Codex: [CLI reference](https://developers.openai.com/codex/cli/reference), [non-interactive mode](https://developers.openai.com/codex/non-interactive-mode), [hooks](https://developers.openai.com/codex/hooks), [subagents](https://developers.openai.com/codex/subagents), [skills](https://developers.openai.com/codex/skills), [customization](https://developers.openai.com/codex/concepts/customization), [config sample](https://developers.openai.com/codex/config-sample)
- Cursor: [CLI parameters](https://cursor.com/docs/cli/reference/parameters), [headless](https://cursor.com/docs/cli/headless), [output format](https://cursor.com/docs/cli/reference/output-format), [hooks](https://cursor.com/docs/hooks), [subagents](https://cursor.com/docs/subagents), [skills](https://cursor.com/docs/skills), [changelog](https://cursor.com/docs/cli/changelog) (token usage and hook payload notes), [sessionStart context bug](https://forum.cursor.com/t/sessionstart-hook-additional-context-is-never-injected-into-agents-initial-system-context/158452)
- Gemini CLI: [headless](https://geminicli.com/docs/cli/headless/), [CLI reference](https://geminicli.com/docs/cli/cli-reference/), [configuration](https://geminicli.com/docs/reference/configuration/), [subagents](https://geminicli.com/docs/core/subagents/), [skills](https://geminicli.com/docs/cli/skills/), [sessions](https://geminicli.com/docs/cli/session-management/)
- opencode: [CLI](https://opencode.ai/docs/cli/), [agents](https://opencode.ai/docs/agents/), [permissions](https://opencode.ai/docs/permissions/), [plugins](https://opencode.ai/docs/plugins/), [v2 plugins](https://opencode.ai/v2/docs/build/plugins/)
- Amp: [execute mode](https://ampcode.com/docs/cli/execute-mode), [streaming JSON](https://ampcode.com/docs/cli/streaming-json), [spawning orbs](https://ampcode.com/docs/cli/spawning-orbs), [AGENTS.md](https://ampcode.com/docs/customize/agents-md), [skills](https://ampcode.com/docs/customize/skills), [MCP](https://ampcode.com/docs/customize/mcp), [plugins](https://ampcode.com/docs/customize/plugins), [modes and subagents](https://ampcode.com/docs/models-and-subagents)
- Protocols: [ACP registry](https://agentclientprotocol.com/get-started/registry), [ACP agents list](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/get-started/agents.mdx), [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp), [codex-acp](https://github.com/agentclientprotocol/codex-acp), [A2A and MCP](https://a2a-protocol.org/latest/topics/a2a-and-mcp/)
