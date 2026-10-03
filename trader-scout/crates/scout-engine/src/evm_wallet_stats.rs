//! `wallet-stats` engine for EVM chains (ADR-020 step 2): per wallet, the
//! signer transactions of the window (explorer `txlist` -> RPC receipts and
//! block times), native legs resolved through the trace / archive sources
//! when the endpoint has them, then [`build_evm_wallet_ledger`]. The result
//! is the SAME `SolanaWalletStatsReport` card list the Solana runner
//! produces, so `wallet-stats` / `wallet-rank` render it with one output
//! stack.
//!
//! Per-wallet status mirrors the Solana runner (no wallet disappears):
//! `Error`, `NotScanned` (run stop), `Incomplete` (declared coverage gap),
//! `NoActivity`, `NoTradeActivity`, `Ok`. A native leg that no source could
//! observe is a legitimate Unknown (N/A on the card), NOT a coverage gap.
//! Coverage gaps (Incomplete): explorer txlist truncated by its page cap,
//! swap-shaped logs at emitters outside the verified venue set (an
//! unverified venue touched the wallet's history), malformed logs, trades
//! on an `IdlOnly` venue deployment.

use std::collections::BTreeMap;

use alloy_primitives::Address;
use scout_api::ProviderError;
use scout_core::RawEvmTransaction;
use scout_dex_evm::VENUE_DEPLOYMENTS;
use scout_providers::{BlockscoutEvmSource, EvmHistoryScanner, EvmSourceError, NativeLegResolver};
use tokio_util::sync::CancellationToken;

use crate::analysis_window::AnalysisWindow;
use crate::chain_display::{ChainDisplay, evm_key};
use crate::evm_trade_extraction::{
    EVM_TRADE_EXTRACTION_VERSION, EvmExtractionConfig, EvmTxOutcome, QuoteAsset, extract_evm_trades,
};
use crate::solana_buyer_intersect::SolanaProtocolScope;
use crate::solana_buyer_intersect::{
    ScanFailureKind, classify_provider_error, sanitize_provider_text,
};
use crate::solana_wallet_ledger::{
    EVM_WALLET_LEDGER_SCOPE, EVM_WALLET_LEDGER_VERSION, LedgerOptions, build_evm_wallet_ledger,
};
use crate::solana_wallet_stats::{
    SolanaWalletStats, SolanaWalletStatsReport, WalletScanStatus, failed_card, run_wallet_cards,
};

/// One venue deployment of the chain with its evidence level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmVenueInfo {
    pub venue: &'static str,
    /// PoolManager / factory address, lowercase `0x` hex.
    pub anchor: String,
    pub role: &'static str,
    pub verification: &'static str,
    /// `0` = not pinned.
    pub active_from_block: u64,
}

/// A pinned quote asset and the outcome of the live `decimals()` check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmQuoteInfo {
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
    /// `verified live` / `MISMATCH ...` / `not checked: reason`.
    pub decimals_check: String,
}

/// Facts about an EVM run, for scope text and `run_meta` (invariant #10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmRunInfo {
    pub chain: ChainDisplay,
    pub extraction_version: &'static str,
    pub ledger_version: &'static str,
    pub ledger_scope: &'static str,
    pub venues: Vec<EvmVenueInfo>,
    pub quote_assets: Vec<EvmQuoteInfo>,
    /// How the wallet's history was listed and assembled.
    pub history_source: String,
    /// `supported` / `unsupported (reason)` / `not checked`.
    pub trace: String,
    pub archive_state: String,
    /// Resolved block range of the window (`None`: no window / empty).
    pub block_range: Option<(u64, u64)>,
    /// Native-quoted trades by native-leg source, summed over wallets.
    pub native_leg_counts: BTreeMap<String, u64>,
}

impl EvmRunInfo {
    /// Static part from the extraction config (sources are filled by the
    /// runner / CLI).
    #[must_use]
    pub fn from_config(cfg: &EvmExtractionConfig, history_source: impl Into<String>) -> Self {
        let venues = VENUE_DEPLOYMENTS
            .iter()
            .filter(|d| d.chain_id == cfg.profile.chain_id)
            .map(|d| EvmVenueInfo {
                venue: d.venue.label(),
                anchor: format!("{:#x}", d.anchor),
                role: match d.role {
                    scout_dex_evm::AnchorRole::SwapEmitter => "swap_emitter",
                    scout_dex_evm::AnchorRole::PoolFactory => "pool_factory",
                },
                verification: d.verification.label(),
                active_from_block: d.active_from_block,
            })
            .collect();
        let quote_assets = cfg
            .profile
            .quote_assets
            .iter()
            .map(|q| EvmQuoteInfo {
                symbol: q.symbol.to_string(),
                address: format!("{:#x}", q.address),
                decimals: q.decimals,
                decimals_check: "not checked".to_string(),
            })
            .collect();
        Self {
            chain: ChainDisplay::evm(&cfg.profile),
            extraction_version: EVM_TRADE_EXTRACTION_VERSION,
            ledger_version: EVM_WALLET_LEDGER_VERSION,
            ledger_scope: EVM_WALLET_LEDGER_SCOPE,
            venues,
            quote_assets,
            history_source: history_source.into(),
            trace: "not checked".to_string(),
            archive_state: "not checked".to_string(),
            block_range: None,
            native_leg_counts: BTreeMap::new(),
        }
    }

    /// The stderr/`run_meta` text of the venue evidence, e.g.
    /// `uniswap_v4=FixtureVerified(0x8366..), uniswap_v3=IdlOnly(...)`.
    #[must_use]
    pub fn venues_text(&self) -> String {
        self.venues
            .iter()
            .map(|v| format!("{}={}({})", v.venue, v.verification, v.anchor))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The native-leg policy sentence of the scope text.
    #[must_use]
    pub fn native_leg_text(&self) -> String {
        format!(
            "native leg sources: trace {}; archive balance diff {}; otherwise Unknown \
             (NativeLegNotObserved, never zero)",
            self.trace, self.archive_state
        )
    }
}

/// Everything a stats run reads.
#[derive(Debug)]
pub struct EvmStatsSources<'a> {
    pub scanner: &'a EvmHistoryScanner,
    pub explorer: &'a BlockscoutEvmSource,
    /// `None` = native legs are not resolved (logs and `tx.value` only).
    pub resolver: Option<&'a NativeLegResolver>,
}

fn to_provider_error(e: EvmSourceError) -> ProviderError {
    match e {
        EvmSourceError::Provider(p) => p,
        other => ProviderError::Other(Box::new(other)),
    }
}

fn evm_error_card(wallet: Address, chain: ChainDisplay, e: &EvmSourceError) -> SolanaWalletStats {
    let mut card = failed_card(
        evm_key(wallet),
        chain,
        sanitize_provider_text(&e.to_string()),
    );
    if let EvmSourceError::Provider(p) = e {
        card.failure = Some(classify_provider_error(p));
    } else {
        card.failure = Some(ScanFailureKind::Other);
    }
    card
}

/// Run the EVM wallet-stats scan for `wallets` (duplicates collapsed, first
/// appearance order kept). `Err` only when the window cannot be resolved or
/// credentials are missing; every other failure lands on the wallet.
pub async fn run_evm_wallet_stats(
    cfg: &EvmExtractionConfig,
    sources: &EvmStatsSources<'_>,
    wallets: &[Address],
    window: &AnalysisWindow,
    concurrency: usize,
    cancel: CancellationToken,
) -> Result<SolanaWalletStatsReport, ProviderError> {
    let chain = ChainDisplay::evm(&cfg.profile);
    let bounds = window.bounds();
    let (since, until) = match bounds {
        Some((s, u)) => (u64::try_from(s).ok(), u64::try_from(u).ok()),
        None => (None, None),
    };
    // One window -> block resolution for the whole run.
    let blocks = sources
        .scanner
        .resolve_window(since, until)
        .await
        .map_err(to_provider_error)?;
    let keys: Vec<[u8; 32]> = wallets.iter().map(|w| evm_key(*w)).collect();
    let cards = run_wallet_cards(&keys, chain, concurrency, cancel, |key, token| async move {
        scan_evm_wallet(
            cfg,
            sources,
            crate::chain_display::evm_address_of_key(&key),
            window,
            blocks,
            &token,
        )
        .await
    })
    .await?;

    let mut info = EvmRunInfo::from_config(cfg, "explorer txlist (wallet-centric) + RPC receipts");
    info.block_range = blocks;
    if let Some(r) = sources.resolver
        && let Some(c) = r.capabilities()
    {
        info.trace = c.trace.describe();
        info.archive_state = c.archive_state.describe();
    }
    for w in &cards.wallets {
        if let Some(l) = &w.ledger
            && let Some(ev) = &l.evm
        {
            for (label, n) in &ev.native_leg_counts {
                *info
                    .native_leg_counts
                    .entry((*label).to_string())
                    .or_insert(0) += n;
            }
        }
    }
    Ok(SolanaWalletStatsReport {
        // Never printed for EVM runs (`evm` is checked first).
        scope: SolanaProtocolScope::pump_wallet_ledger(),
        wallets: cards.wallets,
        cancelled: cards.cancelled,
        stop: cards.stop,
        concurrency: cards.concurrency,
        evm: Some(info),
    })
}

async fn scan_evm_wallet(
    cfg: &EvmExtractionConfig,
    sources: &EvmStatsSources<'_>,
    wallet: Address,
    window: &AnalysisWindow,
    blocks: Option<(u64, u64)>,
    cancel: &CancellationToken,
) -> Result<Option<SolanaWalletStats>, ProviderError> {
    let chain = ChainDisplay::evm(&cfg.profile);
    let key = evm_key(wallet);
    let bounded = window.is_bounded();
    let empty_card = |status: WalletScanStatus| SolanaWalletStats {
        wallet: key,
        chain,
        status,
        transactions_scanned: Some(0),
        transactions_in_window: bounded.then_some(0),
        truncated: false,
        unexpected_payloads: 0,
        error: None,
        ledger: None,
        incomplete_reasons: Vec::new(),
        failure: None,
        not_scanned: None,
    };
    let Some((from, to)) = blocks else {
        // The window contains no block: no activity (an empty ledger keeps
        // the card shape).
        let ledger = build_evm_wallet_ledger(
            cfg,
            wallet,
            &[],
            LedgerOptions {
                left_censoring: bounded,
            },
        );
        return Ok(Some(match ledger {
            Ok(l) => SolanaWalletStats {
                ledger: Some(l),
                ..empty_card(WalletScanStatus::NoActivity)
            },
            Err(e) => failed_card(
                key,
                chain,
                sanitize_provider_text(&format!("ledger build failed: {e}")),
            ),
        }));
    };
    let scanned = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(None),
        r = sources.scanner.scan_wallet(sources.explorer, wallet, from, to) => r,
    };
    let out = match scanned {
        Ok(o) => o,
        Err(EvmSourceError::Provider(p @ ProviderError::ConfigurationRequired { .. })) => {
            return Err(p);
        }
        Err(e) => return Ok(Some(evm_error_card(wallet, chain, &e))),
    };
    let mut txs: Vec<RawEvmTransaction> = out.transactions;
    let scanned_n = u64::try_from(txs.len()).unwrap_or(u64::MAX);
    if bounded {
        txs.retain(|t| i64::try_from(t.block_time).is_ok_and(|ts| window.contains(ts)));
    }
    let in_window = u64::try_from(txs.len()).unwrap_or(u64::MAX);

    // Native legs: only for native-quoted trades whose internals no complete
    // source already provided.
    if let Some(resolver) = sources.resolver {
        let (ex, _) = extract_evm_trades(&txs, cfg, Some(wallet), None);
        let wanted: std::collections::BTreeSet<_> = ex
            .iter()
            .filter_map(|e| match &e.outcome {
                EvmTxOutcome::Trade(t) if t.quote == QuoteAsset::Native => Some(e.tx_hash),
                _ => None,
            })
            .collect();
        let indices: Vec<usize> = txs
            .iter()
            .enumerate()
            .filter(|(_, t)| wanted.contains(&t.hash) && t.internal_transfers.is_none())
            .map(|(i, _)| i)
            .collect();
        if !indices.is_empty() {
            let resolved = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                r = resolver.resolve_many(&mut txs, &indices) => r,
            };
            match resolved {
                Ok(_) => {}
                Err(EvmSourceError::Provider(p @ ProviderError::ConfigurationRequired { .. })) => {
                    return Err(p);
                }
                Err(e) => return Ok(Some(evm_error_card(wallet, chain, &e))),
            }
        }
    }

    let ledger = match build_evm_wallet_ledger(
        cfg,
        wallet,
        &txs,
        LedgerOptions {
            left_censoring: bounded,
        },
    ) {
        Ok(l) => l,
        Err(e) => {
            return Ok(Some(failed_card(
                key,
                chain,
                sanitize_provider_text(&format!("ledger build failed: {e}")),
            )));
        }
    };

    let mut reasons = Vec::new();
    if !out.txlist_complete {
        reasons.push(
            "explorer txlist hit its page cap: part of the wallet's transactions in the window \
             was not seen (figures are a subset; opening inventory may be unknown)"
                .to_string(),
        );
    }
    if let Some(ev) = &ledger.evm {
        if ev.ungated_swap_logs > 0 {
            reasons.push(format!(
                "{} swap-shaped log(s) at emitters outside the verified venue set (an unverified \
                 pool/venue touched this wallet's transactions; those trades are not booked)",
                ev.ungated_swap_logs
            ));
        }
        let malformed = ev
            .extraction
            .no_trade
            .get("malformed_log")
            .copied()
            .unwrap_or(0);
        if malformed > 0 {
            reasons.push(format!(
                "{malformed} transaction(s) with a malformed ERC-20/WETH/swap log were not booked"
            ));
        }
    }
    if ledger.trades.idl_only_variant > 0 {
        reasons.push(format!(
            "{} trade(s) via an IdlOnly venue deployment without a verified fixture",
            ledger.trades.idl_only_variant
        ));
    }
    let seen = if bounded { in_window } else { scanned_n };
    let status = if !reasons.is_empty() {
        WalletScanStatus::Incomplete
    } else if seen == 0 {
        WalletScanStatus::NoActivity
    } else if ledger.trades.buys + ledger.trades.sells == 0 {
        WalletScanStatus::NoTradeActivity
    } else {
        WalletScanStatus::Ok
    };
    // Which native source each trade used is in `ledger.evm.trades` and
    // `ledger.evm.native_leg_counts`.
    Ok(Some(SolanaWalletStats {
        wallet: key,
        chain,
        status,
        transactions_scanned: Some(scanned_n),
        transactions_in_window: bounded.then_some(in_window),
        truncated: !out.txlist_complete,
        unexpected_payloads: 0,
        error: None,
        ledger: Some(ledger),
        incomplete_reasons: reasons,
        failure: None,
        not_scanned: None,
    }))
}
