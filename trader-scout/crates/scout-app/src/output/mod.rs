//! Output formatting: JSONL envelope + broken-pipe-safe writer. See
//! docs/CLI.md §7-8 for the contract this implements.
#![allow(clippy::module_inception)]

mod evidence;
mod evm_keys;
mod evm_open_valuation;
mod jsonl;
mod open_valuation;
mod pricing;
mod writer;

pub use evidence::{DecodeEvidenceDto, VenueDiagDto, VenueEventsDto, evidence_dtos, evidence_line};
pub use evm_keys::evm_spelling;
pub use evm_open_valuation::{
    EvmOpenPositionDto, EvmOpenValuationMetaDto, EvmOpenValuationTotalsDto, evm_meta_dto,
    evm_totals_dto, ledger_evm_totals_dto,
};
pub use jsonl::{JsonlRecord, RunStatus, SCHEMA_VERSION, Window};
pub use open_valuation::{
    FeeBpsDto, OpenPositionDto, OpenValuationMetaDto, OpenValuationTotalsDto, ledger_totals_dto,
    open_positions_dto, open_valuation_line, open_valuation_line_evm, open_valuation_meta,
    open_valuation_meta_evm, totals_dto,
};
pub use pricing::{
    PrefetchDto, PriceCoverageDto, PricingMetaDto, PricingMetaInput, pricing_line, pricing_meta,
};
pub use writer::{WriteOutcome, write_lines_to_stdout};
