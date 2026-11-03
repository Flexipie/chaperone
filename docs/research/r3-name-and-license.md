# R3. Name and open-source model

Status: recommendation, checked 2026-11-02. This file is the research output for [RESEARCH.md](../RESEARCH.md). It is ready to lock in `docs/decisions/`. Nothing here has been registered.

## Recommendation

| Choice | Pick |
| --- | --- |
| Product name | **Seamlane** |
| Command, crate, npm package | `seamlane` |
| GitHub | `seamlane/seamlane` (the user/org handle returned 404) |
| License | **MIT OR Apache-2.0** |
| When it becomes public | The first commit that contains code, with `LICENSE-MIT` and `LICENSE-APACHE` in that commit |

Seamlane is the name that still says what the product is (a lane, and the seam between lanes) and that can own `github.com/seamlane` today. Gatestamp names the gates and the version stamp more directly, and loses because `github.com/gatestamp` is already a user account. Cleatline and Bightline are as free on the registries and harder to hear once and spell.

The license is the Rust dual license, chosen before [R6](../RESEARCH.md) picks a language. Users who want the patent grant take Apache-2.0. Users who want the smallest permission set take MIT. Copyleft stays out: this is a local binary, a hosted service is a [non-goal](../VISION.md), and adapters have to sit next to proprietary agent CLIs.

Milestone 8 in the vision stays the bar for a 1.0. It is the release-quality bar, not the day the repository becomes public.

## What the name is for

A local-first harness for parallel agent lanes: versioned process and gates, an event-sourced ledger, and bring-your-own agent CLIs. It is a local binary. It is not an LLM loop, a hosted service, or a chat UI ([VISION.md](../VISION.md)).

The repo folder is `harness`. That word is the category name ("agent harness") and it is already a company: [harness/harness](https://github.com/harness/harness) is the Harness developer platform, and [crates.io/crates/harness](https://crates.io/crates/harness) is a benchmarking library (`harness` 0.0.8). The public name has to change even if the directory is renamed later.

## How the checks were made

Checked 2026-11-02, from this machine:

- GitHub user/org: `https://api.github.com/users/<name>` and `https://github.com/<name>` (404 means the handle is free).
- GitHub repos: `search/repositories?q=<name>+in:name`. This search is a substring, so a total of 0 means the string appears in no repository name.
- crates.io: `https://crates.io/api/v1/crates/<name>`. A miss is `404` with `crate \`<name>\` does not exist`.
- npm: `https://registry.npmjs.org/<name>`. A miss is `404` `{"error":"Not found"}`. Search: `/-/v1/search?text=seamlane` returned `total: 0`.
- PyPI, as a third registry: `https://pypi.org/pypi/<name>/json`. LaneGate is the reason this column exists. Its npm and crates names were free, and the project was already on PyPI.
- Domains: `https://rdap.org/domain/<name>.<tld>` followed across redirects. HTTP 404 means the RDAP server has no registration. `.com` results were checked again with Verisign whois and a DNS lookup. Nothing was purchased.

A free registry name can be taken tomorrow. These tables are a snapshot.

## Candidates

Eight names that could still be the product. The first six are proposable. Seamgate and Spanwork are listed because they look right and lose on a specific collision.

| Name | GitHub handle | Repos `in:name` | crates.io | npm | PyPI | `.com` | `.dev` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **seamlane** | 404, free | 0 | 404 | 404 | 404 | Registered (OVH). Does not resolve. | 404, looks free |
| **gatestamp** | Taken. Existing user, 0 public repos | 0 | 404 | 404 | 404 | Registered (GoDaddy). HTTPS timed out. | 404, looks free |
| **cleatline** | 404, free | 0 | 404 | 404 | 404 | Registered. TLS certificate expired. | 404, looks free |
| **bightline** | 404, free | 0 | 404 | 404 | 404 | Whois: `No match for domain "BIGHTLINE.COM"`. Does not resolve. | 404, looks free |
| **rigcord** | 404, free | 0 | 404 | 404 | 404 | Registered. TLS handshake failed. | 404, looks free |
| **seamgate** | 404, free | 0 | 404 | 404 | 404 | Registered. | 404, looks free |
| **spanwork** | Taken. User, 0 public repos | 2, including [Crowds21/Spanwork](https://github.com/Crowds21/Spanwork) | 404 | 404 | not checked | not checked | not checked |
| **jackstay** | Taken. User, 0 public repos | 4, including [flotilla-org/jackstay](https://github.com/flotilla-org/jackstay) and a keyboard-sharing repo | 404 | 404 | not checked | not checked | not checked |

`.io` and `.app` for Seamlane, Gatestamp, Cleatline, and Bightline also returned RDAP 404.

### Seamlane (pick)

A seam is the boundary a lane owns. The vision already uses both words: lanes as the unit of work, seams as the place two lanes meet. The command is one token, two syllables, and spelled the way it sounds.

`seamlane` on crates.io, npm, and PyPI all 404. GitHub repository search for the string returned 0. `github.com/seamlane` is free. `seamlane.dev`, `seamlane.io`, and `seamlane.app` look unregistered. `seamlane.com` is registered at OVH and has no DNS; treat the `.com` as unavailable and use `.dev`.

The real neighbour is [Seam](https://www.seam.co/), whose CLI is installed as `seam` (`npm install -g seam`, Homebrew `seam`). Seam currently markets that CLI at coding agents. The strings differ (`seamlane` vs `seam`), the products differ (local process harness vs an API for locks and devices), and a web search for the exact string "seamlane" lands on a sold-out clothing style, not on Seam. The binary is `seamlane` in full. A `seam` alias would shadow their command and should not ship.

Install and paths, once there is code: `cargo install seamlane` if R6 picks Rust, `npx seamlane` or a scoped package only if a Node entry point exists, config in `~/.seamlane/` and `.seamlane/`, env `SEAMLANE_HOME`. Publish the primary crate as `seamlane` so the obvious name and the binary stay the same package. Later libraries can be `seamlane-core`, `seamlane-ledger`, and so on.

### Gatestamp

Gates plus the process-version stamp each lane carries. Best description of bet 1 in the vision (a transition the harness refuses, and a version stamped on the lane). Registries are free and no repository is named gatestamp.

`github.com/gatestamp` belongs to an existing user with 0 public repositories. GitHub will not hand that handle over. The org would have to be something like `gatestamp-dev`, which is a permanent tax on a new project. `.com` is registered at GoDaddy.

### Cleatline

A cleat is where a line is made fast. Handle, registries, and PyPI are free, and no repository uses the name. `.com` is registered and serving a host with an expired certificate. `.dev` looks free.

The namespace is as clean as Seamlane. The word needs a glossary line before it means anything, and "cleat" is easy to misspell for anyone who does not sail. Use it if a future trademark pass rejects sharing the token "seam" with Seam.

### Bightline

A bight is a bend or loop in a line. Handle, registries, and PyPI are free, no repository uses the name, and the `.com` appears unregistered (Verisign whois returned no match, RDAP 404, DNS does not resolve). That is the cleanest domain of the set.

Heard aloud, it collides with "bite" and "byte". The spelling has to be corrected every time someone hears it. A free `.com` does not fix a name people cannot type.

### Rigcord

Handle, registries, and PyPI are free, and no repository is named rigcord. `.com` is registered and did not complete a TLS handshake. `.dev` looks free.

One letter away from "ripcord" (the emergency cord, plus [Ripcord Software](https://github.com/RipcordSoftware) and the npm package `ripcord`). A mishear lands on a different word. The rigging metaphor is doing less work than Seamlane's own vocabulary.

### Seamgate

Handle and registries are free, and no repository is named seamgate. It stacks the two ideas (seam and gate) and then reads as a scandal word, the "X-gate" pattern. It also sits closer to Seam's mark than Seamlane does, and `.com` is registered. Dropped.

### Spanwork

Registries are free. The handle is taken, [Crowds21/Spanwork](https://github.com/Crowds21/Spanwork) exists, and [spanwork.net](https://spanwork.net/) is a live consultancy ("Independent CTO-led reviews for software companies"). Dropped.

### Jackstay

A jackstay is a line something travels along, which is a good metaphor and a bad namespace. Registries are free. The GitHub handle is taken, and repository search found [flotilla-org/jackstay](https://github.com/flotilla-org/jackstay) plus a keyboard-and-clipboard sharing project under the same name. Dropped.

## Candidate: leash (rejected, 2026-11-02)

The metaphor fits (keeping agents on a lead without being the agent). The namespace does
not.

| Check | Result |
| --- | --- |
| GitHub `@leash` | **Taken** (user account, HTTP 200) |
| Repos `leash in:name` | **947** hits, including [strongdm/leash](https://github.com/strongdm/leash) (~600 stars): "Leash by StrongDM, take your AI agents for a walk" |
| npm `leash` | **Taken**: JavaScript MongoDB service handler |
| PyPI `leash` | **Taken**: "Leash, AI Agent Identity, Authorization, and Audit Layer" (same category as this project) |
| crates.io `leash` | Registry returned non-JSON (likely exists or blocked); do not assume free |
| `leashed` on PyPI | 404; npm/crates still occupied |

Using **leash** as the product or crate name would collide with an existing agent-control
brand and an PyPI package in identity/audit for agents. Search and `cargo install` would
send people to the wrong product. A CLI alias `leash` would shadow StrongDM's positioning
and is not recommended.

If you want the same metaphor with a free handle, **Seamlane** (pick above) or a compound
(**leashlane**, **firmleash**) would need the same registry pass. **Gatestamp** remains the
descriptive alternative without the pet metaphor.

## Names that are already the category

Kept here so the working title and the obvious metaphors are not reopened.

| Name | What already has it |
| --- | --- |
| harness | [harness/harness](https://github.com/harness/harness), the Harness platform. [crates.io `harness`](https://crates.io/crates/harness) 0.0.8, a benchmarking library. npm name `harness` is also taken (HTTP 200). |
| lanegate | [sudheerdvn/lanegate](https://github.com/sudheerdvn/lanegate), Apache-2.0. PyPI `lanegate` 1.2.0: "Parallel lanes. Protected gates. A git-native delivery engine for AI coding agents." crates.io and npm for the exact name were 404. Registry checks alone would have missed it. |
| lanes | [lanes-sh/app](https://github.com/lanes-sh/app): "Run many CLI (Coding) Agents in parallel lanes." |
| gateward | [torad-labs/gateward](https://github.com/torad-labs/gateward): "Engineering tenets enforced at write time, portable across coding-agent harnesses." Search also hits gatewarden security tools. |
| lineward | [Line-Ward](https://lineward.com/) of Buffalo, a line-laying equipment company since 1972, on `lineward.com`. Also [devAyyoub/Lineward](https://github.com/devAyyoub/Lineward), an identity-governance project. crates.io and npm were 404. |
| fairlead | [crates.io `fairlead`](https://crates.io/crates/fairlead) 0.14.1: "Guardrails and a guided path for coding agents." npm name taken. |
| holdfast | [crates.io `holdfast`](https://crates.io/crates/holdfast) 0.0.0: a name reservation for an MCP server that gives agents a PTY shell. npm name taken. |
| heddle | [roackb2/heddle](https://github.com/roackb2/heddle): "An open-source terminal coding agent runtime and CLI." [crates.io `heddle`](https://crates.io/crates/heddle) is a separate reservation. npm name taken. |
| moorline | [Moorline/moorline](https://github.com/Moorline/moorline): "Moorline host runtime, CLI, contracts, and control API." npm `moorline` 0.0.9. |
| gateline | npm `gateline` 0.0.0: "Every PR walks the gateline. Reserved." |
| stayline | A dozen small repositories already use the name (booking, crew scheduling, a Teams client, a student-copilot prototype). Registries were free. The word is too generic to reclaim. |
| springline | npm `springline` 0.1.1, a CSS easing helper. crates.io was 404. |
| selvedge / selvage | npm `selvedge` taken, crates.io `selvage` taken. The two spellings would split search either way. |
| headgate, hawser, bollard, caisson, sluice, quire, waymark, filament, belay, keelson, drydock, slipway, trestle, lintel, rubric, docket, waybill, lanyard | At least one of crates.io or npm already returned 200. |

## License

The software runs on the operator's machine, works offline, and has no account. [VISION.md](../VISION.md) lists a hosted multi-tenant service as a non-goal. The integration contract is something Claude Code, Codex, Cursor, and a local client such as daisy all have to be able to call. Those constraints pick the license.

| License | Fit for this project |
| --- | --- |
| **MIT OR Apache-2.0** | Permissive either way. Apache-2.0's patent grant is there for whoever chooses it. MIT keeps the crate usable anywhere the Rust stack is usable. This is the Rust project default ([C-PERMISSIVE](https://rust-lang.github.io/api-guidelines/necessities.html)). |
| Apache-2.0 alone | Patent grant on every copy, one file, and the license [LaneGate](https://github.com/sudheerdvn/lanegate) already uses. The same Rust guideline says an Apache-only crate is harder to combine with the rest of the Rust stack, which is mostly dual-licensed or MIT. |
| MIT alone | Simplest text, no patent grant. Weaker fit in a category (workflow engines, multi-agent control) where patents show up. |
| MPL-2.0 | File-level copyleft would pull core fixes back. It also makes the event schema and the integration contract heavier for a closed agent CLI, or for daisy, to vendor. |
| GPL-3.0 | Distribution of a modified binary would require source. Internal patches at a company that never ships the binary would stay private, so the copyleft would not retrieve the patches most likely to exist. Linking a library into another program forces that program's license question into every adapter. |
| AGPL-3.0 | Written for programs users interact with over a network ([Why the Affero GPL](https://www.gnu.org/licenses/why-affero-gpl.html)). Running this harness locally does not trigger that clause. The clause still dominates legal review, including for teams whose agents only shell out to a local binary. |
| BUSL, FSL, SSPL, source-available | A time delay or a service restriction protects a hosted business. Hosted service is a non-goal, and the quality bar in the vision is a normal open-source project. |

### The license to use

**SPDX `MIT OR Apache-2.0`.** One expression, valid if R6 lands on Rust, Go, or TypeScript, so the license decision does not reopen with the language decision.

Apache-2.0 is the half that matters for patents: contributors grant a patent license to the work, and that grant ends for a party who sues over patents in the work ([Apache-2.0, section 3](https://www.apache.org/licenses/LICENSE-2.0#patent)). MIT is the half that keeps a small library crate easy to depend on. Downstream picks one. They do not comply with both.

This only works if inbound contributions are dual-licensed too. The README, when code exists, carries the contribution paragraph from the Rust API guidelines:

```
Licensed under either of

 * Apache License, Version 2.0
   (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   (LICENSE-MIT or https://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
```

Crates use `license = "MIT OR Apache-2.0"`. npm uses `"license": "MIT OR Apache-2.0"`. Texts live at the repo root as `LICENSE-APACHE` and `LICENSE-MIT` (from [choosealicense.com](https://choosealicense.com/licenses/apache-2.0/) and [choosealicense.com/licenses/mit/](https://choosealicense.com/licenses/mit/)). Add a `NOTICE` file only when the tree actually contains third-party notices.

## Public from the first code commit

| Posture | What it does here |
| --- | --- |
| **Public repo and license as soon as code exists** | The decision records, the integration contract, and the event schema are the product, and the vision says those are written down before they are implemented. A public repo is also the thing that holds the GitHub org. |
| Private until the ledger works | The APIs can churn without an audience. The org, the crate, and the npm name stay unclaimed the whole time. LaneGate is already public with this pitch, so waiting does not keep the idea private. |
| Public at milestone 8 | Matches a literal reading of the vision's last step ("open source, if it holds up"). Treat that step as the 1.0 quality bar. A first public commit at the end of the roadmap gives every colliding name months of head start. |

Personal material stays off the public repo either way: tokens, the daisy database, lane transcripts, customer data. The threat model in R13 is the rule for that. The harness source does not need it.

Until 1.0, the README should say the integration contract and the event schema can change. Version the releases `0.x`.

## Reserving the name without squatting

Do these when this recommendation is accepted, and not before:

1. Create the GitHub org `seamlane` and put the repo at `seamlane/seamlane`.
2. Register `seamlane.dev` if it is still free at purchase time. Leave `seamlane.com` alone unless it is dropped.
3. Publish crates and npm packages with the first real artifact, under the dual license, with a repository URL that points at the org.

crates.io is first-come on names, and its [policies](https://crates.io/policies) say a package that exists only to reserve a name, with no functionality and no real development, is squatting and can be deleted. An empty `0.0.0` placeholder is the wrong move. The same patience applies to an empty npm package. Creating the GitHub org is the reservation that does not require a fake release.

## Decision record, when one is written

1. Product name **Seamlane**, command `seamlane`, GitHub `seamlane/seamlane`, primary package `seamlane`.
2. License **MIT OR Apache-2.0**, with the dual-license contribution sentence in the README.
3. Public from the first code commit. Pre-1.0 until the contract and the event schema settle. Milestone 8 remains the 1.0 bar.

## Sources

Checked 2026-11-02 unless the link is a standing document.

- [VISION.md](../VISION.md), [RESEARCH.md](../RESEARCH.md) R3
- Registry and handle results recorded above (crates.io, registry.npmjs.org, PyPI, GitHub API, RDAP, Verisign whois for `bightline.com`)
- [harness/harness](https://github.com/harness/harness), [crates.io/crates/harness](https://crates.io/crates/harness)
- [sudheerdvn/lanegate](https://github.com/sudheerdvn/lanegate), [pypi.org/project/lanegate](https://pypi.org/project/lanegate/)
- [lanes-sh/app](https://github.com/lanes-sh/app)
- [torad-labs/gateward](https://github.com/torad-labs/gateward)
- [lineward.com](https://lineward.com/), [devAyyoub/Lineward](https://github.com/devAyyoub/Lineward)
- [crates.io/crates/fairlead](https://crates.io/crates/fairlead), [crates.io/crates/holdfast](https://crates.io/crates/holdfast), [crates.io/crates/heddle](https://crates.io/crates/heddle)
- [roackb2/heddle](https://github.com/roackb2/heddle), [Moorline/moorline](https://github.com/Moorline/moorline)
- [Seam](https://www.seam.co/), [Seam CLI](https://docs.seam.co/developer-tools/seam-cli)
- [spanwork.net](https://spanwork.net/)
- [Rust API Guidelines, C-PERMISSIVE](https://rust-lang.github.io/api-guidelines/necessities.html)
- [Apache License 2.0, patent grant](https://www.apache.org/licenses/LICENSE-2.0#patent)
- [Why the Affero GPL](https://www.gnu.org/licenses/why-affero-gpl.html)
- [crates.io policies](https://crates.io/policies) (package names, squatting)
- [choosealicense.com Apache-2.0](https://choosealicense.com/licenses/apache-2.0/), [choosealicense.com MIT](https://choosealicense.com/licenses/mit/)
