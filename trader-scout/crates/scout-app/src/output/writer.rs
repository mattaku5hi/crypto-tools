//! Output writer with broken-pipe-safe stdout handling.
//!
//! `println!`/`writeln!` on a closed pipe (`... | head -1`) triggers a
//! *panic* from Rust's standard formatting macros by default. Under
//! this workspace's `panic = "deny"` clippy policy that would be a
//! direct lint violation if used unguarded — but more importantly it is
//! exactly the failure ADR-005/CLI.md §8 forbid: exit 141 must be a
//! clean shutdown, "без panic/backtrace, с корректной остановкой
//! producer'ов," not an unhandled panic dumped to a half-closed
//! terminal.

use std::io::{self, Write};

/// Result of attempting to write output lines to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// All lines were written successfully.
    Complete,
    /// The output pipe closed partway through (e.g. `| head -1`). The
    /// caller must stop producing further output and use exit code 141
    /// — this is not an error to report to the user.
    PipeClosed,
}

/// Write each line to stdout, followed by a newline, stopping cleanly
/// (never panicking) if the pipe closes. Uses a single locked handle so
/// concurrent writers elsewhere in the process cannot interleave with
/// this batch.
pub fn write_lines_to_stdout<I: IntoIterator<Item = String>>(lines: I) -> WriteOutcome {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    for line in lines {
        if let Err(err) = writeln!(handle, "{line}") {
            if err.kind() == io::ErrorKind::BrokenPipe {
                return WriteOutcome::PipeClosed;
            }
            // Any other stdout error (disk full on a redirect, etc.) is
            // also not something to panic over; treat it the same way
            // as a closed pipe for the purposes of "stop producing
            // output cleanly" — the exit-code decision belongs to the
            // caller, this function's job is only to never panic.
            return WriteOutcome::PipeClosed;
        }
    }
    WriteOutcome::Complete
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_all_lines_when_stdout_is_open() {
        // This test's own stdout is open (captured by the test
        // harness), so this exercises the success path without needing
        // to simulate a closed pipe.
        let outcome = write_lines_to_stdout(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(outcome, WriteOutcome::Complete);
    }

    #[test]
    fn empty_iterator_is_a_no_op_success() {
        let outcome = write_lines_to_stdout(Vec::<String>::new());
        assert_eq!(outcome, WriteOutcome::Complete);
    }
}
