//! scout-engine: minimal synchronous orchestration wiring input →
//! provider → normalize → ledger → analytics → report. See
//! ARCHITECTURE.md §11 for the full highload pipeline this is a first
//! offline slice of.
//!
//! Deliberately NOT implemented here yet: bounded async channels,
//! wallet sharding, backpressure, spawn_blocking CPU pool. Those are
//! real concurrency concerns for a live-RPC scanner processing many
//! wallets in parallel; against a `FixtureProvider` with a handful of
//! synthetic fixtures they would be premature complexity that AGENTS.md
//! itself warns against ("Не внедрять custom lock-free/unsafe без
//! профиля, ADR и измеренной необходимости" — the same principle
//! applies to any concurrency machinery: build it when a measured need
//! exists, not speculatively). This crate's job right now is proving
//! the vertical slice wires together correctly end to end.
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

mod analysis_window;
mod buyer_intersect;
mod decode_evidence;
mod solana_buy_qualification;
mod solana_buyer_intersect;
mod solana_open_valuation;
mod solana_wallet_ledger;
mod solana_wallet_rank;
mod solana_wallet_stats;
mod solana_wallet_usd;

pub use analysis_window::{
    AnalysisWindow, MAX_PERIOD_DAYS, WindowError, WindowSource, parse_period_days,
    parse_rfc3339_utc,
};
pub use buyer_intersect::{BuyerIntersectReport, BuyerMatch, run_buyer_intersect};
pub use decode_evidence::{
    DecodeEvidence, EvidenceKind, MAX_EVIDENCE_SAMPLES, merge_evidence, program_name,
};
pub use scout_dex_solana::{PumpTradeVariant, TradeSide, VariantVerification};
pub use solana_buy_qualification::{
    PUMP_BONDING_CURVE_IDL_COMMIT, PUMP_BONDING_CURVE_PROGRAM_ID, QualifiedBuy,
    SOLANA_BUY_QUALIFICATION_VERSION, SOLANA_TRADE_QUALIFICATION_VERSION, ScopeError,
    TxQualification, TxQualificationDiagnostics, UnverifiedVariantBuy, VariantPolicy,
    default_variant_policy, pump_bonding_curve_decoder, pump_bonding_curve_scope,
    qualify_bonding_curve_buys, qualify_bonding_curve_buys_with_policy, solana_mainnet_chain,
};
pub use solana_buyer_intersect::{
    IntersectOptions, ScanFailureKind, ScanStop, SideEvidence, SideFilter,
    SolanaBuyerIntersectReport, SolanaProtocolScope, SolanaTokenScanSummary, TokenScanStatus,
    TokenSideHits, TradeAttributionDiagnostics, classify_provider_error,
    run_solana_buyer_intersect, run_solana_buyer_intersect_with_policy, run_solana_trade_intersect,
    run_solana_trade_intersect_with_policies, run_solana_trade_intersect_with_policy,
    sanitize_provider_text,
};
pub use solana_open_valuation::{
    CurveRead, FeeObservation, LABEL_REALIZABLE_CP_QUOTE, OpenBasis, OpenUsd, OpenValuationRun,
    OpenValuationTotals, OpenValuationView, OpenVenueInfo, PoolRead, PositionValuation,
    SOLANA_OPEN_VALUATION_VERSION, StateSnapshot, UnvaluedReason, VALUATION_COMMITMENT,
    ValuationOutcome, ValuedPosition, VenueAddress, apply_open_valuation, open_usd_requirement,
    value_position, window_is_live,
};
pub use solana_wallet_ledger::{
    ActivityMetrics, ConsumedBasisStatus, DailyActivity, EpisodeOutcome, EpisodePnlBound,
    EpisodeRecord, LedgerDecoders, LedgerDiagnostics, LedgerOptions, LowerBound, OkxOrderPolicy,
    OpenPosition, QuoteUnit, QuoteUnitBlock, QuoteUnitCounts, RouteEvidenceCounts, RouteRejections,
    RouteSwapRecord, SOLANA_QUOTE_UNITS, SOLANA_WALLET_LEDGER_SCOPE, SOLANA_WALLET_LEDGER_VERSION,
    SWAP_VENUE_PROGRAM_IDS, SolanaWalletLedgerError, SolanaWalletLedgerReport, TradeCounts,
    USDC_MINT, USDT_MINT, UnknownReason, VariantTradeCount, Venue, VenueSideCounts, WSOL_MINT,
    WinRateLowerBound, allocate_fee_proportionally, build_solana_wallet_ledger,
    build_solana_wallet_ledger_venues, build_solana_wallet_ledger_with_options,
    default_okx_order_policy, format_quote_money, lamports_to_money, money_to_lamports_trunc,
    money_to_quote_units_trunc, money_to_unit_raw, pump_amm_decoder, quote_unit_decimals,
    quote_unit_label, quote_units_to_money,
};
pub use solana_wallet_rank::{
    DEFAULT_MAX_UNKNOWN_EPISODE_SHARE_PERCENT, DEFAULT_TOP, ExcludedWallet, ExclusionReason,
    OpenExposure, PnlStatus, RankBy, RankPolicy, RankProfile, RankedWallet, Ratio,
    SOLANA_WALLET_RANK_VERSION, WalletRankObservation, WalletRankReport, money_exact_sol_string,
    rank_solana_wallets,
};
pub use solana_wallet_stats::{
    SolanaWalletStats, SolanaWalletStatsReport, WalletScanStatus, format_scaled_decimal,
    lamports_to_sol_string, rational_to_decimal_string, run_solana_wallet_stats,
    run_solana_wallet_stats_windowed, run_solana_wallet_stats_windowed_venues,
};
pub use solana_wallet_usd::{
    SOLANA_WALLET_USD_VERSION, UsdCoverage, UsdDisposal, UsdEpisode, UsdJournal, UsdLedgerView,
    UsdLotSlice, UsdOutcome, UsdPricingRun, apply_usd_pricing, convert_quote_to_usd,
    quote_asset_of,
};
