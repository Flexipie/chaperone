# ADR 0005: Product name Chaperone

Status: accepted, 2026-11-05  
Amends: [ADR 0001](0001-name-license-oss-posture.md) (name, package, and GitHub decisions; license and visibility stand)  
Evidence: [r3-name-and-license.md](../research/r3-name-and-license.md) (the Seamlane pass), registry and trademark checks from 2026-11-05 recorded below

## Context

ADR 0001 picked Seamlane. The owner prefers Chaperone: the word says what the product does, a human keeping watch over agents that are not fully autonomous. Leash was rejected earlier because `leash` is taken on GitHub, npm, and PyPI by an agent-identity product.

Checks on 2026-11-05, from this machine:

| Name | crates.io | npm | PyPI | GitHub |
| --- | --- | --- | --- | --- |
| `chaperone` | taken (0.0.1, bioinformatics tool) | taken (old web tour library) | taken (Docker init daemon) | org `chaperone` exists with 0 public repos |
| `chaperone-cli` | free | free | free | one repo `marckraw/chaperone-cli`, 0 stars |
| `chaperone-ui` | free | free | free | one repo `vmware-archive/chaperone-ui` |

Domains (RDAP, a snapshot): `chaperone.dev`, `.com`, `.ai`, `.run`, `.tools` registered. `chaperone.io`, `chaperone.sh`, `chaperone.cli` returned no registration. `chaperone.app` timed out.

The agent space already uses the word. The closest projects are [agent-chaperone](https://github.com/agent-chaperone/agent-chaperone) (tool-call screening, MCP proxy), [Korsdal/Chaperone](https://github.com/Korsdal/Chaperone) (MCP coordination for concurrent Claude sessions), and [Jai-Keshav-Sharma/chaperone](https://github.com/Jai-Keshav-Sharma/chaperone) (MCP security gate). [RosettaCommons/chaperone](https://github.com/RosettaCommons/chaperone) ships a `chaperone` CLI.

Trademark: CHAPERONE has a pending class 9 application (serial 99444440, filed 2025-10-15 by Wosler Corp, office action response entered June 2026). Dead registrations in classes 9 and 42 are cancelled. Status was read from aggregator sites, not from TSDR.

## Decision

1. **Product name:** Chaperone.
2. **Packages:** `chaperone-cli` (the CLI and its crates or npm package) and `chaperone-ui` (the UI). The bare `chaperone` name is not used for packages.
3. **Repository:** `chaperone`. The owner is not yet fixed, so the repo can start under a personal account and move to an org later.
4. **Carried over from ADR 0001:** license `MIT OR Apache-2.0`, public from the first code commit, `0.x` until the contract and schema settle, milestone 8 as the 1.0 bar.

## Open before first publish

- **Binary name.** `chaperone` is the obvious command, but RosettaCommons already ships one. Choose between `chaperone` (accepting the PATH clash) and `chaperone-cli` as the installed command. Not decided here.
- **Trademark.** Check serial 99444440 on USPTO TSDR and get a clearance opinion before any filing or paid marketing.
- **Domain.** Confirm `chaperone.io` or `chaperone.sh` with a registrar before buying. `chaperone.dev` is taken.
- **Owner of the repo and the GitHub org.** Run `gh repo view <login>/chaperone` before creating it.
- **Differentiation.** The README and tagline should say what this product does that the agent-gate projects above do not: lanes, versioned gates, and the ledger.

## Consequences

- The product name in docs is **Chaperone**. Code names stay `seamlane` (`crates/seamlane-core`, `crates/seamlane`, `crates/seamlane-fake`, the `seamlane` binary, the `seamlane` event `tool` field, and the CLI contract) until a rename pass. That pass is its own change and is not part of this ADR.
- ADR 0001 items 1 to 3 (product name, GitHub, domain) are superseded by this ADR. Its license and visibility decisions stand.
- No empty crates or npm packages for `chaperone-cli` or `chaperone-ui`. The squatting rule in ADR 0001 still applies.
- Do not ship a `seam` alias, unchanged from ADR 0001.
