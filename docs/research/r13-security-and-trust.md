# R13: Security and trust

Status: draft, 2026-11-02. Threat model for the local harness. Closes the open security items in [R4](r4-agent-cli-integration.md), [R5](r5-state-and-storage.md), [R6](r6-language-and-runtime.md), [R8](r8-process-terminal-runtime.md), [R10](r10-verification-review-layer.md), and [R11](r11-observability-cost-metrics.md).

## Verdict

The harness runs as the owner, on one machine, beside CLIs that already hold the owner's GitHub credentials and model keys. The ledger file is readable by every process that user runs. A gate, a merge yes, and a permission change are integrity rules the write path can enforce. Hiding a secret from a lane after it has been written is not enforceable. The rules below follow from that.

Four decisions:

1. **Secrets never land.** Redact at the only write function, then hash, then commit. The append-only log has no delete. A PTY scrollback is not a ledger.
2. **Untrusted text is data.** Review bodies, PR bodies, tool output, and repo instruction files cannot pass a gate, supply a sha, record `human.merge_yes`, send free text to another lane, or change the sandbox. A fence in the prompt is a courtesy. The write API is the control.
3. **The CLI's own sandbox is the lane boundary. A second OS sandbox around the whole CLI is rejected.** It would be a false claim: Claude's sandbox covers shell commands, and file tools, MCP, and hooks run outside it. The harness MCP server has to reach the ledger. Nested Seatbelt fights the CLI's own Seatbelt. `sandbox = off` is an explicit profile bit, recorded on `session.started`, and it is how today's `spawn` must not carry forward.
4. **Adapters are code in the one binary.** No plugin ABI, no runtime download, no MCP server from the worktree. A compromised `claude` or `codex` on `PATH` is accepted. An untrusted repo does not run unattended.

## Actors and assets

| Actor | What it already is |
| --- | --- |
| Lane model | Same OS user as the owner. Sees the worktree, the brief, and MCP results. Obeys some of the bytes it reads. |
| Untrusted bytes | Review findings, PR bodies and comments, command output, repo `CLAUDE.md` / `AGENTS.md` / hooks / MCP config, any tool result. |
| Harness MCP server | Stdio child of the CLI ([R4](r4-agent-cli-integration.md)). The only supported writer the model can reach. One server per lane process. |
| Sibling lane | Another process of the same user, with its own worktree and the same ledger file. |
| Adapter | In-tree translator: argv, one instruction file, one MCP config, a field map ([R6](r6-language-and-runtime.md)). |
| Daisy, backups, a copied ledger | Read the file later, somewhere else. [R5](r5-state-and-storage.md) already keeps the file off iCloud and Dropbox. Mode `0600` stops other OS users. It does not stop this user. |

Assets, in the order the rules protect them:

1. Owner credentials (`gh`, SSH, cloud keys, vendor API keys) and the GitHub identity those credentials are.
2. Ledger integrity: gate results, the sha a verdict is bound to, `human.merge_yes`, leases, pipeline version.
3. Other lanes' worktrees.
4. The durability of anything that was written. An append-only row outlives the lane.

The ledger is an integrity boundary. It is not a confidentiality boundary between lanes.

## Threats

Each row is mitigated or explicitly accepted. "Mitigate" means a rule in the next section. "Accept" means the code must not pretend the control exists.

| Id | Threat | Disposition |
| --- | --- | --- |
| T1 | A secret is inserted into `events.payload_json` (finding body, brief, verdict reason, message, a junk field). | Mitigate. Closed payload schemas. Redact the string fields that are allowed to be prose. Refuse the write on any other key. |
| T2 | Smoke stdout, a screenshot caption, or another evidence blob holds a secret. [R10](r10-verification-review-layer.md) left redaction-before-hash here. | Mitigate. Redact text, then hash the redacted bytes, then commit. The raw bytes are not stored. |
| T3 | `usage.reported.raw` keeps a transcript line, an error body, or an env dump. [R11](r11-observability-cost-metrics.md) stores `raw` for a later remap. | Mitigate. `raw` is an allowlist of usage fields. |
| T4 | Harness logs, `session.started` argv, or the child environment retain a token. | Mitigate. Logs carry ids and sizes. Argv is checked. Env values are not stored. The ledger path is not in the CLI's environment. |
| T5 | The PTY scrollback becomes a second log and is later copied with the ledger. [R8](r8-process-terminal-runtime.md) asked how secrets stay out of it. | Mitigate the retention. Accept the live terminal: attach shows the bytes the CLI printed. Scrollback, if kept, sits outside the ledger directory, mode `0600`, capped, deleted when the lane is done or cancelled. It is never an event. |
| T6 | The agent prints a secret into a commit, a PR, or `gh`. | Accept for v1. The harness does not proxy git or GitHub. Reading keys from outside the worktree gets harder when the CLI sandbox is on (T10). A repo's own pre-commit hook stays the push control. |
| T7 | Review text, a PR body, or tool output tells the model to pass a gate, merge, skip a check, or turn the sandbox off. | Mitigate the authority. Those operations are not model writes and not parses of prose. Accept the residual: the model can still edit the worktree and push a branch. |
| T8 | The harness copies that text into another lane, or into a reviewer prompt, as instructions. | Mitigate. `lane.read` returns the typed inbox from [R9](r9-coordination-between-lanes.md). Reviewer prompts take length-capped data fields. The parser accepts a structured block. |
| T9 | A repo ships `CLAUDE.md`, a hook, or an MCP server that the CLI loads as trusted. | Mitigate for untrusted repos: refuse unattended. The adapter's MCP config lists only the harness server. Accept on `trust = trusted`: dogfood repos are the owner's, and the CLI will load their instructions and hooks. |
| T10 | Unattended means unsandboxed, which is what `spawn` does today (`--dangerously-skip-permissions`, `--dangerously-bypass-approvals-and-sandbox`, `--force`). | Mitigate. Unattended means "do not block on a person" ([R4](r4-agent-cli-integration.md)). Default is the CLI sandbox, fail closed. `sandbox = off` is a profile key, trusted repos only, stored on the session event. |
| T11 | A parent Seatbelt or bubblewrap around `claude` / `codex` / `agent` is expected to contain the lane. | Accept the residual of not wrapping. [R8](r8-process-terminal-runtime.md) states the mechanics: Claude's sandbox ignores file tools, MCP, and hooks; Codex left Landlock because of unix sockets; a wrong wrapper either breaks the MCP server or pretends a hole is a wall. `session.started` records `sandbox: cli` or `sandbox: off`. It never records an OS wrapper. |
| T12 | A lane reads the ledger file or a sibling worktree (Claude file tools sit outside the shell sandbox). | Accept the read. Mitigate the impact: no secrets in the file (T1 to T4), and the MCP server binds the session to one lane so a read is not a write on another lane. |
| T13 | A third-party adapter, plugin, or repo MCP server runs inside the supervisor or as the lane's tool server. | Mitigate. No plugin ABI ([R6](r6-language-and-runtime.md)). Adapters are compiled in. No download at start. The generated MCP config names the harness binary only. |
| T14 | The `claude`, `codex`, or `agent` binary on `PATH` is compromised. | Accept. Record the resolved path on `session.started`. Do not hash-pin CLIs that the owner updates constantly. |
| T15 | A lane binds ports outside its block, or opens arbitrary network connections. | Accept on the default profile. The port block is an environment contract on macOS ([R8](r8-process-terminal-runtime.md)). Landlock bind is a later Linux supervisor feature. A network namespace or a microVM is the untrusted-repo option, and it is not the default parent. |

## Rules the code must follow

### Writes

There is one write function ([R5](r5-state-and-storage.md)). It redacts, validates, then appends. A refused write leaves no event and no row.

Payloads are typed per `kind`. Unknown keys refuse the write. `payload_json` is not a junk drawer. The rebuild test folds redacted payloads, so the log never holds a pre-redaction copy.

Prose fields are redacted in place, not refused: finding `title` and `body`, brief entries, verdict `why` / `proven` / `not_proven` / reason text, verification `note`, smoke `showed` and `error`, seam `shape`, message fields that are strings. Refusing the evidence write would teach the runner to skip the gate. Identifiers are refused if they are not the type the schema names: a sha is hex of the length [R10](r10-verification-review-layer.md) already checks, a path is a relative path, a session id is the opaque id the adapter stored. A token in a sha column is a bad event, not a redacted one.

The redactor replaces matches with `[redacted:<kind>]` and records the count. Patterns, high confidence only:

- A PEM block from `BEGIN PRIVATE KEY` through the matching `END` line (any `BEGIN *PRIVATE KEY`).
- GitHub tokens: `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` plus 20 or more token characters, and `github_pat_` plus 20 or more.
- An AWS access key id: `AKIA` plus 16 uppercase alphanumerics.
- A Slack token: `xoxb-`, `xoxa-`, `xoxp-`, `xoxr-`, `xoxs-` plus 10 or more token characters.
- `Bearer` followed by 20 or more token characters.
- `sk-ant-` and `sk-proj-` plus 20 or more token characters.
- The value side of an assignment whose name ends in `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY`, or `APIKEY`.

A novel token format that matches none of these is an accepted miss. The redactor does not scan the git history of the worktree. Redaction happens before the idempotency key and before the evidence hash, so a retry hits the same redacted bytes and does not insert a second row.

Evidence ([R10](r10-verification-review-layer.md)): the server opens the file. `realpath` must stay inside the lane worktree. Symlinks that escape are rejected. Paths under `.git` and `.chaperone` are rejected. Text is truncated to the last 256 KiB first, then redacted, then hashed. The manifest stores `redactions` and `truncated`. Image bytes are not scanned. They still have to be PNG or WebP under 8 MiB.

`usage.reported.raw` keeps only the usage object the adapter mapped: token counts, cache counts, reasoning, model id, the CLI's own cost if present. Drop message text, prompts, tool arguments, env, and error bodies before insert. A missing usage object stays null ([R11](r11-observability-cost-metrics.md)). It is not filled with the surrounding stream line.

`session.started` stores `session_id`, cwd, model, resolved CLI path, `permission` (`interactive` or `unattended`), `trust` (`trusted` or `untrusted`), and `sandbox` (`cli` or `off`). It stores the names of the extra environment variables that were set (`CHAPERONE_PORT`, `CHAPERONE_PORT_LAST`), not their values, and not the rest of the environment. Argv that matches the redactor is a refused start, not a redacted argv, because a start line with a secret in it is the wrong CLI invocation. Adapters pass vendor credentials through the CLI's own config file. They do not put keys in argv.

Harness logs: event kind, lane id, durations, byte counts. No payload bodies, no evidence bytes, no environment.

### What the model is allowed to mean

The MCP server is spawned with a lane id. The model does not pass one. Tools:

- `lane.read` returns this lane's spec fields, gate statuses, claims, pins, and the inbox fold. Shape text is data, capped at 500 characters ([R9](r9-coordination-between-lanes.md)). It does not return sibling transcripts, PR bodies, tool output, or evidence bytes. [R4](r4-agent-cli-integration.md)'s phrase "sibling notes the harness chooses to show" is this fold and nothing else.
- `lane.message` accepts `seam.ack` and `seam.reject` only. Any other type, including free text, is a refused write.
- `evidence.attach` takes a worktree path or a hash the server already stored. The result is the hash and the redaction count.

There is no tool that records a gate, changes a lease, sets the pipeline version, sets `sandbox`, or writes `human.merge_yes`. [R10](r10-verification-review-layer.md) already refuses `check.finished` when `actor_session` is the lane, and the runner writes the verdict sha from the snapshot it passed in. Those stay the rules. A finding body, a PR body, and a tool result are not parsed for instructions.

Reviewer and verdict prompts are harness-owned templates. The event stores `prompt_sha256`. The PR body enters as a data field capped at 8 KiB. A finding the fix lane sees is the stored field: redacted, and already capped by the record limits in R10. Command output enters as the evidence hash plus the short `showed` string, not as the raw log. The parser accepts the structured report block. Sentences outside that block do not become facts.

The adapter writes a new instruction file. It does not truncate a tracked `CLAUDE.md`, `AGENTS.md`, or `GEMINI.md`. Claude and Cursor get a new rules file (`.claude/rules/chaperone.md`, `.cursor/rules/chaperone.md`). Codex, and any CLI whose only slot is `AGENTS.md`: create the file when it is absent; when it is present, append a marker-delimited section and leave the rest of the bytes alone. Files the adapter creates live under `.chaperone/` or the rules path above and are listed in that worktree's `$GIT_DIR/info/exclude`, so `git add -A` does not pick them up. A section appended to an already tracked file can still be committed. That is accepted on `trust = trusted`.

MCP config for the run is `.chaperone/mcp.json` (or the CLI's config layer the adapter writes for that process). It names the harness binary and the stdio server. It is gitexcluded. It is not a merge of the repo's `.cursor/mcp.json`, `.mcp.json`, or `opencode.json`.

### Permissions

| Profile | Start |
| --- | --- |
| `trusted`, `interactive` | CLI's normal approval mode. Pass the CLI's sandbox switch when that switch still prompts. |
| `trusted`, `unattended` | CLI sandbox on, no approval prompt. Fail the start if the adapter cannot put that in argv. |
| `untrusted`, `interactive` | Sandbox on. The owner is starting a repo whose instruction file the CLI will load. |
| `untrusted`, `unattended` | Refuse. |
| `sandbox = off` | Trusted repos only. Record `sandbox: off` on `session.started`. The combination `untrusted` plus `sandbox = off` is refused. |

Unattended argv, sandbox left on, from [R8](r8-process-terminal-runtime.md):

| CLI | Flags the adapter passes | Flags that do not carry over from `spawn` |
| --- | --- | --- |
| Codex | `--sandbox workspace-write --ask-for-approval never` | `--dangerously-bypass-approvals-and-sandbox` |
| Claude Code | Sandbox enabled for that run, `allowUnsandboxedCommands` false, `sandbox.failIfUnavailable` true. No skip-permissions flag. | `--dangerously-skip-permissions` |
| Cursor | `--sandbox enabled`, plus the unattended edit flag [R4](r4-agent-cli-integration.md) already lists (`--force` still applies edits). `--approve-mcps` only for the MCP config the adapter just wrote. | `--force` as a way to skip the sandbox, `--trust` on a repo that is not `trusted` |

Interactive lanes keep the CLI's normal prompts.

Path intent, for the people writing adapters. The harness enforces the lines it can enforce by where it puts files and which flags it passes. The CLI enforces the rest, and only inside that CLI's sandbox:

- Writable work: the lane worktree. Codex already keeps `.git` and `.codex` read-only inside its own sandbox. The adapter does not switch that off.
- The ledger directory is outside every worktree, mode `0700` on the directory and `0600` on the file, on a local disk ([R5](r5-state-and-storage.md)). Its path is not in the brief, not in argv, and not in the CLI's environment. The MCP server learns the path from the supervisor (inherited fd, or an environment variable on the MCP process only).
- Sibling worktrees are not in the brief and are not a tool argument. Stopping a same-user process from opening them anyway is the CLI sandbox, with the hole in T12 accepted.

`trust = untrusted` does not get a microVM in v1. Until that profile exists, the start is interactive only. Docker, Lima, and a microVM stay the later option [R8](r8-process-terminal-runtime.md) already named, for a repo the owner does not trust. They are not the default parent.

The supervisor does not wrap the CLI in Seatbelt or bubblewrap. Port blocks stay an environment contract. Network stays open. Both are T15.

### Adapters and plugins

Day-one adapters (Claude Code, Codex, Cursor) are modules in the harness binary, built and reviewed with it. The supervisor does not `dlopen` an adapter, does not `npm install` one, and does not fetch one at lane start.

A subprocess tool the supervisor runs comes from an absolute path in the user profile outside the worktree. v1 ships no such path. A path taken from the worktree, from an event, from a PR, or from a skill file is not executed.

Repo hooks, repo plugins, and repo MCP servers are the CLI's behavior on a trusted repo (T9), not a harness plugin API. The harness does not install hooks and does not script Codex's `/hooks` trust prompt. Hook installation stays out of the v0 contract ([R4](r4-agent-cli-integration.md)).

Crate and lockfile auditing for the harness binary itself belongs to the CI plan in R14. It is not a second adapter ecosystem.

## Tests that lock this

- One fixture per redactor pattern. After a successful write, the database bytes and the evidence file contain the marker and do not contain the raw pattern.
- A payload with an unknown key, or a `raw` usage object that still has message text, is refused. The event count does not change.
- `evidence.attach` of a symlink whose `realpath` leaves the worktree is refused.
- `lane.message` with a free-text type is refused.
- `check.finished` and `human.merge_yes` from the lane session are refused (R10). Repeat them here so a security change cannot drop them.
- An unattended start whose argv is missing the sandbox flag fails before `exec`.
- `trust = untrusted` and `unattended` fails before `exec`.
- `session.started` for a normal start has `sandbox: cli` and no environment values.

## What this leaves open

- R14: `cargo-audit` or the equivalent on the one lockfile, and the CI job that runs the tests above.
- R15: `spawn` keeps today's flags until `supervise` is the parent. The ledger schema that has to be right on day one is the `sandbox` and `trust` fields on `session.started`. Daisy's reader already sees finding bodies ([R5](r5-state-and-storage.md)). Those bodies are untrusted data after redaction. The reader does not turn them into a command or a gate.
- A microVM profile for untrusted repos. Specified as a refused unattended start until that profile exists.
- Secret scanning of `git push`. Accepted gap (T6), not a v1 gate.
- Linux Landlock on the port block. Not a v1 security claim.

## Sources

Read 2026-11-02.

This repo:

- [VISION.md](../VISION.md) quality bar: secrets never in the ledger, review text and tool output untrusted, least privilege per lane.
- [RESEARCH.md](../RESEARCH.md) R13.
- [r4-agent-cli-integration.md](r4-agent-cli-integration.md): adapter contract, MCP tools, `unattended` versus sandbox, hooks fail open or require a human trust step, usage raw object.
- [r5-state-and-storage.md](r5-state-and-storage.md): one write function, `payload_json` is not a junk drawer, reader views, ledger off synced disks, evidence by hash.
- [r6-language-and-runtime.md](r6-language-and-runtime.md): in-tree adapters, no plugin ABI.
- [r8-process-terminal-runtime.md](r8-process-terminal-runtime.md): `spawn` flags, CLI sandbox table, why a parent wrapper fails, PTY scrollback is not the ledger, ports as an environment contract.
- [r9-coordination-between-lanes.md](r9-coordination-between-lanes.md): typed inbox, no free-text `lane.message`.
- [r10-verification-review-layer.md](r10-verification-review-layer.md): finding bodies untrusted, redaction-before-hash left open, independent producers, runner-supplied sha, `human.merge_yes`.
- [r11-observability-cost-metrics.md](r11-observability-cost-metrics.md): `usage.reported.raw` must not hold secrets.

Outside, already cited by those notes:

- Claude Code sandbox (shell commands only; file tools, MCP, and hooks run outside it; Seatbelt; bubblewrap): <https://code.claude.com/docs/en/sandboxing>
- Codex sandbox (workspace-write, `.git` and `.codex` read-only, bubblewrap on Linux): <https://learn.chatgpt.com/docs/sandboxing>
- Cursor hooks fail open unless `failClosed` is set: <https://cursor.com/docs/hooks>
