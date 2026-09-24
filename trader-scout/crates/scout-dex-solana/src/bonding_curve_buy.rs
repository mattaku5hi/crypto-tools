//! Generic bonding-curve-shape "buy" instruction decoder.
//!
//! Scope note (AGENTS.md invariant #16): this decodes a generic
//! bonding-curve buy instruction shape — an 8-byte discriminator
//! (Anchor-style) followed by a little-endian u64 SOL-in amount and a
//! little-endian u64 minimum-tokens-out amount, with accounts
//! [buyer, bonding_curve, mint, buyer_token_account] in that positional
//! order. This mirrors the publicly documented shape of Solana
//! launch-style bonding-curve programs (S13 in SOURCES.md) but does
//! **not** claim support for any specific deployed program —
//! `docs/p0/deployment-registry.md` has zero confirmed entries. This
//! module proves the decode *mechanism* (raw instruction -> typed buy)
//! against a synthetic fixture, mirroring what scout-dex-evm's
//! v2_swap.rs does for the EVM vertical slice.
//!
//! Implements `scout_api::TxDecoder` (ADR-008 S4). `DecodeOutcome::NotMine`
//! for an instruction with a different discriminator (normal — a
//! registry trying several decoders against one instruction sees this
//! from every non-matching decoder); `DecodeOutcome::Malformed` for an
//! instruction that matches the discriminator but has a broken account
//! count or data length (invariant #18: never silently skipped).

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{RawSolanaInstruction, SolanaPubkey};

/// This module's own 8-byte discriminator constant for the synthetic
/// "buy" instruction shape it decodes. A real deployment's actual
/// discriminator (derived from its IDL, e.g. Anchor's
/// `sha256("global:buy")[..8]`) is a P0.2 concern — pinning a made-up
/// constant here does not claim any specific program uses it.
pub const BUY_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];

/// A decoded bonding-curve buy: SOL paid in, minimum tokens expected
/// out (the actual tokens received come from post-instruction token
/// balance deltas, not this instruction's static data — this type only
/// carries what the instruction itself declares).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBondingCurveBuy {
    pub buyer: SolanaPubkey,
    pub bonding_curve: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub buyer_token_account: SolanaPubkey,
    pub sol_in: u64,
    pub min_tokens_out: u64,
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
}

/// A `TxDecoder` for the bonding-curve buy instruction shape, scoped to
/// one `DeploymentScope` (AGENTS.md invariant #16: mandatory at
/// registration, never a blanket claim across chains/programs).
#[derive(Debug, Clone)]
pub struct BondingCurveBuyDecoder {
    scope: DeploymentScope,
}

impl BondingCurveBuyDecoder {
    #[must_use]
    pub fn new(scope: DeploymentScope) -> Self {
        Self { scope }
    }
}

impl TxDecoder<RawSolanaInstruction, DecodedBondingCurveBuy> for BondingCurveBuyDecoder {
    fn scope(&self) -> &DeploymentScope {
        &self.scope
    }

    fn decode(&self, instruction: &RawSolanaInstruction) -> DecodeOutcome<DecodedBondingCurveBuy> {
        // Position (slot/tx index) is not known to a decoder in
        // isolation — the caller (engine/registry) attaches it from the
        // transaction context. This trait-based entry point decodes the
        // instruction shape only; use decode_bonding_curve_buy directly
        // when slot/transaction_index are available.
        decode_bonding_curve_buy(instruction, 0, 0)
    }
}

/// Decode a raw instruction as a bonding-curve-shape buy.
///
/// `DecodeOutcome::NotMine` for an instruction with a different
/// discriminator — the expected, common case when scanning a
/// transaction's mixed instructions. `DecodeOutcome::Malformed` for an
/// instruction that matches the discriminator but has a broken account
/// count or data length (AGENTS.md invariant #18).
#[must_use]
pub fn decode_bonding_curve_buy(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> DecodeOutcome<DecodedBondingCurveBuy> {
    if instruction.data.len() < 8 {
        // Too short to even contain a discriminator: this cannot be
        // determined to be "mine" or not, so it's simply not mine.
        return DecodeOutcome::NotMine;
    }
    let Some(discriminator) = instruction.data.get(0..8) else {
        return DecodeOutcome::NotMine;
    };
    if discriminator != BUY_INSTRUCTION_DISCRIMINATOR {
        return DecodeOutcome::NotMine;
    }

    if instruction.data.len() != 24 {
        return DecodeOutcome::Malformed(format!(
            "instruction matches buy discriminator but data has {} bytes, expected 24 (8-byte discriminator + 2x u64)",
            instruction.data.len()
        ));
    }
    if instruction.accounts.len() != 4 {
        return DecodeOutcome::Malformed(format!(
            "instruction matches buy discriminator but has {} accounts, expected 4 (buyer, bonding_curve, mint, buyer_token_account)",
            instruction.accounts.len()
        ));
    }

    let Some(sol_in) = read_u64_le(&instruction.data, 8) else {
        return DecodeOutcome::Malformed(
            "instruction matches buy discriminator but sol_in field is unreadable".to_string(),
        );
    };
    let Some(min_tokens_out) = read_u64_le(&instruction.data, 16) else {
        return DecodeOutcome::Malformed(
            "instruction matches buy discriminator but min_tokens_out field is unreadable"
                .to_string(),
        );
    };

    let (Some(buyer), Some(bonding_curve), Some(mint), Some(buyer_token_account)) = (
        instruction.accounts.first(),
        instruction.accounts.get(1),
        instruction.accounts.get(2),
        instruction.accounts.get(3),
    ) else {
        // Unreachable given the accounts.len() == 4 check above, but
        // avoid indexing_slicing per workspace lint policy.
        return DecodeOutcome::Malformed(
            "instruction matches buy discriminator but accounts are missing".to_string(),
        );
    };

    DecodeOutcome::Decoded(DecodedBondingCurveBuy {
        buyer: *buyer,
        bonding_curve: *bonding_curve,
        mint: *mint,
        buyer_token_account: *buyer_token_account,
        sol_in,
        min_tokens_out,
        slot,
        transaction_index,
        instruction_index: instruction.instruction_index,
    })
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    let slice = data.get(offset..offset + 8)?;
    let array: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic (never claimed as mainnet) bonding-curve buy
    /// instruction. Provenance: hand-constructed fixture proving the
    /// decode mechanism, per ADR-006 / docs/p0/deployment-registry.md's
    /// empty-by-design state.
    fn synthetic_buy_instruction(sol_in: u64, min_tokens_out: u64) -> RawSolanaInstruction {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&sol_in.to_le_bytes());
        data.extend_from_slice(&min_tokens_out.to_le_bytes());

        RawSolanaInstruction {
            program_id: [0xAA; 32],
            accounts: vec![[0x11; 32], [0x22; 32], [0x33; 32], [0x44; 32]],
            data,
            instruction_index: 1,
        }
    }

    #[test]
    fn decodes_a_well_formed_buy_instruction() {
        let instruction = synthetic_buy_instruction(1_000_000_000, 500_000);
        let decoded = decode_bonding_curve_buy(&instruction, 12_345, 3)
            .decoded()
            .unwrap();
        assert_eq!(decoded.sol_in, 1_000_000_000);
        assert_eq!(decoded.min_tokens_out, 500_000);
        assert_eq!(decoded.buyer, [0x11; 32]);
        assert_eq!(decoded.bonding_curve, [0x22; 32]);
        assert_eq!(decoded.mint, [0x33; 32]);
        assert_eq!(decoded.buyer_token_account, [0x44; 32]);
        assert_eq!(decoded.slot, 12_345);
        assert_eq!(decoded.transaction_index, 3);
        assert_eq!(decoded.instruction_index, 1);
    }

    #[test]
    fn wrong_discriminator_is_not_mine_not_an_error() {
        // The same fix as scout-dex-evm's v2_swap.rs: an instruction
        // belonging to a different program/instruction shape must be
        // NotMine, not conflated with Malformed.
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.data[0] = 0xFF;
        let outcome = decode_bonding_curve_buy(&instruction, 1, 0);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn matching_discriminator_with_wrong_account_count_is_malformed() {
        // AGENTS.md invariant #18: unfamiliar shape is surfaced, never
        // silently skipped or conflated with "not my instruction."
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.accounts.pop();
        let outcome = decode_bonding_curve_buy(&instruction, 1, 0);
        assert!(outcome.is_malformed());
        assert!(!outcome.is_not_mine());
    }

    #[test]
    fn matching_discriminator_with_wrong_data_length_is_malformed() {
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.data.push(0x00);
        let outcome = decode_bonding_curve_buy(&instruction, 1, 0);
        assert!(outcome.is_malformed());
    }

    #[test]
    fn preserves_canonical_ordering_fields() {
        // ADR-002/ADR-004: slot + transaction_index + instruction_index
        // must survive decoding unmodified for downstream FIFO ordering.
        let instruction = synthetic_buy_instruction(1, 1);
        let decoded = decode_bonding_curve_buy(&instruction, 999, 5)
            .decoded()
            .unwrap();
        assert_eq!(decoded.slot, 999);
        assert_eq!(decoded.transaction_index, 5);
        assert_eq!(decoded.instruction_index, 1);
    }

    #[test]
    fn max_u64_amounts_do_not_overflow_or_truncate() {
        let instruction = synthetic_buy_instruction(u64::MAX, u64::MAX);
        let decoded = decode_bonding_curve_buy(&instruction, 1, 0)
            .decoded()
            .unwrap();
        assert_eq!(decoded.sol_in, u64::MAX);
        assert_eq!(decoded.min_tokens_out, u64::MAX);
    }

    #[test]
    fn too_short_instruction_data_is_not_mine() {
        let instruction = RawSolanaInstruction {
            program_id: [0xAA; 32],
            accounts: vec![],
            data: vec![0x01, 0x02],
            instruction_index: 0,
        };
        let outcome = decode_bonding_curve_buy(&instruction, 1, 0);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn bonding_curve_buy_decoder_implements_tx_decoder_trait() {
        let scope = DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Solana([0xAA; 32])],
            active_from: 0,
            active_until: None,
        };
        let decoder = BondingCurveBuyDecoder::new(scope);
        let instruction = synthetic_buy_instruction(1, 1);
        let outcome = decoder.decode(&instruction);
        assert!(matches!(outcome, DecodeOutcome::Decoded(_)));
    }
}
