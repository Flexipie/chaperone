# Chaperone (product name; repo still `harness`)

A local harness and orchestrator for running many coding agents at once: Claude Code,
Codex, Cursor and whatever comes next, each in its own lane, with the process they
follow enforced in code instead of hoped for in prompts.

Public name **Chaperone** ([ADR 0005](docs/decisions/0005-chaperone-name.md)).
**herdr** stays where you start and talk to agents; **Chaperone** is the background ledger,
gates, and lane coordination.

The Rust workspace and CI are in place (`crates/chaperone-core`, `crates/chaperone-cli`,
`crates/chaperone-fake`, `crates/chaperone-ui`); the ledger and gates are being built in
milestone 1. To build and test, see [CONTRIBUTING.md](CONTRIBUTING.md) "Setup".

`chaperone-ui` is a read-only window with every open lane, grouped by repo and spec, that
redraws when the ledger changes: `cargo run --release -p chaperone-ui -- [--db PATH]`.
Clicking a lane shows its issue, spec slice and decisions, handoffs, checks, review
findings and its worktree's diff against the base; a feed shows the questions, answers and
decisions between lanes. The change graph tab draws each lane's changed files in its
colour, the files that depend on them by depth and the entry points they reach
(`overlap` and `impact`), with files two lanes share ringed.
It opens the ledger read-only and never writes it; the graph writes only the code graph's
own cache. CI shows "not recorded" until the
ledger has a CI record.

Planning artifacts:

- **[docs/ROADMAP.md](docs/ROADMAP.md)** — locked phases, stack, M1 metrics, herdr vs Chaperone
- [docs/VISION.md](docs/VISION.md) — goals and non-goals
- [docs/RESEARCH.md](docs/RESEARCH.md) — R1–R15 questions and links
- [docs/research/](docs/research/) — evidence notes
- [docs/decisions/](docs/decisions/) — ADRs (0001 name and license, 0002 architecture + roadmap, 0005 Chaperone name)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
