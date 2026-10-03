//! Per-transaction buy qualification for Solana, pump.fun bonding curve
//! scope only (ADR-003).
//!
//! A wallet W qualifies as a buyer of mint M in one transaction iff
//!
//! 1. a decoded bonding-curve buy instruction of a
//!    [`VariantVerification::FixtureVerified`] variant (`buy`, `buy_exact_sol_in` as of v4, `buy_v2`, `buy_exact_quote_in_v2`;
//!    see the decoder's arg-length policy)
//!    exists whose `user == W` and `mint == M` (instruction evidence:
//!    confirmed program `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`,
//!    IDL commit `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`), AND
//! 2. W's net token delta for M in this transaction, summed per OWNER
//!    over `token_balance_changes` (never per token account, never by
//!    position), is strictly positive.
//!
//! A decoded buy of an `IdlOnly` variant (none ships as of qualification
//! v4, when `buy_exact_sol_in` was promoted; the gate stays for any
//! future variant and is exercised through an injected [`VariantPolicy`])
//! whose owner delta WOULD qualify is NOT a
//! qualified buy: it is reported in `unverified_buys` and counted in
//! `unverified_variant_buys` so the caller can mark the buyer set a
//! knowing lower bound (invariants 16 and 18). Known non-trade program
//! instructions are counted; an unknown discriminator under the program
//! is a coverage gap, never skipped.
//!
//! A transaction with a failed execution status (`meta.err` non-null)
//! never yields a buy or any buy-shaped diagnostic; it is only counted
//! in `failed_transactions`.
//!
//! Anything else is not a buy: a decoded buy with a net delta <= 0
//! (atomic roundtrip, ACCEPTANCE B06) or without any owner-keyed
//! balance evidence; a positive delta with no decoded buy instruction
//! for that owner (stays `Unknown`, only counted); a `sell`.
//!
//! The balance-aggregation crate deliberately labels flows `Unknown`
//! and leaves multi-gainer mints `Ambiguous`; decoded instruction
//! evidence is what resolves that here, independently per user. The
//! qualifying flow handed to `classify_buy` is tagged `Swap` ONLY
//! because a confirmed decoder produced it.
//!
//! `DecodeOutcome::Malformed` for the confirmed program is never
//! skipped (AGENTS.md invariant 18): it is counted and the caller must
//! treat coverage as incomplete.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::I256;
use scout_api::{DeploymentScope, TxDecoder};
use scout_core::{
    AddressBytes, AssetKey, ChainFamily, ChainKey, GenesisIdentity, NetworkId,
    RawSolanaTransaction, SignedAmount, SolanaCluster, SolanaPubkey, WalletKey,
};
use scout_dex_solana::{
    BondingCurveBuyDecoder, PumpInstructionOutcome, PumpTradeVariant, TradeSide,
    VariantVerification, hex8,
};
use scout_normalize::{
    ActionKind, AssetFlow, AttributionEvidence, AttributionStatus, NetDeltaInput, classify_buy,
    solana_owner_net_deltas,
};

/// Confirmed pump.fun bonding-curve program
/// (`docs/p0/deployment-registry.md`, "2026-10-01 confirmation").
pub const PUMP_BONDING_CURVE_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
/// Official IDL commit (`pump-fun/pump-public-docs`, `idl/pump.json`).
pub const PUMP_BONDING_CURVE_IDL_COMMIT: &str = "e0687ae9b7e064a0f54efc7297c65eecfbba3a8f";
/// Decoder/qualification rule identifier recorded in reports (invariant 10).
///
/// Meaning: the rule of [`qualify_bonding_curve_buys`] (curve buys only,
/// ADR-003). `buyer-intersect` since ADR-014 reports
/// [`SOLANA_TRADE_QUALIFICATION_VERSION`] instead.
pub const SOLANA_BUY_QUALIFICATION_VERSION: &str = "pump-bonding-curve-buy/idl-e0687ae/v4";
/// Rule identifier of the `buyer-intersect` trade-side qualification
/// (ADR-014): bonding-curve buys and sells, PumpSwap trades and ADR-013
/// route swaps, side in token terms.
pub const SOLANA_TRADE_QUALIFICATION_VERSION: &str = "solana-trade-qualification/v7 (curve+pumpswap+route, ADR-014, ADR-009 26-byte track_volume, ADR-015 Jupiter route legs)";

/// Why the confirmed deployment scope could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    #[error("program id constant is not a valid 32-byte base58 pubkey")]
    InvalidProgramId,
}

/// Solana mainnet `ChainKey` exactly as `scout-app`'s input resolution
/// builds it (`ChainTag::Solana`: Mainnet cluster, `Unverified` genesis),
/// so identities derived from decoded mints equal the input `AssetKey`s.
#[must_use]
pub fn solana_mainnet_chain() -> ChainKey {
    ChainKey {
        family: ChainFamily::Solana,
        network_id: NetworkId::SolanaCluster(SolanaCluster::Mainnet),
        genesis_identity: GenesisIdentity::Unverified,
    }
}

/// The confirmed pump.fun bonding-curve `DeploymentScope`.
pub fn pump_bonding_curve_scope() -> Result<DeploymentScope, ScopeError> {
    let bytes = bs58::decode(PUMP_BONDING_CURVE_PROGRAM_ID)
        .into_vec()
        .map_err(|_| ScopeError::InvalidProgramId)?;
    let program: SolanaPubkey = bytes.try_into().map_err(|_| ScopeError::InvalidProgramId)?;
    Ok(DeploymentScope {
        chain: solana_mainnet_chain(),
        contract_addresses: vec![AddressBytes::Solana(program)],
        // The registry records no activation slot for this program;
        // 0 means "no lower bound claimed", not a verified activation.
        active_from: 0,
        active_until: None,
    })
}

/// Decoder for the confirmed scope.
pub fn pump_bonding_curve_decoder() -> Result<BondingCurveBuyDecoder, ScopeError> {
    Ok(BondingCurveBuyDecoder::new(pump_bonding_curve_scope()?))
}

/// Counters for one transaction (summed by the engine). Every field is
/// an exact count; none is ever an estimate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TxQualificationDiagnostics {
    /// Decoded bonding-curve `buy` instructions (all of them, before
    /// per-(wallet, mint) dedup).
    pub decoded_buys: u64,
    pub decoded_sells: u64,
    /// Instructions of the confirmed program that are known non-trades
    /// (other IDL instructions, Anchor event-CPI). Counted, not gaps.
    pub known_non_trade_instructions: u64,
    /// Instructions of the confirmed program whose discriminator is in
    /// neither the IDL trade nor the known non-trade table. COVERAGE GAP.
    pub unknown_discriminator_instructions: u64,
    /// Decoded trade instructions per variant (`PumpTradeVariant::index`),
    /// all sides, all verification levels.
    pub decoded_by_variant: [u64; PumpTradeVariant::COUNT],
    /// Distinct (user, mint) pairs per variant (`PumpTradeVariant::index`)
    /// with a decoded `IdlOnly` buy and a positive owner delta that was
    /// NOT added to the buyer set. COVERAGE GAP (lower bound). Counted
    /// over all mints here; the engine restricts it to input mints.
    pub unverified_variant_buys: [u64; PumpTradeVariant::COUNT],
    /// Instructions for the confirmed program matching a trade
    /// discriminator but with broken structure (or data < 8 bytes).
    /// COVERAGE GAP.
    pub malformed_instructions: u64,
    /// Distinct (user, mint) pairs with a decoded buy but net delta <= 0
    /// or no owner-keyed balance evidence at all.
    pub buys_without_positive_delta: u64,
    /// Subset of the above: no owner-keyed balance entry existed.
    pub buys_without_balance_evidence: u64,
    /// Balance changes with no owner (cannot be attributed).
    pub unowned_balance_changes: u64,
    /// Transactions whose delta arithmetic exceeded range. COVERAGE GAP.
    pub delta_overflow_transactions: u64,
    /// Transactions whose slot lies outside the decoder scope's
    /// activation range (not decoded). COVERAGE GAP.
    pub out_of_scope_slot_transactions: u64,
    /// Transactions with a non-null `meta.err`. They never qualify a
    /// buy and are not decoded further. Not a coverage gap: a failed
    /// transaction changes no balances and bought nothing.
    pub failed_transactions: u64,
}

impl TxQualificationDiagnostics {
    /// Saturating field-wise sum (counters must never wrap).
    pub fn add(&mut self, other: &Self) {
        self.decoded_buys = self.decoded_buys.saturating_add(other.decoded_buys);
        self.decoded_sells = self.decoded_sells.saturating_add(other.decoded_sells);
        self.known_non_trade_instructions = self
            .known_non_trade_instructions
            .saturating_add(other.known_non_trade_instructions);
        self.unknown_discriminator_instructions = self
            .unknown_discriminator_instructions
            .saturating_add(other.unknown_discriminator_instructions);
        for (a, b) in self
            .decoded_by_variant
            .iter_mut()
            .zip(other.decoded_by_variant)
        {
            *a = a.saturating_add(b);
        }
        for (a, b) in self
            .unverified_variant_buys
            .iter_mut()
            .zip(other.unverified_variant_buys)
        {
            *a = a.saturating_add(b);
        }
        self.malformed_instructions = self
            .malformed_instructions
            .saturating_add(other.malformed_instructions);
        self.buys_without_positive_delta = self
            .buys_without_positive_delta
            .saturating_add(other.buys_without_positive_delta);
        self.buys_without_balance_evidence = self
            .buys_without_balance_evidence
            .saturating_add(other.buys_without_balance_evidence);
        self.unowned_balance_changes = self
            .unowned_balance_changes
            .saturating_add(other.unowned_balance_changes);
        self.delta_overflow_transactions = self
            .delta_overflow_transactions
            .saturating_add(other.delta_overflow_transactions);
        self.out_of_scope_slot_transactions = self
            .out_of_scope_slot_transactions
            .saturating_add(other.out_of_scope_slot_transactions);
        self.failed_transactions = self
            .failed_transactions
            .saturating_add(other.failed_transactions);
    }
}

/// One qualified (wallet, mint) acquisition in one transaction, with its
/// evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedBuy {
    pub wallet: WalletKey,
    pub asset: AssetKey,
    /// Strictly positive owner-keyed net delta, raw base units.
    pub net_delta: SignedAmount,
    /// `amount` argument of the first decoded buy for this pair.
    pub decoded_amount: u64,
    pub instruction_index: u32,
    pub signature: [u8; 64],
    pub slot: u64,
    /// Always `Confident` for a qualified buy; carries why.
    pub attribution: AttributionStatus,
}

/// A decoded buy of an `IdlOnly` variant whose owner delta would
/// qualify. Deliberately NOT a [`QualifiedBuy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedVariantBuy {
    pub variant: PumpTradeVariant,
    pub user: SolanaPubkey,
    pub mint: SolanaPubkey,
}

/// Result of qualifying one transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxQualification {
    pub buys: Vec<QualifiedBuy>,
    /// `IdlOnly`-variant buys with a positive owner delta: not buyers,
    /// but proof the buyer set is a lower bound.
    pub unverified_buys: Vec<UnverifiedVariantBuy>,
    /// Hex of unknown discriminators seen under the program (one per
    /// instruction).
    pub unknown_discriminators: Vec<String>,
    /// `(mint, owner)` pairs that gained tokens with NO decoded buy
    /// instruction for that owner. Not buys. The caller restricts the
    /// count to mints it cares about (a sell makes the curve vault a
    /// positive-delta owner, so the unfiltered set is mostly noise).
    pub uninstructed_positive_deltas: Vec<(SolanaPubkey, SolanaPubkey)>,
    /// Malformed-instruction descriptions for this tx (provider-derived
    /// text; callers must sanitize before display).
    pub malformed_reasons: Vec<String>,
    pub diagnostics: TxQualificationDiagnostics,
}

/// Variant -> verification policy used to gate buys. A plain fn pointer:
/// no global state, `Copy`, trivially `Send + Sync`.
pub type VariantPolicy = fn(PumpTradeVariant) -> VariantVerification;

/// Production policy: the decoder's static per-variant spec table.
#[must_use]
pub fn default_variant_policy(variant: PumpTradeVariant) -> VariantVerification {
    variant.verification()
}

/// Qualify every buyer in one transaction with the production policy.
/// Pure and synchronous. `decoder.scope().chain` supplies the chain
/// identity of produced keys.
#[must_use]
pub fn qualify_bonding_curve_buys(
    tx: &RawSolanaTransaction,
    decoder: &BondingCurveBuyDecoder,
) -> TxQualification {
    qualify_bonding_curve_buys_with_policy(tx, decoder, default_variant_policy)
}

/// As [`qualify_bonding_curve_buys`], with an injected variant policy
/// (production callers use [`default_variant_policy`]; tests inject one
/// to keep the `IdlOnly` path covered).
#[must_use]
pub fn qualify_bonding_curve_buys_with_policy(
    tx: &RawSolanaTransaction,
    decoder: &BondingCurveBuyDecoder,
    policy: VariantPolicy,
) -> TxQualification {
    let mut out = TxQualification::default();
    let scope = decoder.scope();

    // ADR-003: a buy exists only within a successful transaction. A
    // failed one is counted and otherwise ignored (no buys, no IdlOnly
    // or uninstructed-delta diagnostics, no decode diagnostics).
    if !tx.execution.is_success() {
        out.diagnostics.failed_transactions = 1;
        return out;
    }

    if !scope.covers_position(tx.slot) {
        out.diagnostics.out_of_scope_slot_transactions = 1;
        return out;
    }

    // (user, mint) -> (instruction_index, amount) of the first decoded
    // FixtureVerified buy.
    let mut decoded_buys: BTreeMap<(SolanaPubkey, SolanaPubkey), (u32, u64)> = BTreeMap::new();
    // (user, mint) -> first decoded IdlOnly buy variant.
    let mut idl_only_buys: BTreeMap<(SolanaPubkey, SolanaPubkey), PumpTradeVariant> =
        BTreeMap::new();
    for instruction in &tx.instructions {
        match decoder.classify(instruction, tx.slot, tx.transaction_index) {
            PumpInstructionOutcome::NotMine => {}
            PumpInstructionOutcome::NonTrade(_) => {
                out.diagnostics.known_non_trade_instructions = out
                    .diagnostics
                    .known_non_trade_instructions
                    .saturating_add(1);
            }
            PumpInstructionOutcome::UnknownDiscriminator { discriminator } => {
                out.diagnostics.unknown_discriminator_instructions = out
                    .diagnostics
                    .unknown_discriminator_instructions
                    .saturating_add(1);
                out.unknown_discriminators.push(hex8(&discriminator));
            }
            PumpInstructionOutcome::Malformed { reason, .. } => {
                out.diagnostics.malformed_instructions =
                    out.diagnostics.malformed_instructions.saturating_add(1);
                out.malformed_reasons.push(reason);
            }
            PumpInstructionOutcome::Trade(trade) => {
                if let Some(slot) = out
                    .diagnostics
                    .decoded_by_variant
                    .get_mut(trade.variant.index())
                {
                    *slot = slot.saturating_add(1);
                }
                match (trade.side, policy(trade.variant)) {
                    (TradeSide::Sell, _) => {
                        out.diagnostics.decoded_sells =
                            out.diagnostics.decoded_sells.saturating_add(1);
                    }
                    (TradeSide::Buy, VariantVerification::FixtureVerified) => {
                        out.diagnostics.decoded_buys =
                            out.diagnostics.decoded_buys.saturating_add(1);
                        decoded_buys
                            .entry((trade.user, trade.mint))
                            .or_insert((trade.instruction_index, trade.args[0].value));
                    }
                    (TradeSide::Buy, VariantVerification::IdlOnly) => {
                        idl_only_buys
                            .entry((trade.user, trade.mint))
                            .or_insert(trade.variant);
                    }
                }
            }
        }
    }

    let Ok(owner_deltas) = solana_owner_net_deltas(&tx.token_balance_changes) else {
        out.diagnostics.delta_overflow_transactions = 1;
        return out;
    };
    out.diagnostics.unowned_balance_changes =
        u64::try_from(owner_deltas.unowned_changes).unwrap_or(u64::MAX);

    let chain = &scope.chain;
    // wallet -> its confident Swap-kind flows (one per mint).
    let mut flows_by_wallet: BTreeMap<SolanaPubkey, NetDeltaInput> = BTreeMap::new();
    let mut evidence: BTreeMap<(SolanaPubkey, SolanaPubkey), (u32, u64)> = BTreeMap::new();

    for (&(user, mint), &(instruction_index, amount)) in &decoded_buys {
        let Some(&delta) = owner_deltas.deltas.get(&(mint, user)) else {
            out.diagnostics.buys_without_positive_delta = out
                .diagnostics
                .buys_without_positive_delta
                .saturating_add(1);
            out.diagnostics.buys_without_balance_evidence = out
                .diagnostics
                .buys_without_balance_evidence
                .saturating_add(1);
            continue;
        };
        if delta <= 0 {
            out.diagnostics.buys_without_positive_delta = out
                .diagnostics
                .buys_without_positive_delta
                .saturating_add(1);
            continue;
        }
        let Ok(signed) = I256::try_from(delta) else {
            out.diagnostics.delta_overflow_transactions = 1;
            continue;
        };
        let asset = AssetKey::Token(chain.clone(), AddressBytes::Solana(mint));
        flows_by_wallet.entry(user).or_default().flows.insert(
            asset.clone(),
            AssetFlow {
                asset,
                net_delta: SignedAmount::from_i256(signed),
                // Swap only because a confirmed decoder produced the
                // instruction evidence for this exact (owner, mint).
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        evidence.insert((user, mint), (instruction_index, amount));
    }

    for (user, flows) in &flows_by_wallet {
        let wallet = WalletKey {
            chain: chain.clone(),
            address: AddressBytes::Solana(*user),
        };
        for asset in classify_buy(flows) {
            let (Some(flow), AssetKey::Token(_, AddressBytes::Solana(mint))) =
                (flows.flows.get(&asset), &asset)
            else {
                continue;
            };
            let Some(&(instruction_index, decoded_amount)) = evidence.get(&(*user, *mint)) else {
                continue;
            };
            out.buys.push(QualifiedBuy {
                wallet: wallet.clone(),
                asset: asset.clone(),
                net_delta: flow.net_delta,
                decoded_amount,
                instruction_index,
                signature: tx.signature,
                slot: tx.slot,
                // The decoded instruction names `user` and the balance
                // owner equals it: recipient-owner match, owner-keyed.
                attribution: AttributionStatus::Confident {
                    owner: wallet.clone(),
                    evidence: AttributionEvidence::RecipientOwnerMatch,
                },
            });
        }
    }

    for (&(user, mint), &variant) in &idl_only_buys {
        // A verified buy for the same pair already decides this tx.
        if decoded_buys.contains_key(&(user, mint)) {
            continue;
        }
        let positive = owner_deltas
            .deltas
            .get(&(mint, user))
            .is_some_and(|&delta| delta > 0);
        if positive {
            if let Some(slot) = out
                .diagnostics
                .unverified_variant_buys
                .get_mut(variant.index())
            {
                *slot = slot.saturating_add(1);
            }
            out.unverified_buys.push(UnverifiedVariantBuy {
                variant,
                user,
                mint,
            });
        }
    }

    let instructed: BTreeSet<(SolanaPubkey, SolanaPubkey)> = decoded_buys
        .keys()
        .chain(idl_only_buys.keys())
        .map(|&(user, mint)| (mint, user))
        .collect();
    for (&(mint, owner), &delta) in &owner_deltas.deltas {
        if delta > 0 && !instructed.contains(&(mint, owner)) {
            out.uninstructed_positive_deltas.push((mint, owner));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use scout_core::{RawSolanaInstruction, SolanaTokenBalanceChange};
    use scout_dex_solana::{BUY_INSTRUCTION_DISCRIMINATOR, SELL_INSTRUCTION_DISCRIMINATOR};

    use super::*;

    fn pk(b: u8) -> SolanaPubkey {
        [b; 32]
    }

    fn program() -> SolanaPubkey {
        let bytes = bs58::decode(PUMP_BONDING_CURVE_PROGRAM_ID)
            .into_vec()
            .unwrap();
        bytes.try_into().unwrap()
    }

    fn decoder() -> BondingCurveBuyDecoder {
        pump_bonding_curve_decoder().unwrap()
    }

    fn buy_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
        let mut accounts: Vec<SolanaPubkey> = (0..16u8).map(|i| pk(100 + i)).collect();
        accounts[2] = pk(mint);
        accounts[6] = pk(user);
        let mut data = BUY_INSTRUCTION_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&1000u64.to_le_bytes());
        data.extend_from_slice(&2000u64.to_le_bytes());
        data.push(1);
        RawSolanaInstruction {
            program_id: program(),
            accounts,
            data,
            instruction_index: idx,
        }
    }

    fn sell_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
        let mut accounts: Vec<SolanaPubkey> = (0..14u8).map(|i| pk(100 + i)).collect();
        accounts[2] = pk(mint);
        accounts[6] = pk(user);
        let mut data = SELL_INSTRUCTION_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&1000u64.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        RawSolanaInstruction {
            program_id: program(),
            accounts,
            data,
            instruction_index: idx,
        }
    }

    fn bal(mint: u8, owner: Option<u8>, pre: Option<u64>, post: u64) -> SolanaTokenBalanceChange {
        SolanaTokenBalanceChange {
            mint: pk(mint),
            owner: owner.map(pk),
            decimals: 6,
            pre_amount: pre,
            post_amount: post,
            closed: false,
        }
    }

    fn tx(
        instructions: Vec<RawSolanaInstruction>,
        balances: Vec<SolanaTokenBalanceChange>,
    ) -> RawSolanaTransaction {
        RawSolanaTransaction {
            block_time: None,
            signature: [9; 64],
            execution: scout_core::SolanaExecutionStatus::Succeeded,
            slot: 500,
            transaction_index: 1,
            instructions,
            token_balance_changes: balances,
            fee_lamports: 5_000,
            fee_payer: pk(9),
            signers: vec![pk(9)],
            native_balance_changes: vec![],
        }
    }

    fn wallet(b: u8) -> WalletKey {
        WalletKey {
            chain: solana_mainnet_chain(),
            address: AddressBytes::Solana(pk(b)),
        }
    }

    #[test]
    fn decoded_buy_with_positive_owner_delta_qualifies() {
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 3)],
                vec![
                    bal(50, Some(1), None, 700),
                    bal(50, Some(2), Some(900), 200),
                ],
            ),
            &decoder(),
        );
        assert_eq!(q.buys.len(), 1);
        assert_eq!(q.buys[0].wallet, wallet(1));
        assert_eq!(q.buys[0].instruction_index, 3);
        assert_eq!(q.buys[0].decoded_amount, 1000);
        assert_eq!(q.buys[0].net_delta.to_string(), "700");
        assert!(q.buys[0].attribution.is_confident());
        assert_eq!(q.diagnostics.decoded_buys, 1);
    }

    #[test]
    fn failed_transaction_with_positive_delta_and_verified_buy_is_not_a_buy() {
        let mut failed = tx(
            vec![buy_ix(1, 50, 3), buy_ix(2, 50, 4)],
            vec![bal(50, Some(1), None, 700), bal(50, Some(7), None, 5)],
        );
        failed.execution = scout_core::SolanaExecutionStatus::Failed {
            error: "{\"InstructionError\":[4,{\"Custom\":6042}]}".to_string(),
        };
        let q = qualify_bonding_curve_buys(&failed, &decoder());
        assert!(q.buys.is_empty());
        assert!(q.unverified_buys.is_empty());
        assert!(q.uninstructed_positive_deltas.is_empty());
        assert_eq!(q.diagnostics.failed_transactions, 1);
        assert_eq!(q.diagnostics.decoded_buys, 0);
        assert_eq!(q.diagnostics.malformed_instructions, 0);
        // Same tx, Succeeded: control proves the status is what gated it.
        failed.execution = scout_core::SolanaExecutionStatus::Succeeded;
        let ok = qualify_bonding_curve_buys(&failed, &decoder());
        assert_eq!(ok.buys.len(), 1);
        assert_eq!(ok.diagnostics.failed_transactions, 0);
    }

    #[test]
    fn roundtrip_buy_and_sell_with_zero_net_is_not_a_buy() {
        // B06: user buys 500 and sells 500 in one tx; net 0.
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 0), sell_ix(1, 50, 1)],
                vec![bal(50, Some(1), Some(10), 10)],
            ),
            &decoder(),
        );
        assert!(q.buys.is_empty());
        assert_eq!(q.diagnostics.decoded_buys, 1);
        assert_eq!(q.diagnostics.decoded_sells, 1);
        assert_eq!(q.diagnostics.buys_without_positive_delta, 1);
        assert_eq!(q.diagnostics.buys_without_balance_evidence, 0);
    }

    #[test]
    fn positive_delta_without_instruction_is_not_a_buy_and_is_counted() {
        let q =
            qualify_bonding_curve_buys(&tx(vec![], vec![bal(50, Some(1), None, 700)]), &decoder());
        assert!(q.buys.is_empty());
        assert_eq!(q.uninstructed_positive_deltas, vec![(pk(50), pk(1))]);
    }

    #[test]
    fn two_users_each_with_own_buy_are_both_attributed() {
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 0), buy_ix(2, 50, 1)],
                vec![bal(50, Some(1), None, 10), bal(50, Some(2), None, 20)],
            ),
            &decoder(),
        );
        let wallets: Vec<_> = q.buys.iter().map(|b| b.wallet.clone()).collect();
        assert_eq!(wallets, vec![wallet(1), wallet(2)]);
        assert!(q.uninstructed_positive_deltas.is_empty());
    }

    #[test]
    fn gainer_without_instruction_next_to_instructed_buyer_stays_non_qualifying() {
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 0)],
                vec![bal(50, Some(1), None, 10), bal(50, Some(7), None, 5)],
            ),
            &decoder(),
        );
        assert_eq!(q.buys.len(), 1);
        assert_eq!(q.buys[0].wallet, wallet(1));
        assert_eq!(q.uninstructed_positive_deltas, vec![(pk(50), pk(7))]);
    }

    #[test]
    fn buy_where_balance_owner_differs_or_is_missing_is_not_a_buy() {
        // Token account owned by someone else than the instruction user.
        let differs = qualify_bonding_curve_buys(
            &tx(vec![buy_ix(1, 50, 0)], vec![bal(50, Some(9), None, 10)]),
            &decoder(),
        );
        assert!(differs.buys.is_empty());
        assert_eq!(differs.diagnostics.buys_without_balance_evidence, 1);
        assert_eq!(differs.uninstructed_positive_deltas, vec![(pk(50), pk(9))]);

        // Owner not reported at all.
        let none = qualify_bonding_curve_buys(
            &tx(vec![buy_ix(1, 50, 0)], vec![bal(50, None, None, 10)]),
            &decoder(),
        );
        assert!(none.buys.is_empty());
        assert_eq!(none.diagnostics.unowned_balance_changes, 1);
        assert_eq!(none.diagnostics.buys_without_balance_evidence, 1);
    }

    #[test]
    fn many_buys_of_same_pair_yield_one_qualified_buy() {
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 0), buy_ix(1, 50, 4)],
                vec![bal(50, Some(1), None, 10)],
            ),
            &decoder(),
        );
        assert_eq!(q.buys.len(), 1);
        assert_eq!(q.diagnostics.decoded_buys, 2);
        assert_eq!(q.buys[0].instruction_index, 0);
    }

    #[test]
    fn malformed_instruction_is_counted_not_skipped() {
        let mut bad = buy_ix(1, 50, 0);
        bad.data.truncate(23); // matches buy discriminator, shorter than required args
        let q = qualify_bonding_curve_buys(
            &tx(vec![bad], vec![bal(50, Some(1), None, 10)]),
            &decoder(),
        );
        assert!(q.buys.is_empty());
        assert_eq!(q.diagnostics.malformed_instructions, 1);
        assert_eq!(q.malformed_reasons.len(), 1);
    }

    #[test]
    fn foreign_program_with_buy_discriminator_is_not_mine() {
        let mut ix = buy_ix(1, 50, 0);
        ix.program_id = pk(77);
        let q =
            qualify_bonding_curve_buys(&tx(vec![ix], vec![bal(50, Some(1), None, 10)]), &decoder());
        assert!(q.buys.is_empty());
        assert_eq!(q.diagnostics.decoded_buys, 0);
        assert_eq!(q.diagnostics.malformed_instructions, 0);
    }

    #[test]
    fn max_u64_delta_is_exact() {
        let q = qualify_bonding_curve_buys(
            &tx(
                vec![buy_ix(1, 50, 0)],
                vec![bal(50, Some(1), None, u64::MAX)],
            ),
            &decoder(),
        );
        assert_eq!(q.buys[0].net_delta.to_string(), u64::MAX.to_string());
    }

    fn sol_in_idl_only(variant: PumpTradeVariant) -> VariantVerification {
        if variant == PumpTradeVariant::BuyExactSolIn {
            VariantVerification::IdlOnly
        } else {
            variant.verification()
        }
    }

    fn sol_in_ix(user: u8, mint: u8, idx: u32) -> RawSolanaInstruction {
        let mut accounts: Vec<SolanaPubkey> = (0..16u8).map(|i| pk(100 + i)).collect();
        accounts[2] = pk(mint);
        accounts[6] = pk(user);
        let mut data = scout_dex_solana::BUY_EXACT_SOL_IN_INSTRUCTION_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&1000u64.to_le_bytes());
        data.extend_from_slice(&2000u64.to_le_bytes());
        data.push(1);
        RawSolanaInstruction {
            program_id: program(),
            accounts,
            data,
            instruction_index: idx,
        }
    }

    #[test]
    fn injected_idl_only_policy_reports_unverified_buy_instead_of_qualifying() {
        let t = tx(vec![sol_in_ix(1, 50, 0)], vec![bal(50, Some(1), None, 700)]);
        // Default policy: promoted, qualifies.
        let promoted = qualify_bonding_curve_buys(&t, &decoder());
        assert_eq!(promoted.buys.len(), 1);
        assert!(promoted.unverified_buys.is_empty());
        // Injected IdlOnly: never a buyer, reported as unverified.
        let q = qualify_bonding_curve_buys_with_policy(&t, &decoder(), sol_in_idl_only);
        assert!(q.buys.is_empty());
        assert_eq!(q.diagnostics.decoded_buys, 0);
        assert_eq!(
            q.unverified_buys,
            vec![UnverifiedVariantBuy {
                variant: PumpTradeVariant::BuyExactSolIn,
                user: pk(1),
                mint: pk(50),
            }]
        );
        assert_eq!(
            q.diagnostics.unverified_variant_buys[PumpTradeVariant::BuyExactSolIn.index()],
            1
        );
        assert!(q.uninstructed_positive_deltas.is_empty());
    }

    #[test]
    fn injected_idl_only_policy_zero_delta_is_neither_buy_nor_unverified() {
        let q = qualify_bonding_curve_buys_with_policy(
            &tx(
                vec![sol_in_ix(1, 50, 0)],
                vec![bal(50, Some(1), Some(5), 5), bal(50, Some(9), None, 7)],
            ),
            &decoder(),
            sol_in_idl_only,
        );
        assert!(q.buys.is_empty());
        assert!(q.unverified_buys.is_empty());
        assert_eq!(q.uninstructed_positive_deltas, vec![(pk(50), pk(9))]);
    }

    #[test]
    fn chain_matches_scout_app_solana_resolution_shape() {
        let c = solana_mainnet_chain();
        assert_eq!(c.family, ChainFamily::Solana);
        assert_eq!(c.genesis_identity, GenesisIdentity::Unverified);
    }
}
