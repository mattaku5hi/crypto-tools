//! CLI integration tests for `wallet-stats`: run the compiled binary as a
//! subprocess and check exit codes per ADR-005/CLI.md §8.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-stats");

fn run_with_stdin(args: &[&str], stdin_data: &str) -> (i32, String, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes());
    }
    let output = child.wait_with_output().expect("failed to wait on child");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn valid_input_no_provider_is_exit_4() {
    let input = "base:0x1111111111111111111111111111111111111111\n";
    let (code, _stdout, stderr) = run_with_stdin(&["--input", "-"], input);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("no history provider configured"));
}

#[test]
fn empty_input_via_dash_stdin_is_exit_2() {
    // CLI.md §1: `--input -` reads stdin; empty input is a usage error
    // and the binary must exit rather than wait for more input.
    let (code, _stdout, stderr) = run_with_stdin(&["--input", "-"], "");
    assert_eq!(code, 2, "stderr: {stderr}");
}
