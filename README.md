# harness (repo) / Seamlane (product name)

A local harness and orchestrator for running many coding agents at once: Claude Code,
Codex, Cursor and whatever comes next, each in its own lane, with the process they
follow enforced in code instead of hoped for in prompts.

Public name **Seamlane** ([ADR 0001](docs/decisions/0001-name-license-oss-posture.md)).
**herdr** stays where you start and talk to agents; **Seamlane** is the background ledger,
gates, and lane coordination.

Nothing is built yet. Planning artifacts:

- **[docs/ROADMAP.md](docs/ROADMAP.md)** — locked phases, stack, M1 metrics, herdr vs Seamlane
- [docs/VISION.md](docs/VISION.md) — goals and non-goals
- [docs/RESEARCH.md](docs/RESEARCH.md) — R1–R15 questions and links
- [docs/research/](docs/research/) — evidence notes
- [docs/decisions/](docs/decisions/) — ADRs (0001 name, 0002 architecture + roadmap)

No code lands until the research answers the questions that decide the architecture
(language, state model, agent integration, runtime) and those answers are written down
as decision records.
