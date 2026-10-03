//! Output formatting: JSONL envelope + broken-pipe-safe writer. See
//! docs/CLI.md §7-8 for the contract this implements.
#![allow(clippy::module_inception)]

mod evidence;
mod jsonl;
mod open_valuation;
mod pricing;
mod writer;

pub use evidence::{DecodeEvidenceDto, evidence_dtos, evidence_line};
pub use jsonl::{JsonlRecord, RunStatus, SCHEMA_VERSION, Window};
pub use open_valuation::{
    FeeBpsDto, OpenPositionDto, OpenValuationMetaDto, OpenValuationTotalsDto, ledger_totals_dto,
    open_positions_dto, open_valuation_line, open_valuation_meta, totals_dto,
};
pub use pricing::{
    PrefetchDto, PriceCoverageDto, PricingMetaDto, PricingMetaInput, pricing_line, pricing_meta,
};
pub use writer::{WriteOutcome, write_lines_to_stdout};
