//! `wallet-rank`: see docs/CLI.md §4 for the full contract.
//!
//! Solana: when EVERY input wallet is Solana and `SCOUT_HELIUS_API_KEY`
//! is set, a `HeliusProvider` (newest-first scan, bounded page and request
//! budgets) drives `run_solana_wallet_stats` (the SAME analysis
//! `wallet-stats` prints), then `rank_solana_wallets` applies the profile
//! gates and the documented sort. Every input wallet ends up in the ranked
//! list or in the exclusions (with all reasons); nothing is dropped.
//! Without a key the Solana chain fails (exit 4 if it is the only chain). Mixed/multi-chain input is partitioned per chain (see multi.rs).
//!
//! Exit codes (CLI.md §8): 0 the whole wallet universe was scanned within
//! the declared scope (an empty or short shortlist is a normal outcome);
//! 2 usage (incl. `--rank-by period-equity-pnl`, an invalid window:
//! `--period` outside 1d..=365d, non-UTC/non-RFC3339 `--since/--until`, `--period` with
//! `--since`, `--until` without a start, empty window; ADR-011);
//! 3 ranking over a PARTIAL universe (a wallet is incomplete/errored, the
//! request budget ran out, rate-limit stop after at least one wallet
//! produced data, upstream JSONL partial or footerless);
//! 4 infrastructure/credentials (also: every wallet failed, rate-limit
//! stop with no data);
//! 130 cancelled; 141 stdout closed.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::io::{self, IsTerminal, Read};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use scout_api::ProviderError;
use scout_app::{ChainRun, InputFormat, WriteOutcome, write_lines_to_stdout};
use scout_core::{AddressBytes, ChainFamily, ChainKey, SolanaPubkey, WalletKey};
use scout_engine::{
    AnalysisWindow, DEFAULT_TOP, LedgerDecoders, QuoteUnit, RankBy, RankPolicy, RankProfile,
    ScanStop, SolanaWalletStatsReport, WalletRankReport, WindowSource, out_of_sample,
    parse_period_days, pump_amm_decoder, pump_bonding_curve_decoder, rank_solana_wallets,
    run_solana_wallet_stats_concurrent, sanitize_provider_text,
};
use scout_pricing::PriceSource as _;
use scout_providers::{
    HeliusProvider, HeliusRequestOptions, MAX_PAGE_LIMIT, ScanOrder, TokenAccountsFilter,
};
use scout_rpc::DEFAULT_MAX_RETRY_AFTER;
use tokio_util::sync::CancellationToken;

mod evm;
mod multi;
mod output;

use output::{RunMetaInput, SummaryInput};

const HELIUS_KEY_ENV: &str = "SCOUT_HELIUS_API_KEY";
/// Test-only: send requests to this URL instead of Helius (the API key is
/// never sent to the override).
const ENDPOINT_OVERRIDE_ENV: &str = "SCOUT_WALLET_RANK_ENDPOINT";
const HELIUS_TIMEOUT_MS: u64 = 30_000;
const HELIUS_MAX_ATTEMPTS: u32 = 3;

// Defaults of the provider request options: live-verified 2026-10-03, see
// docs/p0/measurements/2026-10-03-helius-filters-live.md. The opt-out flags
// reproduce the legacy request. No status filter is ever applied to wallet
// scans: failed transactions still cost fees (ADR-004).
/// `--page-limit` default (`limit` per `getTransactionsForAddress` page).
const DEFAULT_PAGE_LIMIT_ARG: u32 = 500;
/// `--server-window` default: send the window as `filters.blockTime`.
const DEFAULT_SERVER_WINDOW: bool = true;
/// `--token-accounts` default: `none` or `balance-changed`.
const DEFAULT_TOKEN_ACCOUNTS: &str = "balance-changed";

/// Rank input wallets by realized trading performance (Solana pump.fun
/// bonding-curve + PumpSwap AMM slice, SOL-quoted ledger).
#[derive(Debug, Clone, Parser)]
#[command(name = "wallet-rank", version)]
struct Args {
    /// Input file path, or `-` for stdin.
    #[arg(long)]
    input: Option<String>,

    /// Input format: `lines` (default) or `jsonl` (records of a previous
    /// trader-scout CLI: buyer_match, wallet_ref, wallet_stats, wallet_rank).
    #[arg(long, default_value = "lines", value_parser = ["lines", "jsonl"])]
    input_format: String,

    /// Output format: table or jsonl.
    #[arg(long, default_value = "table", value_parser = ["table", "jsonl"])]
    format: String,

    /// Maximum number of ranked wallets (>= 1); never padded.
    #[arg(
        long,
        default_value_t = u32::try_from(DEFAULT_TOP).unwrap_or(20),
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    top: u32,

    /// Ranking metric. `period-equity-pnl` needs a price source (P5.2) and
    /// is rejected with exit 2.
    #[arg(
        long,
        default_value = "realized-net-pnl",
        value_parser = ["realized-net-pnl", "realized-cost-roi", "profit-factor", "period-equity-pnl"]
    )]
    rank_by: String,

    /// Quote unit of the ranking metrics and the closed-episode gate
    /// (ADR-013): the chain's native unit (`sol` = lamports, default; `eth` = wei on
    /// EVM chains, where the default `sol` also means native), EVM `usdg`, `usdc` or `usdt` (6-dp raw; on BSC `usdt`/`usdc` are the 18-dp Binance-Peg units
    /// units) -- never mixed or converted -- or `usd` (ADR-018): every
    /// leg valued in USD with Coinbase Exchange 1m candles (USDC at par,
    /// labelled), so SOL- and USDC-quoted wallets compare in one column;
    /// the run then also fetches prices (see --max-price-requests).
    ///
    /// Multi-chain input (several chains in one run): `usd` is the default
    /// and the only mode that ranks across chains (USD lower bounds,
    /// ADR-016/018, each row shows its chain). A native/stable unit
    /// (`sol`, `eth`, `bnb`, `usdc`, `usdt`, `usdg`) ranks each chain where
    /// it exists separately (`--top` per chain) and excludes the wallets of
    /// the other chains with `quote_unit_not_on_chain` (not scanned).
    #[arg(long, value_parser = ["sol", "eth", "bnb", "usdc", "usdt", "usdg", "usd"])]
    quote: Option<String>,

    /// Multi-chain input: chains run at once (1..=4, default 2), each with
    /// its own sources and its own `--max-requests` budget. Single-chain
    /// input ignores it. Stderr diagnostics of concurrent chains may
    /// interleave (use 1 for an ordered log).
    #[arg(
        long,
        default_value_t = scout_app::DEFAULT_CHAIN_CONCURRENCY,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(scout_app::MAX_CHAIN_CONCURRENCY))
    )]
    chain_concurrency: u32,

    /// Total HTTP request budget for PRICE candles under `--quote usd`
    /// (retries included), N >= 1. Counted apart from --max-requests as
    /// `requests_made_prices`. When exhausted the remaining legs are
    /// `price_unknown` and the run is incomplete (exit 3).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_price_requests: Option<u64>,

    /// Gate profile: `quality` (20 closed episodes, 7 active days),
    /// `insider` (5 episodes, 3 days, <= 10 mints and <= 30 trades per
    /// active day) or `none` (no sample/activity gates). Research starting
    /// policy, not statistical guarantees.
    #[arg(long, default_value = "quality", value_parser = ["quality", "insider", "none"])]
    profile: String,

    /// Override: minimum known closed episodes.
    #[arg(long)]
    min_closed_episodes: Option<u64>,

    /// Override: minimum distinct active UTC days.
    #[arg(long)]
    min_active_days: Option<u64>,

    /// Override: ceiling on timestamped trades per active day.
    #[arg(long)]
    max_trades_per_day: Option<u64>,

    /// Override: ceiling on distinct mints per active day.
    #[arg(long)]
    max_mints_per_day: Option<u64>,

    /// ADR-016: maximum share of `closed_unknown` among all closed
    /// known+unknown episodes, integer percent 0..=100 (exact integer
    /// comparison; `quality`/`insider` only). 0 = any unknown episode
    /// excludes (the old strict rule). Default 10.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    max_unknown_episode_share: u8,

    /// ADR-016: drop wallets whose worst-case PnL is unbounded (tier 2:
    /// an unknown episode consumed an unknown-basis lot or lots of several
    /// quote units) with exclusion `pnl_unbounded`.
    #[arg(long)]
    exclude_unbounded: bool,

    /// Strict variant: exclude wallets with ANY open position (default:
    /// include and flag `open_exposure`, valued when valuation ran).
    #[arg(long)]
    require_no_open: bool,

    /// ADR-019: exclude wallets with ANY open position that has no
    /// realizable value (`open_exposure_unvalued`: historical window, no
    /// fee observation, migrated pool unknown, venue not supported, state
    /// unavailable). Conflicts with --no-valuation.
    #[arg(long, conflicts_with = "no_valuation")]
    require_valued_open: bool,

    /// Skip the ADR-019 realizable valuation of open positions (no
    /// `getMultipleAccounts` requests; open positions stay unvalued).
    #[arg(long)]
    no_valuation: bool,

    /// EVM only: run a chain whose venue/quote set is not verified yet
    /// (BSC); without it such a run exits 4.
    #[arg(long)]
    allow_unverified_chain: bool,

    /// Provider page budget PER WALLET (Helius full mode: 100 transactions
    /// per page). Newest-first: a wallet needing more pages is `incomplete`
    /// (excluded, exit 3). Retries are not counted here.
    #[arg(
        long,
        default_value_t = 10,
        value_parser = clap::value_parser!(u32).range(1..=200)
    )]
    max_pages_per_wallet: u32,

    /// Transactions per provider page (`limit`, 1..=1000). The page budget
    /// stays in PAGES: per-wallet transaction budget is
    /// `max-pages-per-wallet * page-limit`. Pages above 500 txs raise the
    /// response-size cap (~20 KB/tx, max 64 MiB). Live-verified 2026-10-03.
    #[arg(
        long,
        default_value_t = DEFAULT_PAGE_LIMIT_ARG,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_PAGE_LIMIT))
    )]
    page_limit: u32,

    /// With a window, also send it server-side as `filters.blockTime`
    /// (gte since, lt until) in addition to the newest-first boundary
    /// walk. `--server-window=false` disables. Live-verified 2026-10-03.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        default_value_t = DEFAULT_SERVER_WINDOW,
        action = clap::ArgAction::Set
    )]
    server_window: bool,

    /// `filters.tokenAccounts`: `none` (only txs referencing the wallet) or
    /// `balance-changed` (also txs changing the balance of a token account
    /// the wallet owns; Helius-recommended). Live-verified 2026-10-03.
    #[arg(
        long,
        default_value = DEFAULT_TOKEN_ACCOUNTS,
        value_parser = ["none", "balance-changed"]
    )]
    token_accounts: String,

    /// Max wallets scanned at once (1..=16, default 4). Each wallet scan
    /// keeps one provider request in flight, so this also caps concurrent
    /// requests. The shared `--max-requests` budget stays exact; results do
    /// not depend on the value (cards merged in input order). `1` =
    /// sequential. Time slicing is not available for wallets.
    #[arg(
        long,
        default_value_t = 4,
        value_parser = clap::value_parser!(u32).range(1..=16)
    )]
    concurrency: u32,

    /// Total HTTP attempt budget (retries included) for the whole run.
    /// When spent, the interrupted wallet is `provider_error`, remaining
    /// wallets are `not_scanned` (both excluded); exit 3.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests: Option<u64>,

    #[command(flatten)]
    net: scout_app::EvmNetOptions,

    /// Analysis window start, UTC RFC 3339 `2026-08-01T00:00:00Z` (inclusive;
    /// no offsets). Window `[since, until)`; see ADR-011. Conflicts with --period.
    #[arg(long, conflicts_with = "period")]
    since: Option<String>,
    /// Analysis window end (exclusive), same format; default: run start.
    /// Requires --since or --period.
    #[arg(long)]
    until: Option<String>,
    /// Window of the last N days (`30d`, 1..=365) ending at --until (default
    /// run start). Conflicts with --since.
    #[arg(long, conflicts_with = "since")]
    period: Option<String>,

    /// A3 out-of-sample check (docs/p0/cohort-definitions.md criterion 2):
    /// rank on the --period window ending N days before --until (window A),
    /// then re-check every ranked wallet on the last N days (window B,
    /// disjoint) and report whether its worst-case realized net PnL stayed
    /// positive there. Single-chain input; doubles the scans of the ranked
    /// wallets.
    #[arg(long, requires = "period")]
    validation_period: Option<String>,
}

fn policy_from(args: &Args, quote: &str) -> Result<RankPolicy, String> {
    let rank_by = match args.rank_by.as_str() {
        "realized-net-pnl" => RankBy::RealizedNetPnl,
        "realized-cost-roi" => RankBy::RealizedCostRoi,
        "profit-factor" => RankBy::ProfitFactor,
        "period-equity-pnl" => {
            return Err(
                "--rank-by period-equity-pnl is not supported yet: it needs historical \
                 prices and open-position valuation (P5.2)"
                    .to_string(),
            );
        }
        other => return Err(format!("unknown --rank-by {other}")),
    };
    let profile = match args.profile.as_str() {
        "insider" => RankProfile::Insider,
        "none" => RankProfile::None,
        _ => RankProfile::Quality,
    };
    let mut p = RankPolicy::for_profile(
        profile,
        rank_by,
        usize::try_from(args.top).map_err(|_| "--top out of range".to_string())?,
    );
    if let Some(v) = args.min_closed_episodes {
        p.min_closed_episodes = v;
    }
    if let Some(v) = args.min_active_days {
        p.min_active_days = v;
    }
    if let Some(v) = args.max_trades_per_day {
        p.max_trades_per_day = Some(v);
    }
    if let Some(v) = args.max_mints_per_day {
        p.max_mints_per_day = Some(v);
    }
    p.require_no_open = args.require_no_open;
    p.require_valued_open = args.require_valued_open;
    p.max_unknown_episode_share_percent = args.max_unknown_episode_share;
    p.exclude_unbounded = args.exclude_unbounded;
    p.quote = match quote {
        "usdc" => QuoteUnit::UsdcUnits,
        "usdt" => QuoteUnit::UsdtUnits,
        "usdg" => QuoteUnit::UsdgUnits,
        "usd" => QuoteUnit::ReportCurrency,
        "eth" | "bnb" => QuoteUnit::Wei,
        _ => QuoteUnit::Lamports,
    };
    Ok(p)
}

/// The run's ranking unit on an EVM chain: native (`sol` default or `eth`)
/// -> wei, the chain's pinned stable (`usdg` on Robinhood, `usdc` on Base),
/// `usd`; Solana-only units and the other chain's stable are a usage error.
fn evm_policy(mut p: RankPolicy, chain: &scout_core::ChainKey) -> Result<RankPolicy, String> {
    let units = match chain.network_id {
        scout_core::NetworkId::EvmChainId(id) => {
            scout_engine::ChainDisplay::evm_by_chain_id(id).map(|d| d.quote_units())
        }
        _ => None,
    }
    .unwrap_or(&[QuoteUnit::Wei]);
    p.quote = match p.quote {
        QuoteUnit::Lamports | QuoteUnit::Wei => QuoteUnit::Wei,
        QuoteUnit::ReportCurrency => QuoteUnit::ReportCurrency,
        stable @ (QuoteUnit::UsdgUnits | QuoteUnit::UsdcUnits) if units.contains(&stable) => stable,
        // BSC: `--quote usdt|usdc` means the 18-dp Binance-Peg unit of the chain.
        QuoteUnit::UsdtUnits if units.contains(&QuoteUnit::BinancePegUsdtUnits) => {
            QuoteUnit::BinancePegUsdtUnits
        }
        QuoteUnit::UsdcUnits if units.contains(&QuoteUnit::BinancePegUsdcUnits) => {
            QuoteUnit::BinancePegUsdcUnits
        }
        other => {
            return Err(format!(
                "--quote {} is not a quote unit of an EVM chain (on this chain use eth, {} or usd)",
                scout_engine::quote_unit_label(other),
                units
                    .get(1)
                    .map_or("usd", |u| scout_engine::quote_unit_label(*u))
            ));
        }
    };
    Ok(p)
}

fn main() -> ExitCode {
    let args = Args::parse();

    // ADR-011: the window is resolved (and `as_of` pinned) exactly once.
    let as_of = unix_now();
    let window = match AnalysisWindow::resolve(
        args.period.as_deref(),
        args.since.as_deref(),
        args.until.as_deref(),
        as_of,
    ) {
        Ok(w) => w,
        Err(err) => {
            eprintln!("wallet-rank: {err}");
            return ExitCode::from(2);
        }
    };
    // A3: window A (selection) ends where window B (validation) starts.
    let (window, window_b) = match args.validation_period.as_deref() {
        None => (window, None),
        Some(v) => match parse_period_days(v) {
            Ok(days) => {
                let span = i64::from(days).saturating_mul(86_400);
                let b = AnalysisWindow {
                    since: window.until.saturating_sub(span),
                    until: window.until,
                    as_of,
                    source: WindowSource::Period,
                };
                let a = AnalysisWindow {
                    since: window.since.saturating_sub(span),
                    until: window.until.saturating_sub(span),
                    ..window
                };
                (a, Some(b))
            }
            Err(err) => {
                eprintln!("wallet-rank: --validation-period: {err}");
                return ExitCode::from(2);
            }
        },
    };
    // Validates the flags (exit 2) before any input is read; the unit is
    // chosen again below once the number of chains is known.
    let policy = match policy_from(&args, args.quote.as_deref().unwrap_or("sol")) {
        Ok(p) => p,
        Err(message) => {
            eprintln!("wallet-rank: {message}");
            return ExitCode::from(2);
        }
    };

    let input_text = match read_input(args.input.as_deref()) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("wallet-rank: {message}");
            return ExitCode::from(2);
        }
    };

    let (parsed, upstream) = if args.input_format == "jsonl" {
        match scout_app::parse_jsonl_with_upstream(input_text.as_bytes(), None) {
            Ok(r) => r,
            Err(err) => {
                eprintln!("wallet-rank: {err}");
                return ExitCode::from(2);
            }
        }
    } else {
        match scout_app::parse_input(input_text.as_bytes(), InputFormat::Lines, None) {
            Ok(p) => (p, scout_app::UpstreamInfo::default()),
            Err(err) => {
                eprintln!("wallet-rank: {err}");
                return ExitCode::from(2);
            }
        }
    };

    let wallets = match scout_app::resolve_wallet_keys(&parsed) {
        Ok(wallets) => wallets,
        Err(err) => {
            eprintln!("wallet-rank: {err}");
            return ExitCode::from(2);
        }
    };
    if parsed.duplicate_count > 0 {
        eprintln!(
            "wallet-rank: {} duplicate input identit(ies) collapsed",
            parsed.duplicate_count
        );
    }
    let upstream_complete = upstream.is_complete();
    if !upstream_complete {
        eprintln!(
            "wallet-rank: upstream JSONL run is not complete (run_summary status={}); \
             the wallet universe may be partial, the run will exit 3",
            upstream.summary_status.as_deref().unwrap_or("missing")
        );
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("wallet-rank: could not start async runtime: {err}");
            return ExitCode::from(4);
        }
    };
    let groups = scout_app::partition_by_chain(&wallets, |w| &w.chain);
    if groups.len() > 1 && window_b.is_some() {
        eprintln!("wallet-rank: --validation-period supports single-chain input only");
        return ExitCode::from(2);
    }
    if groups.len() > 1 {
        return multi::run_multi(
            &rt,
            &wallets,
            &groups,
            &args,
            &window,
            parsed.duplicate_count,
            upstream_complete,
        );
    }
    // One chain: the classic run, quote default `sol` (native on any chain).
    let chain = groups
        .first()
        .map_or_else(scout_engine::solana_mainnet_chain, |g| g.chain.clone());
    let policy = if chain.family == ChainFamily::Evm {
        match evm_policy(policy, &chain) {
            Ok(p) => p,
            Err(message) => {
                eprintln!("wallet-rank: {message}");
                return ExitCode::from(2);
            }
        }
    } else if matches!(policy.quote, QuoteUnit::Wei | QuoteUnit::UsdgUnits) {
        eprintln!("wallet-rank: --quote eth/usdg is only valid for EVM input");
        return ExitCode::from(2);
    } else {
        policy
    };
    let r = run_chain(
        &rt,
        &wallets,
        &chain,
        &args,
        &policy,
        &window,
        parsed.duplicate_count,
        upstream_complete,
    );
    if r.run.failure.is_some() {
        return ExitCode::from(r.run.status);
    }
    let mut r = r;
    if let (Some(b), Some(stats_a)) = (window_b.as_ref(), r.stats.as_ref()) {
        let report_a = rank_solana_wallets(&stats_a.wallets, &policy);
        let keys: Vec<WalletKey> = report_a
            .ranked
            .iter()
            .filter_map(|rw| {
                stats_a
                    .wallets
                    .iter()
                    .find(|w| w.wallet == rw.observation.wallet && w.chain == rw.observation.chain)
                    .map(scout_engine::SolanaWalletStats::wallet_key)
            })
            .collect();
        eprintln!(
            "wallet-rank: out-of-sample: re-checking {} ranked wallet(s) on window B",
            keys.len()
        );
        let rb = run_chain(&rt, &keys, &chain, &args, &policy, b, 0, true);
        match rb.stats.as_ref() {
            Some(stats_b) => {
                let rows = out_of_sample(&report_a, &stats_b.wallets);
                let extra = if args.format == "jsonl" {
                    output::validation_jsonl_lines(&rows, b)
                } else {
                    output::validation_table_lines(&rows, b)
                };
                // JSONL keeps run_summary as the last record
                let at = if args.format == "jsonl" {
                    r.run.lines.len().saturating_sub(1)
                } else {
                    r.run.lines.len()
                };
                r.run.lines.splice(at..at, extra);
            }
            None => eprintln!("wallet-rank: out-of-sample: window B produced no cards"),
        }
        r.run.status = r.run.status.max(rb.run.status);
    }
    let outcome = write_lines_to_stdout(r.run.lines);
    if r.run.cancelled {
        return ExitCode::from(130);
    }
    if matches!(outcome, WriteOutcome::PipeClosed) {
        return ExitCode::from(141);
    }
    ExitCode::from(r.run.status)
}

/// One chain's run (its own sources, settings and budget) under `policy`
/// (already translated to this chain's units).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_chain(
    rt: &tokio::runtime::Runtime,
    wallets: &[WalletKey],
    chain: &ChainKey,
    args: &Args,
    policy: &RankPolicy,
    window: &AnalysisWindow,
    duplicates: usize,
    upstream_complete: bool,
) -> RankRun {
    if chain.family == ChainFamily::Evm {
        return evm::run_evm(
            rt,
            wallets,
            chain,
            args,
            policy,
            window,
            duplicates,
            upstream_complete,
        );
    }
    let solana: Vec<SolanaPubkey> = wallets
        .iter()
        .filter_map(|w| match (&w.chain.family, &w.address) {
            (ChainFamily::Solana, AddressBytes::Solana(a)) => Some(*a),
            _ => None,
        })
        .collect();
    let Some(api_key) = std::env::var(HELIUS_KEY_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!(
            "wallet-rank: {} wallet(s) parsed, top={}; no history provider configured: \
             configuration required, set {HELIUS_KEY_ENV}",
            wallets.len(),
            args.top
        );
        return ChainRun::failed(
            4,
            format!("no history provider configured: configuration required, set {HELIUS_KEY_ENV}"),
        )
        .into();
    };
    run_solana(
        rt,
        wallets,
        &solana,
        args,
        policy,
        window,
        &api_key,
        duplicates,
        upstream_complete,
    )
}

/// Outcome of the ADR-018 pricing step (all `None` unless `--quote usd`).
pub(crate) struct PricingOutcome {
    policy: Option<scout_pricing::PricePolicy>,
    endpoint_overridden: bool,
    requests_made: u64,
    max_requests: Option<u64>,
    run: Option<scout_engine::UsdPricingRun>,
}

impl PricingOutcome {
    fn input(&self) -> scout_app::PricingMetaInput<'_> {
        scout_app::PricingMetaInput {
            policy: self.policy.as_ref(),
            endpoint_overridden: self.endpoint_overridden,
            requests_made: self.requests_made,
            max_requests: self.max_requests,
            run: self.run.as_ref(),
        }
    }
}

/// ADR-018: when `enabled`, collect the needed minutes of all wallets,
/// prefetch once and apply the USD views.
pub(crate) fn price_wallets(
    rt: &tokio::runtime::Runtime,
    stats: &mut SolanaWalletStatsReport,
    args: &Args,
    enabled: bool,
) -> PricingOutcome {
    let mut out = PricingOutcome {
        policy: None,
        endpoint_overridden: false,
        requests_made: 0,
        max_requests: args.max_price_requests,
        run: None,
    };
    if !enabled {
        return out;
    }
    let built = if let Some(evm) = &stats.evm {
        if evm.chain.native_label == "bnb" {
            scout_app::build_coinbase_source_evm_bsc(args.max_price_requests)
        } else {
            scout_app::build_coinbase_source_evm(args.max_price_requests)
        }
    } else {
        scout_app::build_coinbase_source(args.max_price_requests)
    };
    match built {
        Ok((source, overridden)) => {
            out.endpoint_overridden = overridden;
            out.policy = Some(source.policy());
            out.run =
                Some(rt.block_on(scout_engine::apply_usd_pricing(&mut stats.wallets, &source)));
            out.requests_made = source.requests_made();
        }
        Err(message) => eprintln!("wallet-rank: {message}; USD figures unavailable"),
    }
    out
}

/// Table lines are built from our own typed values: strip the secret only.
/// (`redact` also caps length at 300 chars, which would cut the wide table.)
fn redact_table_line(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    }
}

pub(crate) fn limit_text(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unlimited".to_string(), |n| n.to_string())
}

pub(crate) fn redact(text: &str, secret: &str) -> String {
    let replaced = if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "<redacted>")
    };
    sanitize_provider_text(&replaced)
}

/// Terminal `RateLimited`: the transport refused to wait out a
/// `Retry-After` above its cap.
pub(crate) fn rate_limited_text(retry_after_secs: Option<u64>) -> String {
    let cap = DEFAULT_MAX_RETRY_AFTER.as_secs();
    match retry_after_secs {
        Some(s) => format!("rate limited; server asked to retry after {s}s (cap {cap}s)"),
        None => "rate limited; server gave no Retry-After".to_string(),
    }
}

/// Run start in unix seconds (pinned once per run as `as_of`).
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Effective provider request options for this run (also echoed in
/// `run_meta` and the stderr scope line). Never sets a status filter.
fn provider_options(args: &Args, window: &AnalysisWindow) -> HeliusRequestOptions {
    let (gte, lt) = match (args.server_window, window.bounds()) {
        (true, Some((since, until))) => (Some(since), Some(until)),
        _ => (None, None),
    };
    HeliusRequestOptions {
        page_limit: args.page_limit,
        block_time_gte: gte,
        block_time_lt: lt,
        token_accounts: if args.token_accounts == "balance-changed" {
            TokenAccountsFilter::BalanceChanged
        } else {
            TokenAccountsFilter::None
        },
        ..HeliusRequestOptions::default()
    }
}

fn build_provider(
    api_key: &str,
    max_pages: NonZeroU32,
    max_requests: Option<u64>,
    window: &AnalysisWindow,
    options: &HeliusRequestOptions,
) -> Result<HeliusProvider, ProviderError> {
    let provider = match std::env::var(ENDPOINT_OVERRIDE_ENV) {
        Ok(url) if !url.is_empty() => HeliusProvider::new_with_endpoint(
            scout_rpc::RpcEndpoint::new(url),
            HELIUS_TIMEOUT_MS,
            HELIUS_MAX_ATTEMPTS,
        )?,
        _ => HeliusProvider::new(api_key, HELIUS_TIMEOUT_MS, HELIUS_MAX_ATTEMPTS)?,
    };
    Ok(provider
        .with_page_limit(options.page_limit)
        .with_block_time_range(options.block_time_gte, options.block_time_lt)
        .with_token_accounts(options.token_accounts)
        .with_max_pages(max_pages)
        .with_scan_order(ScanOrder::NewestFirst)
        .with_stop_before_block_time(window.bounds().map(|(since, _)| since))
        .with_max_total_requests(max_requests))
}

fn provider_error_exit(err: &ProviderError, secret: &str) -> RankRun {
    let text = redact(&err.to_string(), secret);
    match err {
        ProviderError::ConfigurationRequired { .. } => eprintln!("wallet-rank: {text}"),
        _ => eprintln!("wallet-rank: provider error: {text}"),
    }
    ChainRun::failed(4, text).into()
}

/// One chain's run: the rendered output plus what a cross-chain (USD)
/// ranking needs to re-rank the scanned cards.
#[derive(Debug, Default)]
pub(crate) struct RankRun {
    pub run: ChainRun,
    /// The scanned cards (`None` when the run produced nothing).
    pub stats: Option<SolanaWalletStatsReport>,
    pub prices_made: u64,
    pub pricing_line: Option<String>,
    /// Secret substrings of this chain's endpoints (redaction of re-rendered text).
    pub secrets: Vec<String>,
}

impl From<ChainRun> for RankRun {
    fn from(run: ChainRun) -> Self {
        Self {
            run,
            ..Self::default()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_solana(
    rt: &tokio::runtime::Runtime,
    all: &[WalletKey],
    solana: &[SolanaPubkey],
    args: &Args,
    policy: &RankPolicy,
    window: &AnalysisWindow,
    api_key: &str,
    duplicates: usize,
    upstream_complete: bool,
) -> RankRun {
    let Some(max_pages) = NonZeroU32::new(args.max_pages_per_wallet) else {
        eprintln!("wallet-rank: --max-pages-per-wallet must be at least 1");
        return ChainRun::failed(2, "--max-pages-per-wallet must be at least 1".to_string()).into();
    };
    let options = provider_options(args, window);
    let started = std::time::Instant::now();
    let provider = match build_provider(api_key, max_pages, args.max_requests, window, &options) {
        Ok(p) => p,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    let decoder = match pump_bonding_curve_decoder() {
        Ok(d) => d,
        Err(err) => {
            let text = format!("decoder unavailable: {}", redact(&err.to_string(), api_key));
            eprintln!("wallet-rank: {text}");
            return ChainRun::failed(4, text).into();
        }
    };
    let amm = pump_amm_decoder();
    let mut stats = match rt.block_on(run_solana_wallet_stats_concurrent(
        &provider,
        solana,
        &LedgerDecoders {
            curve: &decoder,
            amm: Some(&amm),
            okx_order_policy: scout_engine::default_okx_order_policy,
        },
        window,
        usize::try_from(args.concurrency).unwrap_or(1),
        CancellationToken::new(),
    )) {
        Ok(r) => r,
        Err(err) => return provider_error_exit(&err, api_key),
    };
    // ADR-019: realizable valuation of open positions (live runs only);
    // its account reads share the run's request budget.
    let valuation = if args.no_valuation {
        None
    } else {
        Some(rt.block_on(scout_engine::apply_open_valuation(
            &mut stats.wallets,
            &provider,
            window,
        )))
    };
    let valuation_line = scout_app::open_valuation_line("wallet-rank", valuation.as_ref());
    let requests_made = provider.total_requests_made();
    // ADR-018: prices only for `--quote usd`; other quotes never fetch.
    let pricing = price_wallets(
        rt,
        &mut stats,
        args,
        policy.quote == QuoteUnit::ReportCurrency,
    );
    let pricing_input = pricing.input();
    let pricing_line = scout_app::pricing_line("wallet-rank", &pricing_input);
    let mut price_reasons: Vec<String> = Vec::new();
    if let Some(run) = &pricing.run
        && run.prefetch.pages_skipped_budget > 0
    {
        price_reasons.push(format!(
            "price request budget exhausted ({} page(s) not fetched, max_price_requests={}): \
             affected legs are price_unknown",
            run.prefetch.pages_skipped_budget,
            limit_text(args.max_price_requests)
        ));
    }
    if valuation.as_ref().is_some_and(|v| v.budget_exhausted) {
        price_reasons.push(format!(
            "request budget exhausted during open-position valuation (max_requests={}): \
             affected open positions are unvalued (request_budget_exhausted)",
            limit_text(args.max_requests)
        ));
    }
    let report = rank_solana_wallets(&stats.wallets, policy);

    let incomplete =
        stats.is_coverage_incomplete() || !upstream_complete || !price_reasons.is_empty();
    eprintln!("{valuation_line}");
    if pricing.policy.is_some() {
        eprintln!("{pricing_line}");
    }
    for r in &price_reasons {
        eprintln!("wallet-rank: {r}");
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    print_diagnostics(
        &stats,
        &report,
        api_key,
        args,
        window,
        requests_made,
        elapsed_ms,
    );
    let captured_at = scout_app::now_utc_rfc3339();
    let lines = if args.format == "jsonl" {
        let compact: String = captured_at
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let run_id = format!("wallet-rank-{compact}");
        let meta = RunMetaInput {
            run_id: &run_id,
            captured_at: &captured_at,
            max_pages_per_wallet: args.max_pages_per_wallet,
            provider_options: provider.request_options(),
            server_window: args.server_window,
            concurrency: usize::try_from(args.concurrency).unwrap_or(1),
            max_requests: args.max_requests,
            requests_made,
            input_wallet_count: all.len(),
            input_duplicates: duplicates,
            upstream_complete,
            window: *window,
            pricing: scout_app::pricing_meta(&pricing_input),
            open_valuation: scout_app::open_valuation_meta(valuation.as_ref(), window.as_of),
            evm: None,
        };
        let summary = SummaryInput {
            run_id: &run_id,
            partial: incomplete,
            cancelled: stats.cancelled,
            stop: stats.stop,
            requests_made,
            requests_made_prices: pricing.requests_made,
            incomplete_reasons: stats
                .incomplete_reasons()
                .iter()
                .chain(price_reasons.iter())
                .map(|r| redact(r, api_key))
                .collect(),
        };
        match output::jsonl_lines(&meta, &report, summary, &|t| redact(t, api_key)) {
            Ok(l) => l,
            Err(message) => {
                eprintln!("wallet-rank: could not render output: {message}");
                return ChainRun::failed(4, format!("could not render output: {message}")).into();
            }
        }
    } else {
        output::table_lines(
            &report,
            incomplete,
            window,
            pricing.policy.is_some().then_some(pricing_line.as_str()),
        )
        .into_iter()
        .map(|l| redact_table_line(&l, api_key))
        .collect()
    };
    let status = match stats.stop {
        Some(ScanStop::BudgetExhausted { .. }) => 3,
        Some(ScanStop::RateLimited { .. }) => {
            if stats.any_data() {
                3
            } else {
                4
            }
        }
        None if stats.all_failed() => 4,
        None if incomplete => 3,
        None => 0,
    };
    let mut reasons: Vec<String> = stats
        .incomplete_reasons()
        .iter()
        .map(|r| redact(r, api_key))
        .collect();
    reasons.extend(price_reasons);
    RankRun {
        run: ChainRun {
            lines,
            status,
            cancelled: stats.cancelled,
            requests_made,
            failure: None,
            reasons,
        },
        prices_made: pricing.requests_made,
        pricing_line: pricing.policy.is_some().then_some(pricing_line),
        secrets: vec![api_key.to_string()],
        stats: Some(stats),
    }
}

/// Scope, policy and coverage on stderr (stdout stays the result format).
fn print_diagnostics(
    stats: &SolanaWalletStatsReport,
    report: &WalletRankReport,
    api_key: &str,
    args: &Args,
    window: &AnalysisWindow,
    requests_made: u64,
    elapsed_ms: u64,
) {
    let s = &stats.scope;
    let p = &report.policy;
    eprintln!("wallet-rank: protocol scope (Solana mainnet, SOL-quoted ledger, lamports):");
    eprintln!("  recognized: {}", s.recognized);
    eprintln!("  NOT decoded: {}", s.not_decoded);
    eprintln!(
        "  programs: pump.fun bonding curve {} (IDL {} sha256 {}); PumpSwap AMM {} (IDL {} sha256 {})",
        s.program_id,
        s.idl_commit,
        s.idl_sha256,
        s.amm_program_id.unwrap_or("-"),
        s.amm_idl_commit.unwrap_or("-"),
        s.amm_idl_sha256.unwrap_or("-"),
    );
    let effective = provider_options(args, window);
    eprintln!(
        "  provider options: {} server_window={} (tx budget per wallet = max_pages_per_wallet * page_limit = {})",
        effective.describe(),
        args.server_window,
        u64::from(args.max_pages_per_wallet) * u64::from(effective.page_limit)
    );
    eprintln!(
        "  concurrency={} (max wallets scanned at once; slices not used for wallets) \
         elapsed_ms={elapsed_ms} (stderr only, not in JSONL)",
        stats.concurrency
    );
    eprintln!(
        "  scan: newest-first, max_pages_per_wallet={} (page_limit txs/page; retries not counted), \
         max_requests={}, requests_made={requests_made}; {}",
        args.max_pages_per_wallet,
        args.max_requests
            .map_or_else(|| "unlimited".to_string(), |n| n.to_string()),
        match window.bounds() {
            Some((since, until)) => format!(
                "window [{}, {}) source={} as_of={} (stops after the first page older than the \
                 window start; budget exhausted before it = incomplete; pre-window inventory is \
                 left-censored, ADR-011)",
                scout_app::format_unix_utc(u64::try_from(since).unwrap_or(0)),
                scout_app::format_unix_utc(u64::try_from(until).unwrap_or(0)),
                window.source.label(),
                scout_app::format_unix_utc(u64::try_from(window.as_of).unwrap_or(0))
            ),
            None => "no time window".to_string(),
        }
    );
    match stats.stop {
        Some(ScanStop::BudgetExhausted { limit }) => eprintln!(
            "wallet-rank: request budget exhausted after {requests_made} requests (limit {limit}); \
             the interrupted wallet is provider_error, remaining wallets are not_scanned, ranking is over an incomplete universe"
        ),
        Some(ScanStop::RateLimited { retry_after_secs }) => eprintln!(
            "wallet-rank: {}; remaining wallets were not scanned, ranking is over an incomplete universe",
            rate_limited_text(retry_after_secs)
        ),
        None => {}
    }
    let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |n| n.to_string());
    eprintln!(
        "  policy: rank_by={} quote={} profile={} min_closed_episodes={} min_active_days={} \
         max_trades_per_day={} max_mints_per_day={} unknown_share_gate={} max_unknown_episode_share_percent={} exclude_unbounded={} require_no_open={} require_valued_open={} top={} \
         (research starting policy, not statistical guarantees)",
        p.rank_by.label(),
        scout_engine::quote_unit_label(p.quote),
        p.profile.label(),
        p.min_closed_episodes,
        p.min_active_days,
        opt(p.max_trades_per_day),
        opt(p.max_mints_per_day),
        p.unknown_share_gate,
        p.max_unknown_episode_share_percent,
        p.exclude_unbounded,
        p.require_no_open,
        p.require_valued_open,
        p.top
    );
    eprintln!(
        "  open positions: included and flagged open_exposure (realizable valuation ADR-019 unless --no-valuation; never part of a rank key); --require-no-open / --require-valued-open exclude"
    );
    eprintln!(
        "wallet-rank: {} input, {} eligible, {} ranked, {} excluded",
        report.input_count,
        report.eligible_count,
        report.ranked.len(),
        report.excluded.len()
    );
    for w in &stats.wallets {
        let Some(l) = &w.ledger else { continue };
        let d = &l.diagnostics;
        if d.malformed_trade_instructions
            + d.orphan_trade_events
            + d.unknown_discriminator_instructions
            + d.jupiter_malformed_events
            + d.jupiter_unknown_events
            + d.dflow_malformed_events
            + d.dflow_unknown_events
            + d.okx_malformed_events
            + d.okx_unknown_events
            + d.okx_swap_with_receiver_not_attributed
            + d.okx_idl_only_order_events
            + d.venue_events.coverage_gaps()
            > 0
        {
            eprintln!(
                "  wallet {} coverage gaps: malformed_trade_instructions={} orphan_trade_events={} \
                 unknown_discriminator_instructions={} jupiter_malformed_events={} \
                 jupiter_unknown_events={} dflow_malformed_events={} dflow_unknown_events={} \
                 okx_malformed_events={} okx_unknown_events={} \
                 okx_swap_with_receiver_not_attributed={} okx_idl_only_order_events={} \
                 venue_events={:?}",
                bs58::encode(w.wallet).into_string(),
                d.malformed_trade_instructions,
                d.orphan_trade_events,
                d.unknown_discriminator_instructions,
                d.jupiter_malformed_events,
                d.jupiter_unknown_events,
                d.dflow_malformed_events,
                d.dflow_unknown_events,
                d.okx_malformed_events,
                d.okx_unknown_events,
                d.okx_swap_with_receiver_not_attributed,
                d.okx_idl_only_order_events,
                d.venue_events
            );
        }
        for e in &l.evidence_samples {
            eprintln!(
                "    evidence (up to {}): {}",
                scout_engine::MAX_EVIDENCE_SAMPLES,
                scout_app::evidence_line(e, &|s| redact(s, api_key))
            );
        }
    }
    let reasons = stats.incomplete_reasons();
    if reasons.is_empty() {
        eprintln!("wallet-rank: status=complete within declared protocol scope");
    } else {
        eprintln!("wallet-rank: status=partial (ranking over an incomplete universe):");
        for r in reasons {
            eprintln!("  - {}", redact(&r, api_key));
        }
    }
}

fn read_input(input: Option<&str>) -> Result<String, String> {
    match input {
        Some("-") => read_stdin_to_string(),
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))
        }
        None => {
            if io::stdin().is_terminal() {
                Err("no --input given and stdin is a terminal; pass --input <path>, --input -, or pipe data".to_string())
            } else {
                read_stdin_to_string()
            }
        }
    }
}

fn read_stdin_to_string() -> Result<String, String> {
    let mut buf = String::new();
    let mut lock = io::stdin().lock();
    lock.read_to_string(&mut buf)
        .map_err(|e| format!("could not read stdin: {e}"))?;
    Ok(buf)
}

#[cfg(test)]
mod evm_policy_tests {
    #![allow(clippy::unwrap_used)]
    use scout_core::{ChainFamily, ChainKey, GenesisIdentity, NetworkId};

    use super::*;

    fn chain(id: u64) -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(id),
            genesis_identity: GenesisIdentity::Unverified,
        }
    }

    fn quote(q: QuoteUnit, id: u64) -> Result<QuoteUnit, String> {
        let p = RankPolicy {
            quote: q,
            ..RankPolicy::default()
        };
        evm_policy(p, &chain(id)).map(|p| p.quote)
    }

    #[test]
    fn each_chain_accepts_native_usd_and_its_own_stable_only() {
        // Base 8453: usdc; Robinhood 4663: usdg.
        assert_eq!(quote(QuoteUnit::UsdcUnits, 8453), Ok(QuoteUnit::UsdcUnits));
        assert!(quote(QuoteUnit::UsdgUnits, 8453).is_err());
        assert_eq!(quote(QuoteUnit::UsdgUnits, 4663), Ok(QuoteUnit::UsdgUnits));
        let e = quote(QuoteUnit::UsdcUnits, 4663).unwrap_err();
        assert!(e.contains("use eth, usdg or usd"), "{e}");
        for id in [8453, 4663] {
            assert_eq!(quote(QuoteUnit::Lamports, id), Ok(QuoteUnit::Wei));
            assert_eq!(quote(QuoteUnit::Wei, id), Ok(QuoteUnit::Wei));
            assert_eq!(
                quote(QuoteUnit::ReportCurrency, id),
                Ok(QuoteUnit::ReportCurrency)
            );
            assert!(quote(QuoteUnit::UsdtUnits, id).is_err());
        }
    }
}
