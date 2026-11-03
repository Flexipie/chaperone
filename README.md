# harness (repo) / Seamlane (product name)

A local harness and orchestrator for running many coding agents at once: Claude Code,
Codex, Cursor and whatever comes next, each in its own lane, with the process they
follow enforced in code instead of hoped for in prompts.

Public name **Seamlane** ([ADR 0001](docs/decisions/0001-name-license-oss-posture.md)).
**herdr** stays where you start and talk to agents; **Seamlane** is the background ledger,
gates, and lane coordination.

The Rust workspace and CI are in place (`crates/seamlane-core`, `crates/seamlane`,
`crates/seamlane-fake`); the ledger and gates are being built in milestone 1. To build
and test, see [CONTRIBUTING.md](CONTRIBUTING.md) "Setup".

Planning artifacts:

- **[docs/ROADMAP.md](docs/ROADMAP.md)** — locked phases, stack, M1 metrics, herdr vs Seamlane
- [docs/VISION.md](docs/VISION.md) — goals and non-goals
- [docs/RESEARCH.md](docs/RESEARCH.md) — R1–R15 questions and links
- [docs/research/](docs/research/) — evidence notes
- [docs/decisions/](docs/decisions/) — ADRs (0001 name, 0002 architecture + roadmap)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
