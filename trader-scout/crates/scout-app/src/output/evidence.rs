//! JSONL form of the bounded decode-evidence samples (coverage-gap
//! forensics): where a "malformed trade instruction" / "orphan event" /
//! unknown discriminator counter came from. Shared by the three CLIs.

use scout_sdk::engine::DecodeEvidence;
use serde::Serialize;

/// One evidence sample. `signature` and `program` are full base58.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DecodeEvidenceDto {
    /// `malformed_trade_instruction`, `malformed_event`,
    /// `unknown_discriminator`, `orphan_event`,
    /// `okx_swap_with_receiver_not_attributed` or `okx_unverified_order_event`.
    pub kind: &'static str,
    pub signature: String,
    pub slot: u64,
    pub instruction_index: u32,
    pub program: String,
    /// Address label only (`pump_curve`, `pump_amm`, `jupiter_v6`, `dflow_v4`, `okx_dex_router`).
    pub program_name: &'static str,
    /// IDL name, or the 8-byte discriminator in hex.
    pub variant_or_discriminator: String,
    pub data_len: usize,
    pub accounts_len: usize,
    pub reason: String,
}

impl DecodeEvidenceDto {
    /// `redact` strips secrets from the (chain-derived) reason text.
    #[must_use]
    pub fn new(e: &DecodeEvidence, redact: &dyn Fn(&str) -> String) -> Self {
        Self {
            kind: e.kind.label(),
            signature: e.signature_base58(),
            slot: e.slot,
            instruction_index: e.instruction_index,
            program: e.program_base58(),
            program_name: e.program_name(),
            variant_or_discriminator: e.variant_or_discriminator.clone(),
            data_len: e.data_len,
            accounts_len: e.accounts_len,
            reason: redact(&e.reason),
        }
    }
}

/// One stderr line for a sample (no raw data, bounded reason).
#[must_use]
pub fn evidence_line(e: &DecodeEvidence, redact: &dyn Fn(&str) -> String) -> String {
    format!(
        "{} signature={} slot={} ix={} program={}({}) variant_or_discriminator={} data_len={} \
         accounts_len={} reason={}",
        e.kind.label(),
        e.signature_base58(),
        e.slot,
        e.instruction_index,
        e.program_name(),
        e.program_base58(),
        e.variant_or_discriminator,
        e.data_len,
        e.accounts_len,
        redact(&e.reason)
    )
}

/// DTOs of a bounded sample list.
#[must_use]
pub fn evidence_dtos(
    samples: &[DecodeEvidence],
    redact: &dyn Fn(&str) -> String,
) -> Vec<DecodeEvidenceDto> {
    samples
        .iter()
        .map(|e| DecodeEvidenceDto::new(e, redact))
        .collect()
}
