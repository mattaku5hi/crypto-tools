//! `buyer-intersect` engine for EVM chains (ADR-020 step 2, ADR-014 sides).
//!
//! Token-centric: for every input token the `Transfer` logs of the window are
//! scanned (`eth_getLogs`, adaptive range splitting), the receipts and
//! transactions of the distinct hashes are assembled, and the owner-keyed
//! trade extraction runs with the token as filter. A wallet qualifies for a
//! token with a booked trade of a selected side:
//!
//! * the wallet is the transaction signer (rule a), the swap evidence is a
//!   GATED venue event moving the token (rule b), the net flows are one
//!   token and one quote asset with opposite signs (rule c);
//! * ADR-009: only `FixtureVerified` venue deployments qualify. A trade via
//!   an `IdlOnly` deployment is counted (`idl_only_trades`) and makes the run
//!   partial; swap-shaped logs at emitters outside the verified venue set
//!   are counted per token and make the run partial (an unverified venue
//!   traded the token: the wallet set is a lower bound);
//! * the side is the sign of the wallet's own token delta. The consideration
//!   is NOT needed (and not resolved here): a native sell whose proceeds are
//!   unobserved still has a known side. Native legs are a ledger concern.
//!
//! Sequential over tokens (each scan already fans out over RPC requests with
//! its own bounded concurrency); after a run-terminal stop (budget / terminal
//! rate limit) the remaining tokens are `NotScanned` and no request is made.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256};
use scout_api::ProviderError;
use scout_core::{AddressBytes, AssetKey, WalletKey};
use scout_providers::{EvmHistoryScanner, EvmSourceError};

use crate::analysis_window::AnalysisWindow;
use crate::buyer_intersect::{BuyerIntersectReport, threshold_and_sort_matches};
use crate::evm_trade_extraction::{
    EvmExtractionConfig, EvmExtractionSummary, EvmTxOutcome, TradeSide, extract_evm_trades,
};
use crate::evm_wallet_stats::EvmRunInfo;
use crate::pool_admission::{DEFAULT_MAX_POOL_LOOKUPS, learn_pools};
use crate::solana_buyer_intersect::{
    ScanFailureKind, ScanStop, SideFilter, TokenScanStatus, classify_provider_error,
    sanitize_provider_text,
};
use scout_dex_evm::VenueVerification;

pub const EVM_BUYER_INTERSECT_VERSION: &str = "evm-buyer-intersect/1 (ADR-020 step 2 + ADR-014 sides: token-centric eth_getLogs scan, owner-keyed signer trades via FixtureVerified venue events only, side from the wallet's own token delta)";

/// First-qualifying evidence and count of one side of one (wallet, token):
/// "first" = lowest `(block, tx_index)` (scan-order independent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmSideEvidence {
    pub count: u64,
    pub tx_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub venue: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvmTokenSideHits {
    pub buy: Option<EvmSideEvidence>,
    pub sell: Option<EvmSideEvidence>,
}

/// Per-token result. Failure is explicit; counts of a failed token are
/// unknown (`None`), never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmTokenScanSummary {
    pub token: Address,
    pub status: TokenScanStatus,
    /// `Transfer` logs of the token in the window.
    pub transfer_logs: Option<u64>,
    pub transactions_scanned: Option<u64>,
    /// Extraction counters over the token's transactions.
    pub extraction: Option<EvmExtractionSummary>,
    pub qualified_buyers: u64,
    pub qualified_sellers: u64,
    pub qualified_wallets: u64,
    /// Booked trades of the selected side(s) that did NOT qualify because
    /// the venue deployment is `IdlOnly` (ADR-009). COVERAGE GAP.
    pub idl_only_trades: u64,
    /// Swap-shaped logs at non-gated emitters in the token's transactions.
    /// COVERAGE GAP (an unverified venue traded the token).
    pub ungated_swap_logs: u64,
    /// Swap-shaped logs at non-gated emitters that moved neither the token
    /// nor another token the signer traded (route hops): not a gap.
    pub ungated_hop_swap_logs: u64,
    /// v2/v3 emitters of this token's transactions admitted as pools of a
    /// pinned factory (chain `eth_call`s, see `pool_admission`).
    pub pools_admitted: u64,
    /// v2/v3 emitters checked on chain and refused (not a pool of a pinned
    /// factory): their swaps stay in `ungated_swap_logs`.
    pub pools_refused: u64,
    pub log_splits: u32,
}

impl EvmTokenScanSummary {
    #[must_use]
    pub fn is_failed(&self) -> bool {
        !matches!(self.status, TokenScanStatus::Ok)
    }
}

/// Whole-run result.
#[derive(Debug, Clone)]
pub struct EvmBuyerIntersectReport {
    pub base: BuyerIntersectReport,
    pub side: SideFilter,
    pub window: AnalysisWindow,
    /// Matched wallets' per-token side evidence.
    pub side_hits: BTreeMap<WalletKey, BTreeMap<AssetKey, EvmTokenSideHits>>,
    pub per_token: Vec<EvmTokenScanSummary>,
    pub info: EvmRunInfo,
    pub cancelled: bool,
    pub stop: Option<ScanStop>,
}

impl EvmBuyerIntersectReport {
    /// Why coverage is incomplete (empty = complete within the declared
    /// scope). Native-leg unknowns are not here: sides do not need them.
    #[must_use]
    pub fn incomplete_reasons(&self) -> Vec<String> {
        let mut out = Vec::new();
        for t in &self.per_token {
            let label = format!("{:#x}", t.token);
            match &t.status {
                TokenScanStatus::Failed { message, .. } => {
                    out.push(format!("token {label}: scan failed: {message}"));
                }
                TokenScanStatus::NotScanned { reason } => {
                    out.push(format!("token {label}: not scanned: {}", reason.describe()));
                }
                TokenScanStatus::Ok => {}
            }
            if t.idl_only_trades > 0 {
                out.push(format!(
                    "token {label}: {} trade(s) via an IdlOnly venue deployment never qualify \
                     (ADR-009): the wallet set is a lower bound",
                    t.idl_only_trades
                ));
            }
            if t.ungated_swap_logs > 0 {
                out.push(format!(
                    "token {label}: {} swap-shaped log(s) at emitters outside the verified venue \
                     set (pools not admitted by a pinned factory, or venues without one; {} \
                     emitter(s) refused on chain): trades there are not decoded",
                    t.ungated_swap_logs, t.pools_refused
                ));
            }
            if let Some(ex) = &t.extraction {
                let m = ex.no_trade.get("malformed_log").copied().unwrap_or(0);
                if m > 0 {
                    out.push(format!(
                        "token {label}: {m} transaction(s) with a malformed log were not booked"
                    ));
                }
            }
        }
        if self.cancelled {
            out.push("run cancelled before all tokens were scanned".to_string());
        }
        out
    }

    #[must_use]
    pub fn is_coverage_incomplete(&self) -> bool {
        !self.incomplete_reasons().is_empty()
    }
}

fn to_provider_error(e: EvmSourceError) -> ProviderError {
    match e {
        EvmSourceError::Provider(p) => p,
        other => ProviderError::Other(Box::new(other)),
    }
}

fn evm_token_address(asset: &AssetKey) -> Option<Address> {
    match asset {
        AssetKey::Token(_, AddressBytes::Evm(a)) => Some(Address::from(*a)),
        _ => None,
    }
}

/// Run the EVM trade-intersect over `input_tokens` (EVM token assets, in
/// input order). `Err` only when the window cannot be resolved.
///
/// # Errors
/// A [`ProviderError`] when the block window cannot be resolved.
pub async fn run_evm_buyer_intersect(
    cfg: &EvmExtractionConfig,
    scanner: &EvmHistoryScanner,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    side: SideFilter,
    window: &AnalysisWindow,
    info: EvmRunInfo,
) -> Result<EvmBuyerIntersectReport, ProviderError> {
    let bounds = window.bounds();
    let (since, until) = match bounds {
        Some((s, u)) => (u64::try_from(s).ok(), u64::try_from(u).ok()),
        None => (None, None),
    };
    let blocks = scanner
        .resolve_window(since, until)
        .await
        .map_err(to_provider_error)?;
    let mut info = info;
    info.block_range = blocks;

    // Pools admitted for one token stay admitted for the next (one gate per
    // run), and the lookup cap is per run.
    let mut run_cfg = cfg.clone();
    let mut lookups_left = DEFAULT_MAX_POOL_LOOKUPS;
    let mut per_token: Vec<EvmTokenScanSummary> = Vec::with_capacity(input_tokens.len());
    let mut hits: BTreeMap<WalletKey, BTreeMap<AssetKey, EvmTokenSideHits>> = BTreeMap::new();
    let mut stop: Option<ScanStop> = None;
    let mut seen: BTreeSet<AssetKey> = BTreeSet::new();
    let mut distinct: Vec<&AssetKey> = Vec::new();
    for t in input_tokens {
        if seen.insert(t.clone()) {
            distinct.push(t);
        }
    }

    for asset in &distinct {
        let Some(token) = evm_token_address(asset) else {
            continue;
        };
        let blank = |status: TokenScanStatus| EvmTokenScanSummary {
            token,
            status,
            transfer_logs: None,
            transactions_scanned: None,
            extraction: None,
            qualified_buyers: 0,
            qualified_sellers: 0,
            qualified_wallets: 0,
            idl_only_trades: 0,
            ungated_swap_logs: 0,
            ungated_hop_swap_logs: 0,
            pools_admitted: 0,
            pools_refused: 0,
            log_splits: 0,
        };
        if let Some(reason) = stop {
            per_token.push(blank(TokenScanStatus::NotScanned { reason }));
            continue;
        }
        let Some((from, to)) = blocks else {
            // No block in the window: scanned, nothing to see.
            let mut s = blank(TokenScanStatus::Ok);
            s.transfer_logs = Some(0);
            s.transactions_scanned = Some(0);
            per_token.push(s);
            continue;
        };
        match scanner.scan_token(token, from, to).await {
            Err(e) => {
                let kind = match &e {
                    EvmSourceError::Provider(p) => classify_provider_error(p),
                    _ => ScanFailureKind::Other,
                };
                if let Some(s) = kind.stop() {
                    stop = Some(s);
                }
                per_token.push(blank(TokenScanStatus::Failed {
                    kind,
                    message: sanitize_provider_text(&e.to_string()),
                }));
            }
            Ok(out) => {
                let txs = out.transactions;
                let in_window: Vec<_> = if bounds.is_some() {
                    txs.iter()
                        .filter(|t| i64::try_from(t.block_time).is_ok_and(|ts| window.contains(ts)))
                        .cloned()
                        .collect()
                } else {
                    txs.clone()
                };
                let admission =
                    match learn_pools(&mut run_cfg.gate, scanner.rpc(), &in_window, lookups_left)
                        .await
                    {
                        Ok(a) => a,
                        Err(e) => {
                            let kind = match &e {
                                EvmSourceError::Provider(p) => classify_provider_error(p),
                                _ => ScanFailureKind::Other,
                            };
                            if let Some(s) = kind.stop() {
                                stop = Some(s);
                            }
                            per_token.push(blank(TokenScanStatus::Failed {
                                kind,
                                message: sanitize_provider_text(&format!("pool admission: {e}")),
                            }));
                            continue;
                        }
                    };
                lookups_left =
                    lookups_left.saturating_sub(usize::try_from(admission.lookups).unwrap_or(0));
                let (extractions, summary) =
                    extract_evm_trades(&in_window, &run_cfg, None, Some(token));
                let mut s = blank(TokenScanStatus::Ok);
                s.pools_admitted = admission.admitted;
                s.pools_refused = u64::try_from(admission.refused.len()).unwrap_or(u64::MAX);
                s.transfer_logs = Some(u64::try_from(out.transfer_logs).unwrap_or(u64::MAX));
                s.transactions_scanned = Some(u64::try_from(txs.len()).unwrap_or(u64::MAX));
                s.log_splits = out.log_splits;
                s.ungated_swap_logs = summary.ungated_swap_logs;
                s.ungated_hop_swap_logs = summary.ungated_hop_swap_logs;
                let mut buyers: BTreeSet<Address> = BTreeSet::new();
                let mut sellers: BTreeSet<Address> = BTreeSet::new();
                for e in &extractions {
                    let EvmTxOutcome::Trade(t) = &e.outcome else {
                        continue;
                    };
                    if !side.includes(t.side) {
                        continue;
                    }
                    if t.venue_verification != VenueVerification::FixtureVerified {
                        s.idl_only_trades += 1;
                        continue;
                    }
                    let key = WalletKey {
                        chain: match asset {
                            AssetKey::Token(c, _) => c.clone(),
                            AssetKey::Native(c) => c.clone(),
                        },
                        address: AddressBytes::Evm(t.wallet.into_array()),
                    };
                    let slot = hits
                        .entry(key)
                        .or_default()
                        .entry((*asset).clone())
                        .or_default();
                    let target = match t.side {
                        TradeSide::Buy => {
                            buyers.insert(t.wallet);
                            &mut slot.buy
                        }
                        TradeSide::Sell => {
                            sellers.insert(t.wallet);
                            &mut slot.sell
                        }
                    };
                    let ev = EvmSideEvidence {
                        count: 1,
                        tx_hash: t.tx_hash,
                        block_number: t.block_number,
                        transaction_index: t.transaction_index,
                        venue: t.venue.label(),
                    };
                    *target = Some(match target.take() {
                        None => ev,
                        Some(prev) => {
                            let count = prev.count.saturating_add(1);
                            let first = if (ev.block_number, ev.transaction_index)
                                < (prev.block_number, prev.transaction_index)
                            {
                                ev
                            } else {
                                prev
                            };
                            EvmSideEvidence { count, ..first }
                        }
                    });
                }
                s.qualified_buyers = u64::try_from(buyers.len()).unwrap_or(u64::MAX);
                s.qualified_sellers = u64::try_from(sellers.len()).unwrap_or(u64::MAX);
                s.qualified_wallets =
                    u64::try_from(buyers.union(&sellers).count()).unwrap_or(u64::MAX);
                s.extraction = Some(summary);
                per_token.push(s);
            }
        }
    }

    // Threshold over distinct matched tokens per wallet.
    let candidates: Vec<(WalletKey, Vec<AssetKey>)> = hits
        .iter()
        .map(|(w, per_asset)| {
            let mut assets: Vec<AssetKey> = per_asset
                .iter()
                .filter(|(_, h)| h.buy.is_some() || h.sell.is_some())
                .map(|(a, _)| a.clone())
                .collect();
            assets.sort();
            (w.clone(), assets)
        })
        .collect();
    let matches = threshold_and_sort_matches(candidates, min_token_hits);
    let matched: BTreeSet<&WalletKey> = matches.iter().map(|m| &m.wallet).collect();
    hits.retain(|w, _| matched.contains(w));
    Ok(EvmBuyerIntersectReport {
        base: BuyerIntersectReport {
            matches,
            input_token_count: distinct.len(),
            min_token_hits,
            coverage_truncated: false,
        },
        side,
        window: *window,
        side_hits: hits,
        per_token,
        info,
        cancelled: false,
        stop,
    })
}
