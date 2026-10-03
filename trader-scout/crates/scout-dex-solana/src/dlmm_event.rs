//! Meteora DLMM `Swap` and `Swap2Evt` swap events (ADR-013 section 2b, P4.9).
//!
//! Program `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`. DLMM emits events
//! with Anchor `emit_cpi!`: an inner instruction to the DLMM program itself,
//! `EVENT_CPI_DISCRIMINATOR (8) ++ event discriminator (8) ++ Borsh event`,
//! with the single event-authority account `D1ZN9Wj1...` (226 of 226 live
//! event-CPIs). Each swap instruction (`swap`, `swap2`) emits BOTH a `Swap`
//! and a `Swap2Evt` (111 + 111 events for 111 swap instructions live).
//!
//! ## Schema source
//!
//! On-chain Anchor IDL account `7UZRobkzaKVm1RbCH5WdFaYCGzCRjnu3prziHAsYiSyr`
//! (lb_clmm 0.12.0), zlib-decompressed 2026-10-03, pinned as
//! `fixtures/meteora_dlmm_onchain_idl_2026-10-03.json`, sha256
//! `57ee0b91fb1505f9af4be8d073ecdea65adc395bae49c96a707a263b257eca84`.
//!
//! - `Swap` (`516ce3becdd00ac4`), 129 bytes: `lb_pair, from, start_bin_id
//!   i32, end_bin_id i32, amount_in u64, amount_out u64, swap_for_y bool, fee
//!   u64, protocol_fee u64, fee_bps u128, host_fee u64`.
//! - `Swap2Evt` (`2e7452d7941b544d`), 147 bytes: `lb_pair, from, start_bin_id
//!   i32, end_bin_id i32, swap_for_y bool, fee_bps u128, amount_in u64,
//!   amount_left u64, amount_out u64, mm_fee u64, protocol_fee u64,
//!   limit_order_fee u64, host_fee u64, fees_on_input bool, fees_on_token_x
//!   bool`.
//!
//! Exact lengths; bools strictly 0/1. The events name the pool and the
//! direction (`swap_for_y`: X in, Y out), never the mints: they come from the
//! parent swap instruction (`token_x_mint`=6, `token_y_mint`=7 in all six
//! swap instructions of the IDL).
//!
//! Verification (ADR-009): `swap` and `swap2` instructions have live samples
//! (9 and 102); `swap_exact_out*`, `swap_with_price_impact*` are in the IDL
//! only, so their events are IdlOnly (decoded, counted, never evidence).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, VariantVerification};
use crate::venue_wire::{Reader, SwapInstructionParse, event_name};

/// Meteora DLMM program id (base58).
pub const DLMM_PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
/// Meteora DLMM program id bytes.
pub const DLMM_PROGRAM_ID_BYTES: SolanaPubkey = [
    0x04, 0xe9, 0xe1, 0x2f, 0xbc, 0x84, 0xe8, 0x26, 0xc9, 0x32, 0xcc, 0xe9, 0xe2, 0x64, 0x0c, 0xce,
    0x15, 0x59, 0x0c, 0x1c, 0x62, 0x73, 0xb0, 0x92, 0x57, 0x08, 0xba, 0x3b, 0x85, 0x20, 0xb0, 0xbc,
];
/// Event-authority account every live DLMM event-CPI passed (`D1ZN9Wj1...`).
pub const DLMM_EVENT_AUTHORITY_BYTES: SolanaPubkey = [
    0xb2, 0x70, 0xd6, 0x7f, 0xa9, 0x8c, 0x51, 0xcf, 0x02, 0x13, 0x05, 0x13, 0x58, 0x96, 0x2b, 0xaf,
    0x35, 0x74, 0x2b, 0xed, 0x59, 0xc9, 0xd9, 0x44, 0x5e, 0x9c, 0x0d, 0x0c, 0x85, 0xc7, 0xcd, 0x91,
];
/// sha256 of the pinned on-chain IDL file.
pub const DLMM_IDL_SHA256: &str =
    "57ee0b91fb1505f9af4be8d073ecdea65adc395bae49c96a707a263b257eca84";
/// `Swap` event discriminator.
pub const DLMM_SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [0x51, 0x6c, 0xe3, 0xbe, 0xcd, 0xd0, 0x0a, 0xc4];
/// `Swap2Evt` event discriminator.
pub const DLMM_SWAP2_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x2e, 0x74, 0x52, 0xd7, 0x94, 0x1b, 0x54, 0x4d];
/// Event-CPI header: tag + event discriminator.
pub const DLMM_EVENT_HEADER_LEN: usize = 16;
/// `Swap` payload length.
pub const DLMM_SWAP_EVENT_LEN: usize = 129;
/// `Swap2Evt` payload length.
pub const DLMM_SWAP2_EVENT_LEN: usize = 147;

/// Every event of the pinned IDL (name, discriminator). Only `Swap` and
/// `Swap2Evt` are swaps; the rest are known non-legs.
pub const DLMM_EVENTS: [(&str, [u8; 8]); 30] = [
    (
        "AddLiquidity",
        [0x1f, 0x5e, 0x7d, 0x5a, 0xe3, 0x34, 0x3d, 0xba],
    ),
    (
        "CancelLimitOrderEvt",
        [0x83, 0xea, 0xc2, 0x85, 0x09, 0x0e, 0xbd, 0xd1],
    ),
    ("ClaimFee", [0x4b, 0x7a, 0x9a, 0x30, 0x8c, 0x4a, 0x7b, 0xa3]),
    (
        "ClaimFee2",
        [0xe8, 0xab, 0xf2, 0x61, 0x3a, 0x4d, 0x23, 0x2d],
    ),
    (
        "ClaimReward",
        [0x94, 0x74, 0x86, 0xcc, 0x16, 0xab, 0x55, 0x5f],
    ),
    (
        "ClaimReward2",
        [0x1b, 0x8f, 0xf4, 0x21, 0x50, 0x2b, 0x6e, 0x92],
    ),
    (
        "CloseLimitOrderEvt",
        [0x8e, 0x87, 0x08, 0x4c, 0x5c, 0x3f, 0x76, 0x53],
    ),
    (
        "CompositionFee",
        [0x80, 0x97, 0x7b, 0x6a, 0x11, 0x66, 0x71, 0x8e],
    ),
    (
        "DecreasePositionLength",
        [0x34, 0x76, 0xeb, 0x55, 0xac, 0xa9, 0x0f, 0x80],
    ),
    (
        "DynamicFeeParameterUpdate",
        [0x58, 0x58, 0xb2, 0x87, 0xc2, 0x92, 0x5b, 0xf3],
    ),
    (
        "FeeParameterUpdate",
        [0x30, 0x4c, 0xf1, 0x75, 0x90, 0xd7, 0xf2, 0x2c],
    ),
    (
        "FundReward",
        [0xf6, 0xe4, 0x3a, 0x82, 0x91, 0xaa, 0x4f, 0xcc],
    ),
    ("GoToABin", [0x3b, 0x8a, 0x4c, 0x44, 0x8a, 0x83, 0xb0, 0x43]),
    (
        "IncreaseObservation",
        [0x63, 0xf9, 0x11, 0x79, 0xa6, 0x9c, 0xcf, 0xd7],
    ),
    (
        "IncreasePositionLength",
        [0x9d, 0xef, 0x2a, 0xcc, 0x1e, 0x38, 0xdf, 0x2e],
    ),
    (
        "InitializeReward",
        [0xd3, 0x99, 0x58, 0x3e, 0x95, 0x3c, 0xb1, 0x46],
    ),
    (
        "LbPairCreate",
        [0xb9, 0x4a, 0xfc, 0x7d, 0x1b, 0xd7, 0xbc, 0x6f],
    ),
    (
        "PlaceLimitOrderEvt",
        [0x2b, 0x4f, 0x1b, 0xa9, 0xf4, 0x1c, 0xe1, 0x3f],
    ),
    (
        "PositionClose",
        [0xff, 0xc4, 0x10, 0x6b, 0x1c, 0xca, 0x35, 0x80],
    ),
    (
        "PositionCreate",
        [0x90, 0x8e, 0xfc, 0x54, 0x9d, 0x35, 0x25, 0x79],
    ),
    (
        "Rebalancing",
        [0x00, 0x6d, 0x75, 0xb3, 0x3d, 0x5b, 0xc7, 0xc8],
    ),
    (
        "RemoveLiquidity",
        [0x74, 0xf4, 0x61, 0xe8, 0x67, 0x1f, 0x98, 0x3a],
    ),
    (
        "SetPositionPermissionlessOperationBitsEvt",
        [0xc3, 0xe5, 0x93, 0xf5, 0x1d, 0x7d, 0x30, 0xa8],
    ),
    ("Swap", [0x51, 0x6c, 0xe3, 0xbe, 0xcd, 0xd0, 0x0a, 0xc4]),
    ("Swap2Evt", [0x2e, 0x74, 0x52, 0xd7, 0x94, 0x1b, 0x54, 0x4d]),
    (
        "UpdatePositionLockReleasePoint",
        [0x85, 0xd6, 0x42, 0xe0, 0x40, 0x0c, 0x07, 0xbf],
    ),
    (
        "UpdatePositionOperator",
        [0x27, 0x73, 0x30, 0xcc, 0xf6, 0x2f, 0x42, 0x39],
    ),
    (
        "UpdateRewardDuration",
        [0xdf, 0xf5, 0xe0, 0x99, 0x31, 0x1d, 0xa3, 0xac],
    ),
    (
        "UpdateRewardFunder",
        [0xe0, 0xb2, 0xae, 0x4a, 0xfc, 0xa5, 0x55, 0xb4],
    ),
    (
        "WithdrawIneligibleReward",
        [0xe7, 0xbd, 0x41, 0x95, 0x66, 0xd7, 0x9a, 0xf4],
    ),
];

/// A decoded `Swap` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlmmSwapEvent {
    pub lb_pair: SolanaPubkey,
    pub from: SolanaPubkey,
    pub start_bin_id: i32,
    pub end_bin_id: i32,
    pub amount_in: u64,
    pub amount_out: u64,
    pub swap_for_y: bool,
    pub fee: u64,
    pub protocol_fee: u64,
    pub fee_bps: u128,
    pub host_fee: u64,
}

/// A decoded `Swap2Evt` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlmmSwap2Event {
    pub lb_pair: SolanaPubkey,
    pub from: SolanaPubkey,
    pub start_bin_id: i32,
    pub end_bin_id: i32,
    pub swap_for_y: bool,
    pub fee_bps: u128,
    pub amount_in: u64,
    pub amount_left: u64,
    pub amount_out: u64,
    pub mm_fee: u64,
    pub protocol_fee: u64,
    pub limit_order_fee: u64,
    pub host_fee: u64,
    pub fees_on_input: bool,
    pub fees_on_token_x: bool,
}

/// Classification of one DLMM instruction as an event-CPI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlmmEventOutcome {
    /// Program id is not DLMM.
    NotMine,
    /// DLMM instruction that is not an event-CPI.
    NotEventCpi,
    Swap(DlmmSwapEvent),
    Swap2(DlmmSwap2Event),
    /// IDL event that is not a swap (name only).
    KnownNonLeg {
        name: &'static str,
    },
    /// Event-CPI with a discriminator outside the IDL event set. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Classifies one instruction as a DLMM event-CPI (program-id, tag,
/// event-authority and exact-length gated).
#[must_use]
pub fn classify_dlmm_event(ix: &RawSolanaInstruction) -> DlmmEventOutcome {
    if ix.program_id != DLMM_PROGRAM_ID_BYTES {
        return DlmmEventOutcome::NotMine;
    }
    if ix.data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return DlmmEventOutcome::NotEventCpi;
    }
    let Some(disc) = ix
        .data
        .get(8..DLMM_EVENT_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return DlmmEventOutcome::Malformed {
            reason: format!(
                "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
                ix.data.len()
            ),
        };
    };
    let Some(name) = event_name(&DLMM_EVENTS, &disc) else {
        return DlmmEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    };
    if ix.accounts.len() != 1 || ix.accounts.first() != Some(&DLMM_EVENT_AUTHORITY_BYTES) {
        return DlmmEventOutcome::Malformed {
            reason: format!(
                "event-CPI has {} accounts or an event authority other than D1ZN9Wj1...",
                ix.accounts.len()
            ),
        };
    }
    let body = ix.data.get(DLMM_EVENT_HEADER_LEN..).unwrap_or_default();
    if disc == DLMM_SWAP_EVENT_DISCRIMINATOR {
        return decode_swap(body);
    }
    if disc == DLMM_SWAP2_EVENT_DISCRIMINATOR {
        return decode_swap2(body);
    }
    DlmmEventOutcome::KnownNonLeg { name }
}

fn bad(what: &str) -> DlmmEventOutcome {
    DlmmEventOutcome::Malformed {
        reason: format!("{what} fields do not fit the payload or a bool is not 0/1"),
    }
}

fn decode_swap(body: &[u8]) -> DlmmEventOutcome {
    if body.len() != DLMM_SWAP_EVENT_LEN {
        return DlmmEventOutcome::Malformed {
            reason: format!(
                "Swap payload has {} bytes, expected exactly {DLMM_SWAP_EVENT_LEN}",
                body.len()
            ),
        };
    }
    let mut r = Reader::new(body);
    let parsed = (|| {
        Some(DlmmSwapEvent {
            lb_pair: r.pubkey()?,
            from: r.pubkey()?,
            start_bin_id: r.i32()?,
            end_bin_id: r.i32()?,
            amount_in: r.u64()?,
            amount_out: r.u64()?,
            swap_for_y: r.bool()?,
            fee: r.u64()?,
            protocol_fee: r.u64()?,
            fee_bps: r.u128()?,
            host_fee: r.u64()?,
        })
    })();
    parsed.map_or_else(|| bad("Swap"), DlmmEventOutcome::Swap)
}

fn decode_swap2(body: &[u8]) -> DlmmEventOutcome {
    if body.len() != DLMM_SWAP2_EVENT_LEN {
        return DlmmEventOutcome::Malformed {
            reason: format!(
                "Swap2Evt payload has {} bytes, expected exactly {DLMM_SWAP2_EVENT_LEN}",
                body.len()
            ),
        };
    }
    let mut r = Reader::new(body);
    let parsed = (|| {
        Some(DlmmSwap2Event {
            lb_pair: r.pubkey()?,
            from: r.pubkey()?,
            start_bin_id: r.i32()?,
            end_bin_id: r.i32()?,
            swap_for_y: r.bool()?,
            fee_bps: r.u128()?,
            amount_in: r.u64()?,
            amount_left: r.u64()?,
            amount_out: r.u64()?,
            mm_fee: r.u64()?,
            protocol_fee: r.u64()?,
            limit_order_fee: r.u64()?,
            host_fee: r.u64()?,
            fees_on_input: r.bool()?,
            fees_on_token_x: r.bool()?,
        })
    })();
    parsed.map_or_else(|| bad("Swap2Evt"), DlmmEventOutcome::Swap2)
}

/// Swap instruction variants of the IDL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlmmSwapVariant {
    Swap,
    Swap2,
    SwapExactOut,
    SwapExactOut2,
    SwapWithPriceImpact,
    SwapWithPriceImpact2,
}

impl DlmmSwapVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Swap => "swap",
            Self::Swap2 => "swap2",
            Self::SwapExactOut => "swap_exact_out",
            Self::SwapExactOut2 => "swap_exact_out2",
            Self::SwapWithPriceImpact => "swap_with_price_impact",
            Self::SwapWithPriceImpact2 => "swap_with_price_impact2",
        }
    }

    /// `swap` and `swap2` have live samples; the other four are IDL only.
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        match self {
            Self::Swap | Self::Swap2 => VariantVerification::FixtureVerified,
            _ => VariantVerification::IdlOnly,
        }
    }

    const fn discriminator(self) -> [u8; 8] {
        match self {
            Self::Swap => DLMM_SWAP_DISCRIMINATOR,
            Self::Swap2 => DLMM_SWAP2_DISCRIMINATOR,
            Self::SwapExactOut => DLMM_SWAP_EXACT_OUT_DISCRIMINATOR,
            Self::SwapExactOut2 => DLMM_SWAP_EXACT_OUT2_DISCRIMINATOR,
            Self::SwapWithPriceImpact => DLMM_SWAP_WITH_PRICE_IMPACT_DISCRIMINATOR,
            Self::SwapWithPriceImpact2 => DLMM_SWAP_WITH_PRICE_IMPACT2_DISCRIMINATOR,
        }
    }

    /// Minimum instruction data length (discriminator + required args).
    const fn min_data_len(self) -> usize {
        match self {
            Self::Swap | Self::SwapExactOut => 24,
            // + `RemainingAccountsInfo` (u32 vec length).
            Self::Swap2 | Self::SwapExactOut2 => 28,
            // amount_in u64, Option<i32> (1 or 5 bytes), u16.
            Self::SwapWithPriceImpact => 19,
            Self::SwapWithPriceImpact2 => 23,
        }
    }

    /// `true` when the data length must equal [`Self::min_data_len`].
    const fn exact_len(self) -> bool {
        matches!(self, Self::Swap | Self::SwapExactOut)
    }

    const ALL: [Self; 6] = [
        Self::Swap,
        Self::Swap2,
        Self::SwapExactOut,
        Self::SwapExactOut2,
        Self::SwapWithPriceImpact,
        Self::SwapWithPriceImpact2,
    ];
}

/// `swap` instruction discriminator.
pub const DLMM_SWAP_DISCRIMINATOR: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
/// `swap2` instruction discriminator.
pub const DLMM_SWAP2_DISCRIMINATOR: [u8; 8] = [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88];
/// `swap_exact_out` instruction discriminator.
pub const DLMM_SWAP_EXACT_OUT_DISCRIMINATOR: [u8; 8] =
    [0xfa, 0x49, 0x65, 0x21, 0x26, 0xcf, 0x4b, 0xb8];
/// `swap_exact_out2` instruction discriminator.
pub const DLMM_SWAP_EXACT_OUT2_DISCRIMINATOR: [u8; 8] =
    [0x2b, 0xd7, 0xf7, 0x84, 0x89, 0x3c, 0xf3, 0x51];
/// `swap_with_price_impact` instruction discriminator.
pub const DLMM_SWAP_WITH_PRICE_IMPACT_DISCRIMINATOR: [u8; 8] =
    [0x38, 0xad, 0xe6, 0xd0, 0xad, 0xe4, 0x9c, 0xcd];
/// `swap_with_price_impact2` instruction discriminator.
pub const DLMM_SWAP_WITH_PRICE_IMPACT2_DISCRIMINATOR: [u8; 8] =
    [0x4a, 0x62, 0xc0, 0xd6, 0xb1, 0x33, 0x4b, 0x33];

/// A parsed DLMM swap instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlmmSwapIx {
    pub variant: DlmmSwapVariant,
    pub lb_pair: SolanaPubkey,
    pub token_x_mint: SolanaPubkey,
    pub token_y_mint: SolanaPubkey,
}

/// Account positions shared by all six swap instructions (IDL).
const LB_PAIR_IDX: usize = 0;
const TOKEN_X_MINT_IDX: usize = 6;
const TOKEN_Y_MINT_IDX: usize = 7;
/// Accounts up to and including `token_y_program` (the v1 instructions have
/// 15 IDL accounts, the v2 ones 16; only positions 0..=7 are read).
const MIN_ACCOUNTS: usize = 14;

/// Parses one instruction ASSUMED to belong to DLMM (an event-CPI is not a
/// swap instruction: `NotSwap`).
#[must_use]
pub fn parse_dlmm_swap_instruction(ix: &RawSolanaInstruction) -> SwapInstructionParse<DlmmSwapIx> {
    let Some(disc) = ix.data.get(..8) else {
        return SwapInstructionParse::NotSwap;
    };
    let Some(variant) = DlmmSwapVariant::ALL
        .into_iter()
        .find(|v| v.discriminator() == disc)
    else {
        return SwapInstructionParse::NotSwap;
    };
    let len_ok = if variant.exact_len() {
        ix.data.len() == variant.min_data_len()
    } else {
        ix.data.len() >= variant.min_data_len()
    };
    if !len_ok || ix.accounts.len() < MIN_ACCOUNTS {
        return SwapInstructionParse::Malformed {
            reason: format!(
                "{} has {} data bytes / {} accounts (need {}{} bytes, >= {MIN_ACCOUNTS} accounts)",
                variant.name(),
                ix.data.len(),
                ix.accounts.len(),
                if variant.exact_len() {
                    "exactly "
                } else {
                    ">= "
                },
                variant.min_data_len()
            ),
        };
    }
    match (
        ix.accounts.get(LB_PAIR_IDX),
        ix.accounts.get(TOKEN_X_MINT_IDX),
        ix.accounts.get(TOKEN_Y_MINT_IDX),
    ) {
        (Some(&lb_pair), Some(&token_x_mint), Some(&token_y_mint)) => {
            SwapInstructionParse::Swap(DlmmSwapIx {
                variant,
                lb_pair,
                token_x_mint,
                token_y_mint,
            })
        }
        _ => SwapInstructionParse::Malformed {
            reason: "pool/mint accounts missing".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const IDL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/meteora_dlmm_onchain_idl_2026-10-03.json"
    );

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn idl() -> serde_json::Value {
        let bytes = std::fs::read(IDL_PATH).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), DLMM_IDL_SHA256);
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

    fn fields(idl: &serde_json::Value, name: &str) -> Vec<(String, String)> {
        let ty = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        ty["type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().unwrap().to_owned(),
                    f["type"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn idl_equality_event_set_discriminators_and_swap_layouts() {
        let idl = idl();
        assert_eq!(idl["address"], DLMM_PROGRAM_ID);
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), DLMM_EVENTS.len());
        for (name, d) in DLMM_EVENTS {
            let e = events.iter().find(|e| e["name"] == name).unwrap();
            assert_eq!(idl_bytes(&e["discriminator"]), d, "{name} (IDL)");
            assert_eq!(disc("event", name), d, "{name} (sha256)");
        }
        assert_eq!(disc("event", "Swap"), DLMM_SWAP_EVENT_DISCRIMINATOR);
        assert_eq!(disc("event", "Swap2Evt"), DLMM_SWAP2_EVENT_DISCRIMINATOR);
        let p = |n: &str, t: &str| (n.to_owned(), t.to_owned());
        assert_eq!(
            fields(&idl, "Swap"),
            vec![
                p("lb_pair", "pubkey"),
                p("from", "pubkey"),
                p("start_bin_id", "i32"),
                p("end_bin_id", "i32"),
                p("amount_in", "u64"),
                p("amount_out", "u64"),
                p("swap_for_y", "bool"),
                p("fee", "u64"),
                p("protocol_fee", "u64"),
                p("fee_bps", "u128"),
                p("host_fee", "u64"),
            ]
        );
        assert_eq!(
            fields(&idl, "Swap2Evt"),
            vec![
                p("lb_pair", "pubkey"),
                p("from", "pubkey"),
                p("start_bin_id", "i32"),
                p("end_bin_id", "i32"),
                p("swap_for_y", "bool"),
                p("fee_bps", "u128"),
                p("amount_in", "u64"),
                p("amount_left", "u64"),
                p("amount_out", "u64"),
                p("mm_fee", "u64"),
                p("protocol_fee", "u64"),
                p("limit_order_fee", "u64"),
                p("host_fee", "u64"),
                p("fees_on_input", "bool"),
                p("fees_on_token_x", "bool"),
            ]
        );
        assert_eq!(
            32 + 32 + 4 + 4 + 8 + 8 + 1 + 8 + 8 + 16 + 8,
            DLMM_SWAP_EVENT_LEN
        );
        assert_eq!(
            32 + 32 + 4 + 4 + 1 + 16 + 7 * 8 + 1 + 1,
            DLMM_SWAP2_EVENT_LEN
        );
    }

    #[test]
    fn idl_equality_swap_instructions_and_account_positions() {
        let idl = idl();
        let ixs = idl["instructions"].as_array().unwrap();
        for v in DlmmSwapVariant::ALL {
            let i = ixs.iter().find(|i| i["name"] == v.name()).unwrap();
            assert_eq!(
                idl_bytes(&i["discriminator"]),
                v.discriminator(),
                "{}",
                v.name()
            );
            assert_eq!(
                disc("global", v.name()),
                v.discriminator(),
                "{} (sha256)",
                v.name()
            );
            let names: Vec<&str> = i["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["name"].as_str().unwrap())
                .collect();
            assert_eq!(names[LB_PAIR_IDX], "lb_pair");
            assert_eq!(names[TOKEN_X_MINT_IDX], "token_x_mint");
            assert_eq!(names[TOKEN_Y_MINT_IDX], "token_y_mint");
            assert!(names.len() >= MIN_ACCOUNTS, "{}", v.name());
        }
        assert_eq!(
            DLMM_SWAP_DISCRIMINATOR,
            DlmmSwapVariant::Swap.discriminator()
        );
    }

    fn event_ix(disc: [u8; 8], body: &[u8]) -> RawSolanaInstruction {
        let mut data = EVENT_CPI_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&disc);
        data.extend_from_slice(body);
        RawSolanaInstruction {
            program_id: DLMM_PROGRAM_ID_BYTES,
            accounts: vec![DLMM_EVENT_AUTHORITY_BYTES],
            data,
            instruction_index: 3,
        }
    }

    fn swap_body() -> Vec<u8> {
        let mut b = vec![1u8; 32];
        b.extend_from_slice(&[2u8; 32]);
        b.extend_from_slice(&(-5i32).to_le_bytes());
        b.extend_from_slice(&6i32.to_le_bytes());
        b.extend_from_slice(&1000u64.to_le_bytes());
        b.extend_from_slice(&2000u64.to_le_bytes());
        b.push(1);
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&8u64.to_le_bytes());
        b.extend_from_slice(&9u128.to_le_bytes());
        b.extend_from_slice(&10u64.to_le_bytes());
        b
    }

    fn swap2_body() -> Vec<u8> {
        let mut b = vec![1u8; 32];
        b.extend_from_slice(&[2u8; 32]);
        b.extend_from_slice(&(-5i32).to_le_bytes());
        b.extend_from_slice(&6i32.to_le_bytes());
        b.push(0);
        b.extend_from_slice(&9u128.to_le_bytes());
        for v in [1000u64, 11, 2000, 12, 13, 14, 15] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[1, 0]);
        b
    }

    #[test]
    fn swap_and_swap2_decode_exactly() {
        let DlmmEventOutcome::Swap(s) =
            classify_dlmm_event(&event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &swap_body()))
        else {
            panic!("swap");
        };
        assert_eq!((s.lb_pair, s.from), ([1; 32], [2; 32]));
        assert_eq!((s.start_bin_id, s.end_bin_id), (-5, 6));
        assert_eq!(
            (s.amount_in, s.amount_out, s.swap_for_y),
            (1000, 2000, true)
        );
        assert_eq!(
            (s.fee, s.protocol_fee, s.fee_bps, s.host_fee),
            (7, 8, 9, 10)
        );
        let DlmmEventOutcome::Swap2(s) =
            classify_dlmm_event(&event_ix(DLMM_SWAP2_EVENT_DISCRIMINATOR, &swap2_body()))
        else {
            panic!("swap2");
        };
        assert!(!s.swap_for_y);
        assert_eq!((s.amount_in, s.amount_left, s.amount_out), (1000, 11, 2000));
        assert_eq!(
            (s.mm_fee, s.protocol_fee, s.limit_order_fee, s.host_fee),
            (12, 13, 14, 15)
        );
        assert!(s.fees_on_input && !s.fees_on_token_x);
    }

    #[test]
    fn gates_reject_length_authority_bool_program_and_unknown() {
        let mut long = swap_body();
        long.push(0);
        assert!(matches!(
            classify_dlmm_event(&event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &long)),
            DlmmEventOutcome::Malformed { .. }
        ));
        let mut short = swap2_body();
        short.pop();
        assert!(matches!(
            classify_dlmm_event(&event_ix(DLMM_SWAP2_EVENT_DISCRIMINATOR, &short)),
            DlmmEventOutcome::Malformed { .. }
        ));
        let mut bad_bool = swap_body();
        bad_bool[32 + 32 + 4 + 4 + 8 + 8] = 9;
        assert!(matches!(
            classify_dlmm_event(&event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &bad_bool)),
            DlmmEventOutcome::Malformed { .. }
        ));
        let mut wrong_auth = event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &swap_body());
        wrong_auth.accounts = vec![[9; 32]];
        assert!(matches!(
            classify_dlmm_event(&wrong_auth),
            DlmmEventOutcome::Malformed { .. }
        ));
        let mut other_program = event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &swap_body());
        other_program.program_id = [5; 32];
        assert_eq!(
            classify_dlmm_event(&other_program),
            DlmmEventOutcome::NotMine
        );
        let mut not_cpi = event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &swap_body());
        not_cpi.data[0] ^= 1;
        assert_eq!(classify_dlmm_event(&not_cpi), DlmmEventOutcome::NotEventCpi);
        assert_eq!(
            classify_dlmm_event(&event_ix([1; 8], &[])),
            DlmmEventOutcome::UnknownEvent {
                discriminator: [1; 8]
            }
        );
        assert_eq!(
            classify_dlmm_event(&event_ix(disc("event", "ClaimFee"), &[])),
            DlmmEventOutcome::KnownNonLeg { name: "ClaimFee" }
        );
    }

    fn swap_ix(variant: DlmmSwapVariant, len: usize, accounts: usize) -> RawSolanaInstruction {
        let mut data = variant.discriminator().to_vec();
        data.resize(len, 0);
        RawSolanaInstruction {
            program_id: DLMM_PROGRAM_ID_BYTES,
            accounts: (0..accounts)
                .map(|i| [u8::try_from(i).unwrap(); 32])
                .collect(),
            data,
            instruction_index: 0,
        }
    }

    #[test]
    fn swap_instructions_parse_with_idl_positions_and_verification() {
        for (v, len, ver) in [
            (
                DlmmSwapVariant::Swap,
                24,
                VariantVerification::FixtureVerified,
            ),
            (
                DlmmSwapVariant::Swap2,
                28,
                VariantVerification::FixtureVerified,
            ),
            (
                DlmmSwapVariant::SwapExactOut,
                24,
                VariantVerification::IdlOnly,
            ),
            (
                DlmmSwapVariant::SwapExactOut2,
                40,
                VariantVerification::IdlOnly,
            ),
            (
                DlmmSwapVariant::SwapWithPriceImpact,
                19,
                VariantVerification::IdlOnly,
            ),
            (
                DlmmSwapVariant::SwapWithPriceImpact2,
                30,
                VariantVerification::IdlOnly,
            ),
        ] {
            let SwapInstructionParse::Swap(s) = parse_dlmm_swap_instruction(&swap_ix(v, len, 17))
            else {
                panic!("{}", v.name());
            };
            assert_eq!(
                (s.lb_pair, s.token_x_mint, s.token_y_mint),
                ([0; 32], [6; 32], [7; 32])
            );
            assert_eq!(v.verification(), ver);
        }
        assert!(matches!(
            parse_dlmm_swap_instruction(&swap_ix(DlmmSwapVariant::Swap, 25, 17)),
            SwapInstructionParse::Malformed { .. }
        ));
        assert!(matches!(
            parse_dlmm_swap_instruction(&swap_ix(DlmmSwapVariant::Swap2, 28, 13)),
            SwapInstructionParse::Malformed { .. }
        ));
        // An event-CPI and an unrelated instruction are not swaps.
        assert!(matches!(
            parse_dlmm_swap_instruction(&event_ix(DLMM_SWAP_EVENT_DISCRIMINATOR, &swap_body())),
            SwapInstructionParse::NotSwap
        ));
    }
}
