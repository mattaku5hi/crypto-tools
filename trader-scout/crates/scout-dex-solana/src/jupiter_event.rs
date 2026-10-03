//! Jupiter v6 swap-leg events (ADR-015): `SwapEvent`, `SwapsEvent`,
//! `FeeEvent`, decoded from Anchor event-CPI self-invocations of the Jupiter
//! aggregator program.
//!
//! Wire form: `EVENT_CPI_DISCRIMINATOR (8) ++ event discriminator (8) ++
//! Borsh event`, emitted by Jupiter to itself with the single event-authority
//! account (`D8cy77BB...`). The program-id gate plus the event tag is the
//! whole gate: an Anchor program rejects the tag unless the event-authority
//! PDA signed, so only Jupiter itself can produce it.
//!
//! ## Layouts (and where each comes from)
//!
//! - `SwapEvent` (`sha256("event:SwapEvent")[..8] = 40c6cde8260871e2`) --
//!   2024 pin (`jupiter_v6_idl_12bc5f67.json`) and on-chain IDL, 112 bytes:
//!   `amm, input_mint, input_amount u64, output_mint, output_amount u64`.
//! - `FeeEvent` (`494f4e7fb8d50ddc`) -- pinned IDL, 72 bytes:
//!   `account, mint, amount u64`. No live sample: `IdlOnly`, never a leg.
//! - `SwapsEvent` (`982f4eebc0606e6a` = `sha256("event:SwapsEvent")[..8]`) --
//!   **not in the 2024 pin**, but **defined by the on-chain IDL** pinned
//!   2026-10-03 (`jupiter_v6_onchain_idl_2026-10-03.json`, sha256
//!   `12a08561...cdca8`): `SwapsEvent { swap_events: Vec<SwapEventV2> }`,
//!   `SwapEventV2 { input_mint, input_amount u64, output_mint, output_amount
//!   u64, amm }` (note: `amm` LAST, unlike `SwapEvent`). Borsh `Vec` =
//!   `u32 count ++ count * 112` bytes. 110 of 111 live hops (2026-10-02
//!   fixtures) arrive in it with exactly this layout; see the ADR-015 table.
//! - Known non-leg events (names and discriminators only, from the on-chain
//!   IDL): `CandidateSwapResults`, `CandidateSwapQuoteError`,
//!   `BestSwapOutAmountViolation`. They are **never** decoded into legs; they
//!   classify as [`JupiterEventOutcome::KnownNonLegEvent`] (not unknown).
//!
//! `amm` is the **venue program id** (e.g. PumpSwap `pAMMBay6...`, Meteora
//! DLMM `LBUZKhRx...`), not a pool address, in every live sample.
//!
//! Length policy: exact. Any trailing byte, a count that does not match the
//! length, an empty list or more than [`MAX_SWAPS_PER_EVENT`] elements is
//! `Malformed` (never trusted, reported). No live sample had trailing bytes.
//!
//! Evidence role (ADR-015 §2): a leg proves that the transaction swapped
//! `input_mint -> output_mint`. It carries no owner and no consideration.

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, VariantVerification};

/// Jupiter v6 aggregator program id (base58).
pub const JUPITER_V6_PROGRAM_ID: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
/// Jupiter v6 program id bytes.
pub const JUPITER_V6_PROGRAM_ID_BYTES: SolanaPubkey = [
    0x04, 0x79, 0xd5, 0x5b, 0xf2, 0x31, 0xc0, 0x6e, 0xee, 0x74, 0xc5, 0x6e, 0xce, 0x68, 0x15, 0x07,
    0xfd, 0xb1, 0xb2, 0xde, 0xa3, 0xf4, 0x8e, 0x51, 0x02, 0xb1, 0xcd, 0xa2, 0x56, 0xbc, 0x13, 0x8f,
];
/// Event-authority account every live event-CPI passed (`D8cy77BB...`).
pub const JUPITER_EVENT_AUTHORITY_BYTES: SolanaPubkey = [
    0xb4, 0x3f, 0xfa, 0x27, 0xf5, 0xd7, 0xf6, 0x4a, 0x74, 0xc0, 0x9b, 0x1f, 0x29, 0x58, 0x79, 0xde,
    0x4b, 0x09, 0xab, 0x36, 0xdf, 0xc9, 0xdd, 0x51, 0x4b, 0x32, 0x1a, 0xa7, 0xb3, 0x8c, 0xe5, 0xe8,
];
/// `jup-ag/jupiter-cpi` commit of the pinned IDL.
pub const JUPITER_IDL_COMMIT: &str = "12bc5f67b94a2c3edc74d6e721a19442124a0bad";
/// sha256 of the pinned IDL file.
pub const JUPITER_IDL_SHA256: &str =
    "764ea6d71b77458fd33aeb308d6e6bb19e660fc5320c5359f3b9cac96eba5c50";
/// sha256 of the on-chain Anchor IDL pinned 2026-10-03 (authoritative for
/// `SwapsEvent`; `jupiter_v6_onchain_idl_2026-10-03.json`).
pub const JUPITER_ONCHAIN_IDL_SHA256: &str =
    "12a0856158b2b6927d683a2ba21566f82e39989aca476e23c02d845fc38cdca8";

/// `SwapEvent` discriminator.
pub const JUPITER_SWAP_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
/// `FeeEvent` discriminator.
pub const JUPITER_FEE_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x49, 0x4f, 0x4e, 0x7f, 0xb8, 0xd5, 0x0d, 0xdc];
/// `SwapsEvent` discriminator (not in the pinned IDL).
pub const JUPITER_SWAPS_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x98, 0x2f, 0x4e, 0xeb, 0xc0, 0x60, 0x6e, 0x6a];
/// `CandidateSwapResults` discriminator (on-chain IDL; never a leg).
pub const JUPITER_CANDIDATE_SWAP_RESULTS_DISCRIMINATOR: [u8; 8] =
    [45, 9, 244, 30, 229, 52, 168, 123];
/// `CandidateSwapQuoteError` discriminator (on-chain IDL; never a leg).
pub const JUPITER_CANDIDATE_SWAP_QUOTE_ERROR_DISCRIMINATOR: [u8; 8] =
    [248, 134, 37, 55, 145, 177, 114, 79];
/// `BestSwapOutAmountViolation` discriminator (on-chain IDL; never a leg).
pub const JUPITER_BEST_SWAP_OUT_AMOUNT_VIOLATION_DISCRIMINATOR: [u8; 8] =
    [124, 66, 196, 51, 218, 173, 46, 93];
/// Event-CPI header: tag + event discriminator.
pub const JUPITER_EVENT_HEADER_LEN: usize = 16;
/// Bytes of one swap element (`SwapEvent` payload or one `SwapsEvent` item).
pub const JUPITER_SWAP_ITEM_LEN: usize = 112;
/// `FeeEvent` payload length.
pub const JUPITER_FEE_EVENT_LEN: usize = 72;
/// Upper bound on elements of one `SwapsEvent` (a route has far fewer hops).
pub const MAX_SWAPS_PER_EVENT: usize = 64;

/// Which wire event carried a leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JupiterEventKind {
    /// IDL `SwapEvent` (one hop).
    SwapEvent,
    /// `SwapsEvent` (all hops of a route; layout from live data).
    SwapsEvent,
}

impl JupiterEventKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SwapEvent => "SwapEvent",
            Self::SwapsEvent => "SwapsEvent",
        }
    }

    /// Evidence level (ADR-009/ADR-015). Both are verified by the live
    /// fixtures reconciled in `scout-engine/tests/jupiter_swap_legs.rs`;
    /// `SwapsEvent` is absent from the 2024 pin but defined by the on-chain
    /// IDL of 2026-10-03 (see module docs).
    #[must_use]
    pub const fn verification(self) -> VariantVerification {
        VariantVerification::FixtureVerified
    }
}

/// One executed hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JupiterSwapLeg {
    /// Venue program id as reported by the event (`amm`).
    pub amm: SolanaPubkey,
    pub input_mint: SolanaPubkey,
    pub input_amount: u64,
    pub output_mint: SolanaPubkey,
    pub output_amount: u64,
}

/// A decoded `FeeEvent` (IDL only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JupiterFeeEvent {
    pub account: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub amount: u64,
}

/// Classification of one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JupiterEventOutcome {
    /// Program id is not Jupiter v6.
    NotMine,
    /// Jupiter instruction that is not an event-CPI.
    NotEventCpi,
    /// Swap legs in execution order.
    Swaps {
        kind: JupiterEventKind,
        legs: Vec<JupiterSwapLeg>,
        instruction_index: u32,
    },
    Fee(JupiterFeeEvent),
    /// Event of the on-chain IDL that is not a swap leg (name only; its
    /// payload is not decoded).
    KnownNonLegEvent {
        name: &'static str,
    },
    /// Event-CPI with a discriminator outside the known set. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Program-id-gated Jupiter event decoder (no state).
#[derive(Debug, Clone, Copy, Default)]
pub struct JupiterEventDecoder;

impl JupiterEventDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// `true` iff the instruction belongs to Jupiter v6.
    #[must_use]
    pub fn is_program(&self, instruction: &RawSolanaInstruction) -> bool {
        instruction.program_id == JUPITER_V6_PROGRAM_ID_BYTES
    }

    #[must_use]
    pub fn classify(&self, instruction: &RawSolanaInstruction) -> JupiterEventOutcome {
        if !self.is_program(instruction) {
            return JupiterEventOutcome::NotMine;
        }
        classify_jupiter_event(instruction)
    }
}

fn read_pubkey(buf: &[u8], pos: usize) -> Option<SolanaPubkey> {
    buf.get(pos..pos.checked_add(32)?)?.try_into().ok()
}

fn read_u64(buf: &[u8], pos: usize) -> Option<u64> {
    let a: [u8; 8] = buf.get(pos..pos.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(a))
}

/// IDL `SwapEvent` element: `amm, input_mint, input_amount, output_mint, output_amount`.
fn idl_leg(b: &[u8]) -> Option<JupiterSwapLeg> {
    Some(JupiterSwapLeg {
        amm: read_pubkey(b, 0)?,
        input_mint: read_pubkey(b, 32)?,
        input_amount: read_u64(b, 64)?,
        output_mint: read_pubkey(b, 72)?,
        output_amount: read_u64(b, 104)?,
    })
}

/// `SwapsEvent` element: `input_mint, input_amount, output_mint, output_amount, amm`.
fn swaps_item_leg(b: &[u8]) -> Option<JupiterSwapLeg> {
    Some(JupiterSwapLeg {
        input_mint: read_pubkey(b, 0)?,
        input_amount: read_u64(b, 32)?,
        output_mint: read_pubkey(b, 40)?,
        output_amount: read_u64(b, 72)?,
        amm: read_pubkey(b, 80)?,
    })
}

/// Events of the on-chain IDL that carry no swap leg.
fn known_non_leg_name(disc: &[u8; 8]) -> Option<&'static str> {
    if *disc == JUPITER_CANDIDATE_SWAP_RESULTS_DISCRIMINATOR {
        Some("CandidateSwapResults")
    } else if *disc == JUPITER_CANDIDATE_SWAP_QUOTE_ERROR_DISCRIMINATOR {
        Some("CandidateSwapQuoteError")
    } else if *disc == JUPITER_BEST_SWAP_OUT_AMOUNT_VIOLATION_DISCRIMINATOR {
        Some("BestSwapOutAmountViolation")
    } else {
        None
    }
}

fn malformed(reason: String) -> JupiterEventOutcome {
    JupiterEventOutcome::Malformed { reason }
}

/// Classify one instruction ASSUMED to belong to Jupiter (no program check).
#[must_use]
pub fn classify_jupiter_event(instruction: &RawSolanaInstruction) -> JupiterEventOutcome {
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return JupiterEventOutcome::NotEventCpi;
    }
    let Some(disc) = data
        .get(8..JUPITER_EVENT_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return malformed(format!(
            "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
            data.len()
        ));
    };
    let payload = data.get(JUPITER_EVENT_HEADER_LEN..).unwrap_or_default();
    let known = disc == JUPITER_SWAP_EVENT_DISCRIMINATOR
        || disc == JUPITER_SWAPS_EVENT_DISCRIMINATOR
        || disc == JUPITER_FEE_EVENT_DISCRIMINATOR
        || known_non_leg_name(&disc).is_some();
    if !known {
        return JupiterEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    }
    if instruction.accounts.len() != 1
        || instruction.accounts.first() != Some(&JUPITER_EVENT_AUTHORITY_BYTES)
    {
        return malformed(format!(
            "event-CPI has {} accounts or an event authority other than D8cy77BB...",
            instruction.accounts.len()
        ));
    }
    let idx = instruction.instruction_index;
    if let Some(name) = known_non_leg_name(&disc) {
        return JupiterEventOutcome::KnownNonLegEvent { name };
    }
    if disc == JUPITER_SWAP_EVENT_DISCRIMINATOR {
        if payload.len() != JUPITER_SWAP_ITEM_LEN {
            return malformed(format!(
                "SwapEvent payload has {} bytes, expected exactly {JUPITER_SWAP_ITEM_LEN}",
                payload.len()
            ));
        }
        return match idl_leg(payload) {
            Some(leg) => JupiterEventOutcome::Swaps {
                kind: JupiterEventKind::SwapEvent,
                legs: vec![leg],
                instruction_index: idx,
            },
            None => malformed("SwapEvent fields do not fit the payload".to_owned()),
        };
    }
    if disc == JUPITER_FEE_EVENT_DISCRIMINATOR {
        if payload.len() != JUPITER_FEE_EVENT_LEN {
            return malformed(format!(
                "FeeEvent payload has {} bytes, expected exactly {JUPITER_FEE_EVENT_LEN}",
                payload.len()
            ));
        }
        return match (
            read_pubkey(payload, 0),
            read_pubkey(payload, 32),
            read_u64(payload, 64),
        ) {
            (Some(account), Some(mint), Some(amount)) => {
                JupiterEventOutcome::Fee(JupiterFeeEvent {
                    account,
                    mint,
                    amount,
                })
            }
            _ => malformed("FeeEvent fields do not fit the payload".to_owned()),
        };
    }
    // SwapsEvent: u32 count ++ count * 112.
    let Some(count) = payload
        .get(0..4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
        .and_then(|c| usize::try_from(c).ok())
    else {
        return malformed("SwapsEvent payload shorter than its 4-byte count".to_owned());
    };
    if count == 0 || count > MAX_SWAPS_PER_EVENT {
        return malformed(format!(
            "SwapsEvent count {count} outside 1..={MAX_SWAPS_PER_EVENT}"
        ));
    }
    let expected = count
        .checked_mul(JUPITER_SWAP_ITEM_LEN)
        .and_then(|n| n.checked_add(4));
    if expected != Some(payload.len()) {
        return malformed(format!(
            "SwapsEvent payload has {} bytes, count {count} implies {expected:?} (trailing or missing bytes)",
            payload.len()
        ));
    }
    let mut legs = Vec::with_capacity(count);
    for i in 0..count {
        let start = 4 + i * JUPITER_SWAP_ITEM_LEN;
        let leg = payload
            .get(start..start + JUPITER_SWAP_ITEM_LEN)
            .and_then(swaps_item_leg);
        match leg {
            Some(l) => legs.push(l),
            None => return malformed("SwapsEvent element does not fit the payload".to_owned()),
        }
    }
    JupiterEventOutcome::Swaps {
        kind: JupiterEventKind::SwapsEvent,
        legs,
        instruction_index: idx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn ix(data: Vec<u8>, accounts: Vec<SolanaPubkey>) -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: JUPITER_V6_PROGRAM_ID_BYTES,
            accounts,
            data,
            instruction_index: 4,
        }
    }

    fn header(disc: [u8; 8]) -> Vec<u8> {
        let mut v = EVENT_CPI_DISCRIMINATOR.to_vec();
        v.extend_from_slice(&disc);
        v
    }

    fn b58(s: &str) -> SolanaPubkey {
        bs58::decode(s).into_vec().unwrap().try_into().unwrap()
    }

    fn event_ix(disc: [u8; 8], payload: &[u8]) -> RawSolanaInstruction {
        let mut d = header(disc);
        d.extend_from_slice(payload);
        ix(d, vec![JUPITER_EVENT_AUTHORITY_BYTES])
    }

    fn swaps_payload(legs: &[JupiterSwapLeg]) -> Vec<u8> {
        let mut p = u32::try_from(legs.len()).unwrap().to_le_bytes().to_vec();
        for l in legs {
            p.extend_from_slice(&l.input_mint);
            p.extend_from_slice(&l.input_amount.to_le_bytes());
            p.extend_from_slice(&l.output_mint);
            p.extend_from_slice(&l.output_amount.to_le_bytes());
            p.extend_from_slice(&l.amm);
        }
        p
    }

    fn leg(n: u8) -> JupiterSwapLeg {
        JupiterSwapLeg {
            amm: [n; 32],
            input_mint: [n + 1; 32],
            input_amount: 1000 + u64::from(n),
            output_mint: [n + 2; 32],
            output_amount: 2000 + u64::from(n),
        }
    }

    #[test]
    fn program_id_constants_match_base58() {
        assert_eq!(b58(JUPITER_V6_PROGRAM_ID), JUPITER_V6_PROGRAM_ID_BYTES);
        assert_eq!(
            b58("D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf"),
            JUPITER_EVENT_AUTHORITY_BYTES
        );
    }

    #[test]
    fn program_id_gate() {
        let mut i = event_ix(JUPITER_SWAP_EVENT_DISCRIMINATOR, &[0u8; 112]);
        i.program_id = [9; 32];
        assert_eq!(
            JupiterEventDecoder::new().classify(&i),
            JupiterEventOutcome::NotMine
        );
    }

    #[test]
    fn non_event_cpi_is_not_event() {
        let i = ix(vec![1, 2, 3, 4, 5, 6, 7, 8, 9], vec![]);
        assert_eq!(
            JupiterEventDecoder::new().classify(&i),
            JupiterEventOutcome::NotEventCpi
        );
    }

    #[test]
    fn swaps_event_roundtrip_and_hop_order() {
        let legs = [leg(1), leg(5)];
        let i = event_ix(JUPITER_SWAPS_EVENT_DISCRIMINATOR, &swaps_payload(&legs));
        match JupiterEventDecoder::new().classify(&i) {
            JupiterEventOutcome::Swaps {
                kind,
                legs: got,
                instruction_index,
            } => {
                assert_eq!(kind, JupiterEventKind::SwapsEvent);
                assert_eq!(got, legs);
                assert_eq!(instruction_index, 4);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn idl_swap_event_field_order_is_amm_first() {
        let l = leg(3);
        let mut p = Vec::new();
        p.extend_from_slice(&l.amm);
        p.extend_from_slice(&l.input_mint);
        p.extend_from_slice(&l.input_amount.to_le_bytes());
        p.extend_from_slice(&l.output_mint);
        p.extend_from_slice(&l.output_amount.to_le_bytes());
        let i = event_ix(JUPITER_SWAP_EVENT_DISCRIMINATOR, &p);
        match JupiterEventDecoder::new().classify(&i) {
            JupiterEventOutcome::Swaps { kind, legs, .. } => {
                assert_eq!(kind, JupiterEventKind::SwapEvent);
                assert_eq!(legs, vec![l]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn malformed_lengths_are_never_trusted() {
        let dec = JupiterEventDecoder::new();
        let good = swaps_payload(&[leg(1)]);
        // trailing byte
        let mut t = good.clone();
        t.push(0);
        assert!(matches!(
            dec.classify(&event_ix(JUPITER_SWAPS_EVENT_DISCRIMINATOR, &t)),
            JupiterEventOutcome::Malformed { .. }
        ));
        // truncated
        assert!(matches!(
            dec.classify(&event_ix(
                JUPITER_SWAPS_EVENT_DISCRIMINATOR,
                &good[..good.len() - 1]
            )),
            JupiterEventOutcome::Malformed { .. }
        ));
        // zero and oversized count
        for c in [0u32, 65, u32::MAX] {
            assert!(matches!(
                dec.classify(&event_ix(
                    JUPITER_SWAPS_EVENT_DISCRIMINATOR,
                    &c.to_le_bytes()
                )),
                JupiterEventOutcome::Malformed { .. }
            ));
        }
        // SwapEvent wrong length
        assert!(matches!(
            dec.classify(&event_ix(JUPITER_SWAP_EVENT_DISCRIMINATOR, &[0u8; 113])),
            JupiterEventOutcome::Malformed { .. }
        ));
        // header only
        assert!(matches!(
            dec.classify(&ix(EVENT_CPI_DISCRIMINATOR.to_vec(), vec![])),
            JupiterEventOutcome::Malformed { .. }
        ));
        // wrong event authority
        let mut w = event_ix(JUPITER_SWAP_EVENT_DISCRIMINATOR, &[0u8; 112]);
        w.accounts = vec![[7; 32]];
        assert!(matches!(
            dec.classify(&w),
            JupiterEventOutcome::Malformed { .. }
        ));
    }

    #[test]
    fn unknown_discriminator_is_reported() {
        let i = event_ix([1, 2, 3, 4, 5, 6, 7, 8], &[]);
        assert_eq!(
            JupiterEventDecoder::new().classify(&i),
            JupiterEventOutcome::UnknownEvent {
                discriminator: [1, 2, 3, 4, 5, 6, 7, 8]
            }
        );
    }

    #[test]
    fn fee_event_decodes() {
        let mut p = vec![1u8; 32];
        p.extend_from_slice(&[2u8; 32]);
        p.extend_from_slice(&77u64.to_le_bytes());
        let i = event_ix(JUPITER_FEE_EVENT_DISCRIMINATOR, &p);
        assert_eq!(
            JupiterEventDecoder::new().classify(&i),
            JupiterEventOutcome::Fee(JupiterFeeEvent {
                account: [1; 32],
                mint: [2; 32],
                amount: 77
            })
        );
    }

    fn idl() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/p0/measurements/fixtures/jupiter_v6_idl_12bc5f67.json"
        );
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), JUPITER_IDL_SHA256);
        serde_json::from_slice(&bytes).unwrap()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn disc(name: &str) -> [u8; 8] {
        let h = Sha256::digest(format!("event:{name}").as_bytes());
        h[..8].try_into().unwrap()
    }

    fn field_layout(event: &serde_json::Value) -> Vec<(String, String)> {
        event["fields"]
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
    fn idl_equality_discriminators_and_field_order() {
        let idl = idl();
        let events = idl["events"].as_array().unwrap();
        let find = |n: &str| events.iter().find(|e| e["name"] == n).unwrap();
        assert_eq!(disc("SwapEvent"), JUPITER_SWAP_EVENT_DISCRIMINATOR);
        assert_eq!(disc("FeeEvent"), JUPITER_FEE_EVENT_DISCRIMINATOR);
        // SwapsEvent is not in the pinned IDL; its discriminator is the
        // Anchor derivation of the name (evidence is live data only).
        assert_eq!(disc("SwapsEvent"), JUPITER_SWAPS_EVENT_DISCRIMINATOR);
        assert!(events.iter().all(|e| e["name"] != "SwapsEvent"));
        let p = |n: &str, t: &str| (n.to_owned(), t.to_owned());
        assert_eq!(
            field_layout(find("SwapEvent")),
            vec![
                p("amm", "publicKey"),
                p("inputMint", "publicKey"),
                p("inputAmount", "u64"),
                p("outputMint", "publicKey"),
                p("outputAmount", "u64"),
            ]
        );
        assert_eq!(
            field_layout(find("FeeEvent")),
            vec![
                p("account", "publicKey"),
                p("mint", "publicKey"),
                p("amount", "u64"),
            ]
        );
        assert_eq!(32 + 32 + 8 + 32 + 8, JUPITER_SWAP_ITEM_LEN);
        assert_eq!(32 + 32 + 8, JUPITER_FEE_EVENT_LEN);
        assert_eq!(events.len(), 2);
    }
    // ---- on-chain IDL (2026-10-03) equality ----

    fn onchain_idl() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/p0/measurements/fixtures/jupiter_v6_onchain_idl_2026-10-03.json"
        );
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(hex(&Sha256::digest(&bytes)), JUPITER_ONCHAIN_IDL_SHA256);
        serde_json::from_slice(&bytes).unwrap()
    }

    fn by_name<'a>(list: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} not in the on-chain IDL"))
    }

    fn idl_disc(event: &serde_json::Value) -> Vec<u8> {
        event["discriminator"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect()
    }

    /// `(name, type)` of a struct's fields, the type rendered as `pubkey`,
    /// `u64` or `vec<Name>`.
    fn onchain_layout(types: &serde_json::Value, name: &str) -> Vec<(String, String)> {
        let t = by_name(types, name);
        assert_eq!(t["type"]["kind"], "struct", "{name}");
        t["type"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let ty = &f["type"];
                let rendered = match ty.as_str() {
                    Some(s) => s.to_owned(),
                    None => format!("vec<{}>", ty["vec"]["defined"]["name"].as_str().unwrap()),
                };
                (f["name"].as_str().unwrap().to_owned(), rendered)
            })
            .collect()
    }

    #[test]
    fn onchain_idl_equality_discriminators() {
        let idl = onchain_idl();
        let events = &idl["events"];
        assert_eq!(idl["address"], JUPITER_V6_PROGRAM_ID);
        for (name, constant) in [
            ("SwapEvent", JUPITER_SWAP_EVENT_DISCRIMINATOR),
            ("SwapsEvent", JUPITER_SWAPS_EVENT_DISCRIMINATOR),
            ("FeeEvent", JUPITER_FEE_EVENT_DISCRIMINATOR),
            (
                "CandidateSwapResults",
                JUPITER_CANDIDATE_SWAP_RESULTS_DISCRIMINATOR,
            ),
            (
                "CandidateSwapQuoteError",
                JUPITER_CANDIDATE_SWAP_QUOTE_ERROR_DISCRIMINATOR,
            ),
            (
                "BestSwapOutAmountViolation",
                JUPITER_BEST_SWAP_OUT_AMOUNT_VIOLATION_DISCRIMINATOR,
            ),
        ] {
            assert_eq!(idl_disc(by_name(events, name)), constant, "{name} (IDL)");
            assert_eq!(disc(name), constant, "{name} (sha256)");
        }
        // The IDL defines exactly these six events.
        assert_eq!(events.as_array().unwrap().len(), 6);
    }

    #[test]
    fn onchain_idl_equality_swaps_event_layout() {
        let idl = onchain_idl();
        let types = &idl["types"];
        let p = |n: &str, t: &str| (n.to_owned(), t.to_owned());
        // SwapsEvent { swap_events: Vec<SwapEventV2> }
        assert_eq!(
            onchain_layout(types, "SwapsEvent"),
            vec![p("swap_events", "vec<SwapEventV2>")]
        );
        // SwapEventV2: the element order `swaps_item_leg` reads (amm LAST).
        assert_eq!(
            onchain_layout(types, "SwapEventV2"),
            vec![
                p("input_mint", "pubkey"),
                p("input_amount", "u64"),
                p("output_mint", "pubkey"),
                p("output_amount", "u64"),
                p("amm", "pubkey"),
            ]
        );
        // SwapEvent (single hop): amm FIRST, as `idl_leg` reads it.
        assert_eq!(
            onchain_layout(types, "SwapEvent"),
            vec![
                p("amm", "pubkey"),
                p("input_mint", "pubkey"),
                p("input_amount", "u64"),
                p("output_mint", "pubkey"),
                p("output_amount", "u64"),
            ]
        );
        assert_eq!(
            onchain_layout(types, "FeeEvent"),
            vec![
                p("account", "pubkey"),
                p("mint", "pubkey"),
                p("amount", "u64"),
            ]
        );
        assert_eq!(32 + 8 + 32 + 8 + 32, JUPITER_SWAP_ITEM_LEN);
        assert_eq!(32 + 32 + 8, JUPITER_FEE_EVENT_LEN);
    }

    #[test]
    fn onchain_idl_swaps_event_item_bytes_match_the_decoder() {
        // Build the item from the IDL's own field order and decode it.
        let idl = onchain_idl();
        let layout = onchain_layout(&idl["types"], "SwapEventV2");
        let l = leg(9);
        let mut item = Vec::new();
        for (name, _) in &layout {
            match name.as_str() {
                "input_mint" => item.extend_from_slice(&l.input_mint),
                "input_amount" => item.extend_from_slice(&l.input_amount.to_le_bytes()),
                "output_mint" => item.extend_from_slice(&l.output_mint),
                "output_amount" => item.extend_from_slice(&l.output_amount.to_le_bytes()),
                "amm" => item.extend_from_slice(&l.amm),
                other => panic!("unexpected field {other}"),
            }
        }
        let mut payload = 1u32.to_le_bytes().to_vec();
        payload.extend(item);
        match JupiterEventDecoder::new()
            .classify(&event_ix(JUPITER_SWAPS_EVENT_DISCRIMINATOR, &payload))
        {
            JupiterEventOutcome::Swaps { kind, legs, .. } => {
                assert_eq!(kind, JupiterEventKind::SwapsEvent);
                assert_eq!(legs, vec![l]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn known_non_leg_events_are_named_never_legs_never_unknown() {
        let dec = JupiterEventDecoder::new();
        for (name, d) in [
            (
                "CandidateSwapResults",
                JUPITER_CANDIDATE_SWAP_RESULTS_DISCRIMINATOR,
            ),
            (
                "CandidateSwapQuoteError",
                JUPITER_CANDIDATE_SWAP_QUOTE_ERROR_DISCRIMINATOR,
            ),
            (
                "BestSwapOutAmountViolation",
                JUPITER_BEST_SWAP_OUT_AMOUNT_VIOLATION_DISCRIMINATOR,
            ),
        ] {
            // Whatever the payload (even one shaped like a swap), no leg.
            for payload in [&[][..], &[0u8; 112][..], &swaps_payload(&[leg(1)])[..]] {
                assert_eq!(
                    dec.classify(&event_ix(d, payload)),
                    JupiterEventOutcome::KnownNonLegEvent { name }
                );
            }
            // Still event-authority gated.
            let mut wrong = event_ix(d, &[]);
            wrong.accounts = vec![[7; 32]];
            assert!(matches!(
                dec.classify(&wrong),
                JupiterEventOutcome::Malformed { .. }
            ));
        }
    }
}
