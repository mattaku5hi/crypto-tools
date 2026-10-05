//! EVM trade extraction (ADR-020 §2): owner-keyed net flows per transaction.
//!
//! A trade of wallet `W` is booked only when
//! a. `W == tx.from` (EOA signer; smart-wallet/AA ownership out of scope);
//! b. a **gated** venue swap event in the tx involves the traded token `T`
//!    (the event's emitter — pool or PoolManager — moved `T` in a `Transfer`
//!    of the same tx; `Swap.sender`/pool event amounts are never used);
//! c. `W`'s net deltas are exactly one traded token `T` and one quote asset
//!    `Q` (native with WETH/WBNB merged, or a configured quote token) of
//!    opposite sign;
//! d. consideration = `W`'s own net `Q` delta (exact); gas is fee-payer-only
//!    (`W == tx.from` pays it, including Base's L1 data fee).
//!
//! Native leg honesty: native flows that are not logs are `tx.value` (from
//! `tx.from`) and internal transfers. With `internal_transfers = None` the
//! native *inflow* of a sell cannot be seen: the trade is kept with
//! `Consideration::Unknown(NativeLegNotObserved)`, never priced from the pool
//! event. A native *buy* is booked from `tx.value` (exact unless the router
//! refunds unspent value through an unobserved internal transfer; the trade
//! carries `NativeLegStatus::LogsAndValueOnly` so callers can see it).
//!
//! WETH `Deposit`/`Withdrawal` have WETH9 semantics (no `Transfer`); when a
//! WETH implementation also emits the matching mint/burn `Transfer`, the
//! `Deposit`/`Withdrawal` is not counted a second time.
//!
//! Failed transactions never produce trades; only the fee is reported.
//! Pure, synchronous and deterministic.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256, I256, U256};
use scout_api::DecodeOutcome;
use scout_core::{ChainKey, EvmTxStatus, NativeSource, NetworkId, RawEvmTransaction};
use scout_dex_evm::{GateOutcome, SwapVenue, SwapVenueGate, VenueVerification};
use scout_evm::{
    EvmChainProfile, WrappedNativeKind, decode_erc20_transfer, decode_wrapped_native_event,
    is_erc721_transfer,
};

pub use scout_dex_solana::TradeSide;

pub const EVM_TRADE_EXTRACTION_VERSION: &str = "evm-trade-extraction-v1";

/// A configured quote token (stablecoin). Addresses are per chain and must
/// be pinned by the caller; nothing is assumed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmQuoteToken {
    pub address: Address,
    pub symbol: String,
}

/// Per-chain extraction configuration.
#[derive(Debug, Clone)]
pub struct EvmExtractionConfig {
    pub profile: EvmChainProfile,
    pub quote_tokens: Vec<EvmQuoteToken>,
    pub gate: SwapVenueGate,
}

impl EvmExtractionConfig {
    #[must_use]
    pub fn new(profile: EvmChainProfile, gate: SwapVenueGate) -> Self {
        Self {
            profile,
            quote_tokens: Vec::new(),
            gate,
        }
    }

    #[must_use]
    pub fn with_quote_token(mut self, address: Address, symbol: impl Into<String>) -> Self {
        self.quote_tokens.push(EvmQuoteToken {
            address,
            symbol: symbol.into(),
        });
        self
    }

    /// Config for a chain profile: a fresh venue gate and the profile's pinned
    /// quote tokens (Robinhood: USDG). Native/wrapped-native is merged and
    /// needs no entry.
    #[must_use]
    pub fn for_profile(profile: EvmChainProfile) -> Self {
        let mut cfg = Self::new(profile, SwapVenueGate::new(profile.chain_id));
        for q in profile.quote_assets {
            cfg.quote_tokens.push(EvmQuoteToken {
                address: q.address,
                symbol: q.symbol.to_string(),
            });
        }
        cfg
    }

    fn is_quote_token(&self, token: &Address) -> bool {
        self.quote_tokens.iter().any(|q| q.address == *token)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuoteAsset {
    /// Native ETH/BNB, WETH/WBNB merged.
    Native,
    Token(Address),
}

/// How the native quote leg was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLegStatus {
    /// Quote is not native.
    NotInvolved,
    /// Observed through a complete source (trace internal transfers, a
    /// complete explorer listing, or an archive balance difference).
    Observed(NativeSource),
    /// Only `tx.value` and logs; internal transfers not observed.
    LogsAndValueOnly,
}

impl NativeLegStatus {
    /// Stable label for reports (`not_involved`, `trace`,
    /// `explorer_internal`, `alchemy_internal`, `balance_diff`, `logs_and_value_only`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotInvolved => "not_involved",
            Self::Observed(s) => s.label(),
            Self::LogsAndValueOnly => "logs_and_value_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownConsideration {
    /// Native inflow could only arrive via an unobserved internal transfer.
    NativeLegNotObserved,
}

/// Quote amount of the trade in raw units of `quote` (wei for native).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consideration {
    Exact(U256),
    Unknown(UnknownConsideration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownFee {
    /// OP-stack chain but the receipt carried no `l1Fee`.
    L1FeeMissing,
    /// `gas_used * price` overflowed (corrupt input).
    Overflow,
}

/// Transaction fee in wei, paid by `tx.from` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvmFee {
    Known(U256),
    Unknown(UnknownFee),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmAttributedTrade {
    pub chain: ChainKey,
    pub tx_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub block_time: u64,
    pub wallet: Address,
    pub token: Address,
    pub side: TradeSide,
    /// Raw units of `token` the wallet's net delta moved.
    pub token_amount: U256,
    pub quote: QuoteAsset,
    pub consideration: Consideration,
    pub native_leg: NativeLegStatus,
    pub fee: EvmFee,
    /// Venue of the best gated swap event that involved `token`.
    pub venue: SwapVenue,
    pub venue_verification: VenueVerification,
    /// Contract that emitted the venue's swap event (pool / PoolManager /
    /// launchpad manager).
    pub venue_emitter: Address,
    /// Uniswap v4 pool id (`None` for v2/v3 pools and launchpads).
    pub venue_pool_id: Option<B256>,
    /// The venue emitter's token flow differs from the wallet's own net
    /// delta (fee-on-transfer / hook fee / multi-hop evidence). A label for
    /// the exit valuation (`transfer_tax_not_modelled`), not an accounting input.
    pub token_flow_shortfall: bool,
}

impl EvmAttributedTrade {
    /// Canonical chronological key (invariant #12).
    #[must_use]
    pub fn order_key(&self) -> (u64, u64) {
        (self.block_number, self.transaction_index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoTradeReason {
    /// `tx.chain` differs from the configured profile (invariant #3).
    ChainMismatch,
    /// Requested wallet is not `tx.from` (rule a).
    NotTxSigner,
    /// No non-quote token with a non-zero net delta for the wallet.
    NoTradedToken,
    /// Traded token is not the requested one.
    TokenFilterMismatch,
    /// More than one traded token or more than one quote asset.
    MultiAsset,
    /// Token and quote deltas have the same sign.
    SameSign,
    /// A token moved but no quote leg exists (transfer/airdrop-like).
    NoQuoteLeg,
    /// No gated venue swap event involves the traded token (rule b).
    NoVerifiedSwapEvent,
    /// A gated launchpad (four.meme) event names the traded token but its
    /// `account` is not the wallet: a router/bot contract traded, the wallet
    /// is not attributed (invariant #2). Counted, never booked.
    LaunchpadAccountNotWallet,
    /// A gated launchpad CURVE event (Pons V2 / Bags) names the wallet as
    /// buyer/seller but another `recipient` receives the output: a
    /// swap-with-receiver, counted and never booked (invariant #2).
    LaunchpadRecipientNotWallet,
    /// Broken log structure (invariant #18).
    MalformedLog(String),
    /// Delta arithmetic overflowed.
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvmTxOutcome {
    Trade(Box<EvmAttributedTrade>),
    NoTrade(NoTradeReason),
    /// Execution failed: no trade; the fee is still reported.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmTxExtraction {
    pub tx_hash: B256,
    pub wallet: Address,
    /// `Some` only when the wallet paid the fee (`wallet == tx.from`).
    pub fee: Option<EvmFee>,
    pub outcome: EvmTxOutcome,
    /// Swap-shaped logs at non-gated emitters (never evidence, counted).
    pub ungated_swap_logs: u32,
    /// ERC-721-shaped `Transfer` logs (Uniswap v4 PositionManager NFTs,
    /// other NFTs): not fungible flows, counted and never used.
    pub nft_transfer_logs: u32,
    /// Swap logs of gated, verified venues in the transaction (> 0 makes a
    /// rejection a "swap-shaped transaction that was not booked").
    pub gated_swap_logs: u32,
}

fn fee_of(tx: &RawEvmTransaction, profile: &EvmChainProfile) -> EvmFee {
    let Some(base) = U256::from(tx.gas_used).checked_mul(tx.effective_gas_price) else {
        return EvmFee::Unknown(UnknownFee::Overflow);
    };
    match (tx.l1_fee, profile.l1_fee_separate) {
        (Some(l1), _) => base
            .checked_add(l1)
            .map_or(EvmFee::Unknown(UnknownFee::Overflow), EvmFee::Known),
        (None, true) => EvmFee::Unknown(UnknownFee::L1FeeMissing),
        (None, false) => EvmFee::Known(base),
    }
}

fn signed(amount: U256) -> Option<I256> {
    I256::try_from(amount).ok()
}

/// Net-flow ledger of one wallet inside one transaction.
#[derive(Default)]
struct Flows {
    tokens: BTreeMap<Address, I256>,
    native: I256,
}

impl Flows {
    fn add_token(&mut self, token: Address, delta: I256) -> Option<()> {
        let e = self.tokens.entry(token).or_insert(I256::ZERO);
        *e = e.checked_add(delta)?;
        Some(())
    }

    fn add_native(&mut self, delta: I256) -> Option<()> {
        self.native = self.native.checked_add(delta)?;
        Some(())
    }
}

fn credit(
    flows: &mut Flows,
    token: Option<Address>,
    amount: U256,
    sign_positive: bool,
) -> Option<()> {
    let v = signed(amount)?;
    let d = if sign_positive { v } else { v.checked_neg()? };
    match token {
        Some(t) => flows.add_token(t, d),
        None => flows.add_native(d),
    }
}

/// Owner-keyed ERC-20 net deltas of `wallet` in `tx` (the wrapped native
/// token is excluded: it is merged into the native quote). ERC-721-shaped and
/// undecodable `Transfer` logs are skipped. `None` on arithmetic overflow.
/// Used by the ledger's inventory-continuity check.
pub(crate) fn wallet_token_deltas(
    tx: &RawEvmTransaction,
    wallet: Address,
    wrapped: Address,
) -> Option<BTreeMap<Address, I256>> {
    let mut flows = Flows::default();
    for log in &tx.logs {
        if is_erc721_transfer(log) {
            continue;
        }
        let DecodeOutcome::Decoded(t) = decode_erc20_transfer(log) else {
            continue;
        };
        if t.token == wrapped {
            continue;
        }
        if t.from == wallet && t.to != wallet {
            credit(&mut flows, Some(t.token), t.amount, false)?;
        } else if t.to == wallet && t.from != wallet {
            credit(&mut flows, Some(t.token), t.amount, true)?;
        }
    }
    Some(flows.tokens)
}

/// Extract the trade (if any) of one transaction.
///
/// `wallet = None` means "the signer" (`tx.from`); `Some(w)` with
/// `w != tx.from` yields `NotTxSigner`. `token_filter` restricts to one
/// traded token (token-centric scans).
#[must_use]
pub fn extract_evm_trade(
    tx: &RawEvmTransaction,
    cfg: &EvmExtractionConfig,
    wallet: Option<Address>,
    token_filter: Option<Address>,
) -> EvmTxExtraction {
    let w = wallet.unwrap_or(tx.from);
    let paid_fee = (w == tx.from).then(|| fee_of(tx, &cfg.profile));
    let mut ungated = 0u32;
    let nft =
        u32::try_from(tx.logs.iter().filter(|l| is_erc721_transfer(l)).count()).unwrap_or(u32::MAX);
    let gated = std::cell::Cell::new(0u32);
    let done = |outcome: EvmTxOutcome, ungated_swap_logs: u32| EvmTxExtraction {
        tx_hash: tx.hash,
        wallet: w,
        fee: paid_fee,
        outcome,
        ungated_swap_logs,
        nft_transfer_logs: nft,
        gated_swap_logs: gated.get(),
    };
    let no = |reason: NoTradeReason, ungated: u32| done(EvmTxOutcome::NoTrade(reason), ungated);

    if tx.chain.network_id != NetworkId::EvmChainId(cfg.profile.chain_id) {
        return no(NoTradeReason::ChainMismatch, 0);
    }
    if w != tx.from {
        return no(NoTradeReason::NotTxSigner, 0);
    }
    if tx.status == EvmTxStatus::Failed {
        return done(EvmTxOutcome::Failed, 0);
    }

    // --- Pass 1: decode ERC-20 transfers, WETH events, venue swaps.
    let wrapped = cfg.profile.wrapped_native;
    let mut flows = Flows::default();
    let mut transfers = Vec::new();
    // WETH implementations that also emit the mint/burn Transfer.
    let mut weth_mint_burn: BTreeSet<(bool, Address, U256)> = BTreeSet::new();
    let mut swaps = Vec::new();

    for log in &tx.logs {
        // ERC-721 mint/burn/transfer (e.g. Uniswap v4 PositionManager
        // position NFTs = liquidity add/remove): not a fungible flow.
        if is_erc721_transfer(log) {
            continue;
        }
        match decode_erc20_transfer(log) {
            DecodeOutcome::Decoded(t) => {
                if t.token == wrapped {
                    if t.from == Address::ZERO {
                        weth_mint_burn.insert((true, t.to, t.amount));
                    }
                    if t.to == Address::ZERO {
                        weth_mint_burn.insert((false, t.from, t.amount));
                    }
                }
                transfers.push(t);
            }
            DecodeOutcome::Malformed(m) => return no(NoTradeReason::MalformedLog(m), ungated),
            DecodeOutcome::NotMine => {}
        }
        match cfg.gate.classify(log) {
            GateOutcome::Verified(v) => {
                gated.set(gated.get().saturating_add(1));
                swaps.push(v);
            }
            GateOutcome::UngatedEmitter { .. } => ungated += 1,
            GateOutcome::Malformed(m) => return no(NoTradeReason::MalformedLog(m), ungated),
            GateOutcome::NotSwap => {}
        }
    }

    // --- Pass 2: wallet net flows.
    let mut ok = Some(());
    for t in &transfers {
        let token = (t.token != wrapped).then_some(t.token);
        if t.from == w && t.to != w {
            ok = ok.and_then(|()| credit(&mut flows, token, t.amount, false));
        } else if t.to == w && t.from != w {
            ok = ok.and_then(|()| credit(&mut flows, token, t.amount, true));
        }
    }
    for log in tx.logs.iter().filter(|l| l.address == wrapped) {
        if let DecodeOutcome::Decoded(e) = decode_wrapped_native_event(log) {
            if e.account != w {
                continue;
            }
            let (positive, key) = match e.kind {
                WrappedNativeKind::Deposit => (true, (true, e.account, e.amount)),
                WrappedNativeKind::Withdrawal => (false, (false, e.account, e.amount)),
            };
            if weth_mint_burn.contains(&key) {
                continue;
            }
            ok = ok.and_then(|()| credit(&mut flows, None, e.amount, positive));
        } else if let DecodeOutcome::Malformed(m) = decode_wrapped_native_event(log) {
            return no(NoTradeReason::MalformedLog(m), ungated);
        }
    }
    // Native: an archive balance difference of the signer is the whole
    // native movement (value, internal inflows, WETH withdraw proceeds; fee
    // excluded) and supersedes the pieces below. Otherwise `tx.value`
    // leaves tx.from (== w) and internal transfers count when observed.
    let balance_diff = tx.native_balance_diff.filter(|d| d.account == w);
    let internals_observed = tx.internal_transfers.is_some() || balance_diff.is_some();
    if let Some(d) = balance_diff {
        ok = ok.and_then(|()| flows.add_native(d.net_excl_fee));
    } else {
        ok = ok.and_then(|()| credit(&mut flows, None, tx.value, false));
        for it in tx.internal_transfers.iter().flatten() {
            if it.from == w && it.to != w {
                ok = ok.and_then(|()| credit(&mut flows, None, it.value, false));
            } else if it.to == w && it.from != w {
                ok = ok.and_then(|()| credit(&mut flows, None, it.value, true));
            }
        }
    }
    if ok.is_none() {
        return no(NoTradeReason::Overflow, ungated);
    }

    // --- Rule c: exactly one traded token and (at most) one quote asset.
    let traded: Vec<(Address, I256)> = flows
        .tokens
        .iter()
        .filter(|(t, d)| !cfg.is_quote_token(t) && **d != I256::ZERO)
        .map(|(t, d)| (*t, *d))
        .collect();
    let mut quotes: Vec<(QuoteAsset, I256)> = flows
        .tokens
        .iter()
        .filter(|(t, d)| cfg.is_quote_token(t) && **d != I256::ZERO)
        .map(|(t, d)| (QuoteAsset::Token(*t), *d))
        .collect();
    if flows.native != I256::ZERO {
        quotes.push((QuoteAsset::Native, flows.native));
    }
    let (token, token_delta) = match traded.as_slice() {
        [] => return no(NoTradeReason::NoTradedToken, ungated),
        [one] => *one,
        _ => return no(NoTradeReason::MultiAsset, ungated),
    };
    if token_filter.is_some_and(|f| f != token) {
        return no(NoTradeReason::TokenFilterMismatch, ungated);
    }

    // --- Rule b: a gated swap event whose emitter moved `token`; a launchpad
    // event (four.meme TokenManager) instead names token and account itself:
    // it counts only for the wallet it names (amounts still come from the
    // wallet's own deltas, never from the event).
    let involved = swaps
        .iter()
        .filter(|s| match s.launchpad {
            Some(lp) => lp.token == token && lp.account == w && lp.recipient.is_none_or(|r| r == w),
            None => transfers
                .iter()
                .any(|t| t.token == token && (t.from == s.emitter || t.to == s.emitter)),
        })
        .max_by_key(|s| s.verification);
    let Some(venue) = involved else {
        if swaps.iter().any(|s| {
            s.launchpad.is_some_and(|lp| {
                lp.token == token && lp.account == w && lp.recipient.is_some_and(|r| r != w)
            })
        }) {
            return no(NoTradeReason::LaunchpadRecipientNotWallet, ungated);
        }
        if swaps.iter().any(|s| {
            s.launchpad
                .is_some_and(|lp| lp.token == token && lp.account != w)
        }) {
            return no(NoTradeReason::LaunchpadAccountNotWallet, ungated);
        }
        return no(NoTradeReason::NoVerifiedSwapEvent, ungated);
    };

    let side = if token_delta.is_negative() {
        TradeSide::Sell
    } else {
        TradeSide::Buy
    };
    let token_amount = token_delta.unsigned_abs();
    // Evidence of a transfer tax: the emitter's own side of the token flow
    // (sent out on a buy, received on a sell) is not the wallet's delta.
    let shortfall = venue.launchpad.is_none() && {
        let emitter_side = transfers
            .iter()
            .filter(|t| t.token == token)
            .filter(|t| {
                if side == TradeSide::Buy {
                    t.from == venue.emitter
                } else {
                    t.to == venue.emitter
                }
            })
            .try_fold(U256::ZERO, |a, t| a.checked_add(t.amount));
        emitter_side.is_some_and(|e| e != token_amount)
    };
    let mk = |quote: QuoteAsset, consideration: Consideration, native_leg: NativeLegStatus| {
        EvmAttributedTrade {
            chain: tx.chain.clone(),
            tx_hash: tx.hash,
            block_number: tx.block_number,
            transaction_index: tx.transaction_index,
            block_time: tx.block_time,
            wallet: w,
            token,
            side,
            token_amount,
            quote,
            consideration,
            native_leg,
            fee: fee_of(tx, &cfg.profile),
            venue: venue.venue,
            venue_verification: venue.verification,
            venue_emitter: venue.emitter,
            venue_pool_id: venue.pool_id,
            token_flow_shortfall: shortfall,
        }
    };
    let native_status = if balance_diff.is_some() {
        NativeLegStatus::Observed(NativeSource::BalanceDiff)
    } else if internals_observed {
        NativeLegStatus::Observed(tx.native_source.unwrap_or(NativeSource::Explorer))
    } else {
        NativeLegStatus::LogsAndValueOnly
    };

    match quotes.as_slice() {
        [] => {
            // A sell whose native proceeds could only be an unobserved
            // internal transfer: keep the trade, consideration Unknown.
            if side == TradeSide::Sell && !internals_observed {
                done(
                    EvmTxOutcome::Trade(Box::new(mk(
                        QuoteAsset::Native,
                        Consideration::Unknown(UnknownConsideration::NativeLegNotObserved),
                        NativeLegStatus::LogsAndValueOnly,
                    ))),
                    ungated,
                )
            } else {
                no(NoTradeReason::NoQuoteLeg, ungated)
            }
        }
        [(quote, delta)] => {
            if delta.is_negative() == token_delta.is_negative() {
                return no(NoTradeReason::SameSign, ungated);
            }
            let leg = match quote {
                QuoteAsset::Native => native_status,
                QuoteAsset::Token(_) => NativeLegStatus::NotInvolved,
            };
            done(
                EvmTxOutcome::Trade(Box::new(mk(
                    *quote,
                    Consideration::Exact(delta.unsigned_abs()),
                    leg,
                ))),
                ungated,
            )
        }
        _ => no(NoTradeReason::MultiAsset, ungated),
    }
}

/// Counts over a batch (every transaction lands in exactly one bucket).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvmExtractionSummary {
    pub transactions: u64,
    pub duplicates_ignored: u64,
    /// ERC-721-shaped `Transfer` logs seen (never fungible flows).
    pub nft_transfer_logs: u64,
    pub trades: u64,
    pub failed: u64,
    pub unknown_consideration: u64,
    pub no_trade: BTreeMap<String, u64>,
    pub ungated_swap_logs: u64,
}

fn reason_label(r: &NoTradeReason) -> &'static str {
    match r {
        NoTradeReason::ChainMismatch => "chain_mismatch",
        NoTradeReason::NotTxSigner => "not_tx_signer",
        NoTradeReason::NoTradedToken => "no_traded_token",
        NoTradeReason::TokenFilterMismatch => "token_filter_mismatch",
        NoTradeReason::MultiAsset => "multi_asset",
        NoTradeReason::SameSign => "same_sign",
        NoTradeReason::NoQuoteLeg => "no_quote_leg",
        NoTradeReason::NoVerifiedSwapEvent => "no_verified_swap_event",
        NoTradeReason::LaunchpadAccountNotWallet => "launchpad_account_not_wallet",
        NoTradeReason::LaunchpadRecipientNotWallet => "launchpad_recipient_not_wallet",
        NoTradeReason::MalformedLog(_) => "malformed_log",
        NoTradeReason::Overflow => "overflow",
    }
}

/// Extract a batch: canonical `(block, tx_index)` order, duplicate tx hashes
/// (at-least-once delivery) processed once (invariants #11/#12).
#[must_use]
pub fn extract_evm_trades(
    txs: &[RawEvmTransaction],
    cfg: &EvmExtractionConfig,
    wallet: Option<Address>,
    token_filter: Option<Address>,
) -> (Vec<EvmTxExtraction>, EvmExtractionSummary) {
    let mut ordered: Vec<&RawEvmTransaction> = txs.iter().collect();
    ordered.sort_by_key(|t| (t.block_number, t.transaction_index, t.hash));
    let mut seen = BTreeSet::new();
    let mut summary = EvmExtractionSummary::default();
    let mut out = Vec::new();
    for tx in ordered {
        if !seen.insert((tx.chain.clone(), tx.hash)) {
            summary.duplicates_ignored += 1;
            continue;
        }
        let e = extract_evm_trade(tx, cfg, wallet, token_filter);
        summary.transactions += 1;
        summary.ungated_swap_logs += u64::from(e.ungated_swap_logs);
        summary.nft_transfer_logs += u64::from(e.nft_transfer_logs);
        match &e.outcome {
            EvmTxOutcome::Trade(t) => {
                summary.trades += 1;
                if matches!(t.consideration, Consideration::Unknown(_)) {
                    summary.unknown_consideration += 1;
                }
            }
            EvmTxOutcome::Failed => summary.failed += 1,
            EvmTxOutcome::NoTrade(r) => {
                *summary
                    .no_trade
                    .entry(reason_label(r).to_string())
                    .or_insert(0) += 1;
            }
        }
        out.push(e);
    }
    (out, summary)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, address};
    use scout_core::{InternalTransfer, RawEvmLog};
    use scout_dex_evm::V4_SWAP_TOPIC0;
    use scout_evm::{
        BASE, ROBINHOOD, TRANSFER_TOPIC0, WETH_DEPOSIT_TOPIC0, WETH_WITHDRAWAL_TOPIC0,
    };

    use super::*;

    const PM: Address = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
    const W: Address = address!("00000000000000000000000000000000000000a1");
    const ROUTER: Address = address!("00000000000000000000000000000000000000b2");
    const TOKEN: Address = address!("00000000000000000000000000000000000000c1");
    const OTHER: Address = address!("00000000000000000000000000000000000000c2");
    const USDC: Address = address!("00000000000000000000000000000000000000d1");

    fn weth() -> Address {
        ROBINHOOD.wrapped_native
    }

    fn lg(address: Address, topics: Vec<B256>, data: Vec<u8>) -> RawEvmLog {
        RawEvmLog {
            address,
            topics,
            data: Bytes::from(data),
            block_number: 10,
            transaction_index: 3,
            log_index: 0,
        }
    }

    fn amt(v: u64) -> Vec<u8> {
        U256::from(v).to_be_bytes::<32>().to_vec()
    }

    fn transfer(token: Address, from: Address, to: Address, v: u64) -> RawEvmLog {
        lg(
            token,
            vec![TRANSFER_TOPIC0, from.into_word(), to.into_word()],
            amt(v),
        )
    }

    fn deposit(account: Address, v: u64) -> RawEvmLog {
        lg(
            weth(),
            vec![WETH_DEPOSIT_TOPIC0, account.into_word()],
            amt(v),
        )
    }

    fn withdrawal(account: Address, v: u64) -> RawEvmLog {
        lg(
            weth(),
            vec![WETH_WITHDRAWAL_TOPIC0, account.into_word()],
            amt(v),
        )
    }

    fn v4_swap(emitter: Address) -> RawEvmLog {
        lg(
            emitter,
            vec![V4_SWAP_TOPIC0, B256::repeat_byte(1), ROUTER.into_word()],
            vec![0u8; 192],
        )
    }

    fn tx(logs: Vec<RawEvmLog>) -> RawEvmTransaction {
        RawEvmTransaction {
            chain: ROBINHOOD.verified_chain_key(),
            hash: B256::repeat_byte(0x11),
            from: W,
            to: Some(ROUTER),
            block_number: 10,
            transaction_index: 3,
            block_time: 1_777_567_931,
            value: U256::ZERO,
            status: EvmTxStatus::Success,
            gas_used: 100,
            effective_gas_price: U256::from(3u8),
            l1_fee: None,
            logs,
            internal_transfers: None,
            native_source: None,
            native_balance_diff: None,
        }
    }

    fn cfg() -> EvmExtractionConfig {
        EvmExtractionConfig::new(ROBINHOOD, SwapVenueGate::new(ROBINHOOD.chain_id))
            .with_quote_token(USDC, "USDC")
    }

    fn trade(e: &EvmTxExtraction) -> &EvmAttributedTrade {
        match &e.outcome {
            EvmTxOutcome::Trade(t) => t,
            other => panic!("expected trade, got {other:?}"),
        }
    }

    fn reason(e: &EvmTxExtraction) -> &NoTradeReason {
        match &e.outcome {
            EvmTxOutcome::NoTrade(r) => r,
            other => panic!("expected no-trade, got {other:?}"),
        }
    }

    #[test]
    fn weth_quoted_buy_is_exact() {
        let t = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.side, TradeSide::Buy);
        assert_eq!((tr.token, tr.token_amount), (TOKEN, U256::from(100u8)));
        assert_eq!(tr.quote, QuoteAsset::Native);
        assert_eq!(tr.consideration, Consideration::Exact(U256::from(5u8)));
        assert_eq!(tr.venue, SwapVenue::UniswapV4);
        assert_eq!(tr.venue_verification, VenueVerification::FixtureVerified);
        assert_eq!(tr.fee, EvmFee::Known(U256::from(300u16)));
        assert_eq!(e.fee, Some(tr.fee));
    }

    #[test]
    fn weth_quoted_sell_is_exact() {
        let t = tx(vec![
            transfer(TOKEN, W, PM, 100),
            transfer(weth(), PM, W, 9),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.side, TradeSide::Sell);
        assert_eq!(tr.consideration, Consideration::Exact(U256::from(9u8)));
    }

    #[test]
    fn usdc_quoted_buy_and_sell_are_exact() {
        let buy = tx(vec![
            transfer(USDC, W, PM, 2_000_000),
            transfer(TOKEN, PM, W, 50),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&buy, &cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.quote, QuoteAsset::Token(USDC));
        assert_eq!(tr.native_leg, NativeLegStatus::NotInvolved);
        assert_eq!(
            (tr.side, tr.consideration),
            (
                TradeSide::Buy,
                Consideration::Exact(U256::from(2_000_000u64))
            )
        );
        let sell = tx(vec![
            transfer(TOKEN, W, PM, 50),
            transfer(USDC, PM, W, 1_500_000),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&sell, &cfg(), None, None);
        assert_eq!(
            trade(&e).consideration,
            Consideration::Exact(U256::from(1_500_000u64))
        );
    }

    #[test]
    fn native_eth_buy_via_tx_value_is_exact_and_router_is_irrelevant() {
        for to in [Some(ROUTER), Some(PM), None] {
            let mut t = tx(vec![
                deposit(ROUTER, 7),
                transfer(weth(), ROUTER, PM, 7),
                transfer(TOKEN, PM, W, 100),
                v4_swap(PM),
            ]);
            t.value = U256::from(7u8);
            t.to = to;
            let e = extract_evm_trade(&t, &cfg(), None, None);
            let tr = trade(&e);
            assert_eq!(tr.side, TradeSide::Buy);
            assert_eq!(tr.quote, QuoteAsset::Native);
            assert_eq!(tr.consideration, Consideration::Exact(U256::from(7u8)));
            assert_eq!(tr.native_leg, NativeLegStatus::LogsAndValueOnly);
        }
    }

    #[test]
    fn native_sell_without_internal_transfers_is_unknown_not_priced_from_pool() {
        let t = tx(vec![
            transfer(TOKEN, W, PM, 100),
            transfer(weth(), PM, ROUTER, 5),
            withdrawal(ROUTER, 5),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.side, TradeSide::Sell);
        assert_eq!(
            tr.consideration,
            Consideration::Unknown(UnknownConsideration::NativeLegNotObserved)
        );
        assert_eq!(tr.token_amount, U256::from(100u8));
    }

    #[test]
    fn native_sell_with_internal_transfers_is_exact() {
        let mut t = tx(vec![
            transfer(TOKEN, W, PM, 100),
            transfer(weth(), PM, ROUTER, 5),
            withdrawal(ROUTER, 5),
            v4_swap(PM),
        ]);
        t.internal_transfers = Some(vec![
            InternalTransfer {
                from: ROUTER,
                to: W,
                value: U256::from(5u8),
            },
            InternalTransfer {
                from: PM,
                to: ROUTER,
                value: U256::from(99u8),
            },
        ]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.consideration, Consideration::Exact(U256::from(5u8)));
        assert_eq!(
            tr.native_leg,
            NativeLegStatus::Observed(NativeSource::Explorer)
        );
        // Observed and empty: nothing came back, so no quote leg at all.
        t.internal_transfers = Some(vec![]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoQuoteLeg);
    }

    #[test]
    fn multi_asset_is_rejected() {
        let two_tokens = tx(vec![
            transfer(TOKEN, PM, W, 100),
            transfer(OTHER, PM, W, 1),
            transfer(weth(), W, PM, 5),
            v4_swap(PM),
        ]);
        assert_eq!(
            reason(&extract_evm_trade(&two_tokens, &cfg(), None, None)),
            &NoTradeReason::MultiAsset
        );
        let two_quotes = tx(vec![
            transfer(TOKEN, PM, W, 100),
            transfer(weth(), W, PM, 5),
            transfer(USDC, W, PM, 5),
            v4_swap(PM),
        ]);
        assert_eq!(
            reason(&extract_evm_trade(&two_quotes, &cfg(), None, None)),
            &NoTradeReason::MultiAsset
        );
    }

    #[test]
    fn no_gated_swap_event_means_no_trade() {
        let flows = vec![transfer(weth(), W, PM, 5), transfer(TOKEN, PM, W, 100)];
        let e = extract_evm_trade(&tx(flows.clone()), &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
        // Same shape at a non-gated emitter is counted but not evidence.
        let mut with_fake = flows.clone();
        with_fake.push(v4_swap(Address::repeat_byte(0x99)));
        let e = extract_evm_trade(&tx(with_fake), &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
        assert_eq!(e.ungated_swap_logs, 1);
        // A gated swap that did not move the traded token is not enough.
        let unrelated = vec![
            transfer(weth(), W, ROUTER, 5),
            transfer(TOKEN, ROUTER, W, 100),
            v4_swap(PM),
        ];
        let e = extract_evm_trade(&tx(unrelated), &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
    }

    #[test]
    fn failed_tx_reports_fee_only() {
        let mut t = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        t.status = EvmTxStatus::Failed;
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(e.outcome, EvmTxOutcome::Failed);
        assert_eq!(e.fee, Some(EvmFee::Known(U256::from(300u16))));
    }

    #[test]
    fn base_fee_includes_l1_fee_and_flags_a_missing_one() {
        let base_cfg = EvmExtractionConfig::new(BASE, SwapVenueGate::new(BASE.chain_id));
        let mut t = tx(vec![]);
        t.chain = BASE.verified_chain_key();
        t.l1_fee = Some(U256::from(1000u16));
        let e = extract_evm_trade(&t, &base_cfg, None, None);
        assert_eq!(e.fee, Some(EvmFee::Known(U256::from(1300u16))));
        t.l1_fee = None;
        let e = extract_evm_trade(&t, &base_cfg, None, None);
        assert_eq!(e.fee, Some(EvmFee::Unknown(UnknownFee::L1FeeMissing)));
    }

    #[test]
    fn only_the_signer_books_and_pays() {
        let t = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&t, &cfg(), Some(ROUTER), None);
        assert_eq!(reason(&e), &NoTradeReason::NotTxSigner);
        assert_eq!(e.fee, None, "sponsored fee is not the wallet's cost");
    }

    #[test]
    fn chain_mismatch_and_token_filter_and_same_sign() {
        let mut t = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        assert_eq!(
            reason(&extract_evm_trade(&t, &cfg(), None, Some(OTHER))),
            &NoTradeReason::TokenFilterMismatch
        );
        let same = tx(vec![
            transfer(USDC, PM, W, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        assert_eq!(
            reason(&extract_evm_trade(&same, &cfg(), None, None)),
            &NoTradeReason::SameSign
        );
        t.chain = BASE.verified_chain_key();
        assert_eq!(
            reason(&extract_evm_trade(&t, &cfg(), None, None)),
            &NoTradeReason::ChainMismatch
        );
    }

    #[test]
    fn plain_transfer_in_has_no_quote_leg() {
        let t = tx(vec![transfer(TOKEN, PM, W, 100), v4_swap(PM)]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoQuoteLeg);
    }

    #[test]
    fn weth_mint_transfer_is_not_double_counted_with_deposit() {
        // W wraps its own 5 wei (tx.value) in a WETH variant that emits
        // Transfer(0 -> W) next to Deposit(W), then pays the WETH to the
        // pool. Double counting would leave +5 WETH and a zero quote.
        let mut t = tx(vec![
            transfer(weth(), Address::ZERO, W, 5),
            deposit(W, 5),
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        t.value = U256::from(5u8);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(
            trade(&e).consideration,
            Consideration::Exact(U256::from(5u8))
        );
    }

    #[test]
    fn malformed_transfer_log_surfaces() {
        // ERC-20 shape (3 topics) with a truncated amount.
        let bad = lg(
            TOKEN,
            vec![TRANSFER_TOPIC0, W.into_word(), PM.into_word()],
            vec![0; 31],
        );
        let e = extract_evm_trade(&tx(vec![bad]), &cfg(), None, None);
        assert!(matches!(reason(&e), NoTradeReason::MalformedLog(_)));
    }

    #[test]
    fn erc721_transfers_are_counted_and_never_fungible_flows() {
        // Uniswap v4 PositionManager NFT mint next to a real swap: the NFT
        // log must neither abort the trade nor count as a token flow.
        let nft = lg(
            OTHER,
            vec![
                TRANSFER_TOPIC0,
                Address::ZERO.into_word(),
                W.into_word(),
                B256::repeat_byte(7),
            ],
            vec![],
        );
        let t = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            nft.clone(),
            v4_swap(PM),
        ]);
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(trade(&e).token, TOKEN);
        assert_eq!(e.nft_transfer_logs, 1);
        // Alone (an LP add/remove): no fungible flow at all.
        let e = extract_evm_trade(&tx(vec![nft]), &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoTradedToken);
        assert_eq!(e.nft_transfer_logs, 1);
    }

    #[test]
    fn batch_is_canonically_ordered_and_idempotent() {
        let mut a = tx(vec![
            transfer(weth(), W, PM, 5),
            transfer(TOKEN, PM, W, 100),
            v4_swap(PM),
        ]);
        a.block_number = 20;
        a.hash = B256::repeat_byte(1);
        let mut b = a.clone();
        b.block_number = 5;
        b.hash = B256::repeat_byte(2);
        let (out, sum) = extract_evm_trades(&[a.clone(), b, a], &cfg(), None, None);
        assert_eq!(sum.transactions, 2);
        assert_eq!(sum.duplicates_ignored, 1);
        assert_eq!(sum.trades, 2);
        let blocks: Vec<u64> = out.iter().map(|e| trade(e).block_number).collect();
        assert_eq!(blocks, vec![5, 20]);
    }

    // --- four.meme (BSC launchpad): the TokenManager names token + account.
    const FM_V2: Address = address!("5c952063c7fc8610FFDB798152D69F0B9550762b");

    fn bsc_tx(logs: Vec<RawEvmLog>, value: u64) -> RawEvmTransaction {
        let mut t = tx(logs);
        t.chain = scout_evm::BSC.verified_chain_key();
        t.value = U256::from(value);
        t
    }

    fn bsc_cfg() -> EvmExtractionConfig {
        EvmExtractionConfig::for_profile(scout_evm::BSC)
    }

    /// V2 `TokenPurchase`/`TokenSale` event of `account` on `token`.
    fn fm_v2(buy: bool, token: Address, account: Address) -> RawEvmLog {
        let mut data = vec![0u8; 256];
        data[12..32].copy_from_slice(token.as_slice());
        data[44..64].copy_from_slice(account.as_slice());
        let topic = if buy {
            scout_dex_evm::FOURMEME_V2_PURCHASE_TOPIC0
        } else {
            scout_dex_evm::FOURMEME_V2_SALE_TOPIC0
        };
        lg(FM_V2, vec![topic], data)
    }

    #[test]
    fn fourmeme_buy_for_the_signer_is_booked_from_the_wallets_own_deltas() {
        // BNB in via tx.value (amounts come from deltas, not from the event).
        let t = bsc_tx(
            vec![transfer(TOKEN, FM_V2, W, 1_000), fm_v2(true, TOKEN, W)],
            7,
        );
        let e = extract_evm_trade(&t, &bsc_cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(
            (tr.side, tr.token, tr.token_amount),
            (TradeSide::Buy, TOKEN, U256::from(1_000u16))
        );
        assert_eq!(tr.consideration, Consideration::Exact(U256::from(7u8)));
        assert_eq!(tr.venue, SwapVenue::FourMemeV2);
        // V2 is FixtureVerified (ADR-020 amendment 7, n = 4); V1 stays IdlOnly.
        assert_eq!(tr.venue_verification, VenueVerification::FixtureVerified);
        assert_eq!(e.gated_swap_logs, 1);
    }

    #[test]
    fn fourmeme_event_naming_another_account_does_not_attribute_the_signer() {
        // A bot/router contract is the event's `account`; the signer ends up
        // with the tokens after a forward. Not attributed, counted.
        let bot = Address::repeat_byte(0x99);
        let t = bsc_tx(
            vec![
                transfer(TOKEN, FM_V2, bot, 1_000),
                transfer(TOKEN, bot, W, 1_000),
                fm_v2(true, TOKEN, bot),
            ],
            7,
        );
        let e = extract_evm_trade(&t, &bsc_cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::LaunchpadAccountNotWallet);
        assert_eq!(e.gated_swap_logs, 1);
        let (_, sum) = extract_evm_trades(&[t], &bsc_cfg(), None, None);
        assert_eq!(sum.no_trade.get("launchpad_account_not_wallet"), Some(&1));
        // An event for another token is not evidence for this one either.
        let t = bsc_tx(
            vec![transfer(TOKEN, FM_V2, W, 1_000), fm_v2(true, OTHER, W)],
            7,
        );
        let e = extract_evm_trade(&t, &bsc_cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
        // The same event from an unpinned address is not evidence at all.
        let mut fake = fm_v2(true, TOKEN, W);
        fake.address = Address::repeat_byte(0x55);
        let t = bsc_tx(vec![transfer(TOKEN, FM_V2, W, 1_000), fake], 7);
        let e = extract_evm_trade(&t, &bsc_cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
        assert_eq!(e.ungated_swap_logs, 1);
    }

    #[test]
    fn fourmeme_sell_without_observed_native_inflow_is_unknown_never_priced_from_the_event() {
        let t = bsc_tx(
            vec![transfer(TOKEN, W, FM_V2, 1_000), fm_v2(false, TOKEN, W)],
            0,
        );
        let e = extract_evm_trade(&t, &bsc_cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(tr.side, TradeSide::Sell);
        assert_eq!(
            tr.consideration,
            Consideration::Unknown(UnknownConsideration::NativeLegNotObserved)
        );
    }

    // --- Robinhood launchpad curves (Pons V2): the CURVE emits; it is
    // admitted by the factory's record and names account + recipient.
    const PONS_FACTORY: Address = address!("7eD598BcEf8bd9Edd8C97A195C6d13f40801EC7e");
    const CURVE: Address = address!("00000000000000000000000000000000000000e1");

    fn curve_cfg() -> EvmExtractionConfig {
        let mut gate = SwapVenueGate::new(ROBINHOOD.chain_id);
        gate.admit_curve(
            SwapVenue::PonsV2Curve,
            CURVE,
            &scout_dex_evm::CurveMetadata {
                factory: Some(PONS_FACTORY),
                token: Some(TOKEN),
                quote: Some(Address::ZERO),
                registered_curve: Some(CURVE),
            },
        )
        .unwrap();
        EvmExtractionConfig::new(ROBINHOOD, gate)
    }

    /// `CurveBuy`/`CurveSell` of `account` paying out to `recipient`.
    fn pons_event(buy: bool, account: Address, recipient: Address) -> RawEvmLog {
        let topic = if buy {
            scout_dex_evm::PONS_V2_CURVE_BUY_TOPIC0
        } else {
            scout_dex_evm::PONS_V2_CURVE_SELL_TOPIC0
        };
        lg(
            CURVE,
            vec![topic, account.into_word(), recipient.into_word()],
            vec![0u8; 128],
        )
    }

    #[test]
    fn pons_curve_buy_for_the_signer_is_booked_from_the_wallets_own_deltas() {
        let t = tx(vec![
            transfer(TOKEN, CURVE, W, 1_000),
            pons_event(true, W, W),
        ]);
        let mut t = t;
        t.value = U256::from(7u8);
        let e = extract_evm_trade(&t, &curve_cfg(), None, None);
        let tr = trade(&e);
        assert_eq!(
            (
                tr.side,
                tr.token,
                tr.token_amount,
                tr.venue,
                tr.venue_emitter
            ),
            (
                TradeSide::Buy,
                TOKEN,
                U256::from(1_000u16),
                SwapVenue::PonsV2Curve,
                CURVE
            )
        );
        assert_eq!(tr.consideration, Consideration::Exact(U256::from(7u8)));
        // IdlOnly until a fixture passes evm_robinhood_launchpads.rs.
        assert_eq!(tr.venue_verification, VenueVerification::IdlOnly);
        assert_eq!(e.gated_swap_logs, 1);
        // A curve that nobody admitted is not evidence (invariant #16).
        let e = extract_evm_trade(&t, &cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::NoVerifiedSwapEvent);
        assert_eq!(e.ungated_swap_logs, 1);
    }

    #[test]
    fn pons_curve_events_for_another_account_or_receiver_are_not_attributed() {
        // A router is the buyer and forwards the tokens to the signer.
        let router = Address::repeat_byte(0x99);
        let t = tx(vec![
            transfer(TOKEN, CURVE, router, 1_000),
            transfer(TOKEN, router, W, 1_000),
            pons_event(true, router, router),
        ]);
        let e = extract_evm_trade(&t, &curve_cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::LaunchpadAccountNotWallet);
        // The signer sells but the proceeds go to another recipient.
        let t = tx(vec![
            transfer(TOKEN, W, CURVE, 1_000),
            pons_event(false, W, router),
        ]);
        let e = extract_evm_trade(&t, &curve_cfg(), None, None);
        assert_eq!(reason(&e), &NoTradeReason::LaunchpadRecipientNotWallet);
        assert_eq!(e.gated_swap_logs, 1);
        let (_, sum) = extract_evm_trades(&[t], &curve_cfg(), None, None);
        assert_eq!(sum.no_trade.get("launchpad_recipient_not_wallet"), Some(&1));
    }
}
