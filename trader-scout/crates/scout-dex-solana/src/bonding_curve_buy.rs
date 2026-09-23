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

use scout_solana::{RawSolanaInstruction, SolanaPubkey};

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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DexDecodeError {
    #[error("instruction does not match the buy discriminator")]
    DiscriminatorMismatch,
    #[error(
        "instruction has {actual} accounts, expected 4 (buyer, bonding_curve, mint, buyer_token_account)"
    )]
    WrongAccountCount { actual: usize },
    #[error("instruction data has {actual} bytes, expected 24 (8-byte discriminator + 2x u64)")]
    WrongDataLength { actual: usize },
}

/// Decode a raw instruction as a bonding-curve-shape buy. Returns a
/// typed error for anything that does not match this specific shape —
/// never a best-effort guess (AGENTS.md invariant #18).
pub fn decode_bonding_curve_buy(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> Result<DecodedBondingCurveBuy, DexDecodeError> {
    if instruction.data.len() != 24 {
        return Err(DexDecodeError::WrongDataLength {
            actual: instruction.data.len(),
        });
    }
    let discriminator = instruction
        .data
        .get(0..8)
        .ok_or(DexDecodeError::WrongDataLength {
            actual: instruction.data.len(),
        })?;
    if discriminator != BUY_INSTRUCTION_DISCRIMINATOR {
        return Err(DexDecodeError::DiscriminatorMismatch);
    }
    if instruction.accounts.len() != 4 {
        return Err(DexDecodeError::WrongAccountCount {
            actual: instruction.accounts.len(),
        });
    }

    let sol_in = read_u64_le(&instruction.data, 8)?;
    let min_tokens_out = read_u64_le(&instruction.data, 16)?;

    let buyer = *instruction
        .accounts
        .first()
        .ok_or(DexDecodeError::WrongAccountCount {
            actual: instruction.accounts.len(),
        })?;
    let bonding_curve = *instruction
        .accounts
        .get(1)
        .ok_or(DexDecodeError::WrongAccountCount {
            actual: instruction.accounts.len(),
        })?;
    let mint = *instruction
        .accounts
        .get(2)
        .ok_or(DexDecodeError::WrongAccountCount {
            actual: instruction.accounts.len(),
        })?;
    let buyer_token_account =
        *instruction
            .accounts
            .get(3)
            .ok_or(DexDecodeError::WrongAccountCount {
                actual: instruction.accounts.len(),
            })?;

    Ok(DecodedBondingCurveBuy {
        buyer,
        bonding_curve,
        mint,
        buyer_token_account,
        sol_in,
        min_tokens_out,
        slot,
        transaction_index,
        instruction_index: instruction.instruction_index,
    })
}

fn read_u64_le(data: &[u8], offset: usize) -> Result<u64, DexDecodeError> {
    let slice = data
        .get(offset..offset + 8)
        .ok_or(DexDecodeError::WrongDataLength { actual: data.len() })?;
    let array: [u8; 8] = slice
        .try_into()
        .map_err(|_| DexDecodeError::WrongDataLength { actual: data.len() })?;
    Ok(u64::from_le_bytes(array))
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
        let decoded = decode_bonding_curve_buy(&instruction, 12_345, 3).unwrap();
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
    fn rejects_instruction_with_wrong_discriminator() {
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.data[0] = 0xFF;
        let result = decode_bonding_curve_buy(&instruction, 1, 0);
        assert_eq!(result, Err(DexDecodeError::DiscriminatorMismatch));
    }

    #[test]
    fn rejects_instruction_with_wrong_account_count() {
        // AGENTS.md invariant #18: unfamiliar shape is a typed error,
        // never a silent skip or guess.
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.accounts.pop();
        let result = decode_bonding_curve_buy(&instruction, 1, 0);
        assert_eq!(result, Err(DexDecodeError::WrongAccountCount { actual: 3 }));
    }

    #[test]
    fn rejects_instruction_with_wrong_data_length() {
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.data.push(0x00);
        let result = decode_bonding_curve_buy(&instruction, 1, 0);
        assert_eq!(result, Err(DexDecodeError::WrongDataLength { actual: 25 }));
    }

    #[test]
    fn preserves_canonical_ordering_fields() {
        // ADR-002/ADR-004: slot + transaction_index + instruction_index
        // must survive decoding unmodified for downstream FIFO ordering.
        let instruction = synthetic_buy_instruction(1, 1);
        let decoded = decode_bonding_curve_buy(&instruction, 999, 5).unwrap();
        assert_eq!(decoded.slot, 999);
        assert_eq!(decoded.transaction_index, 5);
        assert_eq!(decoded.instruction_index, 1);
    }

    #[test]
    fn max_u64_amounts_do_not_overflow_or_truncate() {
        let instruction = synthetic_buy_instruction(u64::MAX, u64::MAX);
        let decoded = decode_bonding_curve_buy(&instruction, 1, 0).unwrap();
        assert_eq!(decoded.sol_in, u64::MAX);
        assert_eq!(decoded.min_tokens_out, u64::MAX);
    }
}
