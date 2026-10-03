//! Orca Whirlpool `Traded` swap event (ADR-013 section 2b, P4.9).
//!
//! Program `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`. The Whirlpool
//! program emits `Traded` with Anchor `emit!`: a `Program data:` log line of
//! the swap instruction itself (NOT an event-CPI), attributed to the program
//! by the invoke/success stack ([`crate::venue_log`]). Live: 20 of 20 swap
//! instructions of the committed fixtures carry exactly one `Traded` line.
//!
//! ## Schema source
//!
//! On-chain Anchor IDL account `2KFqE4RWoPVbvodo8vbggCFeHPS8TDvgpwp79ALMrcyn`
//! (whirlpool 0.9.0), zlib-decompressed 2026-10-03, pinned as
//! `fixtures/orca_whirlpool_onchain_idl_2026-10-03.json`, sha256
//! `7afddfe8766bd24d30ff9ee5b12c7ee89bff5bc3b7b0396620e021a97a7ef63f`.
//!
//! `Traded` (`sha256("event:Traded")[..8] = e1ca49af932ba096`), payload 113
//! bytes: `whirlpool pubkey, a_to_b bool, pre_sqrt_price u128, post_sqrt_price
//! u128, input_amount u64, output_amount u64, input_transfer_fee u64,
//! output_transfer_fee u64, lp_fee u64, protocol_fee u64`. Exact length;
//! bools strictly 0/1.
//!
//! The event names the pool and the direction, never the mints. Mints come
//! from the swap instruction: `swap_v2` names `token_mint_a/b`; the v1 `swap`
//! does not (resolved from the pool's own vault balance deltas, see
//! [`crate::venue_legs`]).
//!
//! Instruction accounts read (IDL names): `swap`: `whirlpool`=2; `swap_v2`:
//! `whirlpool`=4, `token_mint_a`=5, `token_mint_b`=6. Argument `a_to_b` is the
//! byte at data offset 41 of both. `two_hop_swap`/`two_hop_swap_v2` are
//! recognised and NOT supported (no live sample, two pools): their events are
//! never legs.

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::VariantVerification;
use crate::venue_wire::{
    Reader, SwapInstructionParse, VenueEventOutcome, event_name, malformed, split_event,
};

/// Orca Whirlpool program id (base58).
pub const WHIRLPOOL_PROGRAM_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
/// Orca Whirlpool program id bytes.
pub const WHIRLPOOL_PROGRAM_ID_BYTES: SolanaPubkey = [
    0x0e, 0x03, 0x68, 0x5f, 0x8e, 0x90, 0x90, 0x53, 0xe4, 0x58, 0x12, 0x1c, 0x66, 0xf5, 0xa7, 0x6a,
    0xed, 0xc7, 0x70, 0x6a, 0xa1, 0x1c, 0x82, 0xf8, 0xaa, 0x95, 0x2a, 0x8f, 0x2b, 0x78, 0x79, 0xa9,
];
/// sha256 of the pinned on-chain IDL file.
pub const WHIRLPOOL_IDL_SHA256: &str =
    "7afddfe8766bd24d30ff9ee5b12c7ee89bff5bc3b7b0396620e021a97a7ef63f";
/// `Traded` event discriminator.
pub const WHIRLPOOL_TRADED_DISCRIMINATOR: [u8; 8] =
    [0xe1, 0xca, 0x49, 0xaf, 0x93, 0x2b, 0xa0, 0x96];
/// `Traded` payload length (after the 8-byte discriminator).
pub const WHIRLPOOL_TRADED_LEN: usize = 113;

/// `swap` instruction discriminator.
pub const WHIRLPOOL_SWAP_DISCRIMINATOR: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
/// `swap_v2` instruction discriminator.
pub const WHIRLPOOL_SWAP_V2_DISCRIMINATOR: [u8; 8] =
    [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];
/// `two_hop_swap` instruction discriminator (recognised, unsupported).
pub const WHIRLPOOL_TWO_HOP_SWAP_DISCRIMINATOR: [u8; 8] =
    [0xc3, 0x60, 0xed, 0x6c, 0x44, 0xa2, 0xdb, 0xe6];
/// `two_hop_swap_v2` instruction discriminator (recognised, unsupported).
pub const WHIRLPOOL_TWO_HOP_SWAP_V2_DISCRIMINATOR: [u8; 8] =
    [0xba, 0x8f, 0xd1, 0x1d, 0xfe, 0x02, 0xc2, 0x75];

/// Every event of the pinned IDL (name, discriminator). Only `Traded` is a
/// swap; the rest are known non-legs.
pub const WHIRLPOOL_EVENTS: [(&str, [u8; 8]); 6] = [
    (
        "LiquidityDecreased",
        [0xa6, 0x01, 0x24, 0x47, 0x70, 0xca, 0xb5, 0xab],
    ),
    (
        "LiquidityIncreased",
        [0x1e, 0x07, 0x90, 0xb5, 0x66, 0xfe, 0x9b, 0xa1],
    ),
    (
        "LiquidityRepositioned",
        [0x5f, 0x82, 0xb5, 0x84, 0xfb, 0x32, 0xc3, 0x26],
    ),
    (
        "PoolInitialized",
        [0x64, 0x76, 0xad, 0x57, 0x0c, 0xc6, 0xfe, 0xe5],
    ),
    (
        "PositionOpened",
        [0xed, 0xaf, 0xf3, 0xe6, 0x93, 0x75, 0x65, 0x79],
    ),
    ("Traded", [0xe1, 0xca, 0x49, 0xaf, 0x93, 0x2b, 0xa0, 0x96]),
];

/// A decoded `Traded` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhirlpoolTraded {
    pub whirlpool: SolanaPubkey,
    pub a_to_b: bool,
    pub pre_sqrt_price: u128,
    pub post_sqrt_price: u128,
    pub input_amount: u64,
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub lp_fee: u64,
    pub protocol_fee: u64,
}

/// Evidence level of `Traded` (ADR-009): verified by the live fixtures
/// reconciled in `scout-engine/tests/venue_swap_legs.rs`.
#[must_use]
pub const fn whirlpool_traded_verification() -> VariantVerification {
    VariantVerification::FixtureVerified
}

/// Classifies one decoded `Program data:` payload attributed to the
/// Whirlpool program.
#[must_use]
pub fn classify_whirlpool_event(payload: &[u8]) -> VenueEventOutcome<WhirlpoolTraded> {
    let Some((disc, body)) = split_event(payload) else {
        return malformed(format!(
            "Whirlpool event payload has {} bytes, fewer than the 8-byte discriminator",
            payload.len()
        ));
    };
    let Some(name) = event_name(&WHIRLPOOL_EVENTS, &disc) else {
        return VenueEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    };
    if disc != WHIRLPOOL_TRADED_DISCRIMINATOR {
        return VenueEventOutcome::KnownNonLeg { name };
    }
    if body.len() != WHIRLPOOL_TRADED_LEN {
        return malformed(format!(
            "Traded payload has {} bytes, expected exactly {WHIRLPOOL_TRADED_LEN}",
            body.len()
        ));
    }
    let mut r = Reader::new(body);
    let parsed = (|| {
        Some(WhirlpoolTraded {
            whirlpool: r.pubkey()?,
            a_to_b: r.bool()?,
            pre_sqrt_price: r.u128()?,
            post_sqrt_price: r.u128()?,
            input_amount: r.u64()?,
            output_amount: r.u64()?,
            input_transfer_fee: r.u64()?,
            output_transfer_fee: r.u64()?,
            lp_fee: r.u64()?,
            protocol_fee: r.u64()?,
        })
    })();
    parsed.map_or_else(
        || malformed("Traded fields do not fit the payload or a bool is not 0/1".to_owned()),
        VenueEventOutcome::Event,
    )
}

/// Supported swap instruction variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhirlpoolSwapVariant {
    Swap,
    SwapV2,
}

impl WhirlpoolSwapVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Swap => "swap",
            Self::SwapV2 => "swap_v2",
        }
    }

    /// Both variants have live fixture samples (6 `swap`, 14 `swap_v2`).
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        VariantVerification::FixtureVerified
    }
}

/// A parsed Whirlpool swap instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhirlpoolSwapIx {
    pub variant: WhirlpoolSwapVariant,
    pub whirlpool: SolanaPubkey,
    /// `(token_mint_a, token_mint_b)`; `None` for the v1 `swap`.
    pub mints_ab: Option<(SolanaPubkey, SolanaPubkey)>,
    pub a_to_b: bool,
}

/// Byte offset of the `a_to_b` argument in `swap` / `swap_v2` data.
const A_TO_B_OFFSET: usize = 41;

/// Parses one instruction ASSUMED to belong to the Whirlpool program.
#[must_use]
pub fn parse_whirlpool_swap_instruction(
    ix: &RawSolanaInstruction,
) -> SwapInstructionParse<WhirlpoolSwapIx> {
    let Some(disc) = ix.data.get(..8) else {
        return SwapInstructionParse::NotSwap;
    };
    let (variant, whirlpool_idx, min_accounts, min_len, exact_len) =
        if disc == WHIRLPOOL_SWAP_DISCRIMINATOR {
            (WhirlpoolSwapVariant::Swap, 2usize, 11usize, 42usize, true)
        } else if disc == WHIRLPOOL_SWAP_V2_DISCRIMINATOR {
            (WhirlpoolSwapVariant::SwapV2, 4, 15, 43, false)
        } else if disc == WHIRLPOOL_TWO_HOP_SWAP_DISCRIMINATOR {
            return SwapInstructionParse::Unsupported {
                name: "two_hop_swap",
            };
        } else if disc == WHIRLPOOL_TWO_HOP_SWAP_V2_DISCRIMINATOR {
            return SwapInstructionParse::Unsupported {
                name: "two_hop_swap_v2",
            };
        } else {
            return SwapInstructionParse::NotSwap;
        };
    if ix.accounts.len() < min_accounts
        || ix.data.len() < min_len
        || (exact_len && ix.data.len() != min_len)
    {
        return SwapInstructionParse::Malformed {
            reason: format!(
                "{} has {} data bytes / {} accounts (need {}{} bytes, >= {min_accounts} accounts)",
                variant.name(),
                ix.data.len(),
                ix.accounts.len(),
                if exact_len { "exactly " } else { ">= " },
                min_len
            ),
        };
    }
    let a_to_b = match ix.data.get(A_TO_B_OFFSET) {
        Some(0) => false,
        Some(1) => true,
        _ => {
            return SwapInstructionParse::Malformed {
                reason: "a_to_b argument is not 0/1".to_owned(),
            };
        }
    };
    let acct = |i: usize| ix.accounts.get(i).copied();
    let Some(whirlpool) = acct(whirlpool_idx) else {
        return SwapInstructionParse::Malformed {
            reason: "whirlpool account missing".to_owned(),
        };
    };
    let mints_ab = match variant {
        WhirlpoolSwapVariant::Swap => None,
        WhirlpoolSwapVariant::SwapV2 => match (acct(5), acct(6)) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => {
                return SwapInstructionParse::Malformed {
                    reason: "token mint accounts missing".to_owned(),
                };
            }
        },
    };
    SwapInstructionParse::Swap(WhirlpoolSwapIx {
        variant,
        whirlpool,
        mints_ab,
        a_to_b,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const IDL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/orca_whirlpool_onchain_idl_2026-10-03.json"
    );

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn idl() -> serde_json::Value {
        let bytes = std::fs::read(IDL_PATH).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), WHIRLPOOL_IDL_SHA256);
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

    fn traded_payload() -> Vec<u8> {
        let mut p = WHIRLPOOL_TRADED_DISCRIMINATOR.to_vec();
        p.extend_from_slice(&[7; 32]);
        p.push(1);
        p.extend_from_slice(&11u128.to_le_bytes());
        p.extend_from_slice(&12u128.to_le_bytes());
        for v in [100u64, 200, 3, 4, 5, 6] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p
    }

    #[test]
    fn idl_equality_events_discriminators_and_traded_layout() {
        let idl = idl();
        assert_eq!(idl["address"], WHIRLPOOL_PROGRAM_ID);
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), WHIRLPOOL_EVENTS.len());
        for (name, d) in WHIRLPOOL_EVENTS {
            let e = events.iter().find(|e| e["name"] == name).unwrap();
            assert_eq!(idl_bytes(&e["discriminator"]), d, "{name} (IDL)");
            assert_eq!(disc("event", name), d, "{name} (sha256)");
        }
        assert_eq!(disc("event", "Traded"), WHIRLPOOL_TRADED_DISCRIMINATOR);
        let ty = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "Traded")
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
                p("whirlpool", "pubkey"),
                p("a_to_b", "bool"),
                p("pre_sqrt_price", "u128"),
                p("post_sqrt_price", "u128"),
                p("input_amount", "u64"),
                p("output_amount", "u64"),
                p("input_transfer_fee", "u64"),
                p("output_transfer_fee", "u64"),
                p("lp_fee", "u64"),
                p("protocol_fee", "u64"),
            ]
        );
        assert_eq!(32 + 1 + 16 + 16 + 6 * 8, WHIRLPOOL_TRADED_LEN);
    }

    #[test]
    fn idl_equality_swap_instructions() {
        let idl = idl();
        let ixs = idl["instructions"].as_array().unwrap();
        let by = |n: &str| ixs.iter().find(|i| i["name"] == n).unwrap();
        for (name, d) in [
            ("swap", WHIRLPOOL_SWAP_DISCRIMINATOR),
            ("swap_v2", WHIRLPOOL_SWAP_V2_DISCRIMINATOR),
            ("two_hop_swap", WHIRLPOOL_TWO_HOP_SWAP_DISCRIMINATOR),
            ("two_hop_swap_v2", WHIRLPOOL_TWO_HOP_SWAP_V2_DISCRIMINATOR),
        ] {
            assert_eq!(idl_bytes(&by(name)["discriminator"]), d, "{name}");
            assert_eq!(disc("global", name), d, "{name} (sha256)");
        }
        let names = |n: &str| -> Vec<String> {
            by(n)["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["name"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names("swap")[2], "whirlpool");
        let v2 = names("swap_v2");
        assert_eq!(
            (v2[4].as_str(), v2[5].as_str(), v2[6].as_str()),
            ("whirlpool", "token_mint_a", "token_mint_b")
        );
        // a_to_b is the byte after amount, threshold, sqrt_price_limit,
        // amount_specified_is_input.
        let args: Vec<String> = by("swap")["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            args,
            [
                "amount",
                "other_amount_threshold",
                "sqrt_price_limit",
                "amount_specified_is_input",
                "a_to_b"
            ]
        );
        assert_eq!(8 + 8 + 8 + 16 + 1, A_TO_B_OFFSET);
    }

    #[test]
    fn traded_decodes_exactly_and_rejects_every_deviation() {
        let p = traded_payload();
        let VenueEventOutcome::Event(t) = classify_whirlpool_event(&p) else {
            panic!("not decoded");
        };
        assert_eq!(t.whirlpool, [7; 32]);
        assert!(t.a_to_b);
        assert_eq!((t.pre_sqrt_price, t.post_sqrt_price), (11, 12));
        assert_eq!((t.input_amount, t.output_amount), (100, 200));
        assert_eq!(
            (
                t.input_transfer_fee,
                t.output_transfer_fee,
                t.lp_fee,
                t.protocol_fee
            ),
            (3, 4, 5, 6)
        );
        let mut long = p.clone();
        long.push(0);
        assert!(matches!(
            classify_whirlpool_event(&long),
            VenueEventOutcome::Malformed { .. }
        ));
        assert!(matches!(
            classify_whirlpool_event(&p[..p.len() - 1]),
            VenueEventOutcome::Malformed { .. }
        ));
        let mut bad_bool = p.clone();
        bad_bool[8 + 32] = 2;
        assert!(matches!(
            classify_whirlpool_event(&bad_bool),
            VenueEventOutcome::Malformed { .. }
        ));
        assert!(matches!(
            classify_whirlpool_event(&[1, 2, 3]),
            VenueEventOutcome::Malformed { .. }
        ));
    }

    #[test]
    fn other_idl_events_are_named_non_legs_and_foreign_discriminators_unknown() {
        let mut p = disc("event", "LiquidityIncreased").to_vec();
        p.extend_from_slice(&[0; 5]);
        assert_eq!(
            classify_whirlpool_event(&p),
            VenueEventOutcome::KnownNonLeg {
                name: "LiquidityIncreased"
            }
        );
        // The Raydium/Jupiter `SwapEvent` discriminator is NOT a Whirlpool event.
        let foreign = disc("event", "SwapEvent");
        assert_eq!(
            classify_whirlpool_event(&foreign),
            VenueEventOutcome::UnknownEvent {
                discriminator: foreign
            }
        );
    }

    fn swap_ix(
        variant_disc: [u8; 8],
        len: usize,
        accounts: usize,
        a_to_b: u8,
    ) -> RawSolanaInstruction {
        let mut data = variant_disc.to_vec();
        data.resize(len, 0);
        data[A_TO_B_OFFSET] = a_to_b;
        RawSolanaInstruction {
            program_id: WHIRLPOOL_PROGRAM_ID_BYTES,
            accounts: (0..accounts)
                .map(|i| [u8::try_from(i).unwrap(); 32])
                .collect(),
            data,
            instruction_index: 0,
        }
    }

    #[test]
    fn swap_instructions_parse_with_idl_positions() {
        let SwapInstructionParse::Swap(v1) =
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_DISCRIMINATOR, 42, 11, 1))
        else {
            panic!("v1");
        };
        assert_eq!(
            (v1.whirlpool, v1.mints_ab, v1.a_to_b),
            ([2; 32], None, true)
        );
        let SwapInstructionParse::Swap(v2) =
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_V2_DISCRIMINATOR, 43, 15, 0))
        else {
            panic!("v2");
        };
        assert_eq!(v2.whirlpool, [4; 32]);
        assert_eq!(v2.mints_ab, Some(([5; 32], [6; 32])));
        assert!(!v2.a_to_b);
        // v1 is exactly 42 bytes; v2 may carry remaining-accounts info.
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_DISCRIMINATOR, 43, 11, 0)),
            SwapInstructionParse::Malformed { .. }
        ));
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_V2_DISCRIMINATOR, 60, 15, 0)),
            SwapInstructionParse::Swap(_)
        ));
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_V2_DISCRIMINATOR, 43, 14, 0)),
            SwapInstructionParse::Malformed { .. }
        ));
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix(WHIRLPOOL_SWAP_DISCRIMINATOR, 42, 11, 2)),
            SwapInstructionParse::Malformed { .. }
        ));
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix(
                WHIRLPOOL_TWO_HOP_SWAP_DISCRIMINATOR,
                50,
                21,
                0
            )),
            SwapInstructionParse::Unsupported {
                name: "two_hop_swap"
            }
        ));
        assert!(matches!(
            parse_whirlpool_swap_instruction(&swap_ix([9; 8], 50, 21, 0)),
            SwapInstructionParse::NotSwap
        ));
    }
}
