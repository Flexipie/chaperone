# R8: Process and terminal runtime

Status: recommendation, 2026-11-02. Decides who owns the agent process. Evidence for a decision record. The parent swap itself waits for the runtime milestone in [VISION.md](../VISION.md).

## Recommendation

**The harness supervisor is the parent of the agent.** One subcommand of the same binary (`chaperone supervise`) creates the git worktree, assigns the lane a port block, opens the PTY, and execs the agent CLI. It watches that child. It is the only component that restarts it.

herdr, tmux, and zellij are viewers. A person who wants a terminal runs `chaperone attach <lane>` in a pane. Closing the viewer disconnects the keyboard. The child keeps running, because the supervisor holds the PTY master.

| | Who |
| --- | --- |
| Starts the agent | The harness supervisor, on `lane start`. |
| Watches it | The same supervisor: `wait` on the child, plus the adapter events from R4 (`session.started`, `turn.finished`, `session.exited`). A stall timer lives here. |
| Restarts it | The same supervisor, on `lane resume`. The caller is a person or a pipeline rule. The mechanism is the CLI's own `--resume` with the stored session id. |

Default restart rule: none. A pipeline may set `resume-once` (one automatic resume per lane per six hours, then the lane is blocked for a person). A dead process is an event. A shell that quietly starts a fresh agent is a second session and a token bill, and the supervisor refuses to do that on its own.

Until `supervise` exists, today's `spawn` plus herdr remains the parent, and `/orchestrate` remains the watcher. The ledger must not store a herdr pane id as the lane's identity, so that swap does not rewrite history.

## What runs today

On this machine, 2026-11-02: herdr 0.9.5 at `/opt/homebrew/bin/herdr` (Homebrew, Apache-2.0), `HERDR_ENV=1`. tmux 3.6b is installed. zellij is absent. `/usr/bin/sandbox-exec` is present. `bwrap` is absent, which is normal on macOS.

`~/.local/bin/spawn` is the starter. It asks herdr to `worktree create`, splits a pane, claims the daisy work item with the herdr workspace id as `session`, and in the background runs `herdr agent start` with the permission prompts turned off:

| Agent | Flags `spawn` passes |
| --- | --- |
| Claude | `--dangerously-skip-permissions` |
| Codex | `--dangerously-bypass-approvals-and-sandbox` |
| Cursor | `--force --approve-mcps --trust` |

Worktree setup is detected by scraping the root pane for `worktree-setup: (running|done|failed)`, with a queue wait of 3 hours and a setup wait of 45 minutes (`queue_timeout_ms=10800000`, `setup_timeout_ms=2700000`). Nothing in `spawn` assigns a port.

`/orchestrate` is the watcher. It runs in a herdr pane, polls `lanes-watch` every 300 seconds, and reads `herdr agent read`. A dead agent (`agent` null, `stalled: agent gone`, or an expired lease) is reported to the person. `--revive` restarts it once per lane per six hours: reuse a shell pane or split one, `herdr agent start` again, reclaim the daisy lease, and prompt the new process to read git status and continue `/issue`. herdr 0.9.5's `agent` command has list, get, read, send-keys, prompt, rename, focus, wait, attach, start, and explain. It has no restart.

The local source tree at `~/Desktop/Code/herdr-dev` is `v0.6.8-3-gee0cf65`, three commits past the v0.6.8 tag, and its `Cargo.toml` still says AGPL-3.0-or-later. That is the tree [R6](r6-language-and-runtime.md) read. It is behind the installed binary. In that tree a child exit sends `PaneDied`, the pane is removed, and an agent launch clears `respawn_shell_on_exit`, so the thing that comes back is a shell on a launch pane, which is a new shell and a lost agent. Treat those lines as the old tree. The operational fact on 0.9.5 is the CLI: death is something `/orchestrate` notices later, and revival is a new `agent start`.

herdr's public pane and workspace ids are session-scoped and get compacted when panes close (the herdr skill says so). They are the wrong primary key for a lane that must survive a viewer restart. [R5](r5-state-and-storage.md) already keys a lane by worktree path and session. Closing daisy no longer stops the ledger. Closing the process parent still stops the agent. That is the hole this note closes.

R6's license sentence is updated. Installed herdr 0.9.5 is Apache-2.0. GitHub `herdrdev/herdr` reports SPDX `Apache-2.0` (pushed 2026-11-01). The relicense commit is `cd5ea1b`, 2026-07-22, "relicense herdr under apache-2.0". v0.9.5's `Cargo.toml` depends on `portable-pty` 0.9.0, and the vendored copy of that crate is MIT. Apache-2.0 means a socket client is a clean dependency for a `MIT OR Apache-2.0` harness ([ADR 0001](../decisions/0001-name-license-oss-posture.md)). It does not mean the harness should link the herdr crate. The installed binary is 24 MB. [R6](r6-language-and-runtime.md) measured a Rust harness-shaped binary at 2.1 MB. herdr is a terminal multiplexer (ratatui, a vendored libghostty-vt, layouts, themes). Linking it would make that multiplexer the product. [R1](r1-landscape.md) already set that product aside.

## Comparison

| | Parent of the agent | Survives closing the viewer | Knows the CLI session id | Worktree | Port block | Sandbox | Restart | License, this machine |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Harness supervisor | Yes. `wait` on the child. | Yes. The viewer is a client of the PTY master. | Yes. R4 adapter events. | `git worktree add`, path stored on the lane. | Ledger allocation, injected in the environment. | Passes the CLI's own flags. | `lane resume` with `--resume`. | The harness license. `portable-pty` is MIT. |
| herdr 0.9.5 | Yes, today. The PTY master is herdr's. | The herdr server can outlive its TUI client. Quitting the server closes the masters. | Hooks can report a session id into herdr. The lane ledger does not own it. | `herdr worktree create` shells out to git. | None found in `spawn` or the 0.9.5 agent CLI. | None. `spawn` turns the CLI sandboxes off. | No restart command. Screen status is a heuristic plus hooks. | Apache-2.0 since 2026-07-22. |
| tmux 3.6b | Possible, via control mode (`tmux -CC`) or the CLI. | Yes, while the tmux server runs. | No. The harness would still parse the CLI. | No. Claude Squad added git beside it. | No. | No. | `respawn-pane` reruns a command. `remain-on-exit` keeps a dead pane. | ISC. tmux 3.8 was tagged 2026-10-08. |
| zellij | Possible, via `zellij --session S action` and plugins. | Yes, while the session runs. | No. | No. | No. | No. | A command pane can be rerun from the UI. An initial command is ignored when attaching to a session that already exists. | MIT. Not installed here. |
| The agent CLI, unsupervised | The shell the person typed in. | Dies with that shell. | Its own files. | Whatever cwd it inherited. | Whatever the app defaults to. | Its own, if the flags allow it. | The person, or `/orchestrate`. | Proprietary CLIs, per lane. |

### herdr as the viewer

herdr is the right place to type on this machine, and the wrong parent for a lane.

It already does the hard UI work: PTY, scrollback, splits, a socket, agent hooks for Claude and Codex, `agent attach`. [VISION.md](../VISION.md) leaves the conversation in the agent's own terminal and schedules "owning the terminal and worktree runtime (or integrating deeply with herdr)" as milestone 6, after the ledger and the gates. Deep integration that keeps herdr as the parent keeps three owners: herdr for the process, daisy (then the harness) for the lease, `/orchestrate` for "is it dead". A lane stalls when any one of them is the wrong one. The 300 second poll and the screen read are the waiting pattern bet 6 is trying to leave. herdr's own detector, in the v0.6.8 tree, matches the bottom of the pane against per-CLI patterns and then lets hooks override. That is a sidebar light. A gate needs `wait` and `session.exited`.

The end state on a machine that already runs herdr: the pane's command is `chaperone attach <lane>`. herdr keeps the keyboard. The harness keeps the child. No Cargo dependency on herdr. The socket protocol we speak is our own.

### tmux and zellij as viewers, on the same terms

tmux can be a parent. Control mode is a text protocol, sessions survive the client, and `respawn-pane` exists. Claude Squad is that design (one tmux session and one git worktree per task), and [R1](r1-landscape.md) calls it a session switcher: ISC would be fine, AGPL was Claude Squad's license, and the missing pieces are the gates and the ledger. Putting the harness in front of tmux as the brain, with tmux still the parent, adds a second long-lived server the harness does not ship and still leaves session ids, ports, and sandboxes to us. `respawn-pane` reruns argv. It does not call `claude --resume <id>`.

zellij is MIT and has sessions, `zellij action`, `zellij run`, and a plugin ABI. The control surface is younger than tmux control mode, and a trailing command on `attach` does nothing if the session is already there. It is a good viewer for someone who prefers it. It is the same split: viewer in front, supervisor behind.

Windows is out of scope for this note. `portable-pty` speaks ConPTY. tmux does not run natively there. One more reason the parent is the harness.

### The supervisor is small on purpose

Owning a PTY is `portable-pty` (MIT, already what herdr 0.9.5 uses) plus a unix socket. The attach client puts the user's tty in raw mode and copies bytes. There is no tab layout, no theme, no status sidebar, and no screen parser in the supervisor. Those can stay in herdr. Milestone 7's TUI is a client of the same socket, aimed at "what needs me", which is a ledger query ([R1](r1-landscape.md) bet 5).

`supervise` starts on demand when the first lane starts, listens on a socket under the runtime directory, and exits when no lane is left. It is the same binary as the CLI. [R5](r5-state-and-storage.md) rejected Postgres because that is a second product with its own port. This process is the harness, and it is absent when nothing is running. A test lane forks the fake agent CLI directly and skips the socket.

If `supervise` crashes, the children die with it. They are children so the supervisor can `wait`. On the next start, a lane still marked running whose pid is dead gets `session.exited` with a recovered flag. The supervisor does not resume it unless the pipeline rule says so. Detaching the agent from the supervisor (double-fork, ignore SIGHUP) would keep a live process we can no longer wait on, which recreates today's gap.

## Boundary

| Owns | Harness supervisor | Viewer (herdr, tmux, zellij, later the TUI) | Agent CLI |
| --- | --- | --- | --- |
| Git worktree and branch | Yes. `git worktree add` using the profile's branch template. | May display the path. | Works inside the cwd it was given. |
| Setup script | Yes. A child process, gated on its exit code, before the agent is started. |  |  |
| PTY master, pid, exit code | Yes. | Attaches. One writer at a time. A second attach takes the keyboard. | The foreground process. |
| Scrollback file | Yes, size-capped, path stored on the session. Bytes stay out of the event log. | Reads it when attaching. |  |
| Port block | Yes. A ledger transaction. |  | Binds what the environment names. |
| `session_id`, resume | Yes, from adapter events. |  | Mints the id. `--resume` accepts it. |
| Stall, exit, kill | Yes. Events `agent.stalled`, `session.exited`, `agent.killed`. | May show them. |  |
| Permission sandbox for the model's tools | Chooses the profile and passes the CLI flags. |  | Enforces its own Seatbelt or bubblewrap around the commands it runs. |
| Gate refused or accepted | Yes. [R7](r7-process-engine.md). The engine does not spawn. |  | Cannot mark a gate passed ([R4](r4-agent-cli-integration.md)). |
| Layout, theme, splits |  | Yes. |  |

Identity of a lane is the ledger id plus the worktree path. A herdr workspace id may be stored as a viewer hint for the years `spawn` is still the parent. It is not the session the resume command uses, and it is not unique across a herdr restart (ids compact).

Default terminal mode is a PTY, because the lanes people open today are interactive CLIs and [VISION.md](../VISION.md) keeps the conversation in that terminal. A pipeline may set `terminal = pipe` for a headless run (`claude -p`, `codex exec`). The parent, the watch, and the resume rule are the same. Headless is how R4's JSON streams are easiest to read. The PTY path still needs the adapter events. Scraping the PTY is the fallback the viewer can use for its own icon, and the ledger ignores it.

## Ports and resource limits

Two lanes in one repo both start the app's default server. `spawn` does not pass a port, and herdr 0.9.5's agent CLI does not allocate one. The collision is accidental, and the fix is assignment.

The supervisor keeps a range on the repo profile (starting suggestion: base 20000, width 10, range through 29999). `lane start` takes the next free block inside one `BEGIN IMMEDIATE`, the same transaction style as [R5](r5-state-and-storage.md), after checking that the first port of the block accepts a bind on `127.0.0.1`. The lane row stores `port_base` and `port_count`. The child environment gets `CHAPERONE_PORT` and `CHAPERONE_PORT_LAST`. Setup scripts and the lane brief read those. Releasing the block is part of the lane finishing or being cancelled.

That stops two honest lanes from both grabbing 3000. A process can still bind something else. Enforcing the block is a sandbox feature, and it is Linux-only in any practical way:

- Landlock ABI 4 (mainline 6.7) adds `LANDLOCK_ACCESS_NET_BIND_TCP`. A later supervisor can allow bind only on the block. ABI 4's connect right is the wrong tool here: restricting outbound connect-by-port would block GitHub unless every remote port was listed. UDP bind arrives at ABI 10. Query the ABI. Do not guess from `uname`.
- A network namespace per lane would isolate ports completely and would also cut off GitHub unless a second userspace network stack was added. That is a daemon. Leave it.

macOS has no Landlock and no cgroups. `sandbox-exec` is on this machine and is the Seatbelt entry Claude and Codex already use inside their own processes. A second Seatbelt wrapped around the CLI fights the first one (the CLI calls `sandbox-exec` itself). The port block on macOS stays an environment contract until [R13](../RESEARCH.md) takes the threat model.

Resource limits, first supervisor:

- `RLIMIT_NOFILE` raised or capped to a finite value when the inherited limit is unlimited, so a leak fails at a known point. Suggestion: 8192.
- No `RLIMIT_AS` and no `RLIMIT_CPU`. Node and rustc blow a tight address cap, and a CPU-second cap kills a compile that was the work.
- No cgroup by default. On Linux a profile may later set `MemoryHigh` through a systemd user scope (`systemd-run --user --scope`). Off unless the profile sets a byte count. macOS cannot do this.
- RSS of the lane is dominated by the agent CLI ([R6](r6-language-and-runtime.md)). The cost that burns a day is tokens. That budget is [R11](../RESEARCH.md), recorded from `usage.reported`, and it is the knob that stops a lane. A memory cap is a backstop for a runaway build, not the budget.

## Sandbox

The supervisor passes the CLI's sandbox. It does not wrap the CLI in a second one.

Today's `spawn` flags disable the sandbox so an unattended lane cannot block on an approval prompt. [R4](r4-agent-cli-integration.md) already separates those: `unattended` means the process must not wait on a person, and the sandboxes are not equivalent. The flags to pass once the supervisor is the parent:

| CLI | Unattended, sandbox left on | Where it is enforced |
| --- | --- | --- |
| Codex | `--sandbox workspace-write --ask-for-approval never` | macOS Seatbelt via `/usr/bin/sandbox-exec`. Linux and WSL2: bubblewrap plus seccomp, the default since the landlock path was marked legacy. Writable roots, with `.git` and `.codex` kept read-only. `danger-full-access` is what `spawn` selects today, and it drops the boundary. |
| Claude Code | Sandbox enabled in the settings passed for that run (`sandbox.enabled`, `allowUnsandboxedCommands` false), and no `--dangerously-skip-permissions` | Seatbelt on macOS. bubblewrap and socat on Linux and WSL2. The documented boundary covers shell commands. File tools, MCP servers, and hooks run outside it. If the sandbox cannot start, Claude runs the command unsandboxed unless `sandbox.failIfUnavailable` is set. |
| Cursor | `--sandbox enabled`, plus the unattended flags R4 already lists (`--force` still applies edits) | Cursor's own sandbox switch. |

Interactive lanes keep the CLI's normal approval mode so a person at the terminal can answer.

An OS wrapper around the whole CLI (bubblewrap or Seatbelt as the parent of `claude` itself) is [R13](../RESEARCH.md). The practical problems are already visible: Claude's sandbox ignores MCP and hooks, Codex moved off Landlock because it cannot isolate the unix sockets an app server uses, and our MCP server is a stdio child of the CLI that must keep talking to the ledger. A wrapper that is wrong either breaks the lane or pretends a boundary exists. Sibling worktrees are a git isolation until that threat model says otherwise. [R1](r1-landscape.md) notes Conductor documents the same limit: workspace isolation is development isolation.

Docker, Lima, and a microVM are a later option for an untrusted repo. They are a daemon and a slow start, which fights "one local binary, instant startup". They are not the default parent.

## Hung and dead

| What happened | How the supervisor knows | What it does |
| --- | --- | --- |
| Process exited | `wait` returns. On supervisor startup, a lane marked running has a dead pid. | Append `session.exited` with the code (or `recovered` after a crash). The lane leaves `active`. No new process. |
| Process alive, no PTY bytes and no `turn.finished` for `stall_after` | Timer in the supervisor. Default 15 minutes, overridable per profile. | Append `agent.stalled`. Interactive: surface it. Unattended with `resume-once`: one resume if the six-hour cap is free. Otherwise leave it stalled. |
| CLI is waiting on a person or a permission | Adapter event, when the CLI has one. | Append `agent.blocked`. This is a question, not a hang. |
| Profile sets `kill_after` | The stall timer, only when that key is set. | SIGTERM, 10 seconds, SIGKILL. Append `agent.killed` with the signal. Off unless the profile sets it. |
| Viewer closed | Attach client EOF. | Drop the client. Child untouched. |
| Setup script failed | Its exit code. | Lane `blocked`, agent not started. The person sees the setup log path. |

`resume` reuses the worktree and the port block and execs the CLI with the last `session_id`. A CLI that cannot resume mints a new id, and that is a new `session.started`, recorded as such. The six-hour cap matches today's `/orchestrate` revive cap so the dogfood behavior has a place to land in the pipeline file ([R7](r7-process-engine.md)) instead of in a skill.

`/orchestrate` can still ask for a resume. It asks the supervisor. It stops being the thing that notices death, which today depends on a 300 second poll and a screen.

## What this does not decide

- **The R13 threat model.** Which paths a sandbox may read, how secrets stay out of the PTY log, and whether a whole-CLI wrapper is worth it.
- **Token budgets (R11).** `kill_after` is a process timer. The budget that stops a lane for cost is usage events.
- **The migration week (R15).** `spawn` keeps working until `supervise` can create a worktree, wait on setup, and attach from a herdr pane. The ledger schema from this note is the part milestone 1 has to get right: pid, exit, session id, port block, viewer id as a hint.
- **A terminal UI.** `attach` is a raw tty. The "needs you" screen is milestone 7.
- **Windows.** ConPTY through `portable-pty` is the expected parent there. Not run here.
- **A launchd or systemd unit for the supervisor.** On-demand, exit when idle. A user service is worth adding only if a crash loop of `supervise` itself shows up. The children still die with it, and the next start still marks them exited.

## Sources

Read 2026-11-02 unless noted.

This repo and this machine:

- [VISION.md](../VISION.md) bets 6 and 7, non-goal on a chat UI, milestone 6.
- [RESEARCH.md](../RESEARCH.md) R8. [r1-landscape.md](r1-landscape.md) bet 7 and the Conductor isolation note. [r4-agent-cli-integration.md](r4-agent-cli-integration.md) spawn and permission profile. [r5-state-and-storage.md](r5-state-and-storage.md) lane row, worktree, "closing daisy does not stop a lane". [r6-language-and-runtime.md](r6-language-and-runtime.md) `portable-pty`, the 2.1 MB binary. [r7-process-engine.md](r7-process-engine.md) the engine does not run agents.
- [ADR 0001](../decisions/0001-name-license-oss-posture.md): `MIT OR Apache-2.0`.
- `~/.local/bin/spawn` (worktree, agent flags, setup scrape timeouts). `~/.claude/skills/orchestrate/SKILL.md` (watch interval, revive, herdr as the parent). `~/.claude/skills/herdr/SKILL.md` (ids compact, `agent_status`).
- herdr 0.9.5: `herdr --version`, `herdr agent --help`, `brew info herdr` (Apache-2.0, binary about 24 MB). GitHub `herdrdev/herdr` license via `gh api`, `Cargo.toml` at tag v0.9.5 (`license = "Apache-2.0"`, `portable-pty` 0.9.0). Relicense: commit `cd5ea1b`, 2026-07-22. Vendored `portable-pty` `Cargo.toml` at that tag: MIT.
- Stale tree, used only for the exit path: `~/Desktop/Code/herdr-dev` at `ee0cf65` (`v0.6.8-3`), `src/pty/backend.rs`, `src/events.rs` (`PaneDied`), `src/app/actions.rs` (`handle_pane_died`), `src/app/agent_resume.rs` (clears `respawn_shell_on_exit`), `src/detect/mod.rs`.
- tmux 3.6b on this machine. zellij and `bwrap` absent. `/usr/bin/sandbox-exec` present. Darwin 25.6.0.

Outside:

- herdr license file: <https://github.com/herdrdev/herdr/blob/HEAD/LICENSE>
- tmux ISC and control mode: <https://github.com/tmux/tmux>, <https://github.com/tmux/tmux/wiki/Control-Mode>. `remain-on-exit` and `respawn-pane`: <https://libtmux.org/en/tmux/latest/catalog/options/remain-on-exit/>. Release list including 3.8 on 2026-10-08: <https://tmux.app/>.
- zellij MIT and CLI: <https://github.com/zellij-org/zellij/blob/main/LICENSE.md>, <https://zellij.dev/documentation/commands>, <https://zellij.dev/documentation/controlling-zellij-through-cli>.
- Codex sandbox: <https://learn.chatgpt.com/docs/sandboxing>, Linux helper <https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md>.
- Claude Code sandbox (shell commands only; Seatbelt; bubblewrap and socat): <https://code.claude.com/docs/en/sandboxing>.
- Landlock TCP bind, ABI 4, mainline 6.7: <https://landlock.io/news/4/>, <https://man7.org/linux/man-pages/man7/landlock.7.html>.
- `portable-pty` MIT: <https://crates.io/crates/portable-pty>.
