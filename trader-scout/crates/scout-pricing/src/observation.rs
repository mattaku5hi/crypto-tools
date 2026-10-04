//! Price observation model (ADR-018 §1-3).

use crate::decimal::DecimalPrice;

/// Version tag of the pricing policy for report metadata (invariant #10).
pub const PRICE_POLICY_VERSION: &str = "usd-execution-pricing/1 (ADR-018: Coinbase Exchange 1m candle close, stale <= 5 min, USDC par assumed)";

/// Maximum age, in whole minutes, of a candle used in place of a missing
/// one (ADR-018 §2). Older gaps are `PRICE_UNKNOWN`.
pub const STALENESS_LIMIT_MINUTES: u8 = 5;

/// A quote asset that can be valued in USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuoteAsset {
    Sol,
    Usdc,
    Usdt,
    /// EVM native ETH (Coinbase `ETH-USD`, ADR-020 amendment).
    Eth,
    /// BSC native BNB (Coinbase `BNB-USD`, ADR-020 amendment 5).
    Bnb,
    /// USDG valued at par (`usdg_par_assumed`), no market source.
    Usdg,
}

impl QuoteAsset {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sol => "sol",
            Self::Usdc => "usdc",
            Self::Usdt => "usdt",
            Self::Eth => "eth",
            Self::Bnb => "bnb",
            Self::Usdg => "usdg",
        }
    }
}

/// Start of the one-minute bucket containing unix second `t`.
#[must_use]
pub fn minute_start(t: i64) -> i64 {
    t.div_euclid(60).saturating_mul(60)
}

/// Class of a provider failure (never carries a URL or body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PriceErrorClass {
    /// HTTP 404 (e.g. an unknown product).
    NotFound,
    /// HTTP 401/403.
    Unauthorized,
    /// HTTP 429, retries exhausted.
    RateLimited,
    /// Another HTTP 4xx.
    HttpClient(u16),
    /// HTTP 5xx, retries exhausted.
    HttpServer(u16),
    /// Connect/reset/timeout, retries exhausted.
    Transport,
    /// Body above the byte cap.
    ResponseTooLarge,
    /// 2xx body that is not a candle array.
    MalformedBody,
}

impl PriceErrorClass {
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::NotFound => "http_404".to_string(),
            Self::Unauthorized => "http_unauthorized".to_string(),
            Self::RateLimited => "http_429".to_string(),
            Self::HttpClient(c) | Self::HttpServer(c) => format!("http_{c}"),
            Self::Transport => "transport".to_string(),
            Self::ResponseTooLarge => "response_too_large".to_string(),
            Self::MalformedBody => "malformed_body".to_string(),
        }
    }
}

/// Why a price is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnknownPriceReason {
    /// No candle in the minute or the 5 minutes before it.
    NoCandleWithinStaleLimit,
    /// The page holding the minute failed to load.
    Provider(PriceErrorClass),
    /// The request budget was spent before the page was fetched.
    BudgetExhausted,
    /// The page cache cap was reached before the page was fetched.
    PageCacheFull,
    /// The page was never prefetched (caller error).
    NotPrefetched,
    /// The trade has no usable timestamp.
    NoTimestamp,
}

impl UnknownPriceReason {
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::NoCandleWithinStaleLimit => "no_candle_within_5m".to_string(),
            Self::Provider(c) => format!("provider_{}", c.label()),
            Self::BudgetExhausted => "request_budget_exhausted".to_string(),
            Self::PageCacheFull => "page_cache_full".to_string(),
            Self::NotPrefetched => "not_prefetched".to_string(),
            Self::NoTimestamp => "no_timestamp".to_string(),
        }
    }
}

/// Quality label of one observation (ADR-018 §1-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceLabel {
    /// Close of the candle containing the time (a reference price, not the
    /// on-chain execution price).
    CexReference1m,
    /// Close of the candle `minutes` (1..=5) before the containing minute.
    Stale { minutes: u8 },
    /// USDC valued at exactly 1 USD by assumption (no verified source).
    UsdcParAssumed,
    /// USDG valued at exactly 1 USD by assumption (ADR-020 amendment).
    UsdgParAssumed,
    /// No price. Never zero, never interpolated.
    Unknown { reason: UnknownPriceReason },
}

impl PriceLabel {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::CexReference1m => "cex_reference_1m".to_string(),
            Self::Stale { minutes } => format!("stale_{minutes}m"),
            Self::UsdcParAssumed => "usdc_par_assumed".to_string(),
            Self::UsdgParAssumed => "usdg_par_assumed".to_string(),
            Self::Unknown { .. } => "price_unknown".to_string(),
        }
    }

    #[must_use]
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown { .. })
    }
}

/// USD price of one quote asset at one minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceObservation {
    pub asset: QuoteAsset,
    /// Start (unix seconds) of the minute that was asked for.
    pub minute: i64,
    /// Close of the used candle; `None` exactly for `Unknown`.
    pub value: Option<DecimalPrice>,
    /// Low/high of the used candle: the uncertainty band.
    pub low: Option<DecimalPrice>,
    pub high: Option<DecimalPrice>,
    /// Volume of the used candle (liquidity hint).
    pub volume: Option<DecimalPrice>,
    /// Start of the candle actually used (`minute` or earlier when stale).
    pub candle_minute: Option<i64>,
    pub label: PriceLabel,
    /// Source id, e.g. `coinbase-exchange-candles-1m`.
    pub source: &'static str,
}

impl PriceObservation {
    #[must_use]
    pub fn unknown(
        asset: QuoteAsset,
        minute: i64,
        reason: UnknownPriceReason,
        source: &'static str,
    ) -> Self {
        Self {
            asset,
            minute,
            value: None,
            low: None,
            high: None,
            volume: None,
            candle_minute: None,
            label: PriceLabel::Unknown { reason },
            source,
        }
    }

    /// USDG at par.
    #[must_use]
    pub fn usdg_par(minute: i64, source: &'static str) -> Self {
        Self {
            asset: QuoteAsset::Usdg,
            minute,
            value: Some(DecimalPrice::ONE),
            low: Some(DecimalPrice::ONE),
            high: Some(DecimalPrice::ONE),
            volume: None,
            candle_minute: None,
            label: PriceLabel::UsdgParAssumed,
            source,
        }
    }

    /// USDC at par.
    #[must_use]
    pub fn usdc_par(minute: i64, source: &'static str) -> Self {
        Self {
            asset: QuoteAsset::Usdc,
            minute,
            value: Some(DecimalPrice::ONE),
            low: Some(DecimalPrice::ONE),
            high: Some(DecimalPrice::ONE),
            volume: None,
            candle_minute: None,
            label: PriceLabel::UsdcParAssumed,
            source,
        }
    }
}

/// Policy facts for `run_meta` (invariant #10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricePolicy {
    pub policy_version: &'static str,
    pub source: &'static str,
    pub products: Vec<&'static str>,
    pub granularity_seconds: u32,
    pub staleness_limit_minutes: u8,
    pub usdc_assumption: &'static str,
    pub price_field: &'static str,
}
