//! Stable JSONL output DTOs for `buyer-intersect` (docs/CLI.md §7).
//!
//! These types are the wire contract. They are built field-by-field from
//! engine reports and never derive output from internal types' serde
//! shape, so an internal refactor cannot silently change the schema.
//! Addresses are full strings (base58 for Solana, `0x` lowercase hex for
//! EVM); counts are JSON numbers (all bounded `u64` far below 2^53 here);
//! unknown values are `null` with an explicit status, never `0`.

use scout_app::{SCHEMA_VERSION, chain_profile_name};
use scout_core::{AssetKey, ChainKey, WalletKey};
use std::collections::BTreeMap;

use scout_engine::{
    AnalysisWindow, BuyerMatch, ScanFailureKind, ScanStop, SideEvidence,
    SolanaBuyerIntersectReport, SolanaProtocolScope, SolanaTokenScanSummary, TokenScanStatus,
    TokenSideHits, TradeAttributionDiagnostics, Venue,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WalletDto {
    pub chain: &'static str,
    pub address: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AssetDto {
    Token { chain: &'static str, token: String },
    Native { chain: &'static str, native: bool },
}

#[derive(Debug, Serialize)]
pub struct BuyerMatchRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub wallet: WalletDto,
    pub hit_count: usize,
    pub matched_assets: Vec<AssetDto>,
    /// ADR-014: per matched token the sides observed and the first
    /// qualifying evidence. Absent on the legacy (non-Solana) path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched_tokens: Vec<MatchedTokenDto>,
}

/// First qualifying operation of one side: lowest `(slot, tx index)`.
#[derive(Debug, Serialize)]
pub struct EvidenceDto {
    pub signature: String,
    pub slot: u64,
    /// `bonding_curve`, `pump_amm` or `route`.
    pub venue: &'static str,
    /// IDL instruction name, or `route_swap`.
    pub variant: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MatchedTokenDto {
    pub chain: &'static str,
    pub token: String,
    /// Sides observed under the selected `--side`: `buy` and/or `sell`.
    pub sides: Vec<&'static str>,
    /// Qualifying transactions per side (`null` = side not observed).
    pub buy_count: Option<u64>,
    pub sell_count: Option<u64>,
    pub first_buy: Option<EvidenceDto>,
    pub first_sell: Option<EvidenceDto>,
}

#[derive(Debug, Serialize)]
pub struct VariantDto {
    pub name: &'static str,
    pub side: &'static str,
    pub verification: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ScopeDto {
    pub chain: &'static str,
    pub program_id: &'static str,
    pub idl_commit: &'static str,
    pub idl_sha256: &'static str,
    pub qualification_version: &'static str,
    /// PumpSwap AMM program and IDL pin (ADR-012).
    pub amm_program_id: Option<&'static str>,
    pub amm_idl_commit: Option<&'static str>,
    pub amm_idl_sha256: Option<&'static str>,
    pub recognized: &'static str,
    pub not_decoded: &'static str,
    pub variants: Vec<VariantDto>,
}

/// Run-level budget inputs and measured usage, passed to the renderers.
#[derive(Debug, Clone, Copy)]
pub struct RunBudget {
    pub max_pages_per_token: u32,
    /// `--max-requests`; `None` = unlimited.
    pub max_requests: Option<u64>,
    /// HTTP attempts actually made (retries included).
    pub requests_made: u64,
    /// Effective provider request options (page limit, filters).
    pub provider_options: scout_providers::HeliusRequestOptions,
    /// `--server-window` requested.
    pub server_window: bool,
    /// `--concurrency`: scan units in flight (effective).
    pub concurrency: usize,
    /// `--slices`: effective time slices per token (1 = unsliced).
    pub slices: u32,
}

#[derive(Debug, Serialize)]
pub struct ProviderOptionsDto {
    #[serde(flatten)]
    pub request: scout_providers::HeliusRequestOptions,
    pub server_window: bool,
    /// `max_pages_per_token * page_limit`.
    pub tx_budget_per_token: u64,
}

#[derive(Debug, Serialize)]
pub struct BudgetDto {
    /// Provider pages requested per input token (`--max-pages-per-token`).
    /// Retries are not counted against it.
    pub max_pages_per_token: u32,
    /// Total HTTP attempts allowed (`--max-requests`, retries included);
    /// `null` = unlimited.
    pub max_requests: Option<u64>,
    /// Max scan units (tokens / token slices) in flight (`--concurrency`).
    pub concurrency: usize,
    /// Effective time slices per token (`--slices`; 1 = unsliced).
    pub slices: u32,
}

#[derive(Debug, Serialize)]
pub struct RunMetaRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    pub captured_at: String,
    pub scope: ScopeDto,
    pub budget: BudgetDto,
    /// Effective provider request options (not yet live-verified).
    pub provider_options: ProviderOptionsDto,
    /// Measured HTTP attempts for the run (retries included). Always
    /// present, also when no limit was set.
    pub requests_made: u64,
    pub input_tokens: Vec<AssetDto>,
    pub input_token_count: usize,
    pub min_token_hits: usize,
    /// ADR-014: `buy`, `sell` or `any`.
    pub side: &'static str,
    /// ADR-011 analysis window (`since`/`until` null without one).
    pub window: WindowDto,
    /// `oldest_first` (no window) or `newest_first` (window).
    pub scan_order: &'static str,
}

/// ADR-011 analysis window of the run (`since`/`until` null without one).
#[derive(Debug, Serialize)]
pub struct WindowDto {
    pub since: Option<String>,
    pub until: Option<String>,
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub as_of: String,
    pub as_of_unix: i64,
    /// `period`, `explicit` or `none`.
    pub source: &'static str,
}

fn rfc3339(unix: i64) -> String {
    scout_app::format_unix_utc(u64::try_from(unix).unwrap_or(0))
}

pub fn window_dto(w: &AnalysisWindow) -> WindowDto {
    let bounds = w.bounds();
    WindowDto {
        since: bounds.map(|(s, _)| rfc3339(s)),
        until: bounds.map(|(_, u)| rfc3339(u)),
        since_unix: bounds.map(|(s, _)| s),
        until_unix: bounds.map(|(_, u)| u),
        as_of: rfc3339(w.as_of),
        as_of_unix: w.as_of,
        source: w.source.label(),
    }
}

/// Qualifying operations of one venue by side.
#[derive(Debug, Serialize)]
pub struct VenueSideDto {
    pub buys: u64,
    pub sells: u64,
}

#[derive(Debug, Serialize)]
pub struct VenueOpsDto {
    pub bonding_curve: VenueSideDto,
    pub pump_amm: VenueSideDto,
    pub route: VenueSideDto,
}

#[derive(Debug, Serialize)]
pub struct RouteRejectionsDto {
    pub wallet_not_signer: u64,
    pub multi_asset: u64,
    pub not_opposite_signs: u64,
    pub no_quote_leg: u64,
    pub no_verified_leg: u64,
    pub passthrough_nonzero: u64,
}

#[derive(Debug, Serialize)]
pub struct TokenDiagnosticsDto {
    pub decoded_buys: u64,
    pub decoded_sells: u64,
    pub malformed_instructions: u64,
    pub unknown_discriminator_instructions: u64,
    /// IdlOnly trades that would qualify (any venue): never hits, coverage gap.
    pub unverified_variant_buys: u64,
    pub failed_transactions: u64,
    pub positive_delta_without_instruction: u64,
    /// Qualifying (wallet, side, transaction) operations per venue and side.
    pub qualified_ops: VenueOpsDto,
    /// PumpSwap router-forwards (non-signing zero-net users): not attributed.
    pub router_forwards_not_attributed: u64,
    pub route_rejections: RouteRejectionsDto,
    pub idl_only_trades: u64,
    pub amm_unreconciled: u64,
    pub trades_without_matching_delta: u64,
    pub malformed_trades: u64,
    pub orphan_events: u64,
    /// ADR-015: Jupiter event-CPIs that did not decode exactly / with an
    /// unknown discriminator (never used as swap evidence).
    pub jupiter_malformed_events: u64,
    pub jupiter_unknown_events: u64,
    /// ADR-015 amendment: the same for DFlow Aggregator v4 events.
    pub dflow_malformed_events: u64,
    pub dflow_unknown_events: u64,
    /// ADR-017 draft: OKX DEX Router event coverage.
    pub okx_malformed_events: u64,
    pub okx_unknown_events: u64,
    /// Order events with a distinct receiver: attributed to nobody.
    pub okx_swap_with_receiver_not_attributed: u64,
    /// Order events of an IdlOnly variant: counted, not leg evidence.
    pub okx_idl_only_order_events: u64,
    /// ADR-013 section 2b: direct-venue swap-event coverage.
    pub venue_events: scout_app::VenueEventsDto,
    /// At most 5 samples of malformed trade instructions, unknown
    /// discriminators and orphan events of this token's transactions.
    pub evidence_samples: Vec<scout_app::DecodeEvidenceDto>,
}

/// Typed run-stop reason / failure kind: `kind` is `budget_exhausted`,
/// `rate_limited` (or `other` for a failure kind with no stop).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ReasonDto {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

pub fn stop_dto(stop: ScanStop) -> ReasonDto {
    match stop {
        ScanStop::BudgetExhausted { limit } => ReasonDto {
            kind: "budget_exhausted",
            limit: Some(limit),
            retry_after_secs: None,
        },
        ScanStop::RateLimited { retry_after_secs } => ReasonDto {
            kind: "rate_limited",
            limit: None,
            retry_after_secs,
        },
    }
}

fn failure_dto(kind: ScanFailureKind) -> ReasonDto {
    kind.stop().map_or(
        ReasonDto {
            kind: "other",
            limit: None,
            retry_after_secs: None,
        },
        stop_dto,
    )
}

/// Per-token scan status. For `failed` and `not_scanned` every count is
/// `null`: unknown is never reported as zero. For `truncated` counts are
/// lower bounds.
#[derive(Debug, Serialize)]
pub struct TokenStatusDto {
    pub token: AssetDto,
    /// `ok`, `truncated`, `failed` or `not_scanned`.
    pub status: &'static str,
    pub error: Option<String>,
    /// For `failed`: the classified cause.
    pub error_kind: Option<ReasonDto>,
    /// For `not_scanned`: why the run stopped before this token.
    pub stop_reason: Option<ReasonDto>,
    pub transactions_scanned: Option<u64>,
    /// Windowed scan only: transactions inside `[since, until)`.
    pub transactions_in_window: Option<u64>,
    /// Distinct wallets with a qualified buy of the token.
    pub qualified_buyers: Option<u64>,
    pub qualified_sellers: Option<u64>,
    /// Distinct wallets with a qualifying op of the selected side(s).
    pub qualified_wallets: Option<u64>,
    pub diagnostics: Option<TokenDiagnosticsDto>,
    /// Sliced scans: the slices that did not complete (absent when none).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub truncated_slices: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RunSummaryRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub run_id: String,
    /// `complete` or `partial` (operational completion, CLI.md §7).
    pub status: &'static str,
    pub cancelled: bool,
    pub records: usize,
    pub incomplete_reasons: Vec<String>,
    pub tokens: Vec<TokenStatusDto>,
}

fn chain_name(chain: &ChainKey) -> Result<&'static str, String> {
    chain_profile_name(chain).ok_or_else(|| format!("no output profile name for chain {chain:?}"))
}

pub fn wallet_dto(wallet: &WalletKey) -> Result<WalletDto, String> {
    Ok(WalletDto {
        chain: chain_name(&wallet.chain)?,
        address: wallet.address.to_string(),
    })
}

pub fn asset_dto(asset: &AssetKey) -> Result<AssetDto, String> {
    Ok(match asset {
        AssetKey::Token(chain, address) => AssetDto::Token {
            chain: chain_name(chain)?,
            token: address.to_string(),
        },
        AssetKey::Native(chain) => AssetDto::Native {
            chain: chain_name(chain)?,
            native: true,
        },
    })
}

fn evidence_dto(e: &SideEvidence) -> EvidenceDto {
    EvidenceDto {
        signature: bs58::encode(e.signature).into_string(),
        slot: e.slot,
        venue: e.venue.label(),
        variant: e.variant,
    }
}

fn matched_token_dto(asset: &AssetKey, hits: &TokenSideHits) -> Result<MatchedTokenDto, String> {
    let AssetDto::Token { chain, token } = asset_dto(asset)? else {
        return Err("matched asset is not a token".to_string());
    };
    let mut sides = Vec::new();
    if hits.buy.is_some() {
        sides.push("buy");
    }
    if hits.sell.is_some() {
        sides.push("sell");
    }
    Ok(MatchedTokenDto {
        chain,
        token,
        sides,
        buy_count: hits.buy.as_ref().map(|e| e.count),
        sell_count: hits.sell.as_ref().map(|e| e.count),
        first_buy: hits.buy.as_ref().map(evidence_dto),
        first_sell: hits.sell.as_ref().map(evidence_dto),
    })
}

pub fn buyer_match_record(
    m: &BuyerMatch,
    side_hits: Option<&BTreeMap<AssetKey, TokenSideHits>>,
) -> Result<BuyerMatchRecord, String> {
    let matched_tokens = match side_hits {
        Some(per_asset) => m
            .matched_assets
            .iter()
            .filter_map(|a| per_asset.get(a).map(|h| matched_token_dto(a, h)))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    Ok(BuyerMatchRecord {
        schema_version: SCHEMA_VERSION,
        kind: "buyer_match",
        wallet: wallet_dto(&m.wallet)?,
        hit_count: m.hit_count,
        matched_assets: m
            .matched_assets
            .iter()
            .map(asset_dto)
            .collect::<Result<_, _>>()?,
        matched_tokens,
    })
}

pub fn run_meta_record(
    run_id: &str,
    captured_at: &str,
    report: &SolanaBuyerIntersectReport,
    input_tokens: &[AssetKey],
    budget: RunBudget,
) -> Result<RunMetaRecord, String> {
    let scope: &SolanaProtocolScope = &report.scope;
    Ok(RunMetaRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_meta",
        run_id: run_id.to_string(),
        captured_at: captured_at.to_string(),
        scope: ScopeDto {
            chain: "solana",
            program_id: scope.program_id,
            idl_commit: scope.idl_commit,
            idl_sha256: scope.idl_sha256,
            qualification_version: scope.qualification_version,
            amm_program_id: scope.amm_program_id,
            amm_idl_commit: scope.amm_idl_commit,
            amm_idl_sha256: scope.amm_idl_sha256,
            recognized: scope.recognized,
            not_decoded: scope.not_decoded,
            variants: SolanaProtocolScope::variants()
                .into_iter()
                .map(|(name, side, verification)| VariantDto {
                    name,
                    side,
                    verification,
                })
                .collect(),
        },
        budget: BudgetDto {
            max_pages_per_token: budget.max_pages_per_token,
            max_requests: budget.max_requests,
            concurrency: budget.concurrency,
            slices: budget.slices,
        },
        provider_options: ProviderOptionsDto {
            request: budget.provider_options,
            server_window: budget.server_window,
            tx_budget_per_token: u64::from(budget.max_pages_per_token)
                * u64::from(budget.provider_options.page_limit),
        },
        requests_made: budget.requests_made,
        input_tokens: input_tokens
            .iter()
            .map(asset_dto)
            .collect::<Result<_, _>>()?,
        input_token_count: report.base.input_token_count,
        min_token_hits: report.base.min_token_hits,
        side: report.side.label(),
        window: window_dto(&report.window),
        scan_order: if report.window.is_bounded() {
            "newest_first"
        } else {
            "oldest_first"
        },
    })
}

fn venue_ops(t: &TradeAttributionDiagnostics) -> VenueOpsDto {
    let of = |venue: Venue| VenueSideDto {
        buys: t.ops(venue, scout_engine::TradeSide::Buy),
        sells: t.ops(venue, scout_engine::TradeSide::Sell),
    };
    VenueOpsDto {
        bonding_curve: of(Venue::BondingCurve),
        pump_amm: of(Venue::PumpAmm),
        route: of(Venue::Route),
    }
}

fn token_status(
    token: &SolanaTokenScanSummary,
    redact: &dyn Fn(&str) -> String,
) -> Result<TokenStatusDto, String> {
    let asset = asset_dto(&token.asset)?;
    let truncated_slices: Vec<String> = token
        .truncated_slices
        .iter()
        .map(scout_engine::SliceId::describe)
        .collect();
    match &token.status {
        TokenScanStatus::Ok => {}
        TokenScanStatus::Failed { kind, message } => {
            return Ok(TokenStatusDto {
                token: asset,
                status: "failed",
                error: Some(redact(message)),
                error_kind: Some(failure_dto(*kind)),
                stop_reason: None,
                transactions_scanned: None,
                transactions_in_window: None,
                qualified_buyers: None,
                qualified_sellers: None,
                qualified_wallets: None,
                diagnostics: None,
                truncated_slices,
            });
        }
        TokenScanStatus::NotScanned { reason } => {
            return Ok(TokenStatusDto {
                token: asset,
                status: "not_scanned",
                error: None,
                error_kind: None,
                stop_reason: Some(stop_dto(*reason)),
                transactions_scanned: None,
                transactions_in_window: None,
                qualified_buyers: None,
                qualified_sellers: None,
                qualified_wallets: None,
                diagnostics: None,
                truncated_slices,
            });
        }
    }
    let d = &token.diagnostics;
    Ok(TokenStatusDto {
        token: asset,
        status: if token.truncated { "truncated" } else { "ok" },
        error: None,
        error_kind: None,
        stop_reason: None,
        transactions_scanned: Some(token.transactions_scanned),
        transactions_in_window: token.transactions_in_window,
        qualified_buyers: Some(token.qualified_buyers),
        qualified_sellers: Some(token.qualified_sellers),
        qualified_wallets: Some(token.qualified_wallets),
        diagnostics: Some(TokenDiagnosticsDto {
            decoded_buys: d.decoded_buys,
            decoded_sells: d.decoded_sells,
            malformed_instructions: d.malformed_instructions,
            unknown_discriminator_instructions: d.unknown_discriminator_instructions,
            unverified_variant_buys: token.trade.idl_only_trades,
            failed_transactions: d.failed_transactions,
            positive_delta_without_instruction: token.positive_delta_without_instruction,
            qualified_ops: venue_ops(&token.trade),
            router_forwards_not_attributed: token.trade.router_forwards_not_attributed,
            route_rejections: RouteRejectionsDto {
                wallet_not_signer: token.trade.route_rejections.wallet_not_signer,
                multi_asset: token.trade.route_rejections.multi_asset,
                not_opposite_signs: token.trade.route_rejections.not_opposite_signs,
                no_quote_leg: token.trade.route_rejections.no_quote_leg,
                no_verified_leg: token.trade.route_rejections.no_verified_leg,
                passthrough_nonzero: token.trade.route_rejections.passthrough_nonzero,
            },
            idl_only_trades: token.trade.idl_only_trades,
            amm_unreconciled: token.trade.amm_unreconciled,
            trades_without_matching_delta: token.trade.trades_without_matching_delta,
            malformed_trades: token.trade.malformed_trades,
            orphan_events: token.trade.orphan_events,
            jupiter_malformed_events: token.trade.jupiter_malformed_events,
            jupiter_unknown_events: token.trade.jupiter_unknown_events,
            dflow_malformed_events: token.trade.dflow_malformed_events,
            dflow_unknown_events: token.trade.dflow_unknown_events,
            okx_malformed_events: token.trade.okx_malformed_events,
            okx_unknown_events: token.trade.okx_unknown_events,
            okx_swap_with_receiver_not_attributed: token
                .trade
                .okx_swap_with_receiver_not_attributed,
            okx_idl_only_order_events: token.trade.okx_idl_only_order_events,
            venue_events: (&token.trade.venue_events).into(),
            evidence_samples: scout_app::evidence_dtos(&token.trade.evidence_samples, redact),
        }),
        truncated_slices,
    })
}

pub fn run_summary_record(
    run_id: &str,
    report: &SolanaBuyerIntersectReport,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<RunSummaryRecord, String> {
    Ok(RunSummaryRecord {
        schema_version: SCHEMA_VERSION,
        kind: "run_summary",
        run_id: run_id.to_string(),
        status: if incomplete { "partial" } else { "complete" },
        cancelled: report.cancelled,
        records: report.base.matches.len(),
        incomplete_reasons: report
            .incomplete_reasons()
            .iter()
            .map(|r| redact(r))
            .collect(),
        tokens: report
            .per_token
            .iter()
            .map(|t| token_status(t, redact))
            .collect::<Result<_, _>>()?,
    })
}

/// All JSONL lines for a Solana run: `run_meta`, `buyer_match`*,
/// `run_summary`.
pub fn solana_jsonl_lines(
    run_id: &str,
    captured_at: &str,
    report: &SolanaBuyerIntersectReport,
    input_tokens: &[AssetKey],
    budget: RunBudget,
    incomplete: bool,
    redact: &dyn Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let ser = |r: Result<String, serde_json::Error>| r.map_err(|e| e.to_string());
    let mut lines = Vec::with_capacity(report.base.matches.len() + 2);
    lines.push(ser(serde_json::to_string(&run_meta_record(
        run_id,
        captured_at,
        report,
        input_tokens,
        budget,
    )?))?);
    for m in &report.base.matches {
        let hits = report.side_hits.get(&m.wallet);
        lines.push(ser(serde_json::to_string(&buyer_match_record(m, hits)?))?);
    }
    lines.push(ser(serde_json::to_string(&run_summary_record(
        run_id, report, incomplete, redact,
    )?))?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use scout_core::AddressBytes;
    use scout_engine::{BuyerIntersectReport, solana_mainnet_chain};
    use serde_json::Value;

    const MINT_A: &str = "AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump";
    const MINT_B: &str = "NkpbN7shUNdkvt24F33oai9Cf9rXDzJ4E8Sx2mNpump";

    fn mint(text: &str) -> AssetKey {
        let bytes: [u8; 32] = bs58::decode(text).into_vec().unwrap().try_into().unwrap();
        AssetKey::Token(solana_mainnet_chain(), AddressBytes::Solana(bytes))
    }

    fn token(asset: AssetKey, failed: bool, truncated: bool) -> SolanaTokenScanSummary {
        SolanaTokenScanSummary {
            asset,
            transactions_scanned: 250,
            truncated,
            truncated_slices: Vec::new(),
            status: if failed {
                TokenScanStatus::Failed {
                    kind: ScanFailureKind::Other,
                    message: "scan failed at https://h/?api-key=SECRET99".to_string(),
                }
            } else {
                TokenScanStatus::Ok
            },
            qualified_buyers: 3,
            qualified_sellers: 1,
            qualified_wallets: 4,
            transactions_in_window: None,
            boundary_reached: false,
            missing_block_time: 0,
            diagnostics: scout_engine::TxQualificationDiagnostics::default(),
            trade: TradeAttributionDiagnostics::default(),
            positive_delta_without_instruction: 0,
            unexpected_payloads: 0,
        }
    }

    fn report(failed: bool) -> SolanaBuyerIntersectReport {
        let (a, b) = (mint(MINT_A), mint(MINT_B));
        let wallet = WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana([7; 32]),
        };
        SolanaBuyerIntersectReport {
            base: BuyerIntersectReport {
                matches: vec![BuyerMatch {
                    wallet,
                    hit_count: 2,
                    matched_assets: vec![a.clone(), b.clone()],
                }],
                input_token_count: 2,
                min_token_hits: 2,
                coverage_truncated: false,
            },
            scope: SolanaProtocolScope::pump_trade_intersect(),
            per_token: vec![token(a, false, false), token(b, failed, !failed)],
            diagnostics: scout_engine::TxQualificationDiagnostics::default(),
            trade: TradeAttributionDiagnostics::default(),
            side: scout_engine::SideFilter::Any,
            window: AnalysisWindow::none(1_790_000_000),
            side_hits: BTreeMap::new(),
            positive_delta_without_instruction: 0,
            unexpected_payloads: 0,
            malformed_samples: vec![],
            unknown_discriminator_samples: vec![],
            cancelled: false,
            stop: None,
            concurrency: 4,
            slices: 1,
        }
    }

    fn lines(failed: bool) -> Vec<Value> {
        let r = report(failed);
        let tokens = vec![mint(MINT_A), mint(MINT_B)];
        let incomplete = r.is_coverage_incomplete();
        solana_jsonl_lines(
            "run-1",
            "2026-10-02T12:34:56Z",
            &r,
            &tokens,
            RunBudget {
                max_pages_per_token: 25,
                max_requests: Some(9),
                requests_made: 7,
                provider_options: scout_providers::HeliusRequestOptions {
                    page_limit: 200,
                    status: scout_providers::StatusFilter::Succeeded,
                    block_time_gte: Some(10),
                    block_time_lt: Some(20),
                    ..scout_providers::HeliusRequestOptions::default()
                },
                server_window: true,
                concurrency: 4,
                slices: 1,
            },
            incomplete,
            &|t| t.replace("SECRET99", "<redacted>"),
        )
        .unwrap()
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
    }

    #[test]
    fn record_order_and_wallet_shape() {
        let v = lines(false);
        let kinds: Vec<&str> = v.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["run_meta", "buyer_match", "run_summary"]);
        assert!(v.iter().all(|r| r["schema_version"] == 1));
        let m = &v[1];
        assert_eq!(m["wallet"]["chain"], "solana");
        assert_eq!(
            m["wallet"]["address"].as_str().unwrap(),
            bs58::encode([7u8; 32]).into_string()
        );
        assert_eq!(m["hit_count"], 2);
        assert_eq!(m["matched_assets"][0]["chain"], "solana");
        assert_eq!(m["matched_assets"][0]["token"], MINT_A);
        assert_eq!(m["matched_assets"][1]["token"], MINT_B);
    }

    #[test]
    fn run_meta_carries_scope_budget_and_inputs() {
        let v = lines(false);
        let meta = &v[0];
        println!("{meta}");
        assert_eq!(meta["run_id"], "run-1");
        assert_eq!(meta["captured_at"], "2026-10-02T12:34:56Z");
        assert_eq!(meta["budget"]["max_pages_per_token"], 25);
        assert_eq!(meta["budget"]["max_requests"], 9);
        assert_eq!(meta["requests_made"], 7);
        let po = &meta["provider_options"];
        assert_eq!(po["page_limit"], 200);
        assert_eq!(po["status"], "Succeeded");
        assert_eq!(po["block_time_gte"], 10);
        assert_eq!(po["block_time_lt"], 20);
        assert_eq!(po["token_accounts"], "None");
        assert_eq!(po["server_window"], true);
        assert_eq!(po["tx_budget_per_token"], 25 * 200);
        assert_eq!(meta["input_token_count"], 2);
        assert_eq!(meta["min_token_hits"], 2);
        assert_eq!(meta["input_tokens"][0]["token"], MINT_A);
        let scope = &meta["scope"];
        assert_eq!(scope["chain"], "solana");
        for key in [
            "program_id",
            "idl_commit",
            "idl_sha256",
            "qualification_version",
        ] {
            assert!(scope[key].as_str().is_some_and(|s| !s.is_empty()), "{key}");
        }
        assert!(!scope["variants"].as_array().unwrap().is_empty());
    }

    #[test]
    fn summary_marks_truncated_token_partial_with_counts() {
        let v = lines(false);
        println!("{}", v[1]);
        let s = &v[2];
        println!("{s}");
        assert_eq!(s["status"], "partial");
        assert_eq!(s["records"], 1);
        assert_eq!(s["tokens"][0]["status"], "ok");
        assert_eq!(s["tokens"][1]["status"], "truncated");
        assert_eq!(s["tokens"][1]["transactions_scanned"], 250);
        assert!(!s["incomplete_reasons"].as_array().unwrap().is_empty());
    }

    #[test]
    fn failed_token_counts_are_null_and_error_redacted() {
        let v = lines(true);
        let t = &v[2]["tokens"][1];
        assert_eq!(t["status"], "failed");
        assert!(t["transactions_scanned"].is_null());
        assert!(t["qualified_buyers"].is_null());
        assert!(t["diagnostics"].is_null());
        let text = v[2].to_string();
        assert!(!text.contains("SECRET99"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn not_scanned_token_has_null_counts_and_typed_reason() {
        let mut r = report(true);
        r.per_token[1].status = TokenScanStatus::Failed {
            kind: ScanFailureKind::BudgetExhausted { limit: 4 },
            message: "request budget exhausted".to_string(),
        };
        let stop = ScanStop::BudgetExhausted { limit: 4 };
        let mut third = r.per_token[1].clone();
        third.status = TokenScanStatus::NotScanned { reason: stop };
        r.per_token.push(third);
        let summary = run_summary_record("run-1", &r, true, &|t| t.to_string()).unwrap();
        let v = serde_json::to_value(summary).unwrap();
        let failed = &v["tokens"][1];
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["error_kind"]["kind"], "budget_exhausted");
        assert_eq!(failed["error_kind"]["limit"], 4);
        let skipped = &v["tokens"][2];
        assert_eq!(skipped["status"], "not_scanned");
        assert!(skipped["error"].is_null());
        assert!(skipped["transactions_scanned"].is_null());
        assert!(skipped["qualified_buyers"].is_null());
        assert!(skipped["diagnostics"].is_null());
        assert_eq!(skipped["stop_reason"]["kind"], "budget_exhausted");
        assert_eq!(skipped["stop_reason"]["limit"], 4);
        assert!(
            v["incomplete_reasons"]
                .to_string()
                .contains("not scanned: request budget exhausted (limit 4)")
        );
    }

    #[test]
    fn evm_wallet_is_0x_hex_with_profile_name() {
        let wallet = WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: AddressBytes::Evm([0x11; 20]),
        };
        let dto = serde_json::to_value(wallet_dto(&wallet).unwrap()).unwrap();
        assert_eq!(dto["chain"], "base");
        assert_eq!(dto["address"], "0x1111111111111111111111111111111111111111");
    }
}
