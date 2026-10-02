//! `wallet-stats` engine for Solana: scans each distinct wallet's full
//! address history through a [`HistoryProvider`] and feeds it to
//! [`build_solana_wallet_ledger`] (pump.fun bonding-curve SOL ledger,
//! ADR-010).
//!
//! # Scan order and coverage (design decision)
//! The provider decides the order (`HeliusProvider::with_scan_order`);
//! the CLI selects `NewestFirst`. With a page budget that yields the most
//! recent window; `truncated` then means OLDER history was not seen, so
//! opening inventory is unknown (the ledger books any disposal beyond
//! observed inventory as an `Unknown`-basis lot, never a fabricated buy).
//! `OldestFirst` would give true openings but may never reach recent
//! activity, which is what a stats card is asked about. There is no
//! time-window request in `ScanRequest`, so `--since/--until/--period`
//! cannot be honoured exactly and are not offered.
//!
//! # Per-wallet status (CLI.md §5: no wallet disappears)
//! * `Error`: the scan or the ledger build failed; no figures. One
//!   wallet's failure does not abort the run (only
//!   `ConfigurationRequired` does: that is infrastructure, not a wallet).
//! * `Incomplete`: scanned, but coverage has a declared gap (truncation,
//!   malformed pump instructions, orphan events, `IdlOnly` variant
//!   trades, non-transaction payloads). Figures are a known subset.
//! * `NoActivity`: the scan returned zero transactions.
//! * `NoPumpActivity`: transactions, but no decoded pump trade and no
//!   attributable failed-trade fee.
//! * `Ok`: complete within the declared protocol scope.
//!
//! Unknown PnL (`ClosedUnknown` episodes, unexplained flows) is a
//! legitimate N/A flagged in the report, NOT an operational gap.

use std::collections::BTreeSet;

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ProviderError, ScanRequest, ScanTask};
use scout_core::{AddressBytes, RawPayload, RawSolanaTransaction, SolanaPubkey, WalletKey};
use scout_dex_solana::BondingCurveBuyDecoder;
use tokio_util::sync::CancellationToken;

use crate::solana_buy_qualification::solana_mainnet_chain;
use crate::solana_buyer_intersect::{SolanaProtocolScope, sanitize_provider_text};
use crate::solana_wallet_ledger::{SolanaWalletLedgerReport, build_solana_wallet_ledger};

/// Per-wallet outcome class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletScanStatus {
    Ok,
    NoActivity,
    NoPumpActivity,
    Incomplete,
    Error,
}

impl WalletScanStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoActivity => "no_activity",
            Self::NoPumpActivity => "no_pump_activity",
            Self::Incomplete => "incomplete",
            Self::Error => "error",
        }
    }
}

/// One wallet's card source.
#[derive(Debug, Clone)]
pub struct SolanaWalletStats {
    pub wallet: SolanaPubkey,
    pub status: WalletScanStatus,
    /// `None` when the scan failed (unknown, not zero).
    pub transactions_scanned: Option<u64>,
    /// Provider stopped with an unconsumed cursor.
    pub truncated: bool,
    pub unexpected_payloads: u64,
    /// Sanitized error text for `Error`.
    pub error: Option<String>,
    /// Present unless `Error`.
    pub ledger: Option<SolanaWalletLedgerReport>,
    /// Why coverage is incomplete (empty unless `Incomplete`; for
    /// `Error` the reason is `error`).
    pub incomplete_reasons: Vec<String>,
}

impl SolanaWalletStats {
    /// Wallet identity as a `WalletKey` (Solana mainnet).
    #[must_use]
    pub fn wallet_key(&self) -> WalletKey {
        WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana(self.wallet),
        }
    }

    /// Coverage is complete within the declared scope.
    #[must_use]
    pub fn coverage_complete(&self) -> bool {
        !matches!(
            self.status,
            WalletScanStatus::Incomplete | WalletScanStatus::Error
        )
    }
}

/// Whole-run result: one entry per distinct input wallet, input order.
#[derive(Debug, Clone)]
pub struct SolanaWalletStatsReport {
    pub scope: SolanaProtocolScope,
    pub wallets: Vec<SolanaWalletStats>,
    pub cancelled: bool,
}

impl SolanaWalletStatsReport {
    #[must_use]
    pub fn incomplete_reasons(&self) -> Vec<String> {
        let mut out = Vec::new();
        for w in &self.wallets {
            let label = bs58::encode(w.wallet).into_string();
            if let Some(e) = &w.error {
                out.push(format!("wallet {label}: scan failed: {e}"));
            }
            for r in &w.incomplete_reasons {
                out.push(format!("wallet {label}: {r}"));
            }
        }
        if self.cancelled {
            out.push("run cancelled before all wallets were scanned".to_string());
        }
        out
    }

    #[must_use]
    pub fn is_coverage_incomplete(&self) -> bool {
        self.cancelled || self.wallets.iter().any(|w| !w.coverage_complete())
    }

    /// Every wallet failed (nothing usable was observed): exit 4, not 3.
    #[must_use]
    pub fn all_failed(&self) -> bool {
        !self.wallets.is_empty()
            && self
                .wallets
                .iter()
                .all(|w| w.status == WalletScanStatus::Error)
    }
}

/// Run the Solana wallet-stats scan for `wallets` (duplicates collapsed,
/// first-appearance order kept). Returns `Err` only for
/// `ConfigurationRequired`; every other failure lands on the wallet.
pub async fn run_solana_wallet_stats(
    provider: &dyn HistoryProvider,
    wallets: &[SolanaPubkey],
    decoder: &BondingCurveBuyDecoder,
    cancel: CancellationToken,
) -> Result<SolanaWalletStatsReport, ProviderError> {
    let mut seen: BTreeSet<SolanaPubkey> = BTreeSet::new();
    let distinct: Vec<SolanaPubkey> = wallets
        .iter()
        .copied()
        .filter(|w| seen.insert(*w))
        .collect();

    let mut out: Vec<SolanaWalletStats> = Vec::with_capacity(distinct.len());
    let mut cancelled = false;
    for wallet in distinct {
        if cancelled || cancel.is_cancelled() {
            cancelled = true;
            out.push(failed_card(
                wallet,
                "not scanned: run cancelled".to_string(),
            ));
            continue;
        }
        match scan_wallet(provider, wallet, decoder, &cancel).await? {
            Some(card) => out.push(card),
            None => {
                cancelled = true;
                out.push(failed_card(
                    wallet,
                    "scan interrupted: run cancelled".to_string(),
                ));
            }
        }
    }
    Ok(SolanaWalletStatsReport {
        scope: SolanaProtocolScope::pump_bonding_curve(),
        wallets: out,
        cancelled,
    })
}

fn failed_card(wallet: SolanaPubkey, error: String) -> SolanaWalletStats {
    SolanaWalletStats {
        wallet,
        status: WalletScanStatus::Error,
        transactions_scanned: None,
        truncated: false,
        unexpected_payloads: 0,
        error: Some(error),
        ledger: None,
        incomplete_reasons: Vec::new(),
    }
}

/// `Ok(None)` = cancelled mid-scan.
async fn scan_wallet(
    provider: &dyn HistoryProvider,
    wallet: SolanaPubkey,
    decoder: &BondingCurveBuyDecoder,
    cancel: &CancellationToken,
) -> Result<Option<SolanaWalletStats>, ProviderError> {
    let request = ScanRequest::WalletActivity {
        wallet: WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana(wallet),
        },
    };
    match provider.plan(&request).await {
        Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
        Err(err) => {
            return Ok(Some(failed_card(
                wallet,
                sanitize_provider_text(&err.to_string()),
            )));
        }
        Ok(_) => {}
    }
    let mut stream = provider.scan(
        ScanTask {
            request,
            description: "wallet-stats: wallet activity".to_string(),
        },
        cancel.clone(),
    );
    let mut txs: Vec<RawSolanaTransaction> = Vec::new();
    let mut truncated = false;
    let mut unexpected = 0u64;
    while let Some(item) = stream.next().await {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        match item {
            Ok(envelope) => {
                truncated = truncated || envelope.truncated;
                match envelope.payload {
                    RawPayload::SolanaTransaction(tx) => txs.push(tx),
                    _ => unexpected = unexpected.saturating_add(1),
                }
            }
            Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
            Err(err) => {
                return Ok(Some(failed_card(
                    wallet,
                    sanitize_provider_text(&err.to_string()),
                )));
            }
        }
    }
    drop(stream);
    if cancel.is_cancelled() {
        return Ok(None);
    }

    let scanned = u64::try_from(txs.len()).unwrap_or(u64::MAX);
    let ledger = match build_solana_wallet_ledger(&wallet, &txs, decoder) {
        Ok(l) => l,
        Err(err) => {
            return Ok(Some(failed_card(
                wallet,
                sanitize_provider_text(&format!("ledger build failed: {err}")),
            )));
        }
    };

    let mut reasons = Vec::new();
    if truncated {
        reasons.push(
            "provider page budget exhausted with history remaining: older transactions not \
             seen, opening inventory unknown (figures cover the newest window only)"
                .to_string(),
        );
    }
    let d = &ledger.diagnostics;
    if d.malformed_trade_instructions > 0 {
        reasons.push(format!(
            "{} malformed pump trade instruction(s) not decoded",
            d.malformed_trade_instructions
        ));
    }
    if d.orphan_trade_events > 0 {
        reasons.push(format!(
            "{} trade event(s) not claimed by any decoded trade",
            d.orphan_trade_events
        ));
    }
    if ledger.trades.idl_only_variant > 0 {
        reasons.push(format!(
            "{} trade(s) via IdlOnly variant without verified fixture",
            ledger.trades.idl_only_variant
        ));
    }
    if unexpected > 0 {
        reasons.push(format!(
            "{unexpected} envelope(s) were not Solana transactions"
        ));
    }

    let status = if !reasons.is_empty() {
        WalletScanStatus::Incomplete
    } else if scanned == 0 {
        WalletScanStatus::NoActivity
    } else if ledger.trades.buys + ledger.trades.sells == 0 && ledger.failed_trade_fee_txs == 0 {
        WalletScanStatus::NoPumpActivity
    } else {
        WalletScanStatus::Ok
    };
    Ok(Some(SolanaWalletStats {
        wallet,
        status,
        transactions_scanned: Some(scanned),
        truncated,
        unexpected_payloads: unexpected,
        error: None,
        ledger: Some(ledger),
        incomplete_reasons: reasons,
    }))
}

/// Exact decimal rendering of `units / 10^scale` with `scale` fractional
/// digits; sign only when negative and the value is non-zero.
#[must_use]
pub fn format_scaled_decimal(units: i128, scale: u32) -> String {
    let base = 10u128.pow(scale);
    let mag = units.unsigned_abs();
    let (whole, frac) = (mag.div_euclid(base), mag.rem_euclid(base));
    let sign = if units < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{whole}");
    }
    format!(
        "{sign}{whole}.{frac:0width$}",
        width = usize::try_from(scale).unwrap_or(0)
    )
}

/// Lamports -> SOL with exactly 9 fractional digits (`0.157000000`).
#[must_use]
pub fn lamports_to_sol_string(lamports: i128) -> String {
    format_scaled_decimal(lamports, 9)
}

/// `numerator / denominator` rounded half up to `places` digits using
/// integer arithmetic only; `None` when the denominator is zero.
#[must_use]
pub fn rational_to_decimal_string(numerator: u64, denominator: u64, places: u32) -> Option<String> {
    if denominator == 0 {
        return None;
    }
    let scale = 10u128.pow(places);
    let scaled = (u128::from(numerator) * scale * 2 + u128::from(denominator))
        .div_euclid(u128::from(denominator) * 2);
    i128::try_from(scaled)
        .ok()
        .map(|s| format_scaled_decimal(s, places))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamports_format_exact() {
        assert_eq!(lamports_to_sol_string(0), "0.000000000");
        assert_eq!(lamports_to_sol_string(157_000_000), "0.157000000");
        assert_eq!(lamports_to_sol_string(1), "0.000000001");
        assert_eq!(lamports_to_sol_string(-1), "-0.000000001");
        assert_eq!(lamports_to_sol_string(-1_500_000_000), "-1.500000000");
        assert_eq!(lamports_to_sol_string(12_345_678_901), "12.345678901");
        assert_eq!(
            lamports_to_sol_string(i128::MIN),
            "-170141183460469231731687303715.884105728"
        );
        assert_eq!(
            lamports_to_sol_string(i128::MAX),
            "170141183460469231731687303715.884105727"
        );
    }

    #[test]
    fn rational_rounds_half_up_and_guards_zero() {
        assert_eq!(rational_to_decimal_string(1, 0, 2), None);
        assert_eq!(rational_to_decimal_string(1, 3, 2).unwrap(), "0.33");
        assert_eq!(rational_to_decimal_string(2, 3, 2).unwrap(), "0.67");
        assert_eq!(rational_to_decimal_string(1, 8, 2).unwrap(), "0.13");
        assert_eq!(rational_to_decimal_string(0, 5, 2).unwrap(), "0.00");
        assert_eq!(rational_to_decimal_string(7, 1, 2).unwrap(), "7.00");
    }
}
