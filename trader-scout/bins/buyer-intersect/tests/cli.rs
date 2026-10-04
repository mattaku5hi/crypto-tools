//! CLI integration tests: exercise the actual compiled binary as a
//! subprocess, verifying exit codes per ADR-005/CLI.md §8. This is the
//! only test layer that can observe process::exit's real behavior —
//! unit tests inside library crates never see it.
//!
//! `env!("CARGO_BIN_EXE_...")` only resolves binaries of the package
//! under test, so wallet-rank and wallet-stats have their own
//! `tests/cli.rs`. (An earlier version reached them as dev-dependencies
//! via the target dir; Cargo ignores binary-only dev-dependencies, so
//! those binaries were never built on a clean checkout and CI failed.)
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

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
    // Base is enabled (ADR-020 amendment 4) but has no RPC configured here:
    // refused with exit 4 before any request.
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("SCOUT_BASE_RPC_URL"), "{stderr}");
    // BSC is enabled (ADR-020 amendment 5) but has no RPC configured here:
    // refused with exit 4 before any request, never a fabricated empty success.
    let input = "bsc:0x1111111111111111111111111111111111111111\n\
                 bsc:0x2222222222222222222222222222222222222222\n";
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], input);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("SCOUT_BSC_RPC_URL"), "{stderr}");
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
fn buyer_intersect_supports_dash_for_stdin() {
    // CLI.md §1: "stdin — --input - или отсутствие --input при pipe."
    // The binary must reach a defined exit code (2 for empty input)
    // rather than hang waiting for more stdin. wallet-rank/wallet-stats
    // carry the same test in their own packages' tests/cli.rs.
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let (code, _stdout, stderr) = run_with_stdin(bin, &["--input", "-"], "");
    assert_eq!(code, 2, "stderr: {stderr}");
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

    let file_path_str = file_path.to_string_lossy().to_string();
    let output = Command::new(bin)
        .args(["--input", &file_path_str])
        .output()
        .expect("failed to run binary");

    let _ = std::fs::remove_dir_all(&dir);

    // File-path input reaches the same ConfigurationRequired outcome as
    // stdin input for identical content (CLI.md §1: file/stdin parity).
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn buyer_intersect_solana_input_without_helius_key_is_exit_4() {
    // ADR-005/ADR-006: Solana input with no SCOUT_HELIUS_API_KEY is
    // CONFIGURATION_REQUIRED (exit 4), never a fabricated empty success.
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let input = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n\
                 solana:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump\n";
    let mut child = Command::new(bin)
        .args(["--input", "-"])
        .env_remove("SCOUT_HELIUS_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binary");
    // The binary may exit (usage error) before reading stdin; a broken
    // pipe here is expected, the exit code is what the test checks.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "stderr: {stderr}");
    assert!(stderr.contains("configuration required"));
    assert!(stderr.contains("SCOUT_HELIUS_API_KEY"));
    assert!(output.stdout.is_empty());
}

#[test]
fn buyer_intersect_max_pages_per_token_range_is_validated_exit_2() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let input = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump\n\
                 solana:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump\n";
    for bad in ["0", "201", "abc", "-1"] {
        let (code, _out, stderr) = run_with_stdin(
            bin,
            &["--input", "-", &format!("--max-pages-per-token={bad}")],
            input,
        );
        assert_eq!(code, 2, "value {bad}: {stderr}");
    }
    // Boundary values are accepted by the parser and reach the (EVM-style)
    // configuration outcome instead of exit 2.
    for ok in ["1", "200"] {
        let (code, _out, stderr) = run_with_stdin(
            bin,
            &["--input", "-", &format!("--max-pages-per-token={ok}")],
            "base:0x1111111111111111111111111111111111111111\n\
             base:0x2222222222222222222222222222222222222222\n",
        );
        assert_eq!(code, 4, "value {ok}: {stderr}");
    }
}

#[test]
fn buyer_intersect_unknown_format_is_exit_2() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let (code, _out, _err) = run_with_stdin(bin, &["--input", "-", "--format", "csv"], "");
    assert_eq!(code, 2);
}

#[test]
fn buyer_intersect_help_documents_the_page_budget_honestly() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let out = Command::new(bin).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--max-pages-per-token"));
    assert!(help.contains("max-requests"));
}

const SOL_A: &str = "solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
const SOL_B: &str = "solana:NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump";

fn sol_input() -> String {
    format!("{SOL_A}\n{SOL_B}\n")
}

#[test]
fn buyer_intersect_side_values_are_validated_exit_2() {
    // ADR-014: `--side buy|sell|any`; anything else is a usage error.
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    for bad in ["", "both", "BUY", "buys", "1"] {
        let (code, _o, e) = run_with_stdin(
            bin,
            &["--input", "-", &format!("--side={bad}")],
            &sol_input(),
        );
        assert_eq!(code, 2, "value {bad:?}: {e}");
    }
    // Every valid value reaches the key check (no key: exit 4).
    for ok in ["buy", "sell", "any"] {
        let mut child = Command::new(bin)
            .args(["--input", "-", "--side", ok])
            .env_remove("SCOUT_HELIUS_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(sol_input().as_bytes());
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(4), "{ok}");
    }
}

#[test]
fn buyer_intersect_help_documents_side_and_window() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    let out = Command::new(bin).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--side", "--since", "--until", "--period"] {
        assert!(help.contains(flag), "{flag}");
    }
    assert!(help.contains("[default: any]"));
}

#[test]
fn buyer_intersect_window_options_are_validated_with_exit_2() {
    // Same rules as wallet-stats (ADR-011).
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    for args in [
        vec!["--input", "-", "--period", "0d"],
        vec!["--input", "-", "--period", "30x"],
        vec!["--input", "-", "--period", "366d"],
        vec!["--input", "-", "--since", "2026-08-01T00:00:00+03:00"],
        vec!["--input", "-", "--since", "2026-08-01"],
        vec!["--input", "-", "--until", "2026-09-01T00:00:00Z"],
        vec![
            "--input",
            "-",
            "--period",
            "30d",
            "--since",
            "2026-08-01T00:00:00Z",
        ],
        vec![
            "--input",
            "-",
            "--since",
            "2026-09-01T00:00:00Z",
            "--until",
            "2026-08-01T00:00:00Z",
        ],
    ] {
        let (code, _o, e) = run_with_stdin(bin, &args, &sol_input());
        assert_eq!(code, 2, "{args:?}: {e}");
    }
}

#[test]
fn buyer_intersect_valid_window_options_reach_the_key_check() {
    let bin = env!("CARGO_BIN_EXE_buyer-intersect");
    for args in [
        vec!["--input", "-", "--period", "30d"],
        vec!["--input", "-", "--since", "2026-08-01T00:00:00Z"],
        vec![
            "--input",
            "-",
            "--since",
            "2026-08-01T00:00:00Z",
            "--until",
            "2026-09-01T00:00:00Z",
        ],
    ] {
        let mut child = Command::new(bin)
            .args(&args)
            .env_remove("SCOUT_HELIUS_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(sol_input().as_bytes());
        let o = child.wait_with_output().unwrap();
        let e = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(4), "{args:?}: {e}");
        assert!(e.contains("configuration required"), "{e}");
    }
}
