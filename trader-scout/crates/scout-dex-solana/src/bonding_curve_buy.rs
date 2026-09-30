//! Generic bonding-curve-shape "buy" instruction decoder.
//!
//! Scope note (AGENTS.md invariant #16): this decodes a **synthetic**
//! bonding-curve buy instruction shape — an 8-byte discriminator
//! followed by a little-endian u64 "sol_in" amount and a little-endian
//! u64 "min_tokens_out" amount, with accounts
//! [buyer, bonding_curve, mint, buyer_token_account] in that positional
//! order. **This shape has not been confirmed against a live pump.fun
//! bonding-curve transaction.** A live census of 10 real transactions
//! (docs/p0/deployment-registry.md, "Solana census findings") found:
//!
//! - The bonding-curve program (`6EF8rr...`) never appeared in the
//!   sample — both sampled mints had already migrated to AMM trading,
//!   so this module's actual target instruction shape remains unseen
//!   in live data.
//! - The real pump.fun `buy` instruction (per public IDL/decompiled
//!   sources referenced during that census, not re-verified here)
//!   takes `token_amount` then `max_sol_cost` — the **inverse** order
//!   and inverse semantics of this module's `sol_in`/`min_tokens_out`.
//! - `accounts[0]` in a real pump.fun-family instruction is commonly
//!   the fee payer/signer, not the buyer. No positional buyer slot has
//!   been verified for any real deployment; the only method
//!   demonstrated so far is matching `postTokenBalances[].owner` for
//!   the account whose balance increased for the target mint.
//!
//! `docs/p0/deployment-registry.md` has zero confirmed entries. This
//! module proves the decode *mechanism* (raw instruction -> typed buy)
//! against a synthetic fixture only, mirroring what scout-dex-evm's
//! v2_swap.rs does for the EVM vertical slice — it does not claim to
//! decode any specific deployed program's actual `buy` instruction.
//!
//! Implements `scout_api::TxDecoder` (ADR-008 S4). `DecodeOutcome::NotMine`
//! for an instruction with a different discriminator, OR for an
//! instruction whose `program_id` is not in this decoder's
//! `DeploymentScope::contract_addresses` (see the false-positive
//! regression test below — this gate is load-bearing, not cosmetic).
//! `DecodeOutcome::Malformed` for an instruction that matches both
//! gates but has a broken account count or data length (invariant #18:
//! never silently skipped).

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{RawSolanaInstruction, SolanaPubkey};

/// This module's own 8-byte discriminator constant for the synthetic
/// "buy" instruction shape it decodes. This is **not** an arbitrary
/// made-up value: it is `sha256("global:buy")[..8]`, the standard
/// Anchor instruction-discriminator derivation for a global `buy`
/// instruction name, and would be the correct discriminator *if* a
/// deployment used Anchor's default naming for its buy instruction.
///
/// However, a live census (docs/p0/deployment-registry.md) found this
/// exact 8-byte value also heads an unrelated 24-byte Anchor
/// `#[event_cpi]` self-invoked event log under the PumpSwap AMM
/// program (`pAMMBay6...`) — a coincidental collision in the 8-byte
/// discriminator space, carrying a balance-delta payload completely
/// unrelated to a buy instruction's arguments. Discriminator match
/// alone is therefore not sufficient to claim "Decoded" on real data;
/// `decode()`'s program-id gate against `DeploymentScope` exists
/// specifically to prevent this decoder from firing on that collision
/// (or any other program that happens to share these 8 bytes) until a
/// real deployment address is confirmed and registered.
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
        // Program-id gate (AGENTS.md invariant #16): without this, a
        // matching discriminator alone would fire on ANY program,
        // including an unrelated one that happens to collide on the
        // same 8 bytes -- which is exactly what was observed in live
        // data (see the false-positive regression test below). An
        // empty `contract_addresses` (the honest state until P0.2 has
        // a confirmed deployment) means NOTHING can ever decode here,
        // by design.
        let program_matches = self
            .scope
            .contract_addresses
            .iter()
            .any(|addr| matches!(addr, scout_core::AddressBytes::Solana(p) if *p == instruction.program_id));
        if !program_matches {
            return DecodeOutcome::NotMine;
        }
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

    #[test]
    fn decoder_rejects_matching_discriminator_from_an_unregistered_program() {
        // Same discriminator, same well-formed instruction shape as the
        // "Decoded" case above -- the only difference is program_id is
        // NOT in the decoder's DeploymentScope. Must be NotMine. This
        // is what actually proves the program-id gate exists: the
        // "implements_tx_decoder_trait" test above uses a program_id
        // that IS registered, so it would also pass with no gate at
        // all.
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
        let mut instruction = synthetic_buy_instruction(1, 1);
        instruction.program_id = [0xBB; 32]; // not in contract_addresses
        let outcome = decoder.decode(&instruction);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn real_pumpswap_event_cpi_payload_is_not_decoded_as_a_buy() {
        // Regression test built from REAL bytes captured live (see
        // docs/p0/deployment-registry.md, "Solana census findings").
        // discriminator 66063d1201daebea == sha256("global:buy")[..8]
        // -- exactly BUY_INSTRUCTION_DISCRIMINATOR -- but these 24
        // bytes are an Anchor #[event_cpi] self-invoked event log
        // emitted by the PumpSwap AMM program (pAMMBay6...), not a buy
        // instruction. The trailing 16 bytes decode as two little-
        // endian u64s (206321, 10) matching a token-balance delta
        // observed in that same live transaction, not
        // (sol_in, min_tokens_out).
        //
        // Without the program-id gate, this would incorrectly return
        // DecodeOutcome::Decoded with fabricated economics (sol_in =
        // 206321, min_tokens_out = 10) handed to the ledger as if it
        // were a real buy. The gate must reject it because the AMM's
        // program id is not this decoder's registered pump.fun
        // bonding-curve deployment.
        let scope = DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            // Deliberately NOT the real PumpSwap AMM program id --
            // this decoder has no confirmed deployment to register
            // (docs/p0/deployment-registry.md has zero qualifying
            // entries), so contract_addresses stays empty/placeholder,
            // which alone is enough to prove the gate rejects this.
            contract_addresses: vec![scout_core::AddressBytes::Solana([0xAA; 32])],
            active_from: 0,
            active_until: None,
        };
        let decoder = BondingCurveBuyDecoder::new(scope);
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&hex_decode("f1250300000000000a00000000000000"));
        let instruction = RawSolanaInstruction {
            // Real PumpSwap AMM program id observed in the census.
            program_id: decode_pubkey_for_test("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"),
            accounts: vec![[0x11; 32], [0x22; 32], [0x33; 32], [0x44; 32]],
            data,
            instruction_index: 7,
        };
        let outcome = decoder.decode(&instruction);
        assert_eq!(
            outcome,
            DecodeOutcome::NotMine,
            "matching discriminator from an unregistered program must never be Decoded"
        );
    }

    /// Minimal base58 decode for a Solana pubkey, test-only (avoids a
    /// bs58 dev-dependency just for this one regression test). Uses
    /// `u32::try_from`/`u8::try_from` throughout — workspace lint
    /// policy denies `as` conversions even in test code.
    fn decode_pubkey_for_test(s: &str) -> SolanaPubkey {
        const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let mut bytes = vec![0u8; 1];
        for c in s.chars() {
            let byte_value = u8::try_from(c).expect("test fixture pubkey must be ASCII base58");
            let digit = u32::try_from(
                ALPHABET
                    .iter()
                    .position(|&b| b == byte_value)
                    .expect("test fixture pubkey must be valid base58"),
            )
            .expect("base58 alphabet index fits in u32");
            let mut carry = digit;
            for byte in bytes.iter_mut() {
                carry += u32::from(*byte) * 58;
                *byte = u8::try_from(carry & 0xFF).expect("masked byte fits in u8");
                carry >>= 8;
            }
            while carry > 0 {
                bytes.push(u8::try_from(carry & 0xFF).expect("masked byte fits in u8"));
                carry >>= 8;
            }
        }
        for c in s.chars() {
            if c == '1' {
                bytes.push(0);
            } else {
                break;
            }
        }
        bytes.reverse();
        while bytes.len() < 32 {
            bytes.insert(0, 0);
        }
        let mut out = [0u8; 32];
        let start = bytes.len().saturating_sub(32);
        out.copy_from_slice(&bytes[start..]);
        out
    }

    /// Minimal hex decode, test-only.
    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("test fixture hex must be valid"))
            .collect()
    }
}
