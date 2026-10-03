//! Raydium CLMM `SwapEvent` (ADR-013 section 2b, P4.9).
//!
//! Program `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`. Raydium CLMM emits
//! `SwapEvent` with Anchor `emit!`: a `Program data:` log line of the swap
//! instruction itself (NOT an event-CPI), attributed by the invoke/success
//! stack ([`crate::venue_log`]). Live: 19 of 19 swap instructions of the
//! committed fixtures carry exactly one line.
//!
//! ## Schema source and live deviation
//!
//! Official `raydium-io/raydium-idl` @ `e7e0c96fe77bcf6a020b84a44c47a722aac8e359`
//! (`raydium_clmm/raydium_clmm.json`; there is no on-chain IDL account),
//! pinned as `fixtures/raydium_clmm_idl_e7e0c96f.json`, sha256
//! `040a8c4866317fa028be8a81db54325ce6d9b92aeb10582d89992855bbbce5c1`.
//!
//! `SwapEvent` (`sha256("event:SwapEvent")[..8] = 40c6cde8260871e2`, the SAME
//! discriminator as Jupiter's and CPMM's `SwapEvent`: only the program
//! attribution tells them apart), IDL payload 197 bytes: `pool_state, sender,
//! token_account_0, token_account_1 (4 pubkeys), amount_0 u64,
//! transfer_fee_0 u64, amount_1 u64, transfer_fee_1 u64, zero_for_one bool,
//! sqrt_price_x64 u128, liquidity u128, tick i32`.
//!
//! **Live deviation:** the deployed program emits **213 bytes** in 19 of 19
//! live samples: the IDL layout followed by 16 trailing bytes (two `u64`; in
//! every sample `(x, 0)` when `zero_for_one` else `(0, x)` with `x` about 4 bps
//! of the input amount). The pinned IDL does not describe them, their meaning
//! is not asserted and they are never used. Policy: exactly 213 bytes decode
//! as `Live213` (FixtureVerified); exactly 197 bytes decode as `Idl197`
//! (IdlOnly: no live sample, never evidence); any other length is `Malformed`.
//!
//! Direction: `zero_for_one` means token 0 in (`amount_0`), token 1 out
//! (`amount_1`). The event names no mint; they come from `swap_v2`
//! (`input_vault_mint`=11, `output_vault_mint`=12) or, for the v1 `swap`, the
//! pool's own vault balance deltas ([`crate::venue_legs`]). Instruction
//! gates (both variants, 19/19 live): `pool_state` = account 2;
//! `token_account_0/1` of the event equal the instruction's
//! `input_token_account`=3 / `output_token_account`=4 in the
//! `zero_for_one` order (input is token 0) or reversed.
//! `swap_router_base_in` is recognised and NOT supported.

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::VariantVerification;
use crate::venue_wire::{
    Reader, SwapInstructionParse, VenueEventOutcome, event_name, malformed, split_event,
};

/// Raydium CLMM program id (base58).
pub const RAYDIUM_CLMM_PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
/// Raydium CLMM program id bytes.
pub const RAYDIUM_CLMM_PROGRAM_ID_BYTES: SolanaPubkey = [
    0xa5, 0xd5, 0xca, 0x9e, 0x04, 0xcf, 0x5d, 0xb5, 0x90, 0xb7, 0x14, 0xba, 0x2f, 0xe3, 0x2c, 0xb1,
    0x59, 0x13, 0x3f, 0xc1, 0xc1, 0x92, 0xb7, 0x22, 0x57, 0xfd, 0x07, 0xd3, 0x9c, 0xb0, 0x40, 0x1e,
];
/// `raydium-io/raydium-idl` commit of the pinned IDL.
pub const RAYDIUM_IDL_COMMIT: &str = "e7e0c96fe77bcf6a020b84a44c47a722aac8e359";
/// sha256 of the pinned CLMM IDL file.
pub const RAYDIUM_CLMM_IDL_SHA256: &str =
    "040a8c4866317fa028be8a81db54325ce6d9b92aeb10582d89992855bbbce5c1";
/// `SwapEvent` discriminator.
pub const RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
/// IDL payload length (after the 8-byte discriminator).
pub const RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN: usize = 197;
/// Live payload length: IDL layout + 16 unpinned trailing bytes.
pub const RAYDIUM_CLMM_SWAP_EVENT_LIVE_LEN: usize = 213;

/// `swap` instruction discriminator.
pub const RAYDIUM_CLMM_SWAP_DISCRIMINATOR: [u8; 8] =
    [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
/// `swap_v2` instruction discriminator.
pub const RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR: [u8; 8] =
    [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62];
/// `swap_router_base_in` instruction discriminator (recognised, unsupported).
pub const RAYDIUM_CLMM_SWAP_ROUTER_BASE_IN_DISCRIMINATOR: [u8; 8] =
    [0x45, 0x7d, 0x73, 0xda, 0xf5, 0xba, 0xf2, 0xc4];

/// Every event of the pinned IDL (name, discriminator). Only `SwapEvent` is a
/// swap; the rest are known non-legs.
pub const RAYDIUM_CLMM_EVENTS: [(&str, [u8; 8]); 15] = [
    (
        "CollectPersonalFeeEvent",
        [0xa6, 0xae, 0x69, 0xc0, 0x51, 0xa1, 0x53, 0x69],
    ),
    (
        "CollectProtocolFeeEvent",
        [0xce, 0x57, 0x11, 0x4f, 0x2d, 0x29, 0xd5, 0x3d],
    ),
    (
        "ConfigChangeEvent",
        [0xf7, 0xbd, 0x07, 0x77, 0x6a, 0x70, 0x5f, 0x97],
    ),
    (
        "CreatePersonalPositionEvent",
        [0x64, 0x1e, 0x57, 0xf9, 0xc4, 0xdf, 0x9a, 0xce],
    ),
    (
        "DecreaseLimitOrderEvent",
        [0x46, 0x30, 0x28, 0xdd, 0xdb, 0xed, 0xd4, 0xa3],
    ),
    (
        "DecreaseLiquidityEvent",
        [0x3a, 0xde, 0x56, 0x3a, 0x44, 0x32, 0x55, 0x38],
    ),
    (
        "IncreaseLimitOrderEvent",
        [0x0b, 0x78, 0x0d, 0xcc, 0xc7, 0x57, 0x13, 0xc8],
    ),
    (
        "IncreaseLiquidityEvent",
        [0x31, 0x4f, 0x69, 0xd4, 0x20, 0x22, 0x1e, 0x54],
    ),
    (
        "LiquidityCalculateEvent",
        [0xed, 0x70, 0x94, 0xe6, 0x39, 0x54, 0xb4, 0xa2],
    ),
    (
        "LiquidityChangeEvent",
        [0x7e, 0xf0, 0xaf, 0xce, 0x9e, 0x58, 0x99, 0x6b],
    ),
    (
        "OpenLimitOrderEvent",
        [0x6a, 0x18, 0x47, 0x55, 0x39, 0xa9, 0x9e, 0xd8],
    ),
    (
        "PoolCreatedEvent",
        [0x19, 0x5e, 0x4b, 0x2f, 0x70, 0x63, 0x35, 0x3f],
    ),
    (
        "SettleLimitOrderEvent",
        [0x58, 0x77, 0x4d, 0xa4, 0x7d, 0x7c, 0x0a, 0xc2],
    ),
    (
        "SwapEvent",
        [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2],
    ),
    (
        "UpdateRewardInfosEvent",
        [0x6d, 0x7f, 0xba, 0x4e, 0x72, 0x41, 0x25, 0xec],
    ),
];

/// Which payload shape decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClmmEventLayout {
    /// The pinned IDL layout (197 bytes): no live sample.
    Idl197,
    /// The layout every live sample has (213 bytes).
    Live213,
}

impl ClmmEventLayout {
    /// `Live213` is FixtureVerified (19 samples); `Idl197` is IdlOnly.
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        match self {
            Self::Idl197 => VariantVerification::IdlOnly,
            Self::Live213 => VariantVerification::FixtureVerified,
        }
    }
}

/// A decoded Raydium CLMM `SwapEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaydiumClmmSwapEvent {
    pub layout: ClmmEventLayout,
    pub pool_state: SolanaPubkey,
    pub sender: SolanaPubkey,
    pub token_account_0: SolanaPubkey,
    pub token_account_1: SolanaPubkey,
    pub amount_0: u64,
    pub transfer_fee_0: u64,
    pub amount_1: u64,
    pub transfer_fee_1: u64,
    pub zero_for_one: bool,
    pub sqrt_price_x64: u128,
    pub liquidity: u128,
    pub tick: i32,
    /// The 16 unpinned trailing bytes of `Live213` as two `u64` (never used).
    pub unpinned_tail: Option<[u64; 2]>,
}

impl RaydiumClmmSwapEvent {
    /// `(input_amount, output_amount)` per `zero_for_one`.
    #[must_use]
    pub const fn in_out_amounts(&self) -> (u64, u64) {
        if self.zero_for_one {
            (self.amount_0, self.amount_1)
        } else {
            (self.amount_1, self.amount_0)
        }
    }
}

/// Classifies one decoded `Program data:` payload attributed to the Raydium
/// CLMM program.
#[must_use]
pub fn classify_raydium_clmm_event(payload: &[u8]) -> VenueEventOutcome<RaydiumClmmSwapEvent> {
    let Some((disc, body)) = split_event(payload) else {
        return malformed(format!(
            "CLMM event payload has {} bytes, fewer than the 8-byte discriminator",
            payload.len()
        ));
    };
    let Some(name) = event_name(&RAYDIUM_CLMM_EVENTS, &disc) else {
        return VenueEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    };
    if disc != RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR {
        return VenueEventOutcome::KnownNonLeg { name };
    }
    let layout = match body.len() {
        RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN => ClmmEventLayout::Idl197,
        RAYDIUM_CLMM_SWAP_EVENT_LIVE_LEN => ClmmEventLayout::Live213,
        n => {
            return malformed(format!(
                "SwapEvent payload has {n} bytes, expected exactly {RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN} (IDL) or {RAYDIUM_CLMM_SWAP_EVENT_LIVE_LEN} (live)"
            ));
        }
    };
    let mut r = Reader::new(body);
    let parsed = (|| {
        let pool_state = r.pubkey()?;
        let sender = r.pubkey()?;
        let token_account_0 = r.pubkey()?;
        let token_account_1 = r.pubkey()?;
        let amount_0 = r.u64()?;
        let transfer_fee_0 = r.u64()?;
        let amount_1 = r.u64()?;
        let transfer_fee_1 = r.u64()?;
        let zero_for_one = r.bool()?;
        let sqrt_price_x64 = r.u128()?;
        let liquidity = r.u128()?;
        let tick = r.i32()?;
        let unpinned_tail = match layout {
            ClmmEventLayout::Idl197 => None,
            ClmmEventLayout::Live213 => Some([r.u64()?, r.u64()?]),
        };
        Some(RaydiumClmmSwapEvent {
            layout,
            pool_state,
            sender,
            token_account_0,
            token_account_1,
            amount_0,
            transfer_fee_0,
            amount_1,
            transfer_fee_1,
            zero_for_one,
            sqrt_price_x64,
            liquidity,
            tick,
            unpinned_tail,
        })
    })();
    parsed.map_or_else(
        || malformed("SwapEvent fields do not fit the payload or a bool is not 0/1".to_owned()),
        VenueEventOutcome::Event,
    )
}

/// Supported swap instruction variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaydiumClmmSwapVariant {
    Swap,
    SwapV2,
}

impl RaydiumClmmSwapVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Swap => "swap",
            Self::SwapV2 => "swap_v2",
        }
    }

    /// Both variants have live samples (7 `swap`, 12 `swap_v2`).
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        VariantVerification::FixtureVerified
    }
}

/// A parsed Raydium CLMM swap instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaydiumClmmSwapIx {
    pub variant: RaydiumClmmSwapVariant,
    pub pool_state: SolanaPubkey,
    pub input_token_account: SolanaPubkey,
    pub output_token_account: SolanaPubkey,
    /// `(input_vault_mint, output_vault_mint)`; `None` for the v1 `swap`.
    pub mints_in_out: Option<(SolanaPubkey, SolanaPubkey)>,
}

/// Parses one instruction ASSUMED to belong to Raydium CLMM.
#[must_use]
pub fn parse_raydium_clmm_swap_instruction(
    ix: &RawSolanaInstruction,
) -> SwapInstructionParse<RaydiumClmmSwapIx> {
    let Some(disc) = ix.data.get(..8) else {
        return SwapInstructionParse::NotSwap;
    };
    let (variant, min_accounts) = if disc == RAYDIUM_CLMM_SWAP_DISCRIMINATOR {
        (RaydiumClmmSwapVariant::Swap, 5usize)
    } else if disc == RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR {
        (RaydiumClmmSwapVariant::SwapV2, 13)
    } else if disc == RAYDIUM_CLMM_SWAP_ROUTER_BASE_IN_DISCRIMINATOR {
        return SwapInstructionParse::Unsupported {
            name: "swap_router_base_in",
        };
    } else {
        return SwapInstructionParse::NotSwap;
    };
    // amount u64, other_amount_threshold u64, sqrt_price_limit_x64 u128, is_base_input bool.
    if ix.data.len() != 41 || ix.accounts.len() < min_accounts {
        return SwapInstructionParse::Malformed {
            reason: format!(
                "{} has {} data bytes / {} accounts (need exactly 41 bytes, >= {min_accounts} accounts)",
                variant.name(),
                ix.data.len(),
                ix.accounts.len()
            ),
        };
    }
    let acct = |i: usize| ix.accounts.get(i).copied();
    let (Some(pool_state), Some(input_token_account), Some(output_token_account)) =
        (acct(2), acct(3), acct(4))
    else {
        return SwapInstructionParse::Malformed {
            reason: "pool / token accounts missing".to_owned(),
        };
    };
    let mints_in_out = match variant {
        RaydiumClmmSwapVariant::Swap => None,
        RaydiumClmmSwapVariant::SwapV2 => match (acct(11), acct(12)) {
            (Some(i), Some(o)) => Some((i, o)),
            _ => {
                return SwapInstructionParse::Malformed {
                    reason: "vault mint accounts missing".to_owned(),
                };
            }
        },
    };
    SwapInstructionParse::Swap(RaydiumClmmSwapIx {
        variant,
        pool_state,
        input_token_account,
        output_token_account,
        mints_in_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const IDL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/raydium_clmm_idl_e7e0c96f.json"
    );

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn idl() -> serde_json::Value {
        let bytes = std::fs::read(IDL_PATH).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), RAYDIUM_CLMM_IDL_SHA256);
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
        assert_eq!(idl["address"], RAYDIUM_CLMM_PROGRAM_ID);
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), RAYDIUM_CLMM_EVENTS.len());
        for (name, d) in RAYDIUM_CLMM_EVENTS {
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
                p("pool_state", "pubkey"),
                p("sender", "pubkey"),
                p("token_account_0", "pubkey"),
                p("token_account_1", "pubkey"),
                p("amount_0", "u64"),
                p("transfer_fee_0", "u64"),
                p("amount_1", "u64"),
                p("transfer_fee_1", "u64"),
                p("zero_for_one", "bool"),
                p("sqrt_price_x64", "u128"),
                p("liquidity", "u128"),
                p("tick", "i32"),
            ]
        );
        assert_eq!(
            4 * 32 + 4 * 8 + 1 + 16 + 16 + 4,
            RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN
        );
        assert_eq!(
            RAYDIUM_CLMM_SWAP_EVENT_IDL_LEN + 16,
            RAYDIUM_CLMM_SWAP_EVENT_LIVE_LEN
        );
        assert_eq!(
            disc("event", "SwapEvent"),
            RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR
        );
    }

    #[test]
    fn idl_equality_swap_instructions_and_account_positions() {
        let idl = idl();
        let ixs = idl["instructions"].as_array().unwrap();
        let by = |n: &str| ixs.iter().find(|i| i["name"] == n).unwrap();
        for (name, d) in [
            ("swap", RAYDIUM_CLMM_SWAP_DISCRIMINATOR),
            ("swap_v2", RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR),
            (
                "swap_router_base_in",
                RAYDIUM_CLMM_SWAP_ROUTER_BASE_IN_DISCRIMINATOR,
            ),
        ] {
            assert_eq!(idl_bytes(&by(name)["discriminator"]), d, "{name}");
            assert_eq!(disc("global", name), d, "{name} (sha256)");
        }
        for name in ["swap", "swap_v2"] {
            let names: Vec<&str> = by(name)["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                &names[2..5],
                ["pool_state", "input_token_account", "output_token_account"]
            );
        }
        let v2: Vec<&str> = by("swap_v2")["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!((v2[11], v2[12]), ("input_vault_mint", "output_vault_mint"));
        assert_eq!(v2.len(), 13);
    }

    fn payload(len_tail: usize, zero_for_one: bool) -> Vec<u8> {
        let mut p = RAYDIUM_CLMM_SWAP_EVENT_DISCRIMINATOR.to_vec();
        for k in 1u8..=4 {
            p.extend_from_slice(&[k; 32]);
        }
        for v in [1000u64, 2, 3000, 4] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.push(u8::from(zero_for_one));
        p.extend_from_slice(&5u128.to_le_bytes());
        p.extend_from_slice(&6u128.to_le_bytes());
        p.extend_from_slice(&(-7i32).to_le_bytes());
        p.extend(std::iter::repeat_n(0xAAu8, len_tail));
        p
    }

    #[test]
    fn both_layouts_decode_and_carry_their_verification() {
        let VenueEventOutcome::Event(live) = classify_raydium_clmm_event(&payload(16, true)) else {
            panic!("live");
        };
        assert_eq!(live.layout, ClmmEventLayout::Live213);
        assert_eq!(
            live.layout.verification(),
            VariantVerification::FixtureVerified
        );
        assert_eq!(live.in_out_amounts(), (1000, 3000));
        assert_eq!(live.unpinned_tail, Some([u64::from_le_bytes([0xAA; 8]); 2]));
        assert_eq!((live.pool_state, live.sender), ([1; 32], [2; 32]));
        assert_eq!(
            (live.token_account_0, live.token_account_1),
            ([3; 32], [4; 32])
        );
        assert_eq!((live.sqrt_price_x64, live.liquidity, live.tick), (5, 6, -7));
        let VenueEventOutcome::Event(idl) = classify_raydium_clmm_event(&payload(0, false)) else {
            panic!("idl");
        };
        assert_eq!(idl.layout, ClmmEventLayout::Idl197);
        assert_eq!(idl.layout.verification(), VariantVerification::IdlOnly);
        assert_eq!(idl.in_out_amounts(), (3000, 1000));
        assert_eq!(idl.unpinned_tail, None);
    }

    #[test]
    fn every_other_length_bool_and_discriminator_is_rejected() {
        for tail in [1usize, 8, 15, 17, 32] {
            assert!(matches!(
                classify_raydium_clmm_event(&payload(tail, true)),
                VenueEventOutcome::Malformed { .. }
            ));
        }
        let mut bad = payload(16, true);
        bad[8 + 4 * 32 + 4 * 8] = 7;
        assert!(matches!(
            classify_raydium_clmm_event(&bad),
            VenueEventOutcome::Malformed { .. }
        ));
        let mut p = disc("event", "PoolCreatedEvent").to_vec();
        p.extend_from_slice(&[0; 3]);
        assert_eq!(
            classify_raydium_clmm_event(&p),
            VenueEventOutcome::KnownNonLeg {
                name: "PoolCreatedEvent"
            }
        );
        assert!(matches!(
            classify_raydium_clmm_event(&[9; 8]),
            VenueEventOutcome::UnknownEvent { .. }
        ));
    }

    fn swap_ix(d: [u8; 8], len: usize, accounts: usize) -> RawSolanaInstruction {
        let mut data = d.to_vec();
        data.resize(len, 0);
        RawSolanaInstruction {
            program_id: RAYDIUM_CLMM_PROGRAM_ID_BYTES,
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
            parse_raydium_clmm_swap_instruction(&swap_ix(RAYDIUM_CLMM_SWAP_DISCRIMINATOR, 41, 13))
        else {
            panic!("v1");
        };
        assert_eq!(
            (
                v1.pool_state,
                v1.input_token_account,
                v1.output_token_account,
                v1.mints_in_out
            ),
            ([2; 32], [3; 32], [4; 32], None)
        );
        let SwapInstructionParse::Swap(v2) = parse_raydium_clmm_swap_instruction(&swap_ix(
            RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR,
            41,
            16,
        )) else {
            panic!("v2");
        };
        assert_eq!(v2.mints_in_out, Some(([11; 32], [12; 32])));
        for (d, len, n) in [
            (RAYDIUM_CLMM_SWAP_DISCRIMINATOR, 42, 13),
            (RAYDIUM_CLMM_SWAP_DISCRIMINATOR, 41, 4),
            (RAYDIUM_CLMM_SWAP_V2_DISCRIMINATOR, 41, 12),
        ] {
            assert!(matches!(
                parse_raydium_clmm_swap_instruction(&swap_ix(d, len, n)),
                SwapInstructionParse::Malformed { .. }
            ));
        }
        assert!(matches!(
            parse_raydium_clmm_swap_instruction(&swap_ix(
                RAYDIUM_CLMM_SWAP_ROUTER_BASE_IN_DISCRIMINATOR,
                24,
                8
            )),
            SwapInstructionParse::Unsupported {
                name: "swap_router_base_in"
            }
        ));
        assert!(matches!(
            parse_raydium_clmm_swap_instruction(&swap_ix([1; 8], 41, 13)),
            SwapInstructionParse::NotSwap
        ));
    }
}
