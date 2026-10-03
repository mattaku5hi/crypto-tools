//! DFlow Aggregator v4 swap-leg events (ADR-015 amendment): `SwapEvent` and
//! `FeeEvent`, decoded from Anchor event-CPI self-invocations of the DFlow
//! aggregator program `DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH`.
//!
//! Wire form: `EVENT_CPI_DISCRIMINATOR (8) ++ event discriminator (8) ++
//! Borsh event`, emitted by the program to itself with the single
//! event-authority account (`8xeaWCsJ...`, derived from the committed
//! fixtures: 281 of 281 live event-CPIs). Program id + event tag + authority
//! is the whole runtime gate.
//!
//! ## Schema source (third-party, NOT official)
//!
//! `sevenlabs-hq/carbon` @ `1e6e16b46e0efb0fc9cd6c8684ed9721e7da716a`,
//! `decoders/dflow-aggregator-v4-decoder/src/{types,events}/{swap_event,fee_event}.rs`
//! (Codama-generated; the tree has no IDL json). Pinned as
//! `fixtures/dflow_aggregator_v4_carbon_1e6e16b_events.rs.txt`. The program
//! identity is "DFlow Aggregator v4" per that decoder and solanacompass.
//!
//! - `SwapEvent` (`sha256("event:SwapEvent")[..8] = 40c6cde8260871e2`,
//!   equals the Carbon constant `[64,198,205,232,38,8,113,226]`), 112 bytes:
//!   `amm, input_mint, input_amount u64, output_mint, output_amount u64`
//!   (same layout as Jupiter's IDL `SwapEvent`; the Carbon decoder does not
//!   check for trailing bytes, this decoder does).
//! - `FeeEvent` (`494f4e7fb8d50ddc`, equals `[73,79,78,127,184,213,13,220]`),
//!   72 bytes: `account, mint, amount u64`. Live samples exist (88-byte
//!   instruction) but a fee is never a swap leg.
//!
//! No batched `SwapsEvent`-like variant and no other event discriminator was
//! seen live; any other discriminator is `UnknownEvent` (coverage gap).
//!
//! Length policy: exact. Any trailing or missing byte is `Malformed`.
//!
//! Evidence role (ADR-015): a leg proves that the transaction swapped
//! `input_mint -> output_mint` through a venue. It carries no owner and no
//! consideration.

use scout_core::{RawSolanaInstruction, SolanaPubkey};

use crate::bonding_curve_buy::{EVENT_CPI_DISCRIMINATOR, VariantVerification};

/// DFlow Aggregator v4 program id (base58).
pub const DFLOW_V4_PROGRAM_ID: &str = "DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH";
/// DFlow Aggregator v4 program id bytes.
pub const DFLOW_V4_PROGRAM_ID_BYTES: SolanaPubkey = [
    0xb5, 0xe3, 0x4a, 0x14, 0xe2, 0xbc, 0x73, 0x48, 0x69, 0x0e, 0xe1, 0xf5, 0xaf, 0x5d, 0xee, 0xd6,
    0x55, 0x38, 0x40, 0xa3, 0x6d, 0xaa, 0xb8, 0x60, 0xb0, 0x50, 0x60, 0x73, 0xbd, 0xc0, 0x3c, 0x10,
];
/// Event-authority account every live event-CPI passed (`8xeaWCsJ...`).
pub const DFLOW_EVENT_AUTHORITY_BYTES: SolanaPubkey = [
    0x76, 0x43, 0x3d, 0xfe, 0x7e, 0xee, 0xfd, 0x2a, 0x1d, 0x31, 0xfe, 0x0a, 0x1d, 0x44, 0xe5, 0x46,
    0x35, 0x08, 0x34, 0x99, 0x82, 0x9a, 0x59, 0xda, 0x2a, 0x15, 0x98, 0x28, 0x13, 0xc6, 0xde, 0x78,
];
/// `sevenlabs-hq/carbon` commit of the pinned schema source.
pub const DFLOW_SCHEMA_COMMIT: &str = "1e6e16b46e0efb0fc9cd6c8684ed9721e7da716a";
/// sha256 of the pinned schema-source file.
pub const DFLOW_SCHEMA_SHA256: &str =
    "7322e9994797cdd6ad5a1d7fde9cafb4fcb261400d360a0137f921ec3cfa1252";

/// `SwapEvent` discriminator.
pub const DFLOW_SWAP_EVENT_DISCRIMINATOR: [u8; 8] =
    [0x40, 0xc6, 0xcd, 0xe8, 0x26, 0x08, 0x71, 0xe2];
/// `FeeEvent` discriminator.
pub const DFLOW_FEE_EVENT_DISCRIMINATOR: [u8; 8] = [0x49, 0x4f, 0x4e, 0x7f, 0xb8, 0xd5, 0x0d, 0xdc];
/// Event-CPI header: tag + event discriminator.
pub const DFLOW_EVENT_HEADER_LEN: usize = 16;
/// `SwapEvent` payload length.
pub const DFLOW_SWAP_EVENT_LEN: usize = 112;
/// `FeeEvent` payload length.
pub const DFLOW_FEE_EVENT_LEN: usize = 72;

/// Evidence level of the `SwapEvent` (ADR-009/ADR-015 amendment): verified by
/// the live fixtures reconciled in `scout-engine/tests/dflow_swap_legs.rs`.
#[must_use]
pub const fn dflow_swap_event_verification() -> VariantVerification {
    VariantVerification::FixtureVerified
}

/// One executed hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DflowSwapLeg {
    /// Venue as reported by the event (`amm`).
    pub amm: SolanaPubkey,
    pub input_mint: SolanaPubkey,
    pub input_amount: u64,
    pub output_mint: SolanaPubkey,
    pub output_amount: u64,
}

/// A decoded `FeeEvent` (never a leg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DflowFeeEvent {
    pub account: SolanaPubkey,
    pub mint: SolanaPubkey,
    pub amount: u64,
}

/// Classification of one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DflowEventOutcome {
    /// Program id is not DFlow Aggregator v4.
    NotMine,
    /// DFlow instruction that is not an event-CPI.
    NotEventCpi,
    /// One executed hop.
    Swap {
        leg: DflowSwapLeg,
        instruction_index: u32,
    },
    Fee(DflowFeeEvent),
    /// Event-CPI with a discriminator outside the known set. COVERAGE GAP.
    UnknownEvent {
        discriminator: [u8; 8],
    },
    /// Event-CPI with a broken structure. COVERAGE GAP.
    Malformed {
        reason: String,
    },
}

/// Program-id-gated DFlow event decoder (no state).
#[derive(Debug, Clone, Copy, Default)]
pub struct DflowEventDecoder;

impl DflowEventDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// `true` iff the instruction belongs to DFlow Aggregator v4.
    #[must_use]
    pub fn is_program(&self, instruction: &RawSolanaInstruction) -> bool {
        instruction.program_id == DFLOW_V4_PROGRAM_ID_BYTES
    }

    #[must_use]
    pub fn classify(&self, instruction: &RawSolanaInstruction) -> DflowEventOutcome {
        if !self.is_program(instruction) {
            return DflowEventOutcome::NotMine;
        }
        classify_dflow_event(instruction)
    }
}

fn read_pubkey(buf: &[u8], pos: usize) -> Option<SolanaPubkey> {
    buf.get(pos..pos.checked_add(32)?)?.try_into().ok()
}

fn read_u64(buf: &[u8], pos: usize) -> Option<u64> {
    let a: [u8; 8] = buf.get(pos..pos.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(a))
}

fn malformed(reason: String) -> DflowEventOutcome {
    DflowEventOutcome::Malformed { reason }
}

/// Classify one instruction ASSUMED to belong to DFlow (no program check).
#[must_use]
pub fn classify_dflow_event(instruction: &RawSolanaInstruction) -> DflowEventOutcome {
    let data = &instruction.data;
    if data.get(0..8) != Some(EVENT_CPI_DISCRIMINATOR.as_slice()) {
        return DflowEventOutcome::NotEventCpi;
    }
    let Some(disc) = data
        .get(8..DFLOW_EVENT_HEADER_LEN)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
    else {
        return malformed(format!(
            "event-CPI instruction has {} data bytes, fewer than the 16-byte header",
            data.len()
        ));
    };
    let payload = data.get(DFLOW_EVENT_HEADER_LEN..).unwrap_or_default();
    if disc != DFLOW_SWAP_EVENT_DISCRIMINATOR && disc != DFLOW_FEE_EVENT_DISCRIMINATOR {
        return DflowEventOutcome::UnknownEvent {
            discriminator: disc,
        };
    }
    if instruction.accounts.len() != 1
        || instruction.accounts.first() != Some(&DFLOW_EVENT_AUTHORITY_BYTES)
    {
        return malformed(format!(
            "event-CPI has {} accounts or an event authority other than 8xeaWCsJ...",
            instruction.accounts.len()
        ));
    }
    if disc == DFLOW_SWAP_EVENT_DISCRIMINATOR {
        if payload.len() != DFLOW_SWAP_EVENT_LEN {
            return malformed(format!(
                "SwapEvent payload has {} bytes, expected exactly {DFLOW_SWAP_EVENT_LEN}",
                payload.len()
            ));
        }
        return match (
            read_pubkey(payload, 0),
            read_pubkey(payload, 32),
            read_u64(payload, 64),
            read_pubkey(payload, 72),
            read_u64(payload, 104),
        ) {
            (
                Some(amm),
                Some(input_mint),
                Some(input_amount),
                Some(output_mint),
                Some(output_amount),
            ) => DflowEventOutcome::Swap {
                leg: DflowSwapLeg {
                    amm,
                    input_mint,
                    input_amount,
                    output_mint,
                    output_amount,
                },
                instruction_index: instruction.instruction_index,
            },
            _ => malformed("SwapEvent fields do not fit the payload".to_owned()),
        };
    }
    if payload.len() != DFLOW_FEE_EVENT_LEN {
        return malformed(format!(
            "FeeEvent payload has {} bytes, expected exactly {DFLOW_FEE_EVENT_LEN}",
            payload.len()
        ));
    }
    match (
        read_pubkey(payload, 0),
        read_pubkey(payload, 32),
        read_u64(payload, 64),
    ) {
        (Some(account), Some(mint), Some(amount)) => DflowEventOutcome::Fee(DflowFeeEvent {
            account,
            mint,
            amount,
        }),
        _ => malformed("FeeEvent fields do not fit the payload".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn ix(data: Vec<u8>, accounts: Vec<SolanaPubkey>) -> RawSolanaInstruction {
        RawSolanaInstruction {
            program_id: DFLOW_V4_PROGRAM_ID_BYTES,
            accounts,
            data,
            instruction_index: 4,
        }
    }

    fn b58(s: &str) -> SolanaPubkey {
        bs58::decode(s).into_vec().unwrap().try_into().unwrap()
    }

    fn event_ix(disc: [u8; 8], payload: &[u8]) -> RawSolanaInstruction {
        let mut d = EVENT_CPI_DISCRIMINATOR.to_vec();
        d.extend_from_slice(&disc);
        d.extend_from_slice(payload);
        ix(d, vec![DFLOW_EVENT_AUTHORITY_BYTES])
    }

    fn swap_payload(l: &DflowSwapLeg) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&l.amm);
        p.extend_from_slice(&l.input_mint);
        p.extend_from_slice(&l.input_amount.to_le_bytes());
        p.extend_from_slice(&l.output_mint);
        p.extend_from_slice(&l.output_amount.to_le_bytes());
        p
    }

    fn leg(n: u8) -> DflowSwapLeg {
        DflowSwapLeg {
            amm: [n; 32],
            input_mint: [n + 1; 32],
            input_amount: 1000 + u64::from(n),
            output_mint: [n + 2; 32],
            output_amount: 2000 + u64::from(n),
        }
    }

    fn disc(name: &str) -> [u8; 8] {
        let h = Sha256::digest(format!("event:{name}").as_bytes());
        h[..8].try_into().unwrap()
    }

    #[test]
    fn constants_match_base58_and_anchor_derivation() {
        assert_eq!(b58(DFLOW_V4_PROGRAM_ID), DFLOW_V4_PROGRAM_ID_BYTES);
        assert_eq!(
            b58("8xeaWCsJYxRoudEZGJWURdfrtFhLYZz9b4iHJnW5tb3d"),
            DFLOW_EVENT_AUTHORITY_BYTES
        );
        assert_eq!(disc("SwapEvent"), DFLOW_SWAP_EVENT_DISCRIMINATOR);
        assert_eq!(disc("FeeEvent"), DFLOW_FEE_EVENT_DISCRIMINATOR);
        // The Carbon constants, as printed in the pinned source.
        assert_eq!(
            DFLOW_SWAP_EVENT_DISCRIMINATOR,
            [64, 198, 205, 232, 38, 8, 113, 226]
        );
        assert_eq!(
            DFLOW_FEE_EVENT_DISCRIMINATOR,
            [73, 79, 78, 127, 184, 213, 13, 220]
        );
        assert_eq!(32 + 32 + 8 + 32 + 8, DFLOW_SWAP_EVENT_LEN);
        assert_eq!(32 + 32 + 8, DFLOW_FEE_EVENT_LEN);
    }

    #[test]
    fn pinned_schema_source_matches_constants() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/p0/measurements/fixtures/dflow_aggregator_v4_carbon_1e6e16b_events.rs.txt"
        );
        let bytes = std::fs::read(path).unwrap();
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        assert_eq!(hex, DFLOW_SCHEMA_SHA256);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("[64, 198, 205, 232, 38, 8, 113, 226]"));
        assert!(text.contains("[73, 79, 78, 127, 184, 213, 13, 220]"));
        // Field order of both structs (amm first).
        let swap = text.find("pub struct SwapEvent {").unwrap();
        let body = &text[swap..];
        let pos = |s: &str| body.find(s).unwrap();
        assert!(pos("pub amm") < pos("pub input_mint"));
        assert!(pos("pub input_mint") < pos("pub input_amount: u64"));
        assert!(pos("pub input_amount") < pos("pub output_mint"));
        assert!(pos("pub output_mint") < pos("pub output_amount: u64"));
    }

    #[test]
    fn program_id_gate() {
        let mut i = event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, &[0u8; 112]);
        i.program_id = [9; 32];
        assert_eq!(
            DflowEventDecoder::new().classify(&i),
            DflowEventOutcome::NotMine
        );
    }

    #[test]
    fn non_event_cpi_is_not_event() {
        let i = ix(vec![1, 2, 3, 4, 5, 6, 7, 8, 9], vec![]);
        assert_eq!(
            DflowEventDecoder::new().classify(&i),
            DflowEventOutcome::NotEventCpi
        );
    }

    #[test]
    fn swap_event_roundtrip_amm_first() {
        let l = leg(3);
        let i = event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, &swap_payload(&l));
        assert_eq!(
            DflowEventDecoder::new().classify(&i),
            DflowEventOutcome::Swap {
                leg: l,
                instruction_index: 4
            }
        );
    }

    #[test]
    fn fee_event_decodes() {
        let mut p = vec![1u8; 32];
        p.extend_from_slice(&[2u8; 32]);
        p.extend_from_slice(&77u64.to_le_bytes());
        let i = event_ix(DFLOW_FEE_EVENT_DISCRIMINATOR, &p);
        assert_eq!(
            DflowEventDecoder::new().classify(&i),
            DflowEventOutcome::Fee(DflowFeeEvent {
                account: [1; 32],
                mint: [2; 32],
                amount: 77
            })
        );
    }

    #[test]
    fn malformed_is_never_trusted() {
        let dec = DflowEventDecoder::new();
        let good = swap_payload(&leg(1));
        let mut t = good.clone();
        t.push(0);
        for p in [&t[..], &good[..111], &[][..]] {
            assert!(matches!(
                dec.classify(&event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, p)),
                DflowEventOutcome::Malformed { .. }
            ));
        }
        for n in [71usize, 73] {
            assert!(matches!(
                dec.classify(&event_ix(DFLOW_FEE_EVENT_DISCRIMINATOR, &vec![0u8; n])),
                DflowEventOutcome::Malformed { .. }
            ));
        }
        // header only
        assert!(matches!(
            dec.classify(&ix(EVENT_CPI_DISCRIMINATOR.to_vec(), vec![])),
            DflowEventOutcome::Malformed { .. }
        ));
        // wrong / extra event authority
        let mut w = event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, &good);
        w.accounts = vec![[7; 32]];
        assert!(matches!(
            dec.classify(&w),
            DflowEventOutcome::Malformed { .. }
        ));
        let mut w = event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, &good);
        w.accounts = vec![DFLOW_EVENT_AUTHORITY_BYTES, DFLOW_EVENT_AUTHORITY_BYTES];
        assert!(matches!(
            dec.classify(&w),
            DflowEventOutcome::Malformed { .. }
        ));
        // Jupiter's authority is not DFlow's.
        let mut w = event_ix(DFLOW_SWAP_EVENT_DISCRIMINATOR, &good);
        w.accounts = vec![crate::JUPITER_EVENT_AUTHORITY_BYTES];
        assert!(matches!(
            dec.classify(&w),
            DflowEventOutcome::Malformed { .. }
        ));
    }

    #[test]
    fn unknown_discriminator_is_reported() {
        // Jupiter's batched `SwapsEvent` discriminator is NOT a DFlow event.
        let i = event_ix(crate::JUPITER_SWAPS_EVENT_DISCRIMINATOR, &[]);
        assert_eq!(
            DflowEventDecoder::new().classify(&i),
            DflowEventOutcome::UnknownEvent {
                discriminator: crate::JUPITER_SWAPS_EVENT_DISCRIMINATOR
            }
        );
    }
}
