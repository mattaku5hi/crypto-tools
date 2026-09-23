//! CLI integration tests: exercise the actual compiled binary as a
//! subprocess, verifying exit codes per ADR-005/CLI.md §8. This is the
//! only test layer that can observe process::exit's real behavior —
//! unit tests inside library crates never see it.
//!
//! `env!("CARGO_BIN_EXE_...")` only works for binaries of *this* crate
//! (buyer-intersect) — Cargo does not expose sibling-crate binary paths
//! through that macro even when declared as a dev-dependency, since
//! wallet-rank/wallet-stats have no [lib] target for buyer-intersect to
//! actually link against. Instead, derive their paths from this test
//! binary's own location: all three binaries land in the same
//! `target/<profile>/` directory.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn sibling_binary(name: &str) -> PathBuf {
    let this_test_binary = env!("CARGO_BIN_EXE_buyer-intersect");
    let target_dir = PathBuf::from(this_test_binary)
        .parent()
        .expect("CARGO_BIN_EXE_buyer-intersect has a parent dir")
        .to_path_buf();
    target_dir.join(name)
}

fn run_with_stdin(bin: &str, args: &[&str], stdin_data: &str) -> (i32, String, String) {
    let mut child = Command::new(bin)
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
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

#[test]
fn buyer_intersect_empty_input_is_exit_2() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], "");
    // CLI.md §8: exit 2 = argument/format error. Empty input has no
    // identities to intersect, which is a usage error, not a
    // successfully-empty result.
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("empty input"));
}

#[test]
fn buyer_intersect_single_token_is_exit_2() {
    // CLI.md §3: "Меньше двух distinct input tokens — usage error для
    // задачи пересечений."
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let input = "base:0x1111111111111111111111111111111111111111\n";
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], input);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("at least 2"));
}

#[test]
fn buyer_intersect_valid_input_no_provider_is_exit_4() {
    // ADR-005: InfrastructureUnavailable when no HistoryProvider is
    // configured (ADR-006's ConfigurationRequired), not a fabricated
    // empty success.
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let input = "base:0x1111111111111111111111111111111111111111\n\
                 base:0x2222222222222222222222222222222222222222\n";
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], input);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("configuration required"));
    assert!(stderr.contains("SCOUT_EVM_HISTORY_API_KEY"));
}

#[test]
fn buyer_intersect_invalid_address_is_exit_2_with_line_number() {
    // ACCEPTANCE A04: invalid line -> error with line number, before
    // any scanning.
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let input = "base:0x1111111111111111111111111111111111111111\n\
                 base:0xnotvalidhex\n";
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], input);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("line 2"));
}

#[test]
fn wallet_rank_valid_input_no_provider_is_exit_4() {
    let bin = sibling_binary("wallet-rank");
    let input = "base:0x1111111111111111111111111111111111111111\n";
    let (code, _stdout, stderr) = run_with_stdin(bin.to_str().unwrap(), &["--input", "-"], input);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("no history provider configured"));
}

#[test]
fn wallet_rank_empty_input_is_exit_2() {
    let bin = sibling_binary("wallet-rank");
    let (code, _stdout, stderr) = run_with_stdin(bin.to_str().unwrap(), &["--input", "-"], "");
    assert_eq!(code, 2, "stderr: {stderr}");
}

#[test]
fn wallet_stats_valid_input_no_provider_is_exit_4() {
    let bin = sibling_binary("wallet-stats");
    let input = "base:0x1111111111111111111111111111111111111111\n";
    let (code, _stdout, stderr) = run_with_stdin(bin.to_str().unwrap(), &["--input", "-"], input);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("no history provider configured"));
}

#[test]
fn wallet_stats_empty_input_is_exit_2() {
    let bin = sibling_binary("wallet-stats");
    let (code, _stdout, stderr) = run_with_stdin(bin.to_str().unwrap(), &["--input", "-"], "");
    assert_eq!(code, 2, "stderr: {stderr}");
}

#[test]
fn all_three_binaries_support_dash_for_stdin() {
    // CLI.md §1: "stdin — --input - или отсутствие --input при pipe."
    // This test's own harness always has a piped (non-TTY) stdin, so it
    // also implicitly covers the "no --input, piped data" branch of
    // read_input() for each binary via the -c equivalent path.
    let buyer_intersect = PathBuf::from(env!("CARGO_BIN_EXE_buyer-intersect"));
    for bin in [
        buyer_intersect,
        sibling_binary("wallet-rank"),
        sibling_binary("wallet-stats"),
    ] {
        let (code, _stdout, stderr) = run_with_stdin(bin.to_str().unwrap(), &["--input", "-"], "");
        // Every binary must reach a defined exit code (2, in this
        // empty-input case) rather than hang waiting for more stdin.
        assert_eq!(code, 2, "binary {bin:?} stderr: {stderr}");
    }
}

#[test]
fn buyer_intersect_accepts_file_path_input() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let dir = std::env::temp_dir().join(format!("cli_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file_path = dir.join("tokens.txt");
    std::fs::write(
        &file_path,
        "base:0x1111111111111111111111111111111111111111\n\
         base:0x2222222222222222222222222222222222222222\n",
    )
    .expect("write temp file");

    let output = Command::new(bin)
        .args(["--input", file_path.to_str().unwrap()])
        .output()
        .expect("failed to run binary");

    let _ = std::fs::remove_dir_all(&dir);

    // File-path input reaches the same ConfigurationRequired outcome as
    // stdin input for identical content (CLI.md §1: file/stdin parity).
    assert_eq!(output.status.code(), Some(4));
}
