//! Solana vertical slice for `buyer-intersect` (ADR-014 "trade side"):
//! wallets that BOUGHT and/or SOLD each input token, selected by
//! [`SideFilter`] (CLI `--side buy|sell|any`).
//!
//! Qualifying operations (ADR-014 §2; never a transfer/airdrop, never a
//! router, relayer or fee payer): pump.fun bonding-curve buy/sell, PumpSwap
//! trade with the wallet's own legs reconciled (ADR-012; side in TOKEN
//! terms, reversed pools inverted), and an ADR-013 route swap (side = sign
//! of the wallet's own delta of the token). Attribution is the ledger's
//! (`attribute_transaction_trades`), applied to every candidate wallet of a
//! transaction (signers, decoded leg users, owners with a delta of an input
//! mint), so the two consumers share one implementation. IdlOnly variants
//! never qualify; they are counted and make coverage incomplete (ADR-009).
//!
//! Scope honesty (AGENTS.md invariant 10): Raydium, Meteora DLMM, Orca
//! Whirlpool and every other venue are NOT decoded (a route through them is
//! recognized only via a FixtureVerified pump leg or a Jupiter v6 swap event,
//! ADR-015), so the wallet set is a LOWER BOUND.
//! Counts are "observed", never "total".
//!
//! Window (ADR-014 §4, ADR-011): with a bounded [`AnalysisWindow`] the
//! caller scans newest-first with the provider's stop-before boundary; this
//! engine keeps only transactions inside `[since, until)`, and a token is
//! complete for the window iff the boundary was reached or history ended.
//! Without a window the scan is whatever order the provider uses (oldest
//! first from token creation), with the old truncation semantics.
//!
//! Coverage policy (ADR-005, ACCEPTANCE B08): the declared input-token
//! count N never shrinks. A per-token scan failure (other than
//! `ConfigurationRequired`, which aborts the run as infrastructure
//! unavailable) is recorded on that token and makes the run incomplete
//! (exit 3), not an error. If NO token produced any transaction and at
//! least one failed, the first error is returned (nothing usable was
//! observed; exit 4). Truncation, malformed instructions, unexpected
//! payload shapes, delta overflow, IdlOnly trades and out-of-scope slots
//! all make coverage incomplete.
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
//! Bounded work: at most `concurrency` scan units (a token, or one time
//! slice of a token) are in flight (`buffer_unordered`, nothing spawned, so
//! dropping the run drops all work); each unit keeps one request in flight
//! and consumes/drops each envelope before polling the next. Retained state
//! is the (wallet, input-token) hit set, one signature set per unit
//! (bounded by the page budget) plus capped diagnostic samples.
//!
//! Determinism (invariants 11, 12): every unit accumulates privately and the
//! results are merged ONLY in unit order (token input order, then slice
//! oldest first) with order-independent operations (sums, set unions,
//! min-`(slot, index)` evidence), so completion order never shows. A run
//! stop is the stop of the lowest-index failed unit. Units not yet started
//! when a stop arrives are `NotScanned` (a token with some slices started:
//! `Failed` naming the unstarted slices); in-flight units are cut at their
//! next await and are `Failed` ("interrupted in flight"). Only which units
//! had already finished when the stop arrived depends on timing.
//!
//! Slices (`IntersectOptions::slices`): disjoint `[since, until)` sub-windows,
//! each with its own server-side `blockTime` filter on the SAME shared
//! request budget. A slice only counts transactions whose `blockTime` lies in
//! it, so a boundary transaction re-delivered to a neighbour slice is counted
//! once. A token is complete for the window iff every slice completed.

use std::collections::{BTreeMap, BTreeSet};

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ProviderError, ScanRequest, ScanTask};
use scout_core::{AddressBytes, AssetKey, ChainFamily, RawPayload, SolanaPubkey, WalletKey};
use scout_dex_solana::{
    PUMP_AMM_IDL_COMMIT, PUMP_AMM_IDL_SHA256, PUMP_AMM_PROGRAM_ID, PUMP_IDL_SHA256,
    PumpTradeVariant, TradeSide, VariantVerification,
};
use scout_rpc::RequestBudgetExhausted;
use tokio_util::sync::CancellationToken;

use crate::analysis_window::AnalysisWindow;
use crate::buyer_intersect::{BuyerIntersectReport, threshold_and_sort_matches};
use crate::decode_evidence::{DecodeEvidence, VenueEventDiagnostics, merge_evidence};
use crate::solana_buy_qualification::{
    PUMP_BONDING_CURVE_IDL_COMMIT, PUMP_BONDING_CURVE_PROGRAM_ID, SOLANA_BUY_QUALIFICATION_VERSION,
    SOLANA_TRADE_QUALIFICATION_VERSION, TxQualificationDiagnostics, VariantPolicy,
    default_variant_policy, pump_bonding_curve_decoder, qualify_bonding_curve_buys_with_policy,
    solana_mainnet_chain,
};
use crate::solana_wallet_ledger::{
    LedgerDecoders, OkxOrderPolicy, RouteReject, RouteRejections, Venue,
    attribute_transaction_trades, default_okx_order_policy, pump_amm_decoder,
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

    /// Scope of `buyer-intersect` (ADR-014): bonding curve + PumpSwap +
    /// ADR-013 route swaps, both sides.
    #[must_use]
    pub const fn pump_trade_intersect() -> Self {
        Self {
            program_id: PUMP_BONDING_CURVE_PROGRAM_ID,
            idl_commit: PUMP_BONDING_CURVE_IDL_COMMIT,
            idl_sha256: PUMP_IDL_SHA256,
            qualification_version: SOLANA_TRADE_QUALIFICATION_VERSION,
            amm_program_id: Some(PUMP_AMM_PROGRAM_ID),
            amm_idl_commit: Some(PUMP_AMM_IDL_COMMIT),
            amm_idl_sha256: Some(PUMP_AMM_IDL_SHA256),
            recognized: "trades (buy and/or sell, per --side) of the input tokens through: \
                         pump.fun bonding curve (program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P; \
                         FixtureVerified variant, decoded user = wallet, owner-keyed net delta of \
                         the token with the sign of the side); PumpSwap AMM (program \
                         pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA; decoded user = wallet, \
                         FixtureVerified variant, the wallet's own legs reconcile with the events, \
                         side in token terms, reversed pools inverted, ADR-012; router-forwards \
                         are not attributed); route swaps (ADR-013 rule: signer wallet, \
                         FixtureVerified swap leg = pump leg, Jupiter v6 SwapEvent/SwapsEvent or \
                         DFlow v4 SwapEvent hop trading the token (ADR-015, evidence only, no ownership), an OKX DEX Router order event of a \
                         FixtureVerified variant naming the wallet as owner (ADR-017; only SwapWithFeesCpiEvent2 is verified), or a FixtureVerified \
                         direct-venue swap event (Orca Whirlpool Traded, Meteora DLMM Swap/Swap2Evt, Raydium CLMM/CPMM SwapEvent; ADR-013 section 2b, \
                         evidence only, no ownership) trading the token, one \
                         traded token vs one SOL/USDC/USDT quote, pass-through leg users \
                         netting zero; side = sign of the wallet's own delta). Never transfers or airdrops, never routers, relayers or fee \
                         payers. IdlOnly variants are decoded but never qualify (counted, \
                         coverage incomplete)",
            not_decoded: "Raydium, Meteora (DLMM), Orca Whirlpool as venues beyond their swap events (a swap is recognized only through a verified swap-event leg plus the wallet's own deltas; \
                          Whirlpool two-hop, Raydium CLMM router, DLMM exact-out/price-impact and CPMM base-output swaps are IdlOnly or unsupported), routes without a pump leg, a Jupiter v6 / DFlow v4 swap event or a \
                          direct-venue swap event (e.g. OKX DEX Router routes whose order event is not SwapWithFeesCpiEvent2), PumpSwap liquidity/non-trade instructions \
                          and every other venue; the wallet set is a lower bound (a wallet that \
                          traded only there is not found)",
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
                         (ADR-012); route swaps (ADR-013: signer wallet, FixtureVerified swap \
                         leg = pump leg, Jupiter v6 SwapEvent/SwapsEvent or DFlow v4 SwapEvent hop \
                         trading the token (ADR-015) a FixtureVerified OKX order event of the wallet (ADR-017) or a FixtureVerified direct-venue swap event (Whirlpool, DLMM, Raydium CLMM/CPMM; ADR-013 section 2b), one traded token vs one SOL/USDC/USDT quote asset, \
                         pass-through leg users netting zero) are booked from the wallet's own deltas in the \
                         quote's unit; PnL is per quote unit (SOL, USDC, USDT), never mixed; \
                         one FIFO per (wallet, mint) across venues",
            not_decoded: "Raydium, Meteora and Orca as venues beyond their swap events (a route \
                          is recognized only through a FixtureVerified pump leg, a Jupiter v6 / \
                          DFlow v4 swap event or a direct-venue swap event, plus the wallet's own deltas), PumpSwap liquidity/non-trade instructions and every \
                          other venue; token movements there are continuity breaks (Unknown), \
                          never zero PnL; bot/platform fees stay outside trade PnL (ADR-010 §5)",
        }
    }
}

/// Which trade sides qualify a wallet for an input token (ADR-014 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    Buy,
    Sell,
    /// A buy OR a sell on each token (default, owner decision).
    #[default]
    Any,
}

impl SideFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
            Self::Any => "any",
        }
    }

    #[must_use]
    pub const fn includes(self, side: TradeSide) -> bool {
        matches!(
            (self, side),
            (Self::Any, _) | (Self::Buy, TradeSide::Buy) | (Self::Sell, TradeSide::Sell)
        )
    }
}

/// Options of a trade-intersect run.
#[derive(Debug, Clone, Copy)]
pub struct IntersectOptions {
    pub side: SideFilter,
    /// Bounded window = the caller scans newest-first to the boundary.
    pub window: AnalysisWindow,
    /// Max scan units (a token, or one time slice of a token) in flight at
    /// once, `1..=MAX_SCAN_CONCURRENCY` (clamped). Each unit keeps at most
    /// one provider request in flight, so this also bounds in-flight
    /// requests. The library default is 1 (sequential); the CLI default is 4.
    pub concurrency: usize,
    /// Split a bounded window into this many equal sub-windows per token,
    /// each scanned with its own server-side `blockTime` filter
    /// (`1..=MAX_SCAN_SLICES`, clamped; ignored without a window). Only
    /// meaningful against a provider that honors
    /// [`HistoryProvider::scan_block_time_range`]; others re-deliver the
    /// whole history to every slice (correct, but not cheaper).
    pub slices: u32,
}

/// Upper bound of [`IntersectOptions::concurrency`].
pub const MAX_SCAN_CONCURRENCY: usize = 16;
/// Upper bound of [`IntersectOptions::slices`].
pub const MAX_SCAN_SLICES: u32 = 16;

/// One time slice `[since, until)` of a windowed token scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceId {
    /// 0-based, oldest sub-window first.
    pub index: u32,
    /// Number of slices of the token.
    pub of: u32,
    pub since: i64,
    pub until: i64,
}

impl SliceId {
    /// `slice 2/4 [since, until)` (unix seconds), for reasons and logs.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "slice {}/{} [{}, {})",
            self.index.saturating_add(1),
            self.of,
            self.since,
            self.until
        )
    }
}

impl Default for IntersectOptions {
    fn default() -> Self {
        Self {
            side: SideFilter::default(),
            window: AnalysisWindow::none(0),
            concurrency: 1,
            slices: 1,
        }
    }
}

/// First-qualifying evidence and count for one side of one (wallet, token).
/// "First" = lowest `(slot, transaction_index)` among observed ones, so the
/// evidence does not depend on scan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideEvidence {
    /// Qualifying transactions observed on this side.
    pub count: u64,
    pub signature: [u8; 64],
    pub slot: u64,
    pub transaction_index: u64,
    pub venue: Venue,
    /// IDL instruction name of the decoded variant (`route_swap` for routes).
    pub variant: &'static str,
}

/// Sides observed for one (wallet, token) under the selected filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenSideHits {
    pub buy: Option<SideEvidence>,
    pub sell: Option<SideEvidence>,
}

/// Per-token counters of the trade attribution (ADR-014 §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TradeAttributionDiagnostics {
    /// Qualifying (wallet, side, transaction) operations by venue
    /// (`Venue` order: curve, PumpSwap, route) and side (buy, sell).
    pub qualified_ops: [[u64; 2]; 3],
    /// PumpSwap trades of a non-signing zero-net forwarder (not attributed).
    pub router_forwards_not_attributed: u64,
    /// ADR-013 §2 rejections of signer candidates.
    pub route_rejections: RouteRejections,
    /// Trades of a FixtureVerified-less (IdlOnly) variant of the selected
    /// side with a matching owner delta: never qualify. COVERAGE GAP.
    pub idl_only_trades: u64,
    /// PumpSwap trades of the wallet whose own legs did not reconcile.
    pub amm_unreconciled: u64,
    /// Decoded trades whose owner-keyed delta of the token does not have the
    /// sign of the side (atomic round trips, forwarded trades).
    pub trades_without_matching_delta: u64,
    /// Malformed trade instructions / orphan events (both venues).
    /// COVERAGE GAP.
    pub malformed_trades: u64,
    pub orphan_events: u64,
    /// ADR-015: Jupiter event-CPIs that did not decode exactly / with an
    /// unknown discriminator (never trusted as swap evidence). COVERAGE GAP.
    pub jupiter_malformed_events: u64,
    pub jupiter_unknown_events: u64,
    /// ADR-015 amendment: the same for DFlow Aggregator v4 events.
    pub dflow_malformed_events: u64,
    pub dflow_unknown_events: u64,
    /// ADR-017: OKX DEX Router events that did not decode exactly /
    /// have an unknown discriminator (never trusted). COVERAGE GAP.
    pub okx_malformed_events: u64,
    pub okx_unknown_events: u64,
    /// OKX order events with a distinct receiver: attributed to nobody.
    pub okx_swap_with_receiver_not_attributed: u64,
    /// OKX order events of a variant that is not `FixtureVerified`: decoded,
    /// counted, never leg evidence (lower bound).
    pub okx_idl_only_order_events: u64,
    /// ADR-013 section 2b: direct-venue (Whirlpool, DLMM, Raydium CLMM/CPMM)
    /// swap-event coverage. malformed/unknown are COVERAGE GAPS; idl_only and
    /// unresolved are informational (lower bound).
    pub venue_events: VenueEventDiagnostics,
    /// Up to 5 samples (canonical chain order) of malformed trade
    /// instructions, unknown discriminators and orphan events of this token's
    /// transactions, so each counter can be traced to a signature.
    pub evidence_samples: Vec<DecodeEvidence>,
}

const fn venue_index(venue: Venue) -> usize {
    match venue {
        Venue::BondingCurve => 0,
        Venue::PumpAmm => 1,
        Venue::Route => 2,
    }
}

const fn side_index(side: TradeSide) -> usize {
    match side {
        TradeSide::Buy => 0,
        TradeSide::Sell => 1,
    }
}

impl TradeAttributionDiagnostics {
    /// Qualifying operations of `venue` and `side`.
    #[must_use]
    pub fn ops(&self, venue: Venue, side: TradeSide) -> u64 {
        self.qualified_ops
            .get(venue_index(venue))
            .and_then(|row| row.get(side_index(side)))
            .copied()
            .unwrap_or(0)
    }

    /// Saturating field-wise sum.
    pub fn add(&mut self, o: &Self) {
        for (row, orow) in self.qualified_ops.iter_mut().zip(o.qualified_ops) {
            for (a, b) in row.iter_mut().zip(orow) {
                *a = a.saturating_add(b);
            }
        }
        let s = u64::saturating_add;
        self.router_forwards_not_attributed = s(
            self.router_forwards_not_attributed,
            o.router_forwards_not_attributed,
        );
        let (r, q) = (&mut self.route_rejections, &o.route_rejections);
        r.wallet_not_signer = s(r.wallet_not_signer, q.wallet_not_signer);
        r.multi_asset = s(r.multi_asset, q.multi_asset);
        r.not_opposite_signs = s(r.not_opposite_signs, q.not_opposite_signs);
        r.no_quote_leg = s(r.no_quote_leg, q.no_quote_leg);
        r.no_verified_leg = s(r.no_verified_leg, q.no_verified_leg);
        r.passthrough_nonzero = s(r.passthrough_nonzero, q.passthrough_nonzero);
        self.idl_only_trades = s(self.idl_only_trades, o.idl_only_trades);
        self.amm_unreconciled = s(self.amm_unreconciled, o.amm_unreconciled);
        self.trades_without_matching_delta = s(
            self.trades_without_matching_delta,
            o.trades_without_matching_delta,
        );
        self.malformed_trades = s(self.malformed_trades, o.malformed_trades);
        self.orphan_events = s(self.orphan_events, o.orphan_events);
        self.jupiter_malformed_events =
            s(self.jupiter_malformed_events, o.jupiter_malformed_events);
        self.jupiter_unknown_events = s(self.jupiter_unknown_events, o.jupiter_unknown_events);
        self.dflow_malformed_events = s(self.dflow_malformed_events, o.dflow_malformed_events);
        self.dflow_unknown_events = s(self.dflow_unknown_events, o.dflow_unknown_events);
        self.okx_malformed_events = s(self.okx_malformed_events, o.okx_malformed_events);
        self.okx_unknown_events = s(self.okx_unknown_events, o.okx_unknown_events);
        self.okx_swap_with_receiver_not_attributed = s(
            self.okx_swap_with_receiver_not_attributed,
            o.okx_swap_with_receiver_not_attributed,
        );
        self.okx_idl_only_order_events =
            s(self.okx_idl_only_order_events, o.okx_idl_only_order_events);
        self.venue_events.add(&o.venue_events);
        merge_evidence(
            &mut self.evidence_samples,
            o.evidence_samples.iter().cloned(),
        );
    }

    fn count_route_rejection(&mut self, reject: RouteReject) {
        let r = &mut self.route_rejections;
        let slot = match reject {
            RouteReject::NothingMoved => return,
            RouteReject::NotSigner => &mut r.wallet_not_signer,
            RouteReject::MultiAsset => &mut r.multi_asset,
            RouteReject::NotOppositeSigns => &mut r.not_opposite_signs,
            RouteReject::NoQuoteLeg => &mut r.no_quote_leg,
            RouteReject::NoVerifiedLeg => &mut r.no_verified_leg,
            RouteReject::PassthroughNonzero => &mut r.passthrough_nonzero,
        };
        *slot = slot.saturating_add(1);
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
    /// Time slices (sliced scans only) that did not complete; a token is
    /// complete for the window iff this is empty and `truncated` is false.
    pub truncated_slices: Vec<SliceId>,
    pub status: TokenScanStatus,
    /// Distinct wallets with a qualified BUY of THIS token (0 under
    /// `--side sell`: sells are not tracked then).
    pub qualified_buyers: u64,
    /// Distinct wallets with a qualified sell of THIS token.
    pub qualified_sellers: u64,
    /// Distinct wallets with a qualifying operation of the selected side(s).
    pub qualified_wallets: u64,
    /// Transactions inside the window (`None` without a window).
    pub transactions_in_window: Option<u64>,
    /// Windowed scan: a transaction older than the window start was seen.
    pub boundary_reached: bool,
    /// Windowed scan: transactions without `blockTime` before the boundary
    /// was seen (cannot be placed in the window). COVERAGE GAP.
    pub missing_block_time: u64,
    pub diagnostics: TxQualificationDiagnostics,
    pub trade: TradeAttributionDiagnostics,
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
    /// Sum over tokens.
    pub trade: TradeAttributionDiagnostics,
    pub side: SideFilter,
    pub window: AnalysisWindow,
    /// Sides observed per matched-candidate (wallet, token) with first
    /// qualifying evidence (every wallet with any hit, before K).
    pub side_hits: BTreeMap<WalletKey, BTreeMap<AssetKey, TokenSideHits>>,
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
    /// Effective in-flight scan limit of the run (after clamping).
    pub concurrency: usize,
    /// Effective slices per token (1 = unsliced).
    pub slices: u32,
}

impl SolanaBuyerIntersectReport {
    /// Reasons coverage is incomplete; empty means complete WITHIN the
    /// declared protocol scope (not complete buyer history).
    #[must_use]
    pub fn incomplete_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        for token in &self.per_token {
            if token.truncated && !token.truncated_slices.is_empty() {
                let names = token
                    .truncated_slices
                    .iter()
                    .map(SliceId::describe)
                    .collect::<Vec<_>>()
                    .join(", ");
                reasons.push(format!(
                    "token {}: page budget exhausted (or never scanned) in {names} \
                     (part of the window is not seen)",
                    token.asset_label()
                ));
            } else if token.truncated {
                if self.window.is_bounded() {
                    reasons.push(format!(
                        "token {}: page budget exhausted before the window start was reached \
                         (part of the window is not seen)",
                        token.asset_label()
                    ));
                } else {
                    reasons.push(format!(
                        "token {}: provider reported unconsumed pagination (truncated)",
                        token.asset_label()
                    ));
                }
            }
            if token.missing_block_time > 0 {
                reasons.push(format!(
                    "token {}: {} transaction(s) without blockTime inside the scanned range: \
                     cannot be placed in the window",
                    token.asset_label(),
                    token.missing_block_time
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
                    "{n} trade(s) via IdlOnly curve variant {} would qualify but the variant has \
                     no verified fixture; wallet set is a lower bound",
                    variant.name()
                ));
            }
        }
        let curve_idl_only: u64 = d
            .unverified_variant_buys
            .iter()
            .fold(0u64, |a, &b| a.saturating_add(b));
        let amm_idl_only = self.trade.idl_only_trades.saturating_sub(curve_idl_only);
        if amm_idl_only > 0 {
            reasons.push(format!(
                "{amm_idl_only} PumpSwap trade(s) via IdlOnly variant would qualify but the \
                 variant has no verified fixture; wallet set is a lower bound"
            ));
        }
        if self.trade.malformed_trades > 0 {
            reasons.push(format!(
                "{} malformed trade instruction(s) (curve/PumpSwap) not decoded",
                self.trade.malformed_trades
            ));
        }
        if self.trade.orphan_events > 0 {
            reasons.push(format!(
                "{} trade event(s) not claimed by any decoded trade",
                self.trade.orphan_events
            ));
        }
        if self.trade.jupiter_malformed_events > 0 {
            reasons.push(format!(
                "{} Jupiter event(s) did not decode exactly (not used as swap evidence)",
                self.trade.jupiter_malformed_events
            ));
        }
        if self.trade.jupiter_unknown_events > 0 {
            reasons.push(format!(
                "{} Jupiter event(s) with an unknown discriminator",
                self.trade.jupiter_unknown_events
            ));
        }
        if self.trade.dflow_malformed_events > 0 {
            reasons.push(format!(
                "{} DFlow event(s) did not decode exactly (not used as swap evidence)",
                self.trade.dflow_malformed_events
            ));
        }
        if self.trade.dflow_unknown_events > 0 {
            reasons.push(format!(
                "{} DFlow event(s) with an unknown discriminator",
                self.trade.dflow_unknown_events
            ));
        }
        if self.trade.okx_malformed_events > 0 {
            reasons.push(format!(
                "{} OKX DEX Router event(s) did not decode exactly (not used as swap evidence)",
                self.trade.okx_malformed_events
            ));
        }
        if self.trade.okx_unknown_events > 0 {
            reasons.push(format!(
                "{} OKX DEX Router event(s) with an unknown discriminator",
                self.trade.okx_unknown_events
            ));
        }
        reasons.extend(self.trade.venue_events.gap_reasons());
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
            truncated_slices: Vec::new(),
            status,
            qualified_buyers: 0,
            qualified_sellers: 0,
            qualified_wallets: 0,
            transactions_in_window: None,
            boundary_reached: false,
            missing_block_time: 0,
            diagnostics: TxQualificationDiagnostics::default(),
            trade: TradeAttributionDiagnostics::default(),
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
/// qualifying BUYS only (curve, PumpSwap, route swaps), no window. The
/// CLI default is `--side any`: use [`run_solana_trade_intersect`].
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
    run_solana_trade_intersect_with_policy(
        provider,
        input_tokens,
        min_token_hits,
        IntersectOptions {
            side: SideFilter::Buy,
            window: AnalysisWindow::none(0),
            ..IntersectOptions::default()
        },
        cancel,
        policy,
    )
    .await
}

/// ADR-014 entry point: wallets with a qualifying trade of the selected
/// side(s) on at least `min_token_hits` distinct input tokens.
pub async fn run_solana_trade_intersect(
    provider: &dyn HistoryProvider,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    options: IntersectOptions,
    cancel: CancellationToken,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    run_solana_trade_intersect_with_policy(
        provider,
        input_tokens,
        min_token_hits,
        options,
        cancel,
        default_variant_policy,
    )
    .await
}

/// Per-run immutable context of the transaction qualification.
struct TxContext<'a> {
    decoders: LedgerDecoders<'a>,
    /// The mint of the token being scanned: its own scan alone produces its
    /// hits, so a transaction seen in two token scans is never counted twice.
    focus: BTreeSet<SolanaPubkey>,
    side: SideFilter,
    policy: VariantPolicy,
}

/// Mutable per-token accumulation.
#[derive(Default)]
struct TokenAcc {
    wallets: BTreeSet<WalletKey>,
    buyers: BTreeSet<WalletKey>,
    sellers: BTreeSet<WalletKey>,
}

type SideHitMap = BTreeMap<WalletKey, BTreeMap<AssetKey, TokenSideHits>>;

/// Qualify every attributed trade of one transaction (ADR-014 §2) and
/// record hits. Pure apart from the accumulators it is handed.
fn qualify_transaction(
    tx: &scout_core::RawSolanaTransaction,
    ctx: &TxContext<'_>,
    summary: &mut SolanaTokenScanSummary,
    acc: &mut TokenAcc,
    side_hits: &mut SideHitMap,
) {
    let attribution = match attribute_transaction_trades(tx, &ctx.decoders, &ctx.focus, ctx.policy)
    {
        Ok(a) => a,
        Err(_) => {
            summary.diagnostics.delta_overflow_transactions = summary
                .diagnostics
                .delta_overflow_transactions
                .saturating_add(1);
            return;
        }
    };
    let t = &mut summary.trade;
    t.malformed_trades = t
        .malformed_trades
        .saturating_add(attribution.malformed_trades);
    t.orphan_events = t.orphan_events.saturating_add(attribution.orphan_events);
    t.jupiter_malformed_events = t
        .jupiter_malformed_events
        .saturating_add(attribution.jupiter_malformed);
    t.jupiter_unknown_events = t
        .jupiter_unknown_events
        .saturating_add(attribution.jupiter_unknown);
    t.dflow_malformed_events = t
        .dflow_malformed_events
        .saturating_add(attribution.dflow_malformed);
    t.dflow_unknown_events = t
        .dflow_unknown_events
        .saturating_add(attribution.dflow_unknown);
    t.okx_malformed_events = t
        .okx_malformed_events
        .saturating_add(attribution.okx_malformed);
    t.okx_unknown_events = t.okx_unknown_events.saturating_add(attribution.okx_unknown);
    t.okx_swap_with_receiver_not_attributed = t
        .okx_swap_with_receiver_not_attributed
        .saturating_add(attribution.okx_receiver);
    t.okx_idl_only_order_events = t
        .okx_idl_only_order_events
        .saturating_add(attribution.okx_idl_only);
    t.venue_events.add(&attribution.venue_events);
    merge_evidence(
        &mut t.evidence_samples,
        attribution.evidence.iter().cloned(),
    );
    t.router_forwards_not_attributed = t
        .router_forwards_not_attributed
        .saturating_add(attribution.router_forwards);
    for reject in &attribution.route_rejections {
        t.count_route_rejection(*reject);
    }
    let chain = solana_mainnet_chain();
    // One qualifying operation per (wallet, mint, side) and transaction.
    let mut seen: BTreeSet<(SolanaPubkey, SolanaPubkey, usize)> = BTreeSet::new();
    for trade in &attribution.trades {
        if !ctx.side.includes(trade.side) {
            continue;
        }
        let delta_ok = match trade.side {
            TradeSide::Buy => trade.owner_delta > 0,
            TradeSide::Sell => trade.owner_delta < 0,
        };
        if !delta_ok {
            summary.trade.trades_without_matching_delta = summary
                .trade
                .trades_without_matching_delta
                .saturating_add(1);
            continue;
        }
        if trade.verification != VariantVerification::FixtureVerified {
            if seen.insert((trade.wallet, trade.mint, side_index(trade.side))) {
                summary.trade.idl_only_trades = summary.trade.idl_only_trades.saturating_add(1);
                if trade.venue == Venue::BondingCurve
                    && let Some(v) = PumpTradeVariant::ALL
                        .iter()
                        .find(|v| v.name() == trade.variant)
                    && let Some(slot) = summary
                        .diagnostics
                        .unverified_variant_buys
                        .get_mut(v.index())
                {
                    *slot = slot.saturating_add(1);
                }
            }
            continue;
        }
        if !trade.reconciled {
            summary.trade.amm_unreconciled = summary.trade.amm_unreconciled.saturating_add(1);
            continue;
        }
        if !seen.insert((trade.wallet, trade.mint, side_index(trade.side))) {
            continue;
        }
        let asset = AssetKey::Token(chain.clone(), AddressBytes::Solana(trade.mint));
        let wallet = WalletKey {
            chain: chain.clone(),
            address: AddressBytes::Solana(trade.wallet),
        };
        if let Some(cell) = summary
            .trade
            .qualified_ops
            .get_mut(venue_index(trade.venue))
            .and_then(|row| row.get_mut(side_index(trade.side)))
        {
            *cell = cell.saturating_add(1);
        }
        acc.wallets.insert(wallet.clone());
        match trade.side {
            TradeSide::Buy => acc.buyers.insert(wallet.clone()),
            TradeSide::Sell => acc.sellers.insert(wallet.clone()),
        };
        let hits = side_hits
            .entry(wallet)
            .or_default()
            .entry(asset)
            .or_default();
        let slot = match trade.side {
            TradeSide::Buy => &mut hits.buy,
            TradeSide::Sell => &mut hits.sell,
        };
        let key = (tx.slot, tx.transaction_index);
        match slot {
            Some(e) => {
                e.count = e.count.saturating_add(1);
                if key < (e.slot, e.transaction_index) {
                    e.signature = tx.signature;
                    e.slot = tx.slot;
                    e.transaction_index = tx.transaction_index;
                    e.venue = trade.venue;
                    e.variant = trade.variant;
                }
            }
            None => {
                *slot = Some(SideEvidence {
                    count: 1,
                    signature: tx.signature,
                    slot: tx.slot,
                    transaction_index: tx.transaction_index,
                    venue: trade.venue,
                    variant: trade.variant,
                });
            }
        }
    }
}

/// As [`run_solana_trade_intersect`] with an injected curve variant policy.
pub async fn run_solana_trade_intersect_with_policy(
    provider: &dyn HistoryProvider,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    options: IntersectOptions,
    cancel: CancellationToken,
    policy: VariantPolicy,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    run_solana_trade_intersect_with_policies(
        provider,
        input_tokens,
        min_token_hits,
        options,
        cancel,
        policy,
        default_okx_order_policy,
    )
    .await
}

/// As [`run_solana_trade_intersect_with_policy`] with an injected OKX order
/// event policy as well (tests exercise the `FixtureVerified` path of a
/// variant that is `IdlOnly`; production uses [`default_okx_order_policy`]).
pub async fn run_solana_trade_intersect_with_policies(
    provider: &dyn HistoryProvider,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
    options: IntersectOptions,
    cancel: CancellationToken,
    policy: VariantPolicy,
    okx_order_policy: OkxOrderPolicy,
) -> Result<SolanaBuyerIntersectReport, ProviderError> {
    let curve = pump_bonding_curve_decoder().map_err(|e| ProviderError::Other(Box::new(e)))?;
    let amm = pump_amm_decoder();
    let window = options.window;
    let bounded = window.is_bounded();
    let concurrency = options.concurrency.clamp(1, MAX_SCAN_CONCURRENCY);

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

    // Scan units, token-major then slice (oldest sub-window first). The
    // unit list is the ONLY order anything is merged in.
    let slice_plan = plan_slices(&window, options.slices);
    let slice_count = slice_plan.len().max(1);
    let mut units: Vec<Unit> = Vec::with_capacity(tokens.len().saturating_mul(slice_count));
    for token in &tokens {
        if slice_plan.is_empty() {
            units.push(Unit {
                token: token.clone(),
                slice: None,
            });
        } else {
            for slice in &slice_plan {
                units.push(Unit {
                    token: token.clone(),
                    slice: Some(*slice),
                });
            }
        }
    }

    let shared = UnitShared {
        provider,
        decoders: LedgerDecoders {
            curve: &curve,
            amm: Some(&amm),
            okx_order_policy,
        },
        side: options.side,
        policy,
        window,
    };
    let user_cancel = &cancel;
    // Cancelled on a run-terminal error (or the caller's cancel): units not
    // yet started never start, in-flight ones are cut at their next await.
    let run = cancel.child_token();

    // Bounded admission (invariant 13): `buffer_unordered(concurrency)` polls
    // at most `concurrency` unit futures; the remaining units are not even
    // constructed. Nothing is spawned, so every unit is a plain future of
    // this task and dropping the stream drops all in-flight work. Each unit
    // keeps at most one provider request in flight, so in-flight requests
    // are bounded by `concurrency` as well (slices included).
    let mut ends: Vec<Option<UnitEnd>> = (0..units.len()).map(|_| None).collect();
    {
        let shared = &shared;
        let run_ref = &run;
        let mut pending = futures::stream::iter(units.iter().enumerate())
            .map(|(index, unit)| async move {
                (index, scan_unit(shared, unit, user_cancel, run_ref).await)
            })
            .buffer_unordered(concurrency);
        while let Some((index, result)) = pending.next().await {
            let end = result?;
            if let UnitEnd::Done(done) = &end
                && done
                    .failure
                    .as_ref()
                    .is_some_and(|e| classify_provider_error(e).stop().is_some())
            {
                run.cancel();
            }
            if let Some(slot) = ends.get_mut(index) {
                *slot = Some(end);
            }
        }
    }

    // ---- deterministic merge, in unit order only ------------------------
    let mut ends: Vec<UnitEnd> = ends
        .into_iter()
        .map(|e| {
            e.unwrap_or(UnitEnd::NotStarted {
                cancelled: user_cancel.is_cancelled(),
            })
        })
        .collect();

    // The run stop is the stop of the LOWEST-index failed unit, not of
    // whichever failure happened to complete first.
    let stop: Option<ScanStop> = ends.iter().find_map(|e| match e {
        UnitEnd::Done(d) => d
            .failure
            .as_ref()
            .and_then(|err| classify_provider_error(err).stop()),
        UnitEnd::NotStarted { .. } => None,
    });

    let mut side_hits: SideHitMap = BTreeMap::new();
    let mut malformed_samples: Vec<String> = Vec::new();
    let mut unknown_discriminator_samples: Vec<String> = Vec::new();
    let mut first_error: Option<ProviderError> = None;
    let mut cancelled = false;
    for end in &mut ends {
        match end {
            UnitEnd::NotStarted { cancelled: c } => cancelled |= *c,
            UnitEnd::Done(done) => {
                cancelled |= done.cancelled;
                merge_side_hits(&mut side_hits, std::mem::take(&mut done.side_hits));
                for hex in std::mem::take(&mut done.unknown) {
                    if unknown_discriminator_samples.len() < MAX_UNKNOWN_DISCRIMINATOR_SAMPLES {
                        unknown_discriminator_samples.push(hex);
                    }
                }
                for reason in std::mem::take(&mut done.malformed) {
                    if malformed_samples.len() < MAX_MALFORMED_SAMPLES {
                        malformed_samples.push(reason);
                    }
                }
                // The typed error moves to `first_error` (lowest unit index
                // wins); the token keeps its classification and text.
                done.failure_info = done.failure.take().map(|err| {
                    let info = (
                        classify_provider_error(&err),
                        sanitize_provider_text(&err.to_string()),
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    info
                });
            }
        }
    }

    let mut per_token: Vec<SolanaTokenScanSummary> = Vec::with_capacity(tokens.len());
    for (unit_chunk, chunk) in units.chunks(slice_count).zip(ends.chunks(slice_count)) {
        let Some(first) = unit_chunk.first() else {
            continue;
        };
        let slices = unit_chunk.iter().map(|u| u.slice).collect::<Vec<_>>();
        if let Some(summary) = merge_token(&first.token, &slices, chunk, stop, bounded) {
            per_token.push(summary);
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
    let mut trade = TradeAttributionDiagnostics::default();
    let mut positive_delta_without_instruction = 0u64;
    let mut unexpected_payloads = 0u64;
    for t in &per_token {
        diagnostics.add(&t.diagnostics);
        trade.add(&t.trade);
        positive_delta_without_instruction =
            positive_delta_without_instruction.saturating_add(t.positive_delta_without_instruction);
        unexpected_payloads = unexpected_payloads.saturating_add(t.unexpected_payloads);
    }
    let coverage_truncated = per_token.iter().any(|t| t.truncated);

    let candidates = side_hits.iter().map(|(wallet, per_asset)| {
        (
            wallet.clone(),
            per_asset
                .iter()
                .filter(|(_, h)| h.buy.is_some() || h.sell.is_some())
                .map(|(asset, _)| asset.clone())
                .collect::<Vec<_>>(),
        )
    });
    let matches = threshold_and_sort_matches(candidates, min_token_hits);

    Ok(SolanaBuyerIntersectReport {
        base: BuyerIntersectReport {
            matches,
            input_token_count: tokens.len(),
            min_token_hits,
            coverage_truncated,
        },
        scope: SolanaProtocolScope::pump_trade_intersect(),
        per_token,
        diagnostics,
        trade,
        side: options.side,
        window,
        side_hits,
        positive_delta_without_instruction,
        unexpected_payloads,
        malformed_samples,
        unknown_discriminator_samples,
        cancelled,
        stop,
        concurrency,
        slices: u32::try_from(slice_count).unwrap_or(u32::MAX),
    })
}

/// Split `[since, until)` into at most `requested` non-empty, equal (to
/// within one second) sub-windows, oldest first. Empty = scan the window as
/// one unit (no window, `requested <= 1`, or a one-second window).
fn plan_slices(window: &AnalysisWindow, requested: u32) -> Vec<SliceId> {
    if !window.is_bounded() {
        return Vec::new();
    }
    let span = i128::from(window.until) - i128::from(window.since);
    let count = i128::from(requested.clamp(1, MAX_SCAN_SLICES)).min(span);
    if count <= 1 {
        return Vec::new();
    }
    let of = u32::try_from(count).unwrap_or(MAX_SCAN_SLICES);
    let edge = |k: i128| -> Option<i64> {
        let offset = span.checked_mul(k)?.checked_div(count)?;
        i64::try_from(i128::from(window.since).checked_add(offset)?).ok()
    };
    let mut out = Vec::new();
    for k in 0..count {
        let (Some(since), Some(until)) = (edge(k), edge(k + 1)) else {
            return Vec::new();
        };
        out.push(SliceId {
            index: u32::try_from(k).unwrap_or(0),
            of,
            since,
            until,
        });
    }
    out
}

/// One scan unit: a token (whole window) or one slice of a token.
struct Unit {
    token: AssetKey,
    slice: Option<SliceId>,
}

/// Run-immutable inputs shared by every unit.
struct UnitShared<'a> {
    provider: &'a dyn HistoryProvider,
    decoders: LedgerDecoders<'a>,
    side: SideFilter,
    policy: VariantPolicy,
    window: AnalysisWindow,
}

enum UnitEnd {
    /// The run stopped (or was cancelled) before this unit issued anything.
    NotStarted {
        cancelled: bool,
    },
    Done(Box<UnitOutcome>),
}

/// Everything one unit produced. Merged only in unit order.
struct UnitOutcome {
    /// Counters of this unit only (token-level fields are filled at merge).
    summary: SolanaTokenScanSummary,
    acc: TokenAcc,
    side_hits: SideHitMap,
    malformed: Vec<String>,
    unknown: Vec<String>,
    in_window: u64,
    raw_truncated: bool,
    failure: Option<ProviderError>,
    /// Classification and sanitized text of `failure`, set at merge.
    failure_info: Option<(ScanFailureKind, String)>,
    /// Cut by the run-terminal stop of ANOTHER unit while in flight.
    interrupted: bool,
    /// The caller's cancellation was observed.
    cancelled: bool,
}

/// Scan one unit. `Err` only for `ConfigurationRequired` (aborts the run).
async fn scan_unit(
    shared: &UnitShared<'_>,
    unit: &Unit,
    user_cancel: &CancellationToken,
    run: &CancellationToken,
) -> Result<UnitEnd, ProviderError> {
    if run.is_cancelled() {
        return Ok(UnitEnd::NotStarted {
            cancelled: user_cancel.is_cancelled(),
        });
    }
    let window = shared.window;
    let bounded = window.is_bounded();
    let ctx = TxContext {
        decoders: shared.decoders,
        focus: match &unit.token {
            AssetKey::Token(_, AddressBytes::Solana(mint)) => BTreeSet::from([*mint]),
            _ => BTreeSet::new(),
        },
        side: shared.side,
        policy: shared.policy,
    };
    let curve = shared.decoders.curve;
    let policy = shared.policy;
    let mut out = UnitOutcome {
        summary: SolanaTokenScanSummary::new(unit.token.clone(), TokenScanStatus::Ok),
        acc: TokenAcc::default(),
        side_hits: BTreeMap::new(),
        malformed: Vec::new(),
        unknown: Vec::new(),
        in_window: 0,
        raw_truncated: false,
        failure: None,
        failure_info: None,
        interrupted: false,
        cancelled: false,
    };
    let mut signatures: BTreeSet<[u8; 64]> = BTreeSet::new();

    let request = ScanRequest::TokenMarketActivity {
        asset: unit.token.clone(),
    };
    match shared.provider.plan(&request).await {
        Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
        Err(err) => out.failure = Some(err),
        Ok(_) => {
            let task = ScanTask {
                request,
                description: "buyer-intersect: token market activity".to_string(),
            };
            let mut stream = match unit.slice {
                Some(s) => shared.provider.scan_block_time_range(
                    task,
                    run.clone(),
                    Some(s.since),
                    Some(s.until),
                ),
                None => shared.provider.scan(task, run.clone()),
            };
            loop {
                // `None` = cut by the run cancel (stop of another unit or the
                // caller's cancel); `Some(None)` = natural end of the scan.
                let next = tokio::select! {
                    biased;
                    () = run.cancelled() => None,
                    item = stream.next() => Some(item),
                };
                let item = match next {
                    None => {
                        if user_cancel.is_cancelled() {
                            out.cancelled = true;
                        } else {
                            out.interrupted = true;
                        }
                        break;
                    }
                    Some(None) => break,
                    Some(Some(item)) => item,
                };
                let envelope = match item {
                    Ok(envelope) => envelope,
                    Err(err @ ProviderError::ConfigurationRequired { .. }) => return Err(err),
                    Err(err) => {
                        out.failure = Some(err);
                        break;
                    }
                };
                out.raw_truncated = out.raw_truncated || envelope.truncated;
                let RawPayload::SolanaTransaction(tx) = &envelope.payload else {
                    out.summary.unexpected_payloads =
                        out.summary.unexpected_payloads.saturating_add(1);
                    continue;
                };
                if let (Some(slice), Some(t)) = (unit.slice, tx.block_time)
                    && !(slice.since <= t && t < slice.until)
                {
                    // Not this slice's transaction (a provider that ignores
                    // the range, or a boundary re-delivery): the slice that
                    // owns `t` counts it, so it is never counted twice. It
                    // still proves the window start was passed.
                    if t < window.since {
                        out.summary.boundary_reached = true;
                    }
                    continue;
                }
                if !signatures.insert(tx.signature) {
                    // Idempotence (invariant 11): a re-delivered
                    // transaction is evaluated once.
                    continue;
                }
                out.summary.transactions_scanned =
                    out.summary.transactions_scanned.saturating_add(1);
                if bounded {
                    match tx.block_time {
                        Some(t) if t < window.since => out.summary.boundary_reached = true,
                        Some(_) => {}
                        // Older than the boundary tx by provider order:
                        // outside the window anyway.
                        None if out.summary.boundary_reached => {}
                        None => {
                            out.summary.missing_block_time =
                                out.summary.missing_block_time.saturating_add(1);
                        }
                    }
                    if !tx.block_time.is_some_and(|t| window.contains(t)) {
                        continue;
                    }
                    out.in_window = out.in_window.saturating_add(1);
                }
                let q = qualify_bonding_curve_buys_with_policy(tx, curve, policy);
                // The curve pass supplies the program-instruction
                // diagnostics; the sides and IdlOnly counts come from
                // the shared attribution (`qualify_transaction`).
                let mut tx_diagnostics = q.diagnostics;
                tx_diagnostics.unverified_variant_buys = Default::default();
                out.summary.diagnostics.add(&tx_diagnostics);
                for hex in q.unknown_discriminators {
                    if out.unknown.len() < MAX_UNKNOWN_DISCRIMINATOR_SAMPLES {
                        out.unknown.push(hex);
                    }
                }
                for reason in q.malformed_reasons {
                    if out.malformed.len() < MAX_MALFORMED_SAMPLES {
                        out.malformed.push(sanitize_provider_text(&reason));
                    }
                }
                for (mint, _owner) in &q.uninstructed_positive_deltas {
                    if ctx.focus.contains(mint) {
                        out.summary.positive_delta_without_instruction = out
                            .summary
                            .positive_delta_without_instruction
                            .saturating_add(1);
                    }
                }
                if q.diagnostics.out_of_scope_slot_transactions > 0 {
                    continue;
                }
                qualify_transaction(tx, &ctx, &mut out.summary, &mut out.acc, &mut out.side_hits);
            }
        }
    }
    Ok(UnitEnd::Done(Box::new(out)))
}

fn merge_evidence_slot(dst: &mut Option<SideEvidence>, src: Option<SideEvidence>) {
    let Some(src) = src else { return };
    match dst {
        None => *dst = Some(src),
        Some(d) => {
            let count = d.count.saturating_add(src.count);
            if (src.slot, src.transaction_index) < (d.slot, d.transaction_index) {
                *d = src;
            }
            d.count = count;
        }
    }
}

fn merge_side_hits(into: &mut SideHitMap, from: SideHitMap) {
    for (wallet, per_asset) in from {
        let dst = into.entry(wallet).or_default();
        for (asset, hits) in per_asset {
            let cell = dst.entry(asset).or_default();
            merge_evidence_slot(&mut cell.buy, hits.buy);
            merge_evidence_slot(&mut cell.sell, hits.sell);
        }
    }
}

const fn stop_kind(stop: ScanStop) -> ScanFailureKind {
    match stop {
        ScanStop::BudgetExhausted { limit } => ScanFailureKind::BudgetExhausted { limit },
        ScanStop::RateLimited { retry_after_secs } => {
            ScanFailureKind::RateLimited { retry_after_secs }
        }
    }
}

/// Fold the units of one token (slice order) into its summary. `None` = the
/// token is omitted (the caller cancelled before any of its units started).
///
/// Status rules: every unit not started and a run stop -> `NotScanned`; a
/// failed unit (first by slice order) or a unit cut in flight by the stop ->
/// `Failed`; some units not started after a stop -> `Failed` naming them;
/// any slice truncated -> token `truncated`, slices named.
fn merge_token(
    token: &AssetKey,
    slices: &[Option<SliceId>],
    parts: &[UnitEnd],
    stop: Option<ScanStop>,
    bounded: bool,
) -> Option<SolanaTokenScanSummary> {
    let mut summary = SolanaTokenScanSummary::new(token.clone(), TokenScanStatus::Ok);
    let mut acc = TokenAcc::default();
    let mut in_window = 0u64;
    let mut any_started = false;
    let mut truncated = false;
    let mut failed: Option<(ScanFailureKind, String)> = None;
    let mut unstarted: Vec<SliceId> = Vec::new();
    let label =
        |slice: &Option<SliceId>| slice.map_or_else(String::new, |s| format!("{}: ", s.describe()));
    for (slice, part) in slices.iter().zip(parts) {
        let UnitEnd::Done(done) = part else {
            if let Some(s) = slice {
                unstarted.push(*s);
            }
            continue;
        };
        any_started = true;
        let u = &done.summary;
        let s = u64::saturating_add;
        summary.transactions_scanned = s(summary.transactions_scanned, u.transactions_scanned);
        summary.missing_block_time = s(summary.missing_block_time, u.missing_block_time);
        summary.boundary_reached |= u.boundary_reached;
        summary.positive_delta_without_instruction = s(
            summary.positive_delta_without_instruction,
            u.positive_delta_without_instruction,
        );
        summary.unexpected_payloads = s(summary.unexpected_payloads, u.unexpected_payloads);
        summary.diagnostics.add(&u.diagnostics);
        summary.trade.add(&u.trade);
        in_window = s(in_window, done.in_window);
        acc.wallets.extend(done.acc.wallets.iter().cloned());
        acc.buyers.extend(done.acc.buyers.iter().cloned());
        acc.sellers.extend(done.acc.sellers.iter().cloned());
        if done.raw_truncated && !(bounded && u.boundary_reached) {
            truncated = true;
            if let Some(sl) = slice {
                summary.truncated_slices.push(*sl);
            }
        }
        if failed.is_none() {
            if let Some((kind, text)) = &done.failure_info {
                failed = Some((*kind, format!("{}{text}", label(slice))));
            } else if done.interrupted {
                failed = Some((
                    stop.map_or(ScanFailureKind::Other, stop_kind),
                    format!(
                        "{}interrupted in flight: {}",
                        label(slice),
                        stop.map_or_else(|| "run stopped".to_string(), |s| s.describe())
                    ),
                ));
            }
        }
    }
    if !any_started {
        return stop.map(|reason| {
            SolanaTokenScanSummary::new(token.clone(), TokenScanStatus::NotScanned { reason })
        });
    }
    let count = |s: &BTreeSet<WalletKey>| u64::try_from(s.len()).unwrap_or(u64::MAX);
    summary.qualified_buyers = count(&acc.buyers);
    summary.qualified_sellers = count(&acc.sellers);
    summary.qualified_wallets = count(&acc.wallets);
    summary.transactions_in_window = bounded.then_some(in_window);
    if !unstarted.is_empty() {
        match stop {
            Some(reason) => {
                if failed.is_none() {
                    let names = unstarted
                        .iter()
                        .map(SliceId::describe)
                        .collect::<Vec<_>>()
                        .join(", ");
                    failed = Some((
                        stop_kind(reason),
                        format!("not scanned ({names}): {}", reason.describe()),
                    ));
                }
            }
            None => {
                // Caller cancelled: the slices never scanned are a gap.
                truncated = true;
                summary.truncated_slices.extend(unstarted);
            }
        }
    }
    summary.truncated = truncated;
    if let Some((kind, message)) = failed {
        summary.status = TokenScanStatus::Failed { kind, message };
    }
    Some(summary)
}
