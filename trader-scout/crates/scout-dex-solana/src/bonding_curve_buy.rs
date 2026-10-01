//! pump.fun bonding-curve `buy`/`sell` instruction decoder.
//!
//! Scope note (AGENTS.md invariant #16): decodes
//! `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` specifically — a
//! **confirmed** deployment per `docs/p0/deployment-registry.md`'s
//! "2026-10-01 confirmation" section:
//!
//! - On-chain: `executable: true`, invoked in 5/5 probed transactions.
//! - Official IDL: `github.com/pump-fun/pump-public-docs`,
//!   `idl/pump.json`, commit `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`
//!   (2026-09-12) — the project's own published-docs repository. Its
//!   `buy`/`sell` discriminators, argument names/types, and account
//!   order independently reproduce every fact this workspace derived
//!   empirically from live transaction data *before* that IDL was
//!   consulted (see the registry section's side-by-side table), which
//!   is why it is treated as confirmed rather than merely "found on
//!   GitHub."
//! - Balance-delta cross-validation: the real `amount` argument on a
//!   live `buy` instruction (`2979651581366`) is byte-for-byte equal
//!   to the token-balance delta observed between the bonding-curve
//!   vault and the buyer's ATA in that same transaction's
//!   `postTokenBalances`.
//!
//! `docs/p0/deployment-registry.md`'s four-condition schema table still
//! has no row for this deployment: conditions #1 (on-chain-confirmed
//! address) and #3 (pinned IDL commit hash) are satisfied; #2
//! (activation slot) and #4 (committed golden fixture + decoder
//! exercising it) land with this module and the fixture captured
//! alongside it (`docs/p0/measurements/fixtures/pump_bonding_curve_buy_probe.json`).
//!
//! Real `buy` instruction layout (16 accounts, per the official IDL):
//! `[0] global, [1] fee_recipient, [2] mint, [3] bonding_curve,
//! [4] associated_bonding_curve, [5] associated_user, [6] user,
//! [7] system_program, [8] token_program, [9] creator_vault,
//! [10] event_authority, [11] program, [12] global_volume_accumulator,
//! [13] user_volume_accumulator, [14] fee_config, [15] fee_program]`.
//! Args: `amount: u64, max_sol_cost: u64, track_volume: OptionBool`
//! (Borsh-encodes `OptionBool { 0: bool }` as a single trailing byte —
//! NOT Rust's `Option<bool>` Borsh encoding, which would be a
//! discriminant byte followed by a conditional bool; this type is a
//! one-field struct wrapping a plain `bool`, always exactly 1 byte).
//! Total data: 8 (discriminator) + 8 + 8 + 1 = 25 bytes.
//!
//! Real `sell` instruction layout (14 accounts): `[0] global,
//! [1] fee_recipient, [2] mint, [3] bonding_curve,
//! [4] associated_bonding_curve, [5] associated_user, [6] user,
//! [7] system_program, [8] creator_vault, [9] token_program,
//! [10] event_authority, [11] program, [12] fee_config,
//! [13] fee_program]`. Args: `amount: u64, min_sol_output: u64`. Total
//! data: 8 + 8 + 8 = 24 bytes.
//!
//! `accounts[6]` (`user`) is the buyer/seller signer for THIS
//! deployment specifically -- this is not a general positional rule
//! for Solana programs (the PumpSwap AMM program has no such verified
//! position; see `docs/p0/deployment-registry.md`'s buyer-identification
//! section). It is verified here via SOL-balance delta on a live
//! transaction, confirmed against the IDL's own account name.
//!
//! Implements `scout_api::TxDecoder` (ADR-008 S4). `DecodeOutcome::NotMine`
//! for an instruction with a different discriminator, OR for an
//! instruction whose `program_id` is not in this decoder's
//! `DeploymentScope::contract_addresses` (see the false-positive
//! regression test below -- this gate is load-bearing: the `buy`
//! discriminator `sha256("global:buy")[..8]` was observed colliding
//! with an unrelated Anchor `#[event_cpi]` self-invoked event log under
//! the PumpSwap AMM program in a live transaction. `DecodeOutcome::Malformed`
//! for an instruction that matches both gates but has a broken account
//! count or data length (invariant #18: never silently skipped).

use scout_api::{DecodeOutcome, DeploymentScope, TxDecoder};
use scout_core::{RawSolanaInstruction, SolanaPubkey};

/// `sha256("global:buy")[..8]`, per the official IDL's `buy` instruction.
pub const BUY_INSTRUCTION_DISCRIMINATOR: [u8; 8] = [0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea];

/// `sha256("global:sell")[..8]`, per the official IDL's `sell` instruction.
pub const SELL_INSTRUCTION_DISCRIMINATOR: [u8; 8] =
    [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];

const BUY_ACCOUNT_COUNT: usize = 16;
const BUY_DATA_LEN: usize = 25;
const SELL_ACCOUNT_COUNT: usize = 14;
const SELL_DATA_LEN: usize = 24;

/// Account positions per the official IDL's `buy`/`sell` account lists
/// -- both instructions share the same first 7 positions.
const ACCOUNT_IDX_MINT: usize = 2;
const ACCOUNT_IDX_BONDING_CURVE: usize = 3;
const ACCOUNT_IDX_USER: usize = 6;

/// A decoded bonding-curve `buy`. `max_sol_cost` is the buyer's
/// declared slippage ceiling, not what was actually paid -- actual SOL
/// paid is contaminated by fees/rent/router hops and is explicitly out
/// of scope here (see `scout_core::RawSolanaTransaction`'s doc comment);
/// the actual token amount received is `amount`, cross-validated
/// against live `postTokenBalances` deltas in the deployment-registry
/// confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBondingCurveBuy {
    pub user: SolanaPubkey,
    pub bonding_curve: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub amount: u64,
    pub max_sol_cost: u64,
    /// Borsh-decoded `OptionBool` payload byte, carried through
    /// unmodified -- not re-interpreted as a boolean here beyond
    /// storing the raw value, since the IDL does not document what
    /// semantic effect the value has on-chain beyond its name.
    pub track_volume: u8,
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
}

/// A decoded bonding-curve `sell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBondingCurveSell {
    pub user: SolanaPubkey,
    pub bonding_curve: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub amount: u64,
    pub min_sol_output: u64,
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
}

/// Either a decoded `buy` or `sell` -- a single `TxDecoder` impl covers
/// both since they share the same program, account-position
/// conventions, and discriminator derivation scheme; splitting them
/// into two decoders would force a registry to run the program-id gate
/// twice per instruction for no benefit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedBondingCurveTrade {
    Buy(DecodedBondingCurveBuy),
    Sell(DecodedBondingCurveSell),
}

/// A `TxDecoder` for pump.fun bonding-curve `buy`/`sell`, scoped to one
/// `DeploymentScope` (AGENTS.md invariant #16: mandatory at
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

impl TxDecoder<RawSolanaInstruction, DecodedBondingCurveTrade> for BondingCurveBuyDecoder {
    fn scope(&self) -> &DeploymentScope {
        &self.scope
    }

    fn decode(
        &self,
        instruction: &RawSolanaInstruction,
    ) -> DecodeOutcome<DecodedBondingCurveTrade> {
        // Program-id gate (AGENTS.md invariant #16): without this, a
        // matching discriminator alone would fire on ANY program,
        // including an unrelated one that happens to collide on the
        // same 8 bytes -- which is exactly what was observed in live
        // data (the BUY discriminator collided with an Anchor
        // #[event_cpi] log under the unrelated PumpSwap AMM program).
        // An empty `contract_addresses` means NOTHING can ever decode
        // here, by design.
        let program_matches = self.scope.contract_addresses.iter().any(
            |addr| matches!(addr, scout_core::AddressBytes::Solana(p) if *p == instruction.program_id),
        );
        if !program_matches {
            return DecodeOutcome::NotMine;
        }
        decode_bonding_curve_instruction(instruction, 0, 0)
    }
}

/// Decode a raw instruction as a pump.fun bonding-curve `buy` or `sell`.
///
/// `DecodeOutcome::NotMine` for an instruction with a different
/// discriminator -- the expected, common case when scanning a
/// transaction's mixed instructions. `DecodeOutcome::Malformed` for an
/// instruction that matches a discriminator but has a broken account
/// count or data length (AGENTS.md invariant #18).
#[must_use]
pub fn decode_bonding_curve_instruction(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> DecodeOutcome<DecodedBondingCurveTrade> {
    if instruction.data.len() < 8 {
        return DecodeOutcome::NotMine;
    }
    let Some(discriminator) = instruction.data.get(0..8) else {
        return DecodeOutcome::NotMine;
    };

    if discriminator == BUY_INSTRUCTION_DISCRIMINATOR {
        return decode_buy(instruction, slot, transaction_index)
            .map_or_else(DecodeOutcome::Malformed, |buy| {
                DecodeOutcome::Decoded(DecodedBondingCurveTrade::Buy(buy))
            });
    }
    if discriminator == SELL_INSTRUCTION_DISCRIMINATOR {
        return decode_sell(instruction, slot, transaction_index)
            .map_or_else(DecodeOutcome::Malformed, |sell| {
                DecodeOutcome::Decoded(DecodedBondingCurveTrade::Sell(sell))
            });
    }
    DecodeOutcome::NotMine
}

fn decode_buy(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> Result<DecodedBondingCurveBuy, String> {
    if instruction.data.len() != BUY_DATA_LEN {
        return Err(format!(
            "instruction matches buy discriminator but data has {} bytes, expected {BUY_DATA_LEN} \
             (8-byte discriminator + u64 amount + u64 max_sol_cost + 1-byte OptionBool)",
            instruction.data.len()
        ));
    }
    if instruction.accounts.len() != BUY_ACCOUNT_COUNT {
        return Err(format!(
            "instruction matches buy discriminator but has {} accounts, expected {BUY_ACCOUNT_COUNT} \
             per the official IDL's buy account list",
            instruction.accounts.len()
        ));
    }

    let amount = read_u64_le(&instruction.data, 8).ok_or_else(|| {
        "instruction matches buy discriminator but amount field is unreadable".to_string()
    })?;
    let max_sol_cost = read_u64_le(&instruction.data, 16).ok_or_else(|| {
        "instruction matches buy discriminator but max_sol_cost field is unreadable".to_string()
    })?;
    let track_volume = *instruction.data.get(24).ok_or_else(|| {
        "instruction matches buy discriminator but track_volume byte is unreadable".to_string()
    })?;

    let (mint, bonding_curve, user) =
        read_core_accounts(&instruction.accounts).ok_or_else(|| {
            "instruction matches buy discriminator but core accounts are missing".to_string()
        })?;

    Ok(DecodedBondingCurveBuy {
        user,
        bonding_curve,
        mint,
        amount,
        max_sol_cost,
        track_volume,
        slot,
        transaction_index,
        instruction_index: instruction.instruction_index,
    })
}

fn decode_sell(
    instruction: &RawSolanaInstruction,
    slot: u64,
    transaction_index: u64,
) -> Result<DecodedBondingCurveSell, String> {
    if instruction.data.len() != SELL_DATA_LEN {
        return Err(format!(
            "instruction matches sell discriminator but data has {} bytes, expected {SELL_DATA_LEN} \
             (8-byte discriminator + u64 amount + u64 min_sol_output)",
            instruction.data.len()
        ));
    }
    if instruction.accounts.len() != SELL_ACCOUNT_COUNT {
        return Err(format!(
            "instruction matches sell discriminator but has {} accounts, expected {SELL_ACCOUNT_COUNT} \
             per the official IDL's sell account list",
            instruction.accounts.len()
        ));
    }

    let amount = read_u64_le(&instruction.data, 8).ok_or_else(|| {
        "instruction matches sell discriminator but amount field is unreadable".to_string()
    })?;
    let min_sol_output = read_u64_le(&instruction.data, 16).ok_or_else(|| {
        "instruction matches sell discriminator but min_sol_output field is unreadable".to_string()
    })?;

    let (mint, bonding_curve, user) =
        read_core_accounts(&instruction.accounts).ok_or_else(|| {
            "instruction matches sell discriminator but core accounts are missing".to_string()
        })?;

    Ok(DecodedBondingCurveSell {
        user,
        bonding_curve,
        mint,
        amount,
        min_sol_output,
        slot,
        transaction_index,
        instruction_index: instruction.instruction_index,
    })
}

/// Reads the three accounts this module actually consumes
/// (mint/bonding_curve/user) by their fixed IDL positions, shared by
/// both `buy` and `sell`'s account lists. Per-instruction length checks
/// already happened in the caller; this only guards against an
/// out-of-bounds read, which should be unreachable given those checks.
fn read_core_accounts(
    accounts: &[SolanaPubkey],
) -> Option<(SolanaPubkey, SolanaPubkey, SolanaPubkey)> {
    let mint = *accounts.get(ACCOUNT_IDX_MINT)?;
    let bonding_curve = *accounts.get(ACCOUNT_IDX_BONDING_CURVE)?;
    let user = *accounts.get(ACCOUNT_IDX_USER)?;
    Some((mint, bonding_curve, user))
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    let slice = data.get(offset..offset + 8)?;
    let array: [u8; 8] = slice.try_into().ok()?;
    Some(u64::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real program id, decoded from base58
    /// (`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`), confirmed per
    /// `docs/p0/deployment-registry.md`'s "2026-10-01 confirmation".
    fn real_program_id() -> SolanaPubkey {
        decode_pubkey_for_test("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
    }

    fn real_scope() -> DeploymentScope {
        DeploymentScope {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            contract_addresses: vec![scout_core::AddressBytes::Solana(real_program_id())],
            active_from: 0,
            active_until: None,
        }
    }

    /// Real buy instruction data captured live
    /// (`docs/p0/measurements/fixtures/pump_bonding_curve_buy_probe.json`,
    /// slot 452380124, the AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump
    /// transaction): discriminator + amount=2979651581366 +
    /// max_sol_cost=699200000 + track_volume=1. 18 raw account indices
    /// were captured on-chain for this instruction, but only 16 are
    /// meaningful per the IDL (positions beyond 15 do not appear in
    /// the official buy account list for this build -- the extra 2
    /// observed in the live capture are not modeled by this decoder;
    /// this fixture trims to exactly the IDL's 16 to exercise the
    /// documented contract, not whatever the live transaction's exact
    /// resolved length happened to be).
    fn real_buy_data() -> Vec<u8> {
        hex_decode("66063d1201daebeab6f512c1b502000000f2ac290000000001")
    }

    fn real_sell_data() -> Vec<u8> {
        hex_decode("33e685a4017f83adddede2ca280000000000000000000000")
    }

    fn account_list(count: usize) -> Vec<SolanaPubkey> {
        (0..count)
            .map(|i| {
                let mut key = [0u8; 32];
                key[0] = u8::try_from(i).expect("test account count fits in u8");
                key
            })
            .collect()
    }

    fn buy_instruction() -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: account_list(BUY_ACCOUNT_COUNT),
            data: real_buy_data(),
            instruction_index: 3,
        }
    }

    fn sell_instruction() -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: account_list(SELL_ACCOUNT_COUNT),
            data: real_sell_data(),
            instruction_index: 2,
        }
    }

    #[test]
    fn decodes_a_real_buy_instruction_with_correct_argument_values() {
        let decoded = decode_bonding_curve_instruction(&buy_instruction(), 452_380_124, 645)
            .decoded()
            .unwrap();
        let DecodedBondingCurveTrade::Buy(buy) = decoded else {
            panic!("expected Buy variant");
        };
        // Real values from the live capture, cross-validated in
        // deployment-registry.md against postTokenBalances deltas.
        assert_eq!(buy.amount, 2_979_651_581_366);
        assert_eq!(buy.max_sol_cost, 699_200_000);
        assert_eq!(buy.track_volume, 1);
        assert_eq!(buy.slot, 452_380_124);
        assert_eq!(buy.transaction_index, 645);
        assert_eq!(buy.instruction_index, 3);
    }

    #[test]
    fn buy_reads_mint_bonding_curve_and_user_from_correct_positions() {
        let accounts = account_list(BUY_ACCOUNT_COUNT);
        let decoded = decode_bonding_curve_instruction(&buy_instruction(), 0, 0)
            .decoded()
            .unwrap();
        let DecodedBondingCurveTrade::Buy(buy) = decoded else {
            panic!("expected Buy variant");
        };
        assert_eq!(buy.mint, accounts[ACCOUNT_IDX_MINT]);
        assert_eq!(buy.bonding_curve, accounts[ACCOUNT_IDX_BONDING_CURVE]);
        assert_eq!(buy.user, accounts[ACCOUNT_IDX_USER]);
    }

    #[test]
    fn decodes_a_real_sell_instruction_with_correct_argument_values() {
        let decoded = decode_bonding_curve_instruction(&sell_instruction(), 452_380_124, 671)
            .decoded()
            .unwrap();
        let DecodedBondingCurveTrade::Sell(sell) = decoded else {
            panic!("expected Sell variant");
        };
        assert_eq!(sell.amount, 175_202_561_501);
        assert_eq!(sell.min_sol_output, 0);
    }

    #[test]
    fn wrong_discriminator_is_not_mine_not_an_error() {
        let mut instruction = buy_instruction();
        instruction.data[0] = 0xFF;
        let outcome = decode_bonding_curve_instruction(&instruction, 1, 0);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn buy_with_wrong_account_count_is_malformed_not_not_mine() {
        // AGENTS.md invariant #18: unfamiliar shape is surfaced, never
        // silently skipped or conflated with "not my instruction."
        let mut instruction = buy_instruction();
        instruction.accounts.pop();
        let outcome = decode_bonding_curve_instruction(&instruction, 1, 0);
        assert!(outcome.is_malformed());
        assert!(!outcome.is_not_mine());
    }

    #[test]
    fn buy_with_wrong_data_length_is_malformed() {
        let mut instruction = buy_instruction();
        instruction.data.push(0x00);
        let outcome = decode_bonding_curve_instruction(&instruction, 1, 0);
        assert!(outcome.is_malformed());
    }

    #[test]
    fn sell_with_wrong_account_count_is_malformed() {
        let mut instruction = sell_instruction();
        instruction.accounts.pop();
        let outcome = decode_bonding_curve_instruction(&instruction, 1, 0);
        assert!(outcome.is_malformed());
    }

    #[test]
    fn too_short_instruction_data_is_not_mine() {
        let instruction = RawSolanaInstruction {
            program_id: real_program_id(),
            accounts: vec![],
            data: vec![0x01, 0x02],
            instruction_index: 0,
        };
        let outcome = decode_bonding_curve_instruction(&instruction, 1, 0);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn bonding_curve_decoder_implements_tx_decoder_trait() {
        let decoder = BondingCurveBuyDecoder::new(real_scope());
        let outcome = decoder.decode(&buy_instruction());
        assert!(matches!(
            outcome,
            DecodeOutcome::Decoded(DecodedBondingCurveTrade::Buy(_))
        ));
    }

    #[test]
    fn decoder_rejects_matching_discriminator_from_an_unregistered_program() {
        // Same discriminator, same well-formed instruction shape as the
        // "Decoded" case above -- the only difference is program_id is
        // NOT in the decoder's DeploymentScope. Must be NotMine. This
        // proves the program-id gate exists: the
        // "implements_tx_decoder_trait" test above uses a program_id
        // that IS registered, so it would pass even with no gate.
        let decoder = BondingCurveBuyDecoder::new(real_scope());
        let mut instruction = buy_instruction();
        instruction.program_id = [0xBB; 32]; // not in contract_addresses
        let outcome = decoder.decode(&instruction);
        assert_eq!(outcome, DecodeOutcome::NotMine);
    }

    #[test]
    fn real_pumpswap_event_cpi_payload_is_not_decoded_as_a_trade() {
        // Regression test built from REAL bytes captured live during
        // an earlier session (see docs/p0/deployment-registry.md,
        // "Solana census findings"). discriminator 66063d1201daebea ==
        // sha256("global:buy")[..8] -- exactly
        // BUY_INSTRUCTION_DISCRIMINATOR -- but these 24 bytes are an
        // Anchor #[event_cpi] self-invoked event log emitted by the
        // UNRELATED PumpSwap AMM program (pAMMBay6...), not a real
        // bonding-curve buy instruction. The trailing 16 bytes decode
        // as two little-endian u64s (206321, 10) matching a token-
        // balance delta observed in that same live transaction, not
        // (amount, max_sol_cost). Also note: this payload is only 24
        // bytes, one short of this module's real BUY_DATA_LEN (25,
        // including the track_volume byte) -- it would fail the
        // length check even if the program-id gate were absent, but
        // the gate is what actually prevents a registry from
        // attempting this decoder against the wrong program's
        // instructions at all.
        let decoder = BondingCurveBuyDecoder::new(real_scope());
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_INSTRUCTION_DISCRIMINATOR);
        data.extend_from_slice(&hex_decode("f1250300000000000a00000000000000"));
        let instruction = RawSolanaInstruction {
            // Real PumpSwap AMM program id observed in the census --
            // deliberately NOT real_program_id().
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
    /// `u32::try_from`/`u8::try_from` throughout -- workspace lint
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
