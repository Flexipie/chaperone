//! Chaperone's ledger and engine, and the deadline-bounded child processes
//! (git, daisy-cli) the CLI uses. No CLI, no terminal I/O, no
//! home directory.

pub mod checks;
pub mod child;
pub mod cursor;
pub mod dossier;
mod engine;
pub mod git;
pub mod ledger;
pub mod merge;
pub mod overlap;
pub mod pipeline;
pub mod plan;
pub mod review;
pub mod runner;
pub mod spec;
pub mod stats;
pub mod usage;
pub mod verdict;
pub mod work;
