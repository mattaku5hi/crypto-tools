//! Bounded decode-evidence samples (coverage-gap forensics).
//!
//! Counters such as "2 malformed trade instruction(s)" or "2 trade event(s)
//! not claimed" are useless without a way to find the transaction. For every
//! token (buyer-intersect) and wallet (ledger) the engine keeps at most
//! [`MAX_EVIDENCE_SAMPLES`] samples of: malformed trade instructions,
//! malformed events, unknown discriminators under known programs and
//! orphan events. A sample never carries raw instruction data, only its
//! length and the (bounded, sanitized) decoder reason; chain strings are
//! untrusted input (invariant 17).

use scout_core::{RawSolanaInstruction, RawSolanaTransaction, SolanaPubkey};
use scout_dex_solana::{
    DflowEventDecoder, DflowEventOutcome, JupiterEventDecoder, JupiterEventOutcome,
    PumpAmmInstructionOutcome, PumpEventOutcome, PumpInstructionOutcome, hex8,
};

use crate::solana_wallet_ledger::LedgerDecoders;

/// Samples kept per token / per wallet.
pub const MAX_EVIDENCE_SAMPLES: usize = 5;
/// Upper bound on the bytes of a sample's reason.
const MAX_REASON_BYTES: usize = 200;

/// What kind of coverage gap a sample documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceKind {
    /// Trade discriminator whose structure did not decode.
    MalformedTradeInstruction,
    /// Event-CPI whose structure did not decode.
    MalformedEvent,
    /// Discriminator (instruction or event) of a known program that is in
    /// no table.
    UnknownDiscriminator,
    /// Decoded trade event not claimed by any trade instruction.
    OrphanEvent,
}

impl EvidenceKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MalformedTradeInstruction => "malformed_trade_instruction",
            Self::MalformedEvent => "malformed_event",
            Self::UnknownDiscriminator => "unknown_discriminator",
            Self::OrphanEvent => "orphan_event",
        }
    }
}

/// One evidence sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeEvidence {
    pub kind: EvidenceKind,
    pub signature: [u8; 64],
    pub slot: u64,
    pub transaction_index: u64,
    pub instruction_index: u32,
    pub program: SolanaPubkey,
    /// IDL variant/event name, or the 8-byte discriminator in hex, or
    /// `unknown` when fewer than 8 data bytes exist.
    pub variant_or_discriminator: String,
    pub data_len: usize,
    pub accounts_len: usize,
    pub reason: String,
}

impl DecodeEvidence {
    #[must_use]
    pub fn signature_base58(&self) -> String {
        bs58::encode(self.signature).into_string()
    }

    #[must_use]
    pub fn program_base58(&self) -> String {
        bs58::encode(self.program).into_string()
    }

    /// Friendly label of the program (address label only).
    #[must_use]
    pub fn program_name(&self) -> &'static str {
        program_name(&self.program)
    }

    fn order_key(&self) -> (u64, u64, u32, EvidenceKind, [u8; 64]) {
        (
            self.slot,
            self.transaction_index,
            self.instruction_index,
            self.kind,
            self.signature,
        )
    }
}

/// Address label of the programs the engine decodes.
#[must_use]
pub fn program_name(program: &SolanaPubkey) -> &'static str {
    if *program == scout_dex_solana::PUMP_AMM_PROGRAM_ID_BYTES {
        "pump_amm"
    } else if *program == scout_dex_solana::JUPITER_V6_PROGRAM_ID_BYTES {
        "jupiter_v6"
    } else if *program == scout_dex_solana::DFLOW_V4_PROGRAM_ID_BYTES {
        "dflow_v4"
    } else {
        "pump_curve"
    }
}

/// Merge `from` into `into`: deterministic (canonical chain order), unique,
/// at most [`MAX_EVIDENCE_SAMPLES`] kept.
pub fn merge_evidence<I: IntoIterator<Item = DecodeEvidence>>(
    into: &mut Vec<DecodeEvidence>,
    from: I,
) {
    for e in from {
        if into
            .iter()
            .any(|x| x.order_key() == e.order_key() && x.program == e.program)
        {
            continue;
        }
        into.push(e);
    }
    into.sort_by_key(DecodeEvidence::order_key);
    into.truncate(MAX_EVIDENCE_SAMPLES);
}

fn sanitize_reason(reason: &str) -> String {
    let mut s: String = reason
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if s.len() > MAX_REASON_BYTES {
        let mut cut = MAX_REASON_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

fn discriminator_hex(ix: &RawSolanaInstruction, offset: usize) -> String {
    ix.data
        .get(offset..offset + 8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map_or_else(|| "unknown".to_owned(), |d| hex8(&d))
}

/// Counters and bounded samples of one transaction.
#[derive(Debug, Clone, Default)]
pub(crate) struct TxScan {
    pub evidence: Vec<DecodeEvidence>,
    /// Instructions / events of a known program with a discriminator in no table.
    pub unknown_discriminators: u64,
    pub jupiter_malformed: u64,
    pub jupiter_unknown: u64,
    pub dflow_malformed: u64,
    pub dflow_unknown: u64,
}

impl TxScan {
    fn push(
        &mut self,
        tx: &RawSolanaTransaction,
        ix: &RawSolanaInstruction,
        kind: EvidenceKind,
        variant: String,
        reason: &str,
    ) {
        if self.evidence.len() >= MAX_EVIDENCE_SAMPLES {
            return;
        }
        self.evidence.push(DecodeEvidence {
            kind,
            signature: tx.signature,
            slot: tx.slot,
            transaction_index: tx.transaction_index,
            instruction_index: ix.instruction_index,
            program: ix.program_id,
            variant_or_discriminator: variant,
            data_len: ix.data.len(),
            accounts_len: ix.accounts.len(),
            reason: sanitize_reason(reason),
        });
    }
}

/// Scan every instruction of a transaction for coverage gaps.
/// `orphans` are `(instruction_index, event name)` of events the pairing
/// found unclaimed. `jupiter` enables the aggregator (Jupiter and DFlow) event gates.
pub(crate) fn scan_tx_evidence(
    tx: &RawSolanaTransaction,
    decoders: &LedgerDecoders<'_>,
    jupiter: bool,
    orphans: &[(u32, &'static str)],
) -> TxScan {
    let mut scan = TxScan::default();
    for ix in &tx.instructions {
        match decoders.curve.classify(ix, tx.slot, tx.transaction_index) {
            PumpInstructionOutcome::Malformed { variant, reason } => scan.push(
                tx,
                ix,
                EvidenceKind::MalformedTradeInstruction,
                variant.map_or_else(|| discriminator_hex(ix, 0), |v| v.name().to_owned()),
                &reason,
            ),
            PumpInstructionOutcome::UnknownDiscriminator { discriminator } => {
                scan.unknown_discriminators = scan.unknown_discriminators.saturating_add(1);
                scan.push(
                    tx,
                    ix,
                    EvidenceKind::UnknownDiscriminator,
                    hex8(&discriminator),
                    "instruction discriminator is in neither the trade nor the non-trade table",
                );
            }
            PumpInstructionOutcome::NotMine
            | PumpInstructionOutcome::Trade(_)
            | PumpInstructionOutcome::NonTrade(_) => {}
        }
        match decoders.curve.classify_event(ix) {
            PumpEventOutcome::Malformed { reason } => scan.push(
                tx,
                ix,
                EvidenceKind::MalformedEvent,
                discriminator_hex(ix, 8),
                &reason,
            ),
            PumpEventOutcome::UnknownEvent { discriminator } => {
                scan.unknown_discriminators = scan.unknown_discriminators.saturating_add(1);
                scan.push(
                    tx,
                    ix,
                    EvidenceKind::UnknownDiscriminator,
                    hex8(&discriminator),
                    "event discriminator is not in the pinned IDL",
                );
            }
            _ => {}
        }
        if let Some(amm) = decoders.amm {
            match amm.classify(ix, tx.slot, tx.transaction_index) {
                PumpAmmInstructionOutcome::Malformed { variant, reason } => scan.push(
                    tx,
                    ix,
                    EvidenceKind::MalformedTradeInstruction,
                    variant.map_or_else(|| discriminator_hex(ix, 0), |v| v.name().to_owned()),
                    &reason,
                ),
                PumpAmmInstructionOutcome::UnknownDiscriminator { discriminator } => {
                    scan.unknown_discriminators = scan.unknown_discriminators.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::UnknownDiscriminator,
                        hex8(&discriminator),
                        "instruction discriminator is in neither the trade nor the non-trade table",
                    );
                }
                _ => {}
            }
            match amm.classify_event(ix) {
                scout_dex_solana::PumpAmmEventOutcome::Malformed { reason } => scan.push(
                    tx,
                    ix,
                    EvidenceKind::MalformedEvent,
                    discriminator_hex(ix, 8),
                    &reason,
                ),
                scout_dex_solana::PumpAmmEventOutcome::UnknownEvent { discriminator } => {
                    scan.unknown_discriminators = scan.unknown_discriminators.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::UnknownDiscriminator,
                        hex8(&discriminator),
                        "event discriminator is not in the pinned IDL",
                    );
                }
                _ => {}
            }
        }
        if jupiter {
            match JupiterEventDecoder::new().classify(ix) {
                JupiterEventOutcome::Malformed { reason } => {
                    scan.jupiter_malformed = scan.jupiter_malformed.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::MalformedEvent,
                        discriminator_hex(ix, 8),
                        &reason,
                    );
                }
                JupiterEventOutcome::UnknownEvent { discriminator } => {
                    scan.jupiter_unknown = scan.jupiter_unknown.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::UnknownDiscriminator,
                        hex8(&discriminator),
                        "Jupiter event discriminator is not in the pinned IDL or ADR-015",
                    );
                }
                _ => {}
            }
            match DflowEventDecoder::new().classify(ix) {
                DflowEventOutcome::Malformed { reason } => {
                    scan.dflow_malformed = scan.dflow_malformed.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::MalformedEvent,
                        discriminator_hex(ix, 8),
                        &reason,
                    );
                }
                DflowEventOutcome::UnknownEvent { discriminator } => {
                    scan.dflow_unknown = scan.dflow_unknown.saturating_add(1);
                    scan.push(
                        tx,
                        ix,
                        EvidenceKind::UnknownDiscriminator,
                        hex8(&discriminator),
                        "DFlow event discriminator is not in the pinned schema source",
                    );
                }
                _ => {}
            }
        }
    }
    for (idx, name) in orphans {
        if let Some(ix) = tx.instructions.iter().find(|i| i.instruction_index == *idx) {
            scan.push(
                tx,
                ix,
                EvidenceKind::OrphanEvent,
                (*name).to_owned(),
                "decoded trade event not claimed by any decoded trade instruction",
            );
        }
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(slot: u64, idx: u32, kind: EvidenceKind) -> DecodeEvidence {
        DecodeEvidence {
            kind,
            signature: [u8::try_from(slot).unwrap(); 64],
            slot,
            transaction_index: 0,
            instruction_index: idx,
            program: [1; 32],
            variant_or_discriminator: "buy".into(),
            data_len: 1,
            accounts_len: 2,
            reason: "r".into(),
        }
    }

    #[test]
    fn merge_is_bounded_deterministic_and_unique() {
        let mut v = Vec::new();
        let many: Vec<_> = (0..20u64)
            .rev()
            .map(|s| ev(s, 0, EvidenceKind::OrphanEvent))
            .collect();
        merge_evidence(&mut v, many.clone());
        merge_evidence(&mut v, many);
        assert_eq!(v.len(), MAX_EVIDENCE_SAMPLES);
        assert_eq!(
            v.iter().map(|e| e.slot).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn reason_is_sanitized_and_bounded() {
        let long = format!("a\u{1b}[31m{}", "é".repeat(300));
        let s = sanitize_reason(&long);
        assert!(s.len() <= MAX_REASON_BYTES);
        assert!(!s.chars().any(char::is_control));
    }
}
