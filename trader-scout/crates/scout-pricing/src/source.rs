//! The provider-agnostic [`PriceSource`] port, the shared lookup rule, and
//! an in-memory implementation.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;

use crate::candle::Candle;
use crate::observation::{
    PriceErrorClass, PriceLabel, PriceObservation, PricePolicy, QuoteAsset,
    STALENESS_LIMIT_MINUTES, UnknownPriceReason, minute_start,
};

/// Result of one [`PriceSource::prefetch`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrefetchSummary {
    /// Distinct pages (or equivalent units) the call needed.
    pub pages_needed: u64,
    /// Already cached before the call.
    pub pages_cached: u64,
    pub pages_fetched: u64,
    pub pages_failed: u64,
    /// Not fetched: the request budget was spent.
    pub pages_skipped_budget: u64,
    /// Not fetched: the page cache cap was reached.
    pub pages_skipped_cache_full: u64,
}

impl PrefetchSummary {
    pub fn merge(&mut self, other: &Self) {
        self.pages_needed = self.pages_needed.saturating_add(other.pages_needed);
        self.pages_cached = self.pages_cached.saturating_add(other.pages_cached);
        self.pages_fetched = self.pages_fetched.saturating_add(other.pages_fetched);
        self.pages_failed = self.pages_failed.saturating_add(other.pages_failed);
        self.pages_skipped_budget = self
            .pages_skipped_budget
            .saturating_add(other.pages_skipped_budget);
        self.pages_skipped_cache_full = self
            .pages_skipped_cache_full
            .saturating_add(other.pages_skipped_cache_full);
    }
}

/// USD price provider. Usage: collect the minutes needed, `prefetch` once
/// (batched and cached), then look prices up synchronously.
#[async_trait]
pub trait PriceSource: Send + Sync + std::fmt::Debug {
    /// Source/policy facts for `run_meta`.
    fn policy(&self) -> PricePolicy;

    /// Load everything needed to price `needs` (unix seconds per asset;
    /// any second inside a minute names that minute). Bounded by the
    /// source's request budget and cache cap; failures are recorded and
    /// surface as `Unknown` observations, never as errors.
    async fn prefetch(&self, needs: &BTreeMap<QuoteAsset, BTreeSet<i64>>) -> PrefetchSummary;

    /// USD price of `asset` at unix second `t`, from prefetched data.
    fn usd_price(&self, asset: QuoteAsset, t: i64) -> PriceObservation;

    /// HTTP attempts made so far (retries included); 0 for offline sources.
    fn requests_made(&self) -> u64;
}

/// What is known about one minute of one asset.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MinuteLookup {
    Candle(Candle),
    /// The data for this minute loaded fine and has no candle.
    Absent,
    Unavailable(UnknownPriceReason),
}

/// THE lookup rule (ADR-018 §1-2): the candle of the containing minute
/// (`cex_reference_1m`), else the newest candle within
/// [`STALENESS_LIMIT_MINUTES`] minutes before it (`stale_{k}m`), else
/// unknown. An unavailable minute on the way stops the search as unknown
/// (a failed page cannot be proven to hold no newer candle).
pub(crate) fn resolve(
    asset: QuoteAsset,
    t: i64,
    source: &'static str,
    mut lookup: impl FnMut(i64) -> MinuteLookup,
) -> PriceObservation {
    let minute = minute_start(t);
    for k in 0..=STALENESS_LIMIT_MINUTES {
        let m = minute.saturating_sub(i64::from(k).saturating_mul(60));
        match lookup(m) {
            MinuteLookup::Candle(c) => {
                return PriceObservation {
                    asset,
                    minute,
                    value: Some(c.close),
                    low: Some(c.low),
                    high: Some(c.high),
                    volume: Some(c.volume),
                    candle_minute: Some(c.time),
                    label: if k == 0 {
                        PriceLabel::CexReference1m
                    } else {
                        PriceLabel::Stale { minutes: k }
                    },
                    source,
                };
            }
            MinuteLookup::Absent => {}
            MinuteLookup::Unavailable(reason) => {
                return PriceObservation::unknown(asset, minute, reason, source);
            }
        }
    }
    PriceObservation::unknown(
        asset,
        minute,
        UnknownPriceReason::NoCandleWithinStaleLimit,
        source,
    )
}

/// Candles held in memory: tests, embedding, offline replay. Same lookup
/// rules as every other source; a minute without a candle is `Absent`.
#[derive(Debug, Clone, Default)]
pub struct InMemoryPriceSource {
    candles: BTreeMap<QuoteAsset, BTreeMap<i64, Candle>>,
    failed: BTreeMap<QuoteAsset, PriceErrorClass>,
}

/// Source id of [`InMemoryPriceSource`].
const IN_MEMORY_SOURCE_ID: &str = "in-memory-candles";

impl InMemoryPriceSource {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add candles of `asset` (USDC candles are ignored: USDC is par).
    #[must_use]
    pub fn with_candles(mut self, asset: QuoteAsset, candles: &[Candle]) -> Self {
        if asset != QuoteAsset::Usdc {
            let book = self.candles.entry(asset).or_default();
            for c in candles {
                book.insert(c.time, *c);
            }
        }
        self
    }

    /// Make every lookup of `asset` unknown with a provider failure class.
    #[must_use]
    pub fn with_failure(mut self, asset: QuoteAsset, class: PriceErrorClass) -> Self {
        self.failed.insert(asset, class);
        self
    }
}

#[async_trait]
impl PriceSource for InMemoryPriceSource {
    fn policy(&self) -> PricePolicy {
        PricePolicy {
            policy_version: crate::observation::PRICE_POLICY_VERSION,
            source: IN_MEMORY_SOURCE_ID,
            products: vec!["SOL-USD", "USDT-USD"],
            granularity_seconds: 60,
            staleness_limit_minutes: STALENESS_LIMIT_MINUTES,
            usdc_assumption: "usdc_par_assumed",
            price_field: "close",
        }
    }

    async fn prefetch(&self, _needs: &BTreeMap<QuoteAsset, BTreeSet<i64>>) -> PrefetchSummary {
        PrefetchSummary::default()
    }

    fn usd_price(&self, asset: QuoteAsset, t: i64) -> PriceObservation {
        if asset == QuoteAsset::Usdc {
            return PriceObservation::usdc_par(minute_start(t), IN_MEMORY_SOURCE_ID);
        }
        if let Some(class) = self.failed.get(&asset) {
            return PriceObservation::unknown(
                asset,
                minute_start(t),
                UnknownPriceReason::Provider(*class),
                IN_MEMORY_SOURCE_ID,
            );
        }
        let book = self.candles.get(&asset);
        resolve(asset, t, IN_MEMORY_SOURCE_ID, |m| {
            match book.and_then(|b| b.get(&m)) {
                Some(c) => MinuteLookup::Candle(*c),
                None => MinuteLookup::Absent,
            }
        })
    }

    fn requests_made(&self) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::DecimalPrice;

    fn candle(time: i64, close: &str) -> Candle {
        let p = DecimalPrice::parse(close).unwrap();
        Candle {
            time,
            low: p,
            high: p,
            open: p,
            close: p,
            volume: DecimalPrice::ONE,
        }
    }

    #[test]
    fn exact_minute_is_cex_reference() {
        let s = InMemoryPriceSource::new()
            .with_candles(QuoteAsset::Sol, &[candle(1_790_942_400, "121.89")]);
        let o = s.usd_price(QuoteAsset::Sol, 1_790_942_400 + 37);
        assert_eq!(o.label, PriceLabel::CexReference1m);
        assert_eq!(o.value.unwrap().to_decimal_string(), "121.89");
        assert_eq!(o.minute, 1_790_942_400);
        assert_eq!(o.candle_minute, Some(1_790_942_400));
    }

    #[test]
    fn missing_minute_is_stale_up_to_five_then_unknown() {
        let base = 1_790_942_400;
        let s = InMemoryPriceSource::new().with_candles(QuoteAsset::Sol, &[candle(base, "100")]);
        let o = s.usd_price(QuoteAsset::Sol, base + 180);
        assert_eq!(o.label, PriceLabel::Stale { minutes: 3 });
        assert_eq!(o.value.unwrap().to_decimal_string(), "100");
        let o = s.usd_price(QuoteAsset::Sol, base + 300);
        assert_eq!(o.label, PriceLabel::Stale { minutes: 5 });
        let o = s.usd_price(QuoteAsset::Sol, base + 360);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::NoCandleWithinStaleLimit
            }
        );
        assert!(o.value.is_none());
    }

    #[test]
    fn newer_candle_wins_over_older() {
        let base = 1_790_942_400;
        let s = InMemoryPriceSource::new().with_candles(
            QuoteAsset::Sol,
            &[candle(base, "100"), candle(base + 60, "101")],
        );
        let o = s.usd_price(QuoteAsset::Sol, base + 120);
        assert_eq!(o.label, PriceLabel::Stale { minutes: 1 });
        assert_eq!(o.value.unwrap().to_decimal_string(), "101");
    }

    #[test]
    fn usdc_is_par_and_failure_is_unknown_with_class() {
        let s =
            InMemoryPriceSource::new().with_failure(QuoteAsset::Usdt, PriceErrorClass::NotFound);
        let o = s.usd_price(QuoteAsset::Usdc, 1_790_942_400);
        assert_eq!(o.label, PriceLabel::UsdcParAssumed);
        assert_eq!(o.value, Some(DecimalPrice::ONE));
        let o = s.usd_price(QuoteAsset::Usdt, 1_790_942_400);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::Provider(PriceErrorClass::NotFound)
            }
        );
        assert_eq!(
            match o.label {
                PriceLabel::Unknown { reason } => reason.label(),
                _ => String::new(),
            },
            "provider_http_404"
        );
    }
}
