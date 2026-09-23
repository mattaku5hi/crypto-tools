//! Output formatting: JSONL envelope + broken-pipe-safe writer. See
//! docs/CLI.md §7-8 for the contract this implements.
#![allow(clippy::module_inception)]

mod jsonl;
mod writer;

pub use jsonl::{JsonlRecord, RunStatus, SCHEMA_VERSION, Window};
pub use writer::{WriteOutcome, write_lines_to_stdout};
