//! Raydium CPMM `SwapEvent` (ADR-013 section 2b, P4.9).
//!
//! Program `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C`. Raydium CPMM emits
//! `SwapEvent` with Anchor `emit!`: a `Program data:` log line of the swap
//! instruction itself (NOT an event-CPI), attributed by the invoke/success
//! stack ([`crate::venue_log`]). Live: 14 of 14 swap instructions of the
//! committed fixtures carry exactly one line, 170 bytes including the
//! discriminator = the pinned IDL layout exactly.
//!
//! ## Schema source
//!
//! Official `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`
//! (`raydium_cp_swap/raydium_cp_swap.json` 0.2.0), pinned as
//! `fixtures/raydium_cpmm_idl_e7e0c96f.json`, sha256
//! `1202f6dc8e1c3216598f2ad5c620b9aa8c64ac584563fafed68125c27fb6df81`.
//!
//! `SwapEvent` (`40c6cde8260871e2`, the same discriminator as Jupiter's and
//! CLMM's `SwapEvent`), payload 162 bytes: `pool_id, input_vault_before u64,
//! output_vault_before u64, input_amount u64, output_amount u64,
//! input_transfer_fee u64, output_transfer_fee u64, base_input bool,
//! input_mint, output_mint, trade_fee u64, creator_fee u64,
//! creator_fee_on_input bool`. Exact length; bools strictly 0/1.
//!
//! Unlike the other venues the event itself names both mints. Instruction
//! gates (live 14/14): `pool_state`=3, `input_token_mint`=10,
//! `output_token_mint`=11 equal the event's `pool_id`, `input_mint`,
//! `output_mint`; `base_input` equals "the instruction is `swap_base_input`".
//! `swap_base_input` has live samples; `swap_base_output` is IDL only (its
//! events are IdlOnly: decoded, counted, never evidence).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::VariantVerification;
use crate::venue_wire::{
    Reader, SwapInstructionParse, VenueEventOutcome, event_name, malformed, split_event,
};

/// Raydium CPMM program id (base58).
pub const RAYDIUM_CPMM_PROGRAM_ID: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
/// Raydium CPMM program id bytes.
pub const RAYDIUM_CPMM_PROGRAM_ID_BYTES: SolanaPubkey = [
    0xa9, 0x2a, 0x5a, 0x8b, 0x4f, 0x29, 0x59, 0x52, 0x84, 0x25, 0x50, 0xaa, 0x93, 0xfd, 0x5b, 0x95,
    0xb5, 0xac, 0xe6, 0xa8, 0xeb, 0x92, 0x0c, 0x93, 0x94, 0x2e, 0x43, 0x69, 0x0c, 0x20, 0xec, 0x73,
];
/// sha256 of the pinned CPMM IDL file.
pub const RAYDIUM_CPMM_IDL_SHA256: &str =
    "1202f6dc8e1c3216598f2ad5c620b9aa8c64ac584563fafed68125c27fb6df81";
/// `SwapEvent` discriminator.
pub const RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
/// `SwapEvent` payload length (after the 8-byte discriminator).
pub const RAYDIUM_CPMM_SWAP_EVENT_LEN: usize = 162;

/// `swap_base_input` instruction discriminator.
pub const RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR: [u8; 8] =
    [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde];
/// `swap_base_output` instruction discriminator.
pub const RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR: [u8; 8] =
    [0x37, 0xd9, 0x62, 0x56, 0xa3, 0x4a, 0xb4, 0xad];

/// Every event of the pinned IDL (name, discriminator). Only `SwapEvent` is a
/// swap; `LpChangeEvent` is a known non-leg.
pub const RAYDIUM_CPMM_EVENTS: [(&str, [u8; 8]); 2] = [
    (
        "LpChangeEvent",
        [0x79, 0xa3, 0xcd, 0xc9, 0x39, 0xda, 0x75, 0x3c],
    ),
    (
        "SwapEvent",
        [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2],
    ),
];

/// A decoded Raydium CPMM `SwapEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaydiumCpmmSwapEvent {
    pub pool_id: SolanaPubkey,
    pub input_vault_before: u64,
    pub output_vault_before: u64,
    pub input_amount: u64,
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub base_input: bool,
    pub input_mint: SolanaPubkey,
    pub output_mint: SolanaPubkey,
    pub trade_fee: u64,
    pub creator_fee: u64,
    pub creator_fee_on_input: bool,
}

/// Classifies one decoded `Program data:` payload attributed to the Raydium
/// CPMM program.
#[must_use]
pub fn classify_raydium_cpmm_event(payload: &[u8]) -> VenueEventOutcome<RaydiumCpmmSwapEvent> {
    let Some((disc, body)) = split_event(payload) else {
        return malformed(format!(
            "CPMM event payload has {} bytes, fewer than the 8-byte discriminator",
            payload.len()
        ));
    };
    let Some(name) = event_name(&RAYDIUM_CPMM_EVENTS, &disc) else {
        return VenueEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    };
    if disc != RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR {
        return VenueEventOutcome::KnownNonLeg { name };
    }
    if body.len() != RAYDIUM_CPMM_SWAP_EVENT_LEN {
        return malformed(format!(
            "SwapEvent payload has {} bytes, expected exactly {RAYDIUM_CPMM_SWAP_EVENT_LEN}",
            body.len()
        ));
    }
    let mut r = Reader::new(body);
    let parsed = (|| {
        Some(RaydiumCpmmSwapEvent {
            pool_id: r.pubkey()?,
            input_vault_before: r.u64()?,
            output_vault_before: r.u64()?,
            input_amount: r.u64()?,
            output_amount: r.u64()?,
            input_transfer_fee: r.u64()?,
            output_transfer_fee: r.u64()?,
            base_input: r.bool()?,
            input_mint: r.pubkey()?,
            output_mint: r.pubkey()?,
            trade_fee: r.u64()?,
            creator_fee: r.u64()?,
            creator_fee_on_input: r.bool()?,
        })
    })();
    parsed.map_or_else(
        || malformed("SwapEvent fields do not fit the payload or a bool is not 0/1".to_owned()),
        VenueEventOutcome::Event,
    )
}

/// Swap instruction variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaydiumCpmmSwapVariant {
    SwapBaseInput,
    SwapBaseOutput,
}

impl RaydiumCpmmSwapVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SwapBaseInput => "swap_base_input",
            Self::SwapBaseOutput => "swap_base_output",
        }
    }

    /// `swap_base_input` has live samples (14); `swap_base_output` is IDL only.
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        match self {
            Self::SwapBaseInput => VariantVerification::FixtureVerified,
            Self::SwapBaseOutput => VariantVerification::IdlOnly,
        }
    }

    /// The `base_input` flag the event must carry.
    #[must_use]
    pub const fn is_base_input(self) -> bool {
        matches!(self, Self::SwapBaseInput)
    }
}

/// A parsed Raydium CPMM swap instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaydiumCpmmSwapIx {
    pub variant: RaydiumCpmmSwapVariant,
    pub pool_state: SolanaPubkey,
    /// Owner of both vaults (account 1, `authority`).
    pub authority: SolanaPubkey,
    pub input_mint: SolanaPubkey,
    pub output_mint: SolanaPubkey,
}

/// Parses one instruction ASSUMED to belong to Raydium CPMM.
#[must_use]
pub fn parse_raydium_cpmm_swap_instruction(
    ix: &RawSolanaInstruction,
) -> SwapInstructionParse<RaydiumCpmmSwapIx> {
    let Some(disc) = ix.data.get(..8) else {
        return SwapInstructionParse::NotSwap;
    };
    let variant = if disc == RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR {
        RaydiumCpmmSwapVariant::SwapBaseInput
    } else if disc == RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR {
        RaydiumCpmmSwapVariant::SwapBaseOutput
    } else {
        return SwapInstructionParse::NotSwap;
    };
    // discriminator + two u64 args; the IDL lists 13 accounts.
    if ix.data.len() != 24 || ix.accounts.len() < 13 {
        return SwapInstructionParse::Malformed {
            reason: format!(
                "{} has {} data bytes / {} accounts (need exactly 24 bytes, >= 13 accounts)",
                variant.name(),
                ix.data.len(),
                ix.accounts.len()
            ),
        };
    }
    let acct = |i: usize| ix.accounts.get(i).copied();
    match (acct(1), acct(3), acct(10), acct(11)) {
        (Some(authority), Some(pool_state), Some(input_mint), Some(output_mint)) => {
            SwapInstructionParse::Swap(RaydiumCpmmSwapIx {
                variant,
                pool_state,
                authority,
                input_mint,
                output_mint,
            })
        }
        _ => SwapInstructionParse::Malformed {
            reason: "pool / mint accounts missing".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const IDL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/raydium_cpmm_idl_e7e0c96f.json"
    );

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn idl() -> serde_json::Value {
        let bytes = std::fs::read(IDL_PATH).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), RAYDIUM_CPMM_IDL_SHA256);
        serde_json::from_slice(&bytes).unwrap()
    }

    fn disc(prefix: &str, name: &str) -> [u8; 8] {
        Sha256::digest(format!("{prefix}:{name}").as_bytes())[..8]
            .try_into()
            .unwrap()
    }

    fn idl_bytes(v: &serde_json::Value) -> Vec<u8> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn idl_equality_events_and_swap_event_layout() {
        let idl = idl();
        assert_eq!(idl["address"], RAYDIUM_CPMM_PROGRAM_ID);
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), RAYDIUM_CPMM_EVENTS.len());
        for (name, d) in RAYDIUM_CPMM_EVENTS {
            let e = events.iter().find(|e| e["name"] == name).unwrap();
            assert_eq!(idl_bytes(&e["discriminator"]), d, "{name} (IDL)");
            assert_eq!(disc("event", name), d, "{name} (sha256)");
        }
        let ty = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "SwapEvent")
            .unwrap();
        let fields: Vec<(String, String)> = ty["type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().to_owned(),
                    f["type"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let p = |n: &str, t: &str| (n.to_owned(), t.to_owned());
        assert_eq!(
            fields,
            vec![
                p("pool_id", "pubkey"),
                p("input_vault_before", "u64"),
                p("output_vault_before", "u64"),
                p("input_amount", "u64"),
                p("output_amount", "u64"),
                p("input_transfer_fee", "u64"),
                p("output_transfer_fee", "u64"),
                p("base_input", "bool"),
                p("input_mint", "pubkey"),
                p("output_mint", "pubkey"),
                p("trade_fee", "u64"),
                p("creator_fee", "u64"),
                p("creator_fee_on_input", "bool"),
            ]
        );
        assert_eq!(
            32 + 6 * 8 + 1 + 32 + 32 + 8 + 8 + 1,
            RAYDIUM_CPMM_SWAP_EVENT_LEN
        );
        assert_eq!(
            disc("event", "SwapEvent"),
            RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR
        );
    }

    #[test]
    fn idl_equality_swap_instructions_and_account_positions() {
        let idl = idl();
        let ixs = idl["instructions"].as_array().unwrap();
        for (name, d) in [
            (
                "swap_base_input",
                RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR,
            ),
            (
                "swap_base_output",
                RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR,
            ),
        ] {
            let i = ixs.iter().find(|i| i["name"] == name).unwrap();
            assert_eq!(idl_bytes(&i["discriminator"]), d, "{name}");
            assert_eq!(disc("global", name), d, "{name} (sha256)");
            let names: Vec<&str> = i["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                (names[1], names[3], names[10], names[11]),
                (
                    "authority",
                    "pool_state",
                    "input_token_mint",
                    "output_token_mint"
                )
            );
            assert_eq!(names.len(), 13);
        }
    }

    fn payload(base_input: bool) -> Vec<u8> {
        let mut p = RAYDIUM_CPMM_SWAP_EVENT_DISCRIMINATOR.to_vec();
        p.extend_from_slice(&[1; 32]);
        for v in [10u64, 20, 1000, 2000, 3, 4] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.push(u8::from(base_input));
        p.extend_from_slice(&[2; 32]);
        p.extend_from_slice(&[3; 32]);
        p.extend_from_slice(&5u64.to_le_bytes());
        p.extend_from_slice(&6u64.to_le_bytes());
        p.push(1);
        p
    }

    #[test]
    fn swap_event_decodes_exactly_and_rejects_deviations() {
        let VenueEventOutcome::Event(e) = classify_raydium_cpmm_event(&payload(true)) else {
            panic!("event");
        };
        assert_eq!(e.pool_id, [1; 32]);
        assert_eq!((e.input_vault_before, e.output_vault_before), (10, 20));
        assert_eq!((e.input_amount, e.output_amount), (1000, 2000));
        assert_eq!((e.input_transfer_fee, e.output_transfer_fee), (3, 4));
        assert!(e.base_input && e.creator_fee_on_input);
        assert_eq!((e.input_mint, e.output_mint), ([2; 32], [3; 32]));
        assert_eq!((e.trade_fee, e.creator_fee), (5, 6));
        let mut long = payload(true);
        long.push(0);
        assert!(matches!(
            classify_raydium_cpmm_event(&long),
            VenueEventOutcome::Malformed { .. }
        ));
        let p = payload(true);
        assert!(matches!(
            classify_raydium_cpmm_event(&p[..p.len() - 1]),
            VenueEventOutcome::Malformed { .. }
        ));
        let mut bad = payload(true);
        bad[8 + 32 + 48] = 5;
        assert!(matches!(
            classify_raydium_cpmm_event(&bad),
            VenueEventOutcome::Malformed { .. }
        ));
        let mut lp = disc("event", "LpChangeEvent").to_vec();
        lp.extend_from_slice(&[0; 4]);
        assert_eq!(
            classify_raydium_cpmm_event(&lp),
            VenueEventOutcome::KnownNonLeg {
                name: "LpChangeEvent"
            }
        );
        assert!(matches!(
            classify_raydium_cpmm_event(&[7; 8]),
            VenueEventOutcome::UnknownEvent { .. }
        ));
    }

    fn swap_ix(d: [u8; 8], len: usize, accounts: usize) -> RawSolanaInstruction {
        let mut data = d.to_vec();
        data.resize(len, 0);
        RawSolanaInstruction {
            program_id: RAYDIUM_CPMM_PROGRAM_ID_BYTES,
            accounts: (0..accounts)
                .map(|i| [u8::try_from(i).unwrap(); 32])
                .collect(),
            data,
            instruction_index: 0,
        }
    }

    #[test]
    fn swap_instructions_parse_with_idl_positions_and_verification() {
        let SwapInstructionParse::Swap(s) = parse_raydium_cpmm_swap_instruction(&swap_ix(
            RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR,
            24,
            13,
        )) else {
            panic!("base_input");
        };
        assert_eq!(
            (s.authority, s.pool_state, s.input_mint, s.output_mint),
            ([1; 32], [3; 32], [10; 32], [11; 32])
        );
        assert_eq!(
            s.variant.verification(),
            VariantVerification::FixtureVerified
        );
        assert!(s.variant.is_base_input());
        let SwapInstructionParse::Swap(o) = parse_raydium_cpmm_swap_instruction(&swap_ix(
            RAYDIUM_CPMM_SWAP_BASE_OUTPUT_DISCRIMINATOR,
            24,
            13,
        )) else {
            panic!("base_output");
        };
        assert_eq!(o.variant.verification(), VariantVerification::IdlOnly);
        assert!(!o.variant.is_base_input());
        for (len, n) in [(25, 13), (24, 12)] {
            assert!(matches!(
                parse_raydium_cpmm_swap_instruction(&swap_ix(
                    RAYDIUM_CPMM_SWAP_BASE_INPUT_DISCRIMINATOR,
                    len,
                    n
                )),
                SwapInstructionParse::Malformed { .. }
            ));
        }
        assert!(matches!(
            parse_raydium_cpmm_swap_instruction(&swap_ix([1; 8], 24, 13)),
            SwapInstructionParse::NotSwap
        ));
    }
}
