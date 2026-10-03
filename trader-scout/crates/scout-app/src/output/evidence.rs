//! JSONL form of the bounded decode-evidence samples (coverage-gap
//! forensics): where a "malformed trade instruction" / "orphan event" /
//! unknown discriminator counter came from. Shared by the three CLIs.

use scout_sdk::engine::{DecodeEvidence, VenueDiag, VenueEventDiagnostics};
use serde::Serialize;

/// One evidence sample. `signature` and `program` are full base58.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DecodeEvidenceDto {
    /// `malformed_trade_instruction`, `malformed_event`,
    /// `unknown_discriminator`, `orphan_event`,
    /// `okx_swap_with_receiver_not_attributed`, `okx_unverified_order_event`,
    /// `venue_unresolved_event` or `venue_unverified_event`.
    pub kind: &'static str,
    pub signature: String,
    pub slot: u64,
    pub instruction_index: u32,
    pub program: String,
    /// Address label only (`pump_curve`, `pump_amm`, `jupiter_v6`, `dflow_v4`,
    /// `okx_dex_router`, `whirlpool`, `dlmm`, `raydium_clmm`, `raydium_cpmm`).
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

/// Coverage counters of one direct venue (ADR-013 section 2b). `malformed`
/// and `unknown` are coverage gaps; `idl_only` and `unresolved` are
/// informational.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct VenueDiagDto {
    pub malformed: u64,
    pub unknown: u64,
    pub idl_only: u64,
    pub unresolved: u64,
}

impl From<&VenueDiag> for VenueDiagDto {
    fn from(d: &VenueDiag) -> Self {
        Self {
            malformed: d.malformed,
            unknown: d.unknown,
            idl_only: d.idl_only,
            unresolved: d.unresolved,
        }
    }
}

/// Direct-venue (Orca Whirlpool, Meteora DLMM, Raydium CLMM/CPMM) swap-event
/// coverage counters.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct VenueEventsDto {
    pub whirlpool: VenueDiagDto,
    pub dlmm: VenueDiagDto,
    pub raydium_clmm: VenueDiagDto,
    pub raydium_cpmm: VenueDiagDto,
}

impl From<&VenueEventDiagnostics> for VenueEventsDto {
    fn from(d: &VenueEventDiagnostics) -> Self {
        Self {
            whirlpool: (&d.whirlpool).into(),
            dlmm: (&d.dlmm).into(),
            raydium_clmm: (&d.raydium_clmm).into(),
            raydium_cpmm: (&d.raydium_cpmm).into(),
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
