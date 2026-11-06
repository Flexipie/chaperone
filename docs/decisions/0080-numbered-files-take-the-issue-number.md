# ADR 0080: Numbered docs take the issue number

Status: accepted, 2026-11-07  
Evidence: issue [#80](https://github.com/Flexipie/chaperone/issues/80)

## Context

ADRs and research notes were numbered "next free". On 2026-11-07 two lanes running at once both wrote
`docs/decisions/0006-*.md`: #55 (merged as f4e3308) and #69 (PR #79). Each took the next free number on its own
branch, the files had different slugs so git saw no conflict, and the second lane found out only at merge time, then
renumbered and fixed links. Any file numbered from a repo-wide counter has the same race between live lanes.

## Decision

A new ADR or research note takes the number of the GitHub issue its lane works on, zero-padded to four digits for
ADRs: `docs/decisions/0080-<slug>.md` is "ADR 0080", `docs/research/r80-<slug>.md` is "R80". With no issue, use the
PR's number. GitHub hands out issue and PR numbers from one counter per repo, so two lanes cannot pick the same one.
A lane that writes a second ADR for the same issue adds a letter: `0080b-<slug>.md`. Numbers have gaps, and order by
number is roughly order of filing, not of merge.

ADRs 0001 to 0006 and R1 to R15 keep their numbers.

**Migrations are excluded.** `crates/chaperone-core/migrations/000N_*.sql` is numbered by `PRAGMA user_version`,
which is the file's position in `MIGRATIONS` (`crates/chaperone-core/src/ledger/mod.rs`). It has to stay dense and in
merge order, so it cannot take an issue number. The collision is not silent there: two lanes that each add a
migration both append to `MIGRATIONS`, and git reports a conflict on whichever merges second. That lane takes the next
number when it rebases, renames its file to match, and reruns the migration tests. Schema slices are still
serialised in `/dispatch` (#53 and #54) because a migration is a one-way door, not because of the number.

## Alternatives considered

- **A ledger reservation** (`adr:0007` claimed the way R9 reserves paths and seams). Needs a new command and a
  schema migration, and every lane has to remember to claim before writing. It hands out a counter GitHub already
  hands out. Path claims ([R9](../research/r9-coordination-between-lanes.md)) are roadmap phase 6; if they land, they
  still do not need to cover doc numbers.
- **Dates** (`2026-11-07-<slug>.md`). Unique only per day; two lanes on one day collide again.
- **Slug only, no number.** Nothing to collide, but loses the short "ADR N" name the docs already use in links.

## Consequences

A lane never renumbers a doc at merge. Links to an ADR can be written on the branch and stay right. A lane with
no issue and no PR yet (a draft) picks its number when it opens the PR.
