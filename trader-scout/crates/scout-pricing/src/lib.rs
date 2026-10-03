//! scout-pricing: USD execution pricing (ADR-018).
//!
//! * [`DecimalPrice`]: an exact decimal (`mantissa / 10^scale`, `u128`);
//!   prices are parsed from the provider's raw JSON number text and never
//!   pass through a float (AGENTS invariant #7).
//! * [`PriceSource`]: provider-agnostic port. A caller collects the minutes
//!   it needs, awaits [`PriceSource::prefetch`] once (batched, cached,
//!   budgeted), then looks prices up synchronously with
//!   [`PriceSource::usd_price`]. A price is never zero when missing: it is
//!   [`PriceLabel::Unknown`] with a reason.
//! * [`CoinbasePriceSource`]: Coinbase Exchange public one-minute candles
//!   (`SOL-USD`, `USDT-USD`); USDC is valued at par with an explicit label.
//! * [`InMemoryPriceSource`]: the same lookup rules over supplied candles
//!   (tests, embedding, offline replays).
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

mod candle;
mod coinbase;
mod decimal;
mod observation;
mod source;

pub use candle::{Candle, MAX_CANDLES_PER_BODY, ParseCandlesError, parse_candles};
pub use coinbase::{
    COINBASE_DEFAULT_ENDPOINT, COINBASE_SOURCE_ID, CoinbaseConfig, CoinbasePriceSource,
    PAGE_MINUTES, page_start_of,
};
pub use decimal::{DecimalParseError, DecimalPrice};
pub use observation::{
    PRICE_POLICY_VERSION, PriceErrorClass, PriceLabel, PriceObservation, PricePolicy, QuoteAsset,
    STALENESS_LIMIT_MINUTES, UnknownPriceReason, minute_start,
};
pub use source::{InMemoryPriceSource, PrefetchSummary, PriceSource};
