//! CLI integration tests for `wallet-rank`: run the compiled binary as a
//! subprocess and check exit codes per ADR-005/CLI.md §8.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_wallet-rank");

fn run_with_stdin(args: &[&str], stdin_data: &str) -> (i32, String, String) {
    run_env(args, stdin_data, None)
}

/// `key`: value of SCOUT_HELIUS_API_KEY (None = removed from the env).
fn run_env(args: &[&str], stdin_data: &str, key: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.env_remove("SCOUT_HELIUS_API_KEY");
    if let Some(key) = key {
        cmd.env("SCOUT_HELIUS_API_KEY", key);
    }
    let mut child = cmd
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
    assert!(stderr.contains("configuration required"), "{stderr}");
}

#[test]
fn empty_input_via_dash_stdin_is_exit_2() {
    // CLI.md §1: `--input -` reads stdin; empty input is a usage error
    // and the binary must exit rather than wait for more input.
    let (code, _stdout, stderr) = run_with_stdin(&["--input", "-"], "");
    assert_eq!(code, 2, "stderr: {stderr}");
}

const SOL_A: &str = "HgwBZM6kQE8qpYBdM2aXDxaEs5GTpDryNxuREeVP8f8B";
const SOL_B: &str = "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1";

fn buyer_intersect_jsonl(status: Option<&str>) -> String {
    // Shape per docs/CLI.md §3 and bins/buyer-intersect output.
    let mut out = String::from(
        "{\"schema_version\":1,\"kind\":\"run_meta\",\"run_id\":\"buyer-intersect-1\",\"captured_at\":\"2026-10-02T12:34:56Z\"}\n",
    );
    for a in [SOL_A, SOL_B, SOL_A] {
        out.push_str(&format!(
            "{{\"schema_version\":1,\"kind\":\"buyer_match\",\"wallet\":{{\"chain\":\"solana\",\"address\":\"{a}\"}},\"hit_count\":2,\"matched_assets\":[]}}\n"
        ));
    }
    if let Some(status) = status {
        out.push_str(&format!(
            "{{\"schema_version\":1,\"kind\":\"run_summary\",\"run_id\":\"buyer-intersect-1\",\"status\":\"{status}\",\"cancelled\":false,\"records\":3,\"incomplete_reasons\":[],\"tokens\":[]}}\n"
        ));
    }
    out
}

#[test]
fn solana_input_without_helius_key_is_exit_4_with_message() {
    let input = format!("solana:{SOL_A}\n");
    let (code, stdout, stderr) = run_env(&["--input", "-"], &input, None);
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("SCOUT_HELIUS_API_KEY"));
    assert!(stderr.contains("configuration required"), "{stderr}");
    assert!(stdout.is_empty());
}

#[test]
fn buyer_intersect_jsonl_output_is_read_as_input() {
    let input = buyer_intersect_jsonl(Some("complete"));
    let (code, stdout, stderr) =
        run_env(&["--input", "-", "--input-format", "jsonl"], &input, None);
    // Parsed (2 distinct wallets, 1 duplicate), then stops at the missing key.
    assert_eq!(code, 4, "stderr: {stderr}");
    assert!(stderr.contains("2 wallet(s) parsed"), "{stderr}");
    assert!(stderr.contains("1 duplicate"), "{stderr}");
    assert!(!stderr.contains("upstream JSONL run is not complete"));
    assert!(stdout.is_empty());
}

#[test]
fn wallet_rank_records_are_accepted_and_partial_upstream_is_flagged() {
    let rank = format!(
        "{{\"schema_version\":1,\"kind\":\"wallet_rank\",\"rank\":1,\"wallet\":{{\"chain\":\"solana\",\"address\":\"{SOL_A}\"}},\"metrics\":{{}}}}\n"
    );
    let (code, _o, stderr) = run_env(&["--input", "-", "--input-format", "jsonl"], &rank, None);
    assert_eq!(code, 4, "{stderr}");
    assert!(stderr.contains("1 wallet(s) parsed"), "{stderr}");
    for input in [
        buyer_intersect_jsonl(Some("partial")),
        buyer_intersect_jsonl(None),
    ] {
        let (code, _o, stderr) =
            run_env(&["--input", "-", "--input-format", "jsonl"], &input, None);
        assert_eq!(code, 4, "stderr: {stderr}");
        assert!(
            stderr.contains("upstream JSONL run is not complete"),
            "{stderr}"
        );
    }
}

#[test]
fn wallet_excluded_and_malformed_jsonl_are_exit_2() {
    let (code, _out, stderr) = run_with_stdin(
        &["--input", "-", "--input-format", "jsonl"],
        "{\"schema_version\":1,\"kind\":\"wallet_excluded\",\"wallet\":{}}\n",
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("line 1"), "{stderr}");
    let (code, _o, _e) = run_with_stdin(&["--input", "-", "--input-format", "jsonl"], "nope\n");
    assert_eq!(code, 2);
}

#[test]
fn usage_errors_are_exit_2() {
    let input = format!("solana:{SOL_A}\n");
    for args in [
        vec!["--input", "-", "--format", "csv"],
        vec!["--input", "-", "--profile", "yolo"],
        vec!["--input", "-", "--rank-by", "sharpe"],
        vec!["--input", "-", "--top", "0"],
        vec!["--input", "-", "--top", "-3"],
        vec!["--input", "-", "--min-closed-episodes", "abc"],
        vec!["--input", "-", "--max-requests", "0"],
        vec!["--input", "-", "--input-format", "csv"],
        vec!["--input", "-", "--max-pages-per-wallet=0"],
        vec!["--input", "-", "--max-pages-per-wallet=201"],
    ] {
        let (code, _o, e) = run_with_stdin(&args, &input);
        assert_eq!(code, 2, "{args:?}: {e}");
    }
    let (code, _o, e) = run_with_stdin(&["--input", "-"], "solana:not-an-address\n");
    assert_eq!(code, 2, "{e}");
    let (code, _o, e) = run_with_stdin(&["--input", "-"], &format!("{SOL_A}\n"));
    assert_eq!(code, 2, "bare address needs a chain prefix: {e}");
}

#[test]
fn period_equity_pnl_is_rejected_with_a_clear_message() {
    let input = format!("solana:{SOL_A}\n");
    let (code, _o, e) = run_with_stdin(&["--input", "-", "--rank-by", "period-equity-pnl"], &input);
    assert_eq!(code, 2, "{e}");
    assert!(e.contains("not supported yet") && e.contains("P5.2"), "{e}");
}

#[test]
fn window_options_are_validated_with_exit_2() {
    let input = format!("solana:{SOL_A}\n");
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
        let (code, _o, e) = run_with_stdin(&args, &input);
        assert_eq!(code, 2, "{args:?}: {e}");
    }
}

#[test]
fn valid_window_options_are_accepted_and_reach_the_key_check() {
    let input = format!("solana:{SOL_A}\n");
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
        let (code, _o, e) = run_with_stdin(&args, &input);
        assert_eq!(code, 4, "{args:?}: {e}");
        assert!(e.contains("configuration required"), "{e}");
    }
}

#[test]
fn valid_flags_parse_and_reach_the_key_check() {
    let input = format!("solana:{SOL_A}\n");
    let (code, _o, e) = run_env(
        &[
            "--input",
            "-",
            "--top",
            "5",
            "--rank-by",
            "profit-factor",
            "--profile",
            "insider",
            "--min-closed-episodes",
            "4",
            "--min-active-days",
            "2",
            "--max-trades-per-day",
            "50",
            "--max-mints-per-day",
            "12",
            "--require-no-open",
            "--max-requests",
            "40",
            "--format",
            "jsonl",
        ],
        &input,
        None,
    );
    assert_eq!(code, 4, "{e}");
    assert!(e.contains("SCOUT_HELIUS_API_KEY"), "{e}");
}

#[test]
fn mixed_solana_and_evm_input_is_exit_2_and_never_silently_dropped() {
    let input = format!("solana:{SOL_A}\nbase:0x1111111111111111111111111111111111111111\n");
    let (code, stdout, stderr) = run_env(&["--input", "-"], &input, Some("KEYSECRET777"));
    // ADR-020 step 2: one run = one chain family; mixed input is refused
    // (usage error) before anything is scanned or printed.
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("mixed Solana and EVM"), "{stderr}");
    assert!(stdout.is_empty());
}

#[test]
fn api_key_is_never_printed() {
    let input = "base:0x1111111111111111111111111111111111111111\n";
    let (code, stdout, stderr) = run_env(&["--input", "-"], input, Some("KEYSECRET777"));
    assert_eq!(code, 4);
    assert!(!stdout.contains("KEYSECRET777") && !stderr.contains("KEYSECRET777"));
}

#[test]
fn help_lists_the_documented_surface() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--input-format",
        "--format",
        "--top",
        "--rank-by",
        "--profile",
        "--min-closed-episodes",
        "--min-active-days",
        "--max-trades-per-day",
        "--max-mints-per-day",
        "--require-no-open",
        "--max-pages-per-wallet",
        "--max-requests",
    ] {
        assert!(help.contains(flag), "{flag}");
    }
    for value in ["insider", "realized-cost-roi", "profit-factor"] {
        assert!(help.contains(value), "{value}");
    }
}
