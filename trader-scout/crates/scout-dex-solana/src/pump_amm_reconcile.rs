//! Per-transaction reconciliation of paired PumpSwap AMM trades against the
//! user's owner-keyed balance deltas (the evidence behind
//! [`crate::PumpAmmTradeVariant::verification`] and the router-forward
//! guard of invariant #2).
//!
//! For every distinct `user` (IDL account `user`) of the transaction the
//! paired events give the expected signed quote/base movement per mint:
//!
//! - buy: base `+base_amount_out`, quote `-(quote_amount_in_with_lp_fee +
//!   protocol_fee + coin_creator_fee)`;
//! - sell: base `-base_amount_in`, quote `+quote_amount_out_without_lp_fee -
//!   protocol_fee - coin_creator_fee`.
//!
//! The actual movement per mint is the owner-keyed token delta (all of the
//! user's token accounts of that mint; closed accounts count as 0 after).
//! For wrapped SOL the user's native lamport delta is added, with the
//! transaction fee added back when the user is the fee payer, because
//! wSOL is routinely wrapped/unwrapped inside the transaction (a wSOL ATA
//! created and closed in one transaction has no token delta at all).
//!
//! `residual = actual - expected`. A nonzero quote-leg residual is not
//! classified further here: it is rent deposits (user volume accumulator,
//! token-account creation), tips and bot/router platform fees, which ADR-010
//! §5 keeps out of trading cost. The result carries it so the caller can
//! expose it as a diagnostic.

use std::collections::BTreeMap;

use scout_core::{RawSolanaTransaction, SolanaExecutionStatus, SolanaPubkey};

use crate::bonding_curve_buy::TradeSide;
use crate::pump_amm::{DecodedPumpAmmTrade, PumpAmmDecoder, WRAPPED_SOL_MINT};
use crate::pump_amm_event::{AmmPairingReport, AmmTradeEventPairing, pair_amm_trades_with_events};

/// One mint's expected vs actual movement for one user in one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmMintLeg {
    pub mint: SolanaPubkey,
    /// The mint is the pool BASE mint of at least one of the user's trades.
    pub is_base: bool,
    /// The mint is the pool QUOTE mint of at least one of the user's trades.
    pub is_quote: bool,
    /// Signed raw amount implied by the paired events.
    pub expected: i128,
    /// Signed raw amount actually moved (see module docs for wSOL).
    pub actual: i128,
}

impl AmmMintLeg {
    /// `actual - expected`.
    #[must_use]
    pub fn residual(&self) -> i128 {
        self.actual.saturating_sub(self.expected)
    }
}

/// How the user's balance movement relates to the paired trade events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmmAttribution {
    /// Every leg reconciles exactly.
    Exact,
    /// Base legs exact; the quote leg is off by `residual` (rent, tips,
    /// platform fees, ...), nonzero actual quote movement.
    QuoteResidual,
    /// Base legs exact; the user's quote movement is exactly zero (the
    /// quote was supplied by another account of the transaction).
    QuoteFundedElsewhere,
    /// The user's actual movement is zero on every leg although the trades
    /// moved assets: a router/relayer forwarded everything. NOT attributed.
    NoUserDelta,
    /// A base leg is nonzero but not equal to the event amount.
    BaseLegMismatch,
    /// At least one of the user's trades has no consistent paired event.
    Unpaired,
    /// Event arithmetic over/underflowed (malformed event).
    ConsiderationInvalid,
}

/// Reconciliation of one user's trades within a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmmUserReconciliation {
    pub user: SolanaPubkey,
    pub user_is_signer: bool,
    pub user_is_fee_payer: bool,
    /// Indices into [`AmmTxReconciliation::pairing`]`.trades`.
    pub trade_indices: Vec<usize>,
    pub legs: Vec<AmmMintLeg>,
    pub attribution: AmmAttribution,
}

/// Reconciliation of one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmmTxReconciliation {
    pub succeeded: bool,
    pub pairing: AmmPairingReport,
    /// Empty for a failed transaction (its data describes nothing that
    /// happened on chain).
    pub users: Vec<AmmUserReconciliation>,
}

/// Pair and reconcile every AMM trade of `tx`.
#[must_use]
pub fn reconcile_pump_amm_transaction(
    decoder: &PumpAmmDecoder,
    tx: &RawSolanaTransaction,
) -> AmmTxReconciliation {
    let pairing =
        pair_amm_trades_with_events(decoder, &tx.instructions, tx.slot, tx.transaction_index);
    let succeeded = matches!(tx.execution, SolanaExecutionStatus::Succeeded);
    if !succeeded {
        return AmmTxReconciliation {
            succeeded,
            pairing,
            users: Vec::new(),
        };
    }
    let mut by_user: BTreeMap<SolanaPubkey, Vec<usize>> = BTreeMap::new();
    for (i, p) in pairing.trades.iter().enumerate() {
        by_user.entry(p.trade.user).or_default().push(i);
    }
    let users = by_user
        .into_iter()
        .map(|(user, idx)| reconcile_user(user, idx, &pairing, tx))
        .collect();
    AmmTxReconciliation {
        succeeded,
        pairing,
        users,
    }
}

fn reconcile_user(
    user: SolanaPubkey,
    trade_indices: Vec<usize>,
    pairing: &AmmPairingReport,
    tx: &RawSolanaTransaction,
) -> AmmUserReconciliation {
    let user_is_signer = tx.signers.contains(&user);
    let user_is_fee_payer = tx.fee_payer == user;
    let mut legs: BTreeMap<SolanaPubkey, AmmMintLeg> = BTreeMap::new();
    let mut unpaired = false;
    let mut invalid = false;
    for &i in &trade_indices {
        let Some(p) = pairing.trades.get(i) else {
            unpaired = true;
            continue;
        };
        let AmmTradeEventPairing::Paired(ev) = &p.pairing else {
            unpaired = true;
            continue;
        };
        let (Some(consideration), base) = (ev.quote_consideration(), ev.base_amount()) else {
            invalid = true;
            continue;
        };
        let (base_signed, quote_signed) = signed_legs(&p.trade, base, consideration);
        add_leg(&mut legs, p.trade.base_mint, true, base_signed);
        add_leg(&mut legs, p.trade.quote_mint, false, quote_signed);
    }
    for leg in legs.values_mut() {
        leg.actual = actual_delta(tx, &user, &leg.mint, user_is_fee_payer);
    }
    let legs: Vec<AmmMintLeg> = legs.into_values().collect();
    let attribution = if invalid {
        AmmAttribution::ConsiderationInvalid
    } else if unpaired {
        AmmAttribution::Unpaired
    } else {
        classify(&legs)
    };
    AmmUserReconciliation {
        user,
        user_is_signer,
        user_is_fee_payer,
        trade_indices,
        legs,
        attribution,
    }
}

/// `(base, quote)` signed expected movement of one trade.
fn signed_legs(trade: &DecodedPumpAmmTrade, base: u64, consideration: u64) -> (i128, i128) {
    match trade.side {
        TradeSide::Buy => (i128::from(base), -i128::from(consideration)),
        TradeSide::Sell => (-i128::from(base), i128::from(consideration)),
    }
}

fn add_leg(
    legs: &mut BTreeMap<SolanaPubkey, AmmMintLeg>,
    mint: SolanaPubkey,
    is_base: bool,
    amount: i128,
) {
    let leg = legs.entry(mint).or_insert(AmmMintLeg {
        mint,
        is_base: false,
        is_quote: false,
        expected: 0,
        actual: 0,
    });
    if is_base {
        leg.is_base = true;
    } else {
        leg.is_quote = true;
    }
    // Bounded: at most one u64-sized term per instruction.
    leg.expected = leg.expected.saturating_add(amount);
}

fn actual_delta(
    tx: &RawSolanaTransaction,
    user: &SolanaPubkey,
    mint: &SolanaPubkey,
    user_is_fee_payer: bool,
) -> i128 {
    let mut total: i128 = tx
        .token_balance_changes
        .iter()
        .filter(|c| c.owner.as_ref() == Some(user) && c.mint == *mint)
        .map(|c| i128::from(c.post_amount) - i128::from(c.pre_amount.unwrap_or(0)))
        .sum();
    if *mint == WRAPPED_SOL_MINT {
        total = total.saturating_add(
            tx.native_balance_changes
                .iter()
                .filter(|n| n.account == *user)
                .map(scout_core::SolanaNativeBalanceChange::delta)
                .sum::<i128>(),
        );
        if user_is_fee_payer {
            total = total.saturating_add(i128::from(tx.fee_lamports));
        }
    }
    total
}

fn classify(legs: &[AmmMintLeg]) -> AmmAttribution {
    if legs.iter().all(|l| l.residual() == 0) {
        return AmmAttribution::Exact;
    }
    if legs.iter().all(|l| l.actual == 0) {
        return AmmAttribution::NoUserDelta;
    }
    // A mint that is a base mint of any trade is checked as a base leg.
    let base_exact = legs.iter().filter(|l| l.is_base).all(|l| l.residual() == 0);
    if !base_exact {
        return AmmAttribution::BaseLegMismatch;
    }
    let quote_only = || legs.iter().filter(|l| !l.is_base && l.is_quote);
    if quote_only().all(|l| l.actual == 0) {
        AmmAttribution::QuoteFundedElsewhere
    } else {
        AmmAttribution::QuoteResidual
    }
}
