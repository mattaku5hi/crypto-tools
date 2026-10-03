//! OKX DEX Router (`proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u`, "OKX: DEX
//! Router") events, decoded from Anchor event-CPI self-invocations
//! (ADR-017 draft, see the engineering report; ADR-009 verification levels).
//!
//! ## How the events are emitted
//!
//! Every live event is an **inner instruction** of the router to itself:
//! `EVENT_CPI_DISCRIMINATOR (e445a52e51cb9a1d) ++ event discriminator (8) ++
//! Borsh event`, with exactly one account, the event-authority PDA
//! (`Ag3hiK9s...`, 106 of 106 live event-CPIs in the committed fixtures,
//! failed transactions included). The raw transaction the provider returns
//! already carries these inner instructions, so no log parsing and no
//! provider change is needed. Program id + tag + authority + exact length is
//! the whole runtime gate (an Anchor program rejects the tag unless the
//! event-authority PDA signed).
//!
//! ## Layouts (pinned on-chain IDL, checked field by field by the tests)
//!
//! `docs/p0/measurements/fixtures/okx_dex_router_onchain_idl_2026-10-03.json`,
//! sha256 `c1f85197...54c9`, discriminators `sha256("event:<Name>")[..8]`.
//!
//! * Six **order events** (`SwapCpiEvent2`, `SwapTobV2CpiEvent2`,
//!   `SwapTocV2CpiEvent2`, `SwapWithFeeCpiEventV3`, `SwapWithFeesCpiEvent2`,
//!   `SwapWithFeesCpiEventEnhanced2`), one per router order. They start with
//!   `order_id u64, source_mint, destination_mint,
//!   source_token_account_owner, destination_token_account_owner,
//!   amount_in u64, source_token_change u64, destination_token_change u64`
//!   (160 bytes) followed by a fee tail that differs per event (fixed size
//!   except `SwapWithFeeCpiEventV3`, which carries three `Vec`s and an
//!   `Option`). The tail is validated exactly and not retained.
//! * One **per-hop event** `SwapEvent { dex: Dex, amount_in u64, amount_out
//!   u64 }` (same name and discriminator as Jupiter's, gated by program id).
//!   `Dex` is a Borsh enum of 143 venues, some carrying data; it has no
//!   mints, so a hop alone cannot say *which token* was swapped.
//!
//! Length policy: exact (any trailing or missing byte is `Malformed`).
//!
//! ## Evidence role
//!
//! Order events name the mints and the two token-account **owners** of the
//! order. Verification status per variant is [`OkxOrderEventKind::verification`]
//! (ADR-009). The event carries no consideration: wallet consideration stays
//! the wallet's own deltas (ADR-013 section 2).

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, VariantVerification};
use crate::okx_schema::{
    SWAP_CPI_EVENT2_TAIL, SWAP_TOB_V2_CPI_EVENT2_TAIL, SWAP_TOC_V2_CPI_EVENT2_TAIL,
    SWAP_WITH_FEE_CPI_EVENT_V3_TAIL, SWAP_WITH_FEES_CPI_EVENT_ENHANCED2_TAIL,
    SWAP_WITH_FEES_CPI_EVENT2_TAIL, Ty, read_dex, skip_fields,
};

/// OKX DEX Router program id (base58).
pub const OKX_DEX_ROUTER_PROGRAM_ID: &str = "proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u";
/// OKX DEX Router program id bytes.
pub const OKX_DEX_ROUTER_PROGRAM_ID_BYTES: SolanaPubkey = [
    0x0c, 0x42, 0x9b, 0xd7, 0xc1, 0x8f, 0x50, 0xf8, 0x15, 0x6d, 0x9a, 0xfc, 0x1c, 0xdd, 0xe7, 0x2d,
    0xf6, 0x68, 0xd9, 0xab, 0x3b, 0xec, 0xaf, 0x6b, 0x57, 0x0d, 0x57, 0x66, 0x64, 0x5a, 0xd9, 0xc8,
];
/// Event-authority account every live event-CPI passed (`Ag3hiK9s...`).
pub const OKX_EVENT_AUTHORITY_BYTES: SolanaPubkey = [
    0x8f, 0xb9, 0xe3, 0x94, 0x30, 0xed, 0x4a, 0x55, 0x55, 0x5c, 0x1d, 0x57, 0x56, 0xa3, 0x90, 0x23,
    0x66, 0x4f, 0x13, 0xa1, 0xcd, 0xa0, 0x99, 0x63, 0xae, 0x92, 0x25, 0xb5, 0xae, 0x53, 0x91, 0x57,
];
/// sha256 of the pinned on-chain IDL file.
pub const OKX_IDL_SHA256: &str = "c1f85197a5d96dd43fc2a1b981126cfe04c3eb6d2d68b3731129a259e45d54c9";

/// Per-hop `SwapEvent` discriminator (`sha256("event:SwapEvent")[..8]`).
pub const OKX_SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
/// Event-CPI header: tag + event discriminator.
pub const OKX_EVENT_HEADER_LEN: usize = 16;
/// Bytes of the fields every order event starts with.
pub const OKX_ORDER_EVENT_COMMON_LEN: usize = 160;

/// The six order events of the IDL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OkxOrderEventKind {
    SwapCpiEvent2,
    SwapTobV2CpiEvent2,
    SwapTocV2CpiEvent2,
    SwapWithFeeCpiEventV3,
    SwapWithFeesCpiEvent2,
    SwapWithFeesCpiEventEnhanced2,
}

impl OkxOrderEventKind {
    /// All variants, in IDL order.
    pub const ALL: [Self; 6] = [
        Self::SwapCpiEvent2,
        Self::SwapTobV2CpiEvent2,
        Self::SwapTocV2CpiEvent2,
        Self::SwapWithFeeCpiEventV3,
        Self::SwapWithFeesCpiEvent2,
        Self::SwapWithFeesCpiEventEnhanced2,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SwapCpiEvent2 => "SwapCpiEvent2",
            Self::SwapTobV2CpiEvent2 => "SwapTobV2CpiEvent2",
            Self::SwapTocV2CpiEvent2 => "SwapTocV2CpiEvent2",
            Self::SwapWithFeeCpiEventV3 => "SwapWithFeeCpiEventV3",
            Self::SwapWithFeesCpiEvent2 => "SwapWithFeesCpiEvent2",
            Self::SwapWithFeesCpiEventEnhanced2 => "SwapWithFeesCpiEventEnhanced2",
        }
    }

    /// `sha256("event:<Name>")[..8]`, equal to the IDL discriminator.
    #[must_use]
    pub const fn discriminator(self) -> [u8; 8] {
        match self {
            Self::SwapCpiEvent2 => [0x15, 0x5e, 0xe0, 0x35, 0xdc, 0xe8, 0xc1, 0x5e],
            Self::SwapTobV2CpiEvent2 => [0xf4, 0x69, 0xb8, 0x5a, 0x3c, 0xa5, 0x24, 0x21],
            Self::SwapTocV2CpiEvent2 => [0x66, 0x10, 0xb5, 0x3e, 0xc9, 0x47, 0xd4, 0x29],
            Self::SwapWithFeeCpiEventV3 => [0x2d, 0x35, 0xa7, 0x13, 0xb4, 0xc4, 0x60, 0x96],
            Self::SwapWithFeesCpiEvent2 => [0x0c, 0x86, 0x26, 0x5d, 0xa7, 0x97, 0x2a, 0x45],
            Self::SwapWithFeesCpiEventEnhanced2 => [0x54, 0x4f, 0xdb, 0xf7, 0xda, 0x9c, 0xdb, 0xb1],
        }
    }

    fn tail(self) -> &'static [(&'static str, Ty)] {
        match self {
            Self::SwapCpiEvent2 => SWAP_CPI_EVENT2_TAIL,
            Self::SwapTobV2CpiEvent2 => SWAP_TOB_V2_CPI_EVENT2_TAIL,
            Self::SwapTocV2CpiEvent2 => SWAP_TOC_V2_CPI_EVENT2_TAIL,
            Self::SwapWithFeeCpiEventV3 => SWAP_WITH_FEE_CPI_EVENT_V3_TAIL,
            Self::SwapWithFeesCpiEvent2 => SWAP_WITH_FEES_CPI_EVENT2_TAIL,
            Self::SwapWithFeesCpiEventEnhanced2 => SWAP_WITH_FEES_CPI_EVENT_ENHANCED2_TAIL,
        }
    }

    /// Evidence level (ADR-009). The rule is strict: `FixtureVerified` only
    /// if every committed live sample of the variant has
    /// `source_token_change` / `destination_token_change` **exactly** equal
    /// to the owner-keyed net deltas of the named owner and mint.
    ///
    /// Result of `scout-engine/tests/okx_router_legs.rs` over every fixture
    /// holding the router: 24 successful `SwapWithFeesCpiEvent2` samples and
    /// none of the other five variants. The non-quote side is exact in every
    /// checkable sample, but the stablecoin side is exact in only 7 of 20 (the
    /// wallet also pays or loses fees to other owners that the event does not
    /// carry), so `SwapWithFeesCpiEvent2` stays `IdlOnly`; the other five have
    /// no sample at all.
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        VariantVerification::IdlOnly
    }
}

/// One decoded order event (the common fields; the fee tail is validated and
/// not retained).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OkxOrderEvent {
    pub kind: OkxOrderEventKind,
    pub instruction_index: u32,
    pub order_id: u64,
    pub source_mint: SolanaPubkey,
    pub destination_mint: SolanaPubkey,
    pub source_token_account_owner: SolanaPubkey,
    pub destination_token_account_owner: SolanaPubkey,
    pub amount_in: u64,
    pub source_token_change: u64,
    pub destination_token_change: u64,
}

impl OkxOrderEvent {
    /// `true` iff both token accounts of the order belong to one owner (the
    /// order has no separate receiver).
    #[must_use]
    pub fn single_owner(&self) -> bool {
        self.source_token_account_owner == self.destination_token_account_owner
    }
}

/// One executed hop (`SwapEvent`). No mints: only the venue and amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OkxHop {
    pub instruction_index: u32,
    /// Borsh tag of the `Dex` enum (its index in the IDL).
    pub dex_variant: u8,
    /// IDL name of the variant (data fields, if any, are validated and dropped).
    pub dex: &'static str,
    pub amount_in: u64,
    pub amount_out: u64,
}

/// Classification of one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OkxEventOutcome {
    /// Program id is not the OKX DEX Router.
    NotMine,
    /// Router instruction that is not an event-CPI.
    NotEventCpi,
    Order(OkxOrderEvent),
    Hop(OkxHop),
    /// Event-CPI with a discriminator outside the IDL events. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Program-id-gated OKX event decoder (no state).
#[derive(Debug, Clone, Copy, Default)]
pub struct OkxEventDecoder;

impl OkxEventDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// `true` iff the instruction belongs to the OKX DEX Router.
    #[must_use]
    pub fn is_program(&self, instruction: &RawSolanaInstruction) -> bool {
        instruction.program_id == OKX_DEX_ROUTER_PROGRAM_ID_BYTES
    }

    #[must_use]
    pub fn classify(&self, instruction: &RawSolanaInstruction) -> OkxEventOutcome {
        if !self.is_program(instruction) {
            return OkxEventOutcome::NotMine;
        }
        classify_okx_event(instruction)
    }
}

fn read_pubkey(buf: &[u8], pos: usize) -> Option<SolanaPubkey> {
    buf.get(pos..pos.checked_add(32)?)?.try_into().ok()
}

fn read_u64(buf: &[u8], pos: usize) -> Option<u64> {
    let a: [u8; 8] = buf.get(pos..pos.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(a))
}

fn malformed(reason: String) -> OkxEventOutcome {
    OkxEventOutcome::Malformed { reason }
}

fn decode_order(
    kind: OkxOrderEventKind,
    payload: &[u8],
    instruction_index: u32,
) -> OkxEventOutcome {
    let fields = (
        read_u64(payload, 0),
        read_pubkey(payload, 8),
        read_pubkey(payload, 40),
        read_pubkey(payload, 72),
        read_pubkey(payload, 104),
        read_u64(payload, 136),
        read_u64(payload, 144),
        read_u64(payload, 152),
    );
    let (
        Some(order_id),
        Some(source_mint),
        Some(destination_mint),
        Some(source_token_account_owner),
        Some(destination_token_account_owner),
        Some(amount_in),
        Some(source_token_change),
        Some(destination_token_change),
    ) = fields
    else {
        return malformed(format!(
            "{} payload has {} bytes, fewer than the {OKX_ORDER_EVENT_COMMON_LEN} common bytes",
            kind.name(),
            payload.len()
        ));
    };
    let mut pos = OKX_ORDER_EVENT_COMMON_LEN;
    if skip_fields(payload, &mut pos, kind.tail()).is_none() || pos != payload.len() {
        return malformed(format!(
            "{} fee tail does not match the IDL exactly ({} payload bytes)",
            kind.name(),
            payload.len()
        ));
    }
    OkxEventOutcome::Order(OkxOrderEvent {
        kind,
        instruction_index,
        order_id,
        source_mint,
        destination_mint,
        source_token_account_owner,
        destination_token_account_owner,
        amount_in,
        source_token_change,
        destination_token_change,
    })
}

fn decode_hop(payload: &[u8], instruction_index: u32) -> OkxEventOutcome {
    let mut pos = 0usize;
    let Some((dex_variant, dex)) = read_dex(payload, &mut pos) else {
        return malformed("SwapEvent `dex` does not decode per the IDL".to_owned());
    };
    let (Some(amount_in), Some(amount_out)) = (read_u64(payload, pos), read_u64(payload, pos + 8))
    else {
        return malformed("SwapEvent amounts do not fit the payload".to_owned());
    };
    if pos + 16 != payload.len() {
        return malformed(format!(
            "SwapEvent payload has {} bytes, the {dex} hop needs exactly {}",
            payload.len(),
            pos + 16
        ));
    }
    OkxEventOutcome::Hop(OkxHop {
        instruction_index,
        dex_variant,
        dex,
        amount_in,
        amount_out,
    })
}

/// Classify one instruction ASSUMED to belong to the OKX DEX Router (no
/// program check).
#[must_use]
pub fn classify_okx_event(instruction: &RawSolanaInstruction) -> OkxEventOutcome {
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return OkxEventOutcome::NotEventCpi;
    }
    let Some(disc) = data
        .get(8..OKX_EVENT_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return malformed(format!(
            "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
            data.len()
        ));
    };
    let payload = data.get(OKX_EVENT_HEADER_LEN..).unwrap_or_default();
    let kind = OkxOrderEventKind::ALL
        .into_iter()
        .find(|k| k.discriminator() == disc);
    if kind.is_none() && disc != OKX_SWAP_EVENT_DISCRIMINATOR {
        return OkxEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    }
    if instruction.accounts.len() != 1
        || instruction.accounts.first() != Some(&OKX_EVENT_AUTHORITY_BYTES)
    {
        return malformed(format!(
            "event-CPI has {} accounts or an event authority other than Ag3hiK9s...",
            instruction.accounts.len()
        ));
    }
    match kind {
        Some(k) => decode_order(k, payload, instruction.instruction_index),
        None => decode_hop(payload, instruction.instruction_index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okx_schema::{Def, dex_def};
    use sha2::{Digest, Sha256};

    const IDL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/p0/measurements/fixtures/okx_dex_router_onchain_idl_2026-10-03.json"
    );

    fn b58(s: &str) -> SolanaPubkey {
        bs58::decode(s).into_vec().unwrap().try_into().unwrap()
    }

    fn ix(data: Vec<u8>, accounts: Vec<SolanaPubkey>) -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: OKX_DEX_ROUTER_PROGRAM_ID_BYTES,
            accounts,
            data,
            instruction_index: 7,
        }
    }

    fn event_ix(disc: [u8; 8], payload: &[u8]) -> RawSolanaInstruction {
        let mut d = EVENT_CPI_DISCRIMINATOR.to_vec();
        d.extend_from_slice(&disc);
        d.extend_from_slice(payload);
        ix(d, vec![OKX_EVENT_AUTHORITY_BYTES])
    }

    /// The 160 common bytes.
    fn common() -> Vec<u8> {
        let mut p = 42u64.to_le_bytes().to_vec();
        for n in 1..=4u8 {
            p.extend_from_slice(&[n; 32]);
        }
        for v in [1000u64, 990, 5000] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p
    }

    /// Valid payload of each order event: common bytes plus a zeroed tail of
    /// the IDL size (V3: empty vecs and `None`).
    fn order_payload(kind: OkxOrderEventKind) -> Vec<u8> {
        let mut p = common();
        match kind {
            OkxOrderEventKind::SwapCpiEvent2 => {}
            OkxOrderEventKind::SwapTobV2CpiEvent2 => p.extend(vec![
                0u8;
                1 + 4
                    + 4
                    + 8
                    + 32
                    + 4
                    + 8
                    + 32
                    + 2
                    + 8
                    + 32
                    + 1
                    + 8
                    + 32
            ]),
            OkxOrderEventKind::SwapTocV2CpiEvent2 => {
                p.extend(vec![0u8; 1 + 4 + 4 + 8 + 32 + 4 + 8 + 32 + 2 + 8 + 32])
            }
            OkxOrderEventKind::SwapWithFeeCpiEventV3 => {
                // bool, bool, u32, u64, u8, three empty vecs, None
                p.extend(vec![0u8; 1 + 1 + 4 + 8 + 1 + 4 + 4 + 4 + 1]);
            }
            OkxOrderEventKind::SwapWithFeesCpiEvent2 => {
                p.extend(vec![0u8; 1 + 4 + 8 + 32 + 2 + 8 + 32 + 1 + 8 + 32])
            }
            OkxOrderEventKind::SwapWithFeesCpiEventEnhanced2 => {
                p.extend(vec![
                    0u8;
                    1 + 4 + 8 + 32 + 2 + 8 + 32 + 1 + 8 + 32 + 2 + 8 + 32
                ]);
            }
        }
        p
    }

    #[test]
    fn constants_match_base58() {
        assert_eq!(
            b58(OKX_DEX_ROUTER_PROGRAM_ID),
            OKX_DEX_ROUTER_PROGRAM_ID_BYTES
        );
        assert_eq!(
            b58("Ag3hiK9svNixH9Vu5sD2CmK5fyDWrx9a1iVSbZW22bUS"),
            OKX_EVENT_AUTHORITY_BYTES
        );
    }

    #[test]
    fn every_order_event_roundtrips() {
        for kind in OkxOrderEventKind::ALL {
            let i = event_ix(kind.discriminator(), &order_payload(kind));
            match OkxEventDecoder::new().classify(&i) {
                OkxEventOutcome::Order(e) => {
                    assert_eq!(e.kind, kind);
                    assert_eq!(e.instruction_index, 7);
                    assert_eq!(e.order_id, 42);
                    assert_eq!(e.source_mint, [1; 32]);
                    assert_eq!(e.destination_mint, [2; 32]);
                    assert_eq!(e.source_token_account_owner, [3; 32]);
                    assert_eq!(e.destination_token_account_owner, [4; 32]);
                    assert_eq!(
                        (
                            e.amount_in,
                            e.source_token_change,
                            e.destination_token_change
                        ),
                        (1000, 990, 5000)
                    );
                    assert!(!e.single_owner());
                }
                other => panic!("{}: unexpected {other:?}", kind.name()),
            }
        }
    }

    #[test]
    fn trailing_and_missing_bytes_are_malformed() {
        for kind in OkxOrderEventKind::ALL {
            let good = order_payload(kind);
            let mut t = good.clone();
            t.push(0);
            let short = &good[..good.len() - 1];
            for p in [t.as_slice(), short, &good[..100]] {
                assert!(
                    matches!(
                        OkxEventDecoder::new().classify(&event_ix(kind.discriminator(), p)),
                        OkxEventOutcome::Malformed { .. }
                    ),
                    "{}",
                    kind.name()
                );
            }
        }
    }

    #[test]
    fn v3_vectors_and_option_are_walked_exactly() {
        let kind = OkxOrderEventKind::SwapWithFeeCpiEventV3;
        let mut p = common();
        p.extend([0u8, 1]); // direction, paid in sol
        p.extend(7u32.to_le_bytes());
        p.extend(9u64.to_le_bytes());
        p.push(2); // levels
        p.extend(2u32.to_le_bytes()); // rates
        p.extend([1u8; 8]);
        p.extend(2u32.to_le_bytes()); // amounts
        p.extend([2u8; 16]);
        p.extend(1u32.to_le_bytes()); // accounts
        p.extend([3u8; 32]);
        p.push(1); // Some(TrimFeeInfo)
        p.extend(vec![0u8; 2 + 8 + 32 + 2 + 8 + 32]);
        assert!(matches!(
            OkxEventDecoder::new().classify(&event_ix(kind.discriminator(), &p)),
            OkxEventOutcome::Order(_)
        ));
        // A vec count larger than the remaining bytes is rejected.
        let mut bad = common();
        bad.extend([0u8, 0]);
        bad.extend(0u32.to_le_bytes());
        bad.extend(0u64.to_le_bytes());
        bad.push(0);
        bad.extend(u32::MAX.to_le_bytes());
        assert!(matches!(
            OkxEventDecoder::new().classify(&event_ix(kind.discriminator(), &bad)),
            OkxEventOutcome::Malformed { .. }
        ));
        // Invalid option flag.
        let mut flag = p.clone();
        let n = common().len() + 1 + 1 + 4 + 8 + 1 + 4 + 8 + 4 + 16 + 4 + 32;
        flag[n] = 2;
        assert!(matches!(
            OkxEventDecoder::new().classify(&event_ix(kind.discriminator(), &flag)),
            OkxEventOutcome::Malformed { .. }
        ));
    }

    fn hop_payload(variant: u8, data: &[u8]) -> Vec<u8> {
        let mut p = vec![variant];
        p.extend_from_slice(data);
        p.extend(11u64.to_le_bytes());
        p.extend(22u64.to_le_bytes());
        p
    }

    #[test]
    fn hop_unit_and_data_variants() {
        let dec = OkxEventDecoder::new();
        let i = event_ix(OKX_SWAP_EVENT_DISCRIMINATOR, &hop_payload(0, &[]));
        assert_eq!(
            dec.classify(&i),
            OkxEventOutcome::Hop(OkxHop {
                instruction_index: 7,
                dex_variant: 0,
                dex: "SplTokenSwap",
                amount_in: 11,
                amount_out: 22
            })
        );
        // `PumpfunBuy { is_cash_back: bool }` carries one byte.
        let Def::Enum { variants, .. } = dex_def() else {
            panic!()
        };
        let idx = variants
            .iter()
            .position(|(n, _)| *n == "PumpfunBuy")
            .unwrap();
        let tag = u8::try_from(idx).unwrap();
        match dec.classify(&event_ix(
            OKX_SWAP_EVENT_DISCRIMINATOR,
            &hop_payload(tag, &[1]),
        )) {
            OkxEventOutcome::Hop(h) => assert_eq!((h.dex, h.dex_variant), ("PumpfunBuy", tag)),
            other => panic!("{other:?}"),
        }
        // Missing or extra variant data, an invalid bool, an unknown tag.
        for bad in [
            hop_payload(tag, &[]),
            hop_payload(tag, &[1, 0]),
            hop_payload(tag, &[2]),
            hop_payload(255, &[]),
            hop_payload(0, &[0]),
            vec![],
        ] {
            assert!(
                matches!(
                    dec.classify(&event_ix(OKX_SWAP_EVENT_DISCRIMINATOR, &bad)),
                    OkxEventOutcome::Malformed { .. }
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn gates() {
        let dec = OkxEventDecoder::new();
        let kind = OkxOrderEventKind::SwapCpiEvent2;
        let good = event_ix(kind.discriminator(), &order_payload(kind));
        let mut other_program = good.clone();
        other_program.program_id = [9; 32];
        assert_eq!(dec.classify(&other_program), OkxEventOutcome::NotMine);
        assert_eq!(
            dec.classify(&ix(vec![1, 2, 3, 4, 5, 6, 7, 8, 9], vec![])),
            OkxEventOutcome::NotEventCpi
        );
        let mut wrong_authority = good.clone();
        wrong_authority.accounts = vec![[7; 32]];
        let mut two_accounts = good.clone();
        two_accounts.accounts.push(OKX_EVENT_AUTHORITY_BYTES);
        let mut none = good.clone();
        none.accounts.clear();
        for i in [wrong_authority, two_accounts, none] {
            assert!(matches!(
                dec.classify(&i),
                OkxEventOutcome::Malformed { .. }
            ));
        }
        assert!(matches!(
            dec.classify(&ix(EVENT_CPI_DISCRIMINATOR.to_vec(), vec![])),
            OkxEventOutcome::Malformed { .. }
        ));
        assert_eq!(
            dec.classify(&event_ix([1, 2, 3, 4, 5, 6, 7, 8], &[])),
            OkxEventOutcome::UnknownEvent {
                discriminator: [1, 2, 3, 4, 5, 6, 7, 8]
            }
        );
    }

    #[test]
    fn verification_is_idl_only_until_every_sample_reconciles() {
        for k in OkxOrderEventKind::ALL {
            assert_eq!(k.verification(), VariantVerification::IdlOnly);
        }
    }

    // ---- IDL equality (the pinned on-chain file is the authority) ----

    fn idl() -> serde_json::Value {
        let bytes = std::fs::read(IDL_PATH).unwrap();
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        assert_eq!(hex, OKX_IDL_SHA256);
        serde_json::from_slice(&bytes).unwrap()
    }

    fn disc(name: &str) -> [u8; 8] {
        Sha256::digest(format!("event:{name}").as_bytes())[..8]
            .try_into()
            .unwrap()
    }

    fn named<'a>(list: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} not in IDL"))
    }

    /// `true` iff `ty` equals the IDL type `v`.
    fn ty_matches(ty: &Ty, v: &serde_json::Value, idl: &serde_json::Value) -> bool {
        if let Some(s) = v.as_str() {
            return matches!(
                (ty, s),
                (Ty::Bool, "bool")
                    | (Ty::U8, "u8")
                    | (Ty::U16, "u16")
                    | (Ty::U32, "u32")
                    | (Ty::U64, "u64")
                    | (Ty::I64, "i64")
                    | (Ty::U128, "u128")
                    | (Ty::Pubkey, "pubkey")
                    | (Ty::Bytes, "bytes")
            );
        }
        match ty {
            Ty::Vec(inner) => v.get("vec").is_some_and(|t| ty_matches(inner, t, idl)),
            Ty::Option(inner) => v.get("option").is_some_and(|t| ty_matches(inner, t, idl)),
            Ty::Array(inner, n) => v.get("array").and_then(|a| a.as_array()).is_some_and(|a| {
                a.len() == 2
                    && ty_matches(inner, &a[0], idl)
                    && a[1].as_u64() == Some(u64::try_from(*n).unwrap())
            }),
            Ty::Defined(def) => v["defined"]["name"]
                .as_str()
                .is_some_and(|n| def_matches(def, named(&idl["types"], n), idl)),
            _ => false,
        }
    }

    fn field_type(f: &serde_json::Value) -> &serde_json::Value {
        if f.get("type").is_some() {
            &f["type"]
        } else {
            f
        }
    }

    fn def_matches(def: &Def, t: &serde_json::Value, idl: &serde_json::Value) -> bool {
        match def {
            Def::Struct { name, fields } => {
                let idl_fields = t["type"]["fields"].as_array().unwrap();
                t["name"] == *name
                    && t["type"]["kind"] == "struct"
                    && idl_fields.len() == fields.len()
                    && idl_fields
                        .iter()
                        .zip(fields.iter())
                        .all(|(f, (n, ty))| f["name"] == *n && ty_matches(ty, &f["type"], idl))
            }
            Def::Enum { name, variants } => {
                let idl_variants = t["type"]["variants"].as_array().unwrap();
                t["name"] == *name
                    && t["type"]["kind"] == "enum"
                    && idl_variants.len() == variants.len()
                    && idl_variants
                        .iter()
                        .zip(variants.iter())
                        .all(|(v, (n, tys))| {
                            let fs: &[serde_json::Value] = v
                                .get("fields")
                                .and_then(|f| f.as_array())
                                .map_or(&[], Vec::as_slice);
                            v["name"] == *n
                                && fs.len() == tys.len()
                                && fs
                                    .iter()
                                    .zip(tys.iter())
                                    .all(|(f, ty)| ty_matches(ty, field_type(f), idl))
                        })
            }
        }
    }

    #[test]
    fn idl_equality_event_set_and_discriminators() {
        let idl = idl();
        let events = idl["events"].as_array().unwrap();
        assert_eq!(events.len(), 7);
        for k in OkxOrderEventKind::ALL {
            let e = named(&idl["events"], k.name());
            let idl_disc: Vec<u8> = e["discriminator"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
                .collect();
            assert_eq!(idl_disc, k.discriminator(), "{}", k.name());
            assert_eq!(disc(k.name()), k.discriminator(), "{}", k.name());
        }
        let e = named(&idl["events"], "SwapEvent");
        assert_eq!(e["discriminator"].as_array().unwrap().len(), 8);
        assert_eq!(disc("SwapEvent"), OKX_SWAP_EVENT_DISCRIMINATOR);
        assert_eq!(idl["address"], OKX_DEX_ROUTER_PROGRAM_ID);
        assert_eq!(idl["metadata"]["name"], "OKX: DEX Router");
    }

    #[test]
    fn idl_equality_order_event_fields() {
        let idl = idl();
        let common_fields: [(&str, &str); 8] = [
            ("order_id", "u64"),
            ("source_mint", "pubkey"),
            ("destination_mint", "pubkey"),
            ("source_token_account_owner", "pubkey"),
            ("destination_token_account_owner", "pubkey"),
            ("amount_in", "u64"),
            ("source_token_change", "u64"),
            ("destination_token_change", "u64"),
        ];
        assert_eq!(8 + 32 * 4 + 8 * 3, OKX_ORDER_EVENT_COMMON_LEN);
        for k in OkxOrderEventKind::ALL {
            let t = named(&idl["types"], k.name());
            let fields = t["type"]["fields"].as_array().unwrap();
            let tail = k.tail();
            assert_eq!(fields.len(), 8 + tail.len(), "{}", k.name());
            for (f, (n, ty)) in fields.iter().zip(common_fields) {
                assert_eq!(f["name"], n, "{}", k.name());
                assert_eq!(f["type"], ty, "{}", k.name());
            }
            for (f, (n, ty)) in fields[8..].iter().zip(tail.iter()) {
                assert_eq!(f["name"], *n, "{}", k.name());
                assert!(ty_matches(ty, &f["type"], &idl), "{} {n}", k.name());
            }
        }
    }

    #[test]
    fn idl_equality_dex_enum_and_hop_event() {
        let idl = idl();
        let swap_event = named(&idl["types"], "SwapEvent");
        let f = swap_event["type"]["fields"].as_array().unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(
            (
                f[0]["name"].as_str(),
                f[0]["type"]["defined"]["name"].as_str()
            ),
            (Some("dex"), Some("Dex"))
        );
        assert_eq!(
            (f[1]["name"].as_str(), f[1]["type"].as_str()),
            (Some("amount_in"), Some("u64"))
        );
        assert_eq!(
            (f[2]["name"].as_str(), f[2]["type"].as_str()),
            (Some("amount_out"), Some("u64"))
        );
        let dex = named(&idl["types"], "Dex");
        assert!(
            def_matches(dex_def(), dex, &idl),
            "Dex enum differs from the IDL"
        );
        let Def::Enum { variants, .. } = dex_def() else {
            panic!()
        };
        assert!(variants.len() == 143);
    }
}
