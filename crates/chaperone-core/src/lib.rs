//! Chaperone's ledger and engine, and the deadline-bounded child processes
//! (git, daisy-cli) the CLI and the UI share. No CLI, no terminal I/O, no
//! home directory.

pub mod checks;
pub mod child;
pub mod cursor;
pub mod dossier;
pub mod git;
mod engine;
pub mod ledger;
pub mod pipeline;
pub mod spec;
pub mod stats;
pub mod usage;
pub mod work;
