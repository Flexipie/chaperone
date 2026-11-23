//! A repo's pipeline profile as its base branch holds it (`[tests]`,
//! `[smoke]`, `[pr]`, `[diff] code`, companions), for the checks the CLI
//! runs, and the path globs those tables use. Reads git, never the ledger.

mod entry;
mod worktree;

pub use entry::glob;
pub use worktree::{
    BaseCode, BaseTests, Companion, Error, PrRules, REPO_PROFILE, Required, Tests, code_globs,
    commands_in, declares, pr_rules,
};
