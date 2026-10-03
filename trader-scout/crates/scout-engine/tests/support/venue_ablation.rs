//! Test support: switch the direct-venue swap evidence (ADR-013 section 2b)
//! OFF so the historical Jupiter / DFlow / OKX measurements stay comparable
//! (each of those tests measures its own evidence by removing it from a
//! baseline). The venue effect itself is measured in `venue_swap_legs.rs`.
//!
//! Venue evidence needs two inputs: `emit!` log lines (`log_messages`) and
//! DLMM `emit_cpi!` instructions. The first is dropped, the second is made
//! not an event-CPI by flipping the first tag byte (instruction indices and
//! counts stay identical).
#![allow(dead_code)]

use scout_core::RawSolanaTransaction;
use scout_dex_solana::{DLMM_PROGRAM_ID_BYTES, EVENT_CPI_DISCRIMINATOR};

pub fn without_venue_events(txs: &[RawSolanaTransaction]) -> Vec<RawSolanaTransaction> {
    txs.iter()
        .map(|t| {
            let mut t = t.clone();
            t.log_messages = None;
            for ix in &mut t.instructions {
                if ix.program_id == DLMM_PROGRAM_ID_BYTES
                    && ix.data.starts_with(&EVENT_CPI_DISCRIMINATOR)
                {
                    ix.data[0] ^= 0xff;
                }
            }
            t
        })
        .collect()
}
