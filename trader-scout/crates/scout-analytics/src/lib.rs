//! scout-analytics: episode cohorts, win rate, profit factor. See
//! ADR-004 for the RatioStatus contract (no float sentinels for
//! Infinity/NaN/undefined).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod episode;
mod ratio;

pub use episode::{Episode, EpisodeCohort};
pub use ratio::{RatioStatus, profit_factor, win_rate};
