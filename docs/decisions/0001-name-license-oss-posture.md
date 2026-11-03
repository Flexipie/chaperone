# ADR 0001: Product name, license, and open-source posture

Status: accepted, 2026-11-02  
Decides: [RESEARCH.md](../RESEARCH.md) R3  
Evidence: [r3-name-and-license.md](../research/r3-name-and-license.md)

## Context

The repository directory is `harness`, but that name collides with the Harness platform and crates.io. We need a public name, package identifiers, a license, and when the repo becomes visible before implementation starts.

## Decision

1. **Product name:** Seamlane (command, primary crate, and npm package: `seamlane`).
2. **GitHub:** org and repo `seamlane/seamlane` when the org is created (handle was free at check time).
3. **Domain:** prefer `seamlane.dev`; do not rely on `seamlane.com` (registered, no useful resolution).
4. **License:** SPDX `MIT OR Apache-2.0`, with `LICENSE-MIT`, `LICENSE-APACHE`, and the Rust-style dual-license contribution paragraph in the README when code exists.
5. **Visibility:** public from the first commit that contains code, with `0.x` releases until the integration contract and event schema stabilize. Milestone 8 in [VISION.md](../VISION.md) remains the 1.0 quality bar, not the day the repo goes public.

## Alternatives considered

- **Gatestamp:** clearer description of gates and version stamps; rejected because `github.com/gatestamp` is an existing user handle.
- **Cleatline / Bightline:** clean registries; rejected for pronunciation and spelling cost (Bightline) or metaphor opacity (Cleatline).
- **Keep "harness":** rejected due to namespace collision with Harness and crates.io.
- **Apache-2.0 only:** viable (LaneGate uses it); rejected in favor of dual license so small library crates can pick MIT while patent grant remains available.
- **AGPL / source-available:** rejected; local-only product, adapters beside proprietary CLIs, and non-goal of hosted SaaS make copyleft/network clauses poor fit.
- **Private until milestone 1 or 8:** rejected; decision records and schemas are part of the product, and waiting does not hide the idea (similar projects are already public).

## Consequences

- README and user-facing docs should say **Seamlane**; the git remote path can stay `harness` until rename is worth the churn.
- Do not publish empty crates.io or npm placeholders (squatting policy); reserve via GitHub org first.
- Do not ship a `seam` CLI alias (collision with Seam's CLI).
- R6 (Rust) aligns with this license default; no license reopen when language is accepted.

## Follow-up (not blocking research)

- Create GitHub org when you accept this ADR.
- Register `seamlane.dev` if still available at purchase time.
- Add ADR 0002 for language (R6) and storage (R5) after those items are accepted.
