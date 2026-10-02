//! First real Solana vertical slice for `buyer-intersect`: pump.fun
//! bonding-curve buys only. See `solana_buy_qualification` for the
//! per-transaction rule (ADR-003).
//!
//! Scope honesty (AGENTS.md invariant 10): only buys executed through
//! the bonding-curve program are recognized. Buys on PumpSwap AMM,
//! Raydium, Jupiter-routed legs that do not CPI the bonding curve, or
//! any other venue are NOT decoded, so for tokens that migrated the
//! buyer set is a LOWER BOUND, not the full buyer set. Counts are
//! "observed", never "total".
//!
//! Coverage policy (ADR-005, ACCEPTANCE B08): the declared input-token
//! count N never shrinks. A per-token scan failure (other than
//! `ConfigurationRequired`, which aborts the run as infrastructure
//! unavailable) is recorded on that token and makes the run incomplete
//! (exit 3), not an error. If NO token produced any transaction and at
//! least one failed, the first error is returned (nothing usable was
//! observed; exit 4). Truncation, malformed bonding-curve instructions,
//! unexpected payload shapes, delta overflow and out-of-scope slots all
//! make coverage incomplete.
//!
//! Run-terminal errors (typed, [`ScanStop`]): a `RequestBudgetExhausted`
//! (user-chosen `--max-requests`) or a `RateLimited` (Retry-After above
//! the transport cap) will fail every further request too. On the first
//! one the run stops: the interrupted token is `Failed` with its kind,
//! every remaining token is `NotScanned { reason }`, and NO further
//! provider request is issued. All other per-token errors keep going
//! with the next token. Classification of a `ProviderError` lives in
//! one place, [`classify_provider_error`].
//!
//! Bounded work: tokens are scanned sequentially, each envelope is
//! consumed and dropped before the next is polled; retained state is
//! the (wallet, input-token) hit set plus capped diagnostic samples.

use std::collections::{BTreeMap, BTreeSet};

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ProviderError, ScanRequest, ScanTask};
use scout_core::{AddressBytes, AssetKey, ChainFamily, RawPayload, WalletKey};
use scout_dex_solana::{
    PUMP_AMM_IDL_COMMIT, PUMP_AMM_IDL_SHA256, PUMP_AMM_PROGRAM_ID, PUMP_IDL_SHA256,
    PumpTradeVariant,
};
use scout_rpc::RequestBudgetExhausted;
use tokio_util::sync::CancellationToken;

use crate::buyer_intersect::{BuyerIntersectReport, threshold_and_sort_matches};
use crate::solana_buy_qualification::{
    PUMP_BONDING_CURVE_IDL_COMMIT, PUMP_BONDING_CURVE_PROGRAM_ID, SOLANA_BUY_QUALIFICATION_VERSION,
    TxQualificationDiagnostics, VariantPolicy, default_variant_policy, pump_bonding_curve_decoder,
    qualify_bonding_curve_buys_with_policy,
};

/// Max malformed-instruction reason samples retained per run.
const MAX_MALFORMED_SAMPLES: usize = 5;
/// Max unknown-discriminator samples retained per run.
const MAX_UNKNOWN_DISCRIMINATOR_SAMPLES: usize = 5;
/// Max length (chars) of any retained provider-derived string.
const MAX_TEXT_LEN: usize = 300;

/// Declared protocol scope of a Solana run (invariant 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaProtocolScope {
    pub program_id: &'static str,
    pub idl_commit: &'static str,
    /// sha256 of the committed IDL file the decoder tables derive from.
    pub idl_sha256: &'static str,
    pub qualification_version: &'static str,
    /// Second decoded program (PumpSwap AMM, ADR-012) of the wallet-ledger
    /// scope; `None` for the bonding-curve-only buyer scope.
    pub amm_program_id: Option<&'static str>,
    pub amm_idl_commit: Option<&'static str>,
    pub amm_idl_sha256: Option<&'static str>,
    /// What is recognized.
    pub recognized: &'static str,
    /// What is explicitly NOT decoded.
    pub not_decoded: &'static str,
}

impl SolanaProtocolScope {
    /// Every recognized trade variant with its verification status
    /// (`(idl name, side, status)`), for scope output.
    #[must_use]
    pub fn variants() -> Vec<(&'static str, &'static str, &'static str)> {
        PumpTradeVariant::ALL
            .iter()
            .map(|v| {
                let side = match v.side() {
                    scout_dex_solana::TradeSide::Buy => "buy",
                    scout_dex_solana::TradeSide::Sell => "sell",
                };
                (v.name(), side, v.verification().label())
            })
            .collect()
    }

    #[must_use]
    pub const fn pump_bonding_curve() -> Self {
        Self {
            program_id: PUMP_BONDING_CURVE_PROGRAM_ID,
            idl_commit: PUMP_BONDING_CURVE_IDL_COMMIT,
            idl_sha256: PUMP_IDL_SHA256,
            qualification_version: SOLANA_BUY_QUALIFICATION_VERSION,
            amm_program_id: None,
            amm_idl_commit: None,
            amm_idl_sha256: None,
            recognized: "pump.fun bonding-curve buy via a FixtureVerified variant (program \
                         6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P) with positive owner-keyed \
                         net token delta in the same transaction; IdlOnly buy variants are decoded \
                         but never enter the buyer set (reported as unverified, coverage incomplete)",
            not_decoded: "PumpSwap AMM, Raydium, Orca, Meteora and every other venue; \
                          buyer set is a lower bound for tokens that migrated off the bonding curve",
        }
    }

    /// Scope of the wallet ledger (ADR-010 + ADR-012): the bonding curve and
    /// the PumpSwap AMM, each with its own IDL pin.
    #[must_use]
    pub const fn pump_wallet_ledger() -> Self {
        Self {
            program_id: PUMP_BONDING_CURVE_PROGRAM_ID,
            idl_commit: PUMP_BONDING_CURVE_IDL_COMMIT,
            idl_sha256: PUMP_IDL_SHA256,
            qualification_version: crate::solana_wallet_ledger::SOLANA_WALLET_LEDGER_VERSION,
            amm_program_id: Some(PUMP_AMM_PROGRAM_ID),
            amm_idl_commit: Some(PUMP_AMM_IDL_COMMIT),
            amm_idl_sha256: Some(PUMP_AMM_IDL_SHA256),
            recognized: "pump.fun bonding-curve trades (program \
                         6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P, IDL e0687ae9) and \
                         PumpSwap AMM trades (program pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA, \
                         IDL e0687ae9; buy, buy_exact_quote_in, sell; wSOL-quoted normal and \
                         reversed pools), each priced from its paired event; PumpSwap trades are \
                         attributed only when the wallet's own owner-keyed legs reconcile \
                         (ADR-012); one FIFO per (wallet, mint) across both venues",
            not_decoded: "Raydium, Meteora, Orca, Jupiter-only routes not ending in the two \
                          programs above, PumpSwap liquidity/non-trade instructions and every \
                          other venue; token movements there are continuity breaks (Unknown), \
                          never zero PnL; bot/platform fees stay outside trade PnL (ADR-010 §5)",
        }
    }
}

/// Why a run stopped before scanning every token. Both causes are
/// terminal for the whole run, not just one token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStop {
    /// The client's total request budget (`--max-requests`) is spent.
    BudgetExhausted { limit: u64 },
    /// The server asked to wait longer than the transport's cap.
    RateLimited { retry_after_secs: Option<u64> },
}

/// Classified cause of a failed token scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFailureKind {
    BudgetExhausted {
        limit: u64,
    },
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    /// Any other provider error; the run continues with the next token.
    Other,
}

impl ScanFailureKind {
    /// The run-terminal stop this failure implies, if any.
    #[must_use]
    pub const fn stop(self) -> Option<ScanStop> {
        match self {
            Self::BudgetExhausted { limit } => Some(ScanStop::BudgetExhausted { limit }),
            Self::RateLimited { retry_after_secs } => {
                Some(ScanStop::RateLimited { retry_after_secs })
            }
            Self::Other => None,
        }
    }
}

/// The ONE place a `ProviderError` is classified (budget exhaustion is
/// a boxed `scout_rpc::RequestBudgetExhausted`).
#[must_use]
pub fn classify_provider_error(err: &ProviderError) -> ScanFailureKind {
    match err {
        ProviderError::RateLimited { retry_after } => ScanFailureKind::RateLimited {
            retry_after_secs: retry_after.map(|d| d.as_secs()),
        },
        ProviderError::Other(inner) => inner
            .downcast_ref::<RequestBudgetExhausted>()
            .map_or(ScanFailureKind::Other, |e| {
                ScanFailureKind::BudgetExhausted { limit: e.limit }
            }),
        _ => ScanFailureKind::Other,
    }
}

/// Per-token outcome. Never inferred from counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenScanStatus {
    /// Scan ended without a provider error (may still be `truncated`).
    Ok,
    /// Scan (or plan) failed; `message` is sanitized.
    Failed {
        kind: ScanFailureKind,
        message: String,
    },
    /// No request was issued: the run stopped earlier.
    NotScanned { reason: ScanStop },
}

/// Per-token scan result. Failure/truncation are explicit, never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaTokenScanSummary {
    pub asset: AssetKey,
    /// `RawPayload::SolanaTransaction` envelopes qualified for this token.
    pub transactions_scanned: u64,
    pub truncated: bool,
    pub status: TokenScanStatus,
    /// Distinct wallets with a qualified buy of THIS token so far.
    pub qualified_buyers: u64,
    pub diagnostics: TxQualificationDiagnostics,
    /// Positive-delta owners of THIS input token with no decoded buy.
    pub positive_delta_without_instruction: u64,
    /// Envelopes whose payload was not a Solana transaction. COVERAGE GAP.
    pub unexpected_payloads: u64,
}

/// Full Solana run result. `base` is the same shape the pre-built-flow
/// path returns, so output code is shared.
#[derive(Debug, Clone)]
pub struct SolanaBuyerIntersectReport {
    pub base: BuyerIntersectReport,
    pub scope: SolanaProtocolScope,
    pub per_token: Vec<SolanaTokenScanSummary>,
    /// Sum over tokens.
    pub diagnostics: TxQualificationDiagnostics,
    pub positive_delta_without_instruction: u64,
    pub unexpected_payloads: u64,
    /// Up to 5 sanitized malformed-instruction reasons.
    pub malformed_samples: Vec<String>,
    /// Up to 5 hex discriminators of unknown instructions of the program.
    pub unknown_discriminator_samples: Vec<String>,
    pub cancelled: bool,
    /// Set when the run stopped early on a run-terminal error; tokens
    /// after the interrupted one are `NotScanned`.
    pub stop: Option<ScanStop>,
}

impl SolanaBuyerIntersectReport {
    /// Reasons coverage is incomplete; empty means complete WITHIN the
    /// declared protocol scope (not complete buyer history).
    #[must_use]
    pub fn incomplete_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        for token in &self.per_token {
            if token.truncated {
                reasons.push(format!(
                    "token {}: provider reported unconsumed pagination (truncated)",
                    token.asset_label()
                ));
            }
            match &token.status {
                TokenScanStatus::Ok => {}
                TokenScanStatus::Failed { message, .. } => reasons.push(format!(
                    "token {}: scan failed: {message}",
                    token.asset_label()
                )),
                TokenScanStatus::NotScanned { reason } => reasons.push(format!(
                    "token {}: not scanned: {}",
                    token.asset_label(),
                    reason.describe()
                )),
            }
        }
        let d = &self.diagnostics;
        if d.malformed_instructions > 0 {
            reasons.push(format!(
                "{} malformed bonding-curve instruction(s) not decoded",
                d.malformed_instructions
            ));
        }
        if d.unknown_discriminator_instructions > 0 {
            reasons.push(format!(
                "{} instruction(s) of the pump.fun program have a discriminator not in the \
                 pinned IDL (unknown format, not decoded)",
                d.unknown_discriminator_instructions
            ));
        }
        for variant in PumpTradeVariant::ALL {
            let n = d
                .unverified_variant_buys
                .get(variant.index())
                .copied()
                .unwrap_or(0);
            if n > 0 {
                reasons.push(format!(
                    "{n} buy(s) via IdlOnly variant {} would qualify but the variant has no \
                     verified fixture; buyer set is a lower bound",
                    variant.name()
                ));
            }
        }
        if self.unexpected_payloads > 0 {
            reasons.push(format!(
                "{} envelope(s) were not Solana transactions",
                self.unexpected_payloads
            ));
        }
        if d.delta_overflow_transactions > 0 {
            reasons.push(format!(
                "{} transaction(s) with out-of-range balance deltas",
                d.delta_overflow_transactions
            ));
        }
        if d.out_of_scope_slot_transactions > 0 {
            reasons.push(format!(
                "{} transaction(s) outside decoder scope slot range",
                d.out_of_scope_slot_transactions
            ));
        }
        if self.cancelled {
            reasons.push("run cancelled before all tokens were scanned".to_string());
        }
        reasons
    }

    #[must_use]
    pub fn is_coverage_incomplete(&self) -> bool {
        !self.incomplete_reasons().is_empty()
    }
}

impl ScanStop {
    /// Short human text (no secrets: numbers only).
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::BudgetExhausted { limit } => {
                format!("request budget exhausted (limit {limit})")
            }
            Self::RateLimited {
                retry_after_secs: Some(s),
            } => format!("rate limited (server asked to retry after {s}s)"),
            Self::RateLimited {
                retry_after_secs: None,
            } => "rate limited (no Retry-After)".to_string(),
        }
    }
}

impl SolanaTokenScanSummary {
    fn new(asset: AssetKey, status: TokenScanStatus) -> Self {
        Self {
            asset,
            transactions_scanned: 0,
            truncated: false,
            status,
            qualified_buyers: 0,
            diagnostics: TxQualificationDiagnostics::default(),
            positive_delta_without_instruction: 0,
            unexpected_payloads: 0,
        }
    }

    /// True when the token was not scanned or its scan failed: counts
    /// are unknown, not zero.
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        !matches!(self.status, TokenScanStatus::Ok)
    }

    /// Sanitized failure text, if the scan failed.
    #[must_use]
    pub fn error_text(&self) -> Option<&str> {
        match &self.status {
            TokenScanStatus::Failed { message, .. } => Some(message),
            _ => None,
        }
    }

    /// Display label (base58 mint when available).
    #[must_use]
    pub fn asset_label(&self) -> String {
        match &self.asset {
            AssetKey::Token(_, address) => address.to_string(),
            AssetKey::Native(_) => "native".to_string(),
        }
    }
}

/// Remove API-key query values and control characters, and cap length.
/// Provider/transport error text can embed request URLs.
#[must_use]
pub fn sanitize_provider_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_TEXT_LEN));
    let mut rest = raw;
    while let Some(pos) = rest.to_ascii_lowercase().find("api-key=") {
        let (head, tail) = rest.split_at(pos);
        out.push_str(head);
        out.push_str("api-key=<redacted>");
        let skip = tail
            .char_indices()
            .skip("api-key=".len())
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
            .map_or(tail.len(), |(i, _)| i);
        rest = tail.get(skip..).unwrap_or("");
    }
    out.push_str(rest);
    out.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TEXT_LEN)
        .collect()
}

/// Run `buyer-intersect` for Solana input tokens against `provider`,
/// recognizing pump.fun bonding-curve buys only.
///
/// Errors: `Unsupported` for a non-Solana/native input, `ConfigurationRequired`
/// (propagated at once), or the first scan error when no token yielded
/// any transaction. Otherwise coverage problems are reported in the
/// returned report, never as an `Err`.
pub async fn run_solana_buyer_intersect(
    provider: &dyn HistoryProvider,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    cancel: CancellationToken,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    run_solana_buyer_intersect_with_policy(
        provider,
        input_tokens,
        min_token_hits,
        cancel,
        default_variant_policy,
    )
    .await
}

/// As [`run_solana_buyer_intersect`] with an injected variant policy.
/// Production callers use [`run_solana_buyer_intersect`] (static spec
/// table); this exists so the `IdlOnly` path stays testable.
pub async fn run_solana_buyer_intersect_with_policy(
    provider: &dyn HistoryProvider,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    cancel: CancellationToken,
    policy: VariantPolicy,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    let decoder = pump_bonding_curve_decoder().map_err(|e| ProviderError::Other(Box::new(e)))?;

    // Distinct tokens, input order preserved; N is this count.
    let mut seen: BTreeSet<AssetKey> = BTreeSet::new();
    let mut tokens: Vec<AssetKey> = Vec::new();
    for token in input_tokens {
        match token {
            AssetKey::Token(chain, AddressBytes::Solana(_))
                if chain.family == ChainFamily::Solana =>
            {
                if seen.insert(token.clone()) {
                    tokens.push(token.clone());
                }
            }
            _ => {
                return Err(ProviderError::Unsupported {
                    capability: "solana buyer-intersect requires Solana SPL mint inputs"
                        .to_string(),
                });
            }
        }
    }

    let mut hits: BTreeMap<WalletKey, BTreeSet<AssetKey>> = BTreeMap::new();
    let mut per_token: Vec<SolanaTokenScanSummary> = Vec::with_capacity(tokens.len());
    let mut malformed_samples: Vec<String> = Vec::new();
    let mut unknown_discriminator_samples: Vec<String> = Vec::new();
    let mut first_error: Option<ProviderError> = None;
    let mut cancelled = false;
    let mut stop: Option<ScanStop> = None;

    'tokens: for (index, token) in tokens.iter().enumerate() {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let mut summary = SolanaTokenScanSummary::new(token.clone(), TokenScanStatus::Ok);
        let mut token_buyers: BTreeSet<WalletKey> = BTreeSet::new();

        let request = ScanRequest::TokenMarketActivity {
            asset: token.clone(),
        };
        let mut failure: Option<ProviderError> = None;
        match provider.plan(&request).await {
            Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
            Err(err) => failure = Some(err),
            Ok(_) => {
                let mut stream = provider.scan(
                    ScanTask {
                        request,
                        description: "buyer-intersect: token market activity".to_string(),
                    },
                    cancel.clone(),
                );
                while let Some(item) = stream.next().await {
                    if cancel.is_cancelled() {
                        cancelled = true;
                        per_token.push(finish(summary, &token_buyers));
                        break 'tokens;
                    }
                    let envelope = match item {
                        Ok(envelope) => envelope,
                        Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
                        Err(err) => {
                            failure = Some(err);
                            break;
                        }
                    };
                    summary.truncated = summary.truncated || envelope.truncated;
                    let RawPayload::SolanaTransaction(tx) = &envelope.payload else {
                        summary.unexpected_payloads = summary.unexpected_payloads.saturating_add(1);
                        continue;
                    };
                    summary.transactions_scanned = summary.transactions_scanned.saturating_add(1);
                    let q = qualify_bonding_curve_buys_with_policy(tx, &decoder, policy);
                    // Unverified-variant buys only matter for declared
                    // input mints; a buy of some other mint seen in the
                    // same transaction does not make THIS run's buyer
                    // set a lower bound.
                    let mut tx_diagnostics = q.diagnostics;
                    tx_diagnostics.unverified_variant_buys = Default::default();
                    for unverified in &q.unverified_buys {
                        let is_input_mint = tokens.iter().any(|t| {
                            matches!(t, AssetKey::Token(_, AddressBytes::Solana(m)) if *m == unverified.mint)
                        });
                        if is_input_mint
                            && let Some(slot) = tx_diagnostics
                                .unverified_variant_buys
                                .get_mut(unverified.variant.index())
                        {
                            *slot = slot.saturating_add(1);
                        }
                    }
                    summary.diagnostics.add(&tx_diagnostics);
                    for hex in q.unknown_discriminators {
                        if unknown_discriminator_samples.len() < MAX_UNKNOWN_DISCRIMINATOR_SAMPLES {
                            unknown_discriminator_samples.push(hex);
                        }
                    }
                    for reason in q.malformed_reasons {
                        if malformed_samples.len() < MAX_MALFORMED_SAMPLES {
                            malformed_samples.push(sanitize_provider_text(&reason));
                        }
                    }
                    for (mint, _owner) in &q.uninstructed_positive_deltas {
                        let is_input_mint = tokens.iter().any(|t| {
                            matches!(t, AssetKey::Token(_, AddressBytes::Solana(m)) if m == mint)
                        });
                        if is_input_mint {
                            summary.positive_delta_without_instruction =
                                summary.positive_delta_without_instruction.saturating_add(1);
                        }
                    }
                    for buy in q.buys {
                        // Only declared input tokens count (ADR-003; one
                        // hit per (wallet, token) regardless of buy count).
                        if seen.contains(&buy.asset) {
                            token_buyers.insert(buy.wallet.clone());
                            hits.entry(buy.wallet).or_default().insert(buy.asset);
                        }
                    }
                }
            }
        }

        let mut run_stop = None;
        if let Some(err) = failure {
            let kind = classify_provider_error(&err);
            run_stop = kind.stop();
            summary.status = TokenScanStatus::Failed {
                kind,
                message: sanitize_provider_text(&err.to_string()),
            };
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
        per_token.push(finish(summary, &token_buyers));
        if let Some(reason) = run_stop {
            // Every further request would fail the same way: do not
            // issue any; remaining tokens are explicitly not scanned.
            stop = Some(reason);
            for rest in tokens.iter().skip(index + 1) {
                per_token.push(SolanaTokenScanSummary::new(
                    rest.clone(),
                    TokenScanStatus::NotScanned { reason },
                ));
            }
            break 'tokens;
        }
    }

    let total_txs: u64 = per_token
        .iter()
        .fold(0u64, |acc, t| acc.saturating_add(t.transactions_scanned));
    if total_txs == 0
        && !cancelled
        && let Some(err) = first_error
    {
        return Err(err);
    }

    let mut diagnostics = TxQualificationDiagnostics::default();
    let mut positive_delta_without_instruction = 0u64;
    let mut unexpected_payloads = 0u64;
    for t in &per_token {
        diagnostics.add(&t.diagnostics);
        positive_delta_without_instruction =
            positive_delta_without_instruction.saturating_add(t.positive_delta_without_instruction);
        unexpected_payloads = unexpected_payloads.saturating_add(t.unexpected_payloads);
    }
    let coverage_truncated = per_token.iter().any(|t| t.truncated);

    let candidates = hits
        .into_iter()
        .map(|(wallet, assets)| (wallet, assets.into_iter().collect::<Vec<_>>()));
    let matches = threshold_and_sort_matches(candidates, min_token_hits);

    Ok(SolanaBuyerIntersectReport {
        base: BuyerIntersectReport {
            matches,
            input_token_count: tokens.len(),
            min_token_hits,
            coverage_truncated,
        },
        scope: SolanaProtocolScope::pump_bonding_curve(),
        per_token,
        diagnostics,
        positive_delta_without_instruction,
        unexpected_payloads,
        malformed_samples,
        unknown_discriminator_samples,
        cancelled,
        stop,
    })
}

fn finish(
    mut summary: SolanaTokenScanSummary,
    buyers: &BTreeSet<WalletKey>,
) -> SolanaTokenScanSummary {
    summary.qualified_buyers = u64::try_from(buyers.len()).unwrap_or(u64::MAX);
    summary
}
