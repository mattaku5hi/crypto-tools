//! ADR-018 price source / policy metadata and price-coverage DTOs shared by
//! `wallet-stats` and `wallet-rank` (`run_meta.pricing`, per-wallet
//! `stats.usd.price_coverage`). Plain data in, serializable data out.

use std::collections::BTreeMap;

use scout_pricing::PricePolicy;
use scout_sdk::engine::{UsdCoverage, UsdPricingRun};
use serde::Serialize;

/// Prefetch counters of the run (pages are 300-minute candle pages).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrefetchDto {
    pub pages_needed: u64,
    pub pages_cached: u64,
    pub pages_fetched: u64,
    pub pages_failed: u64,
    pub pages_skipped_budget: u64,
    pub pages_skipped_cache_full: u64,
}

/// Price coverage of a wallet (or of the whole run).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PriceCoverageDto {
    /// Conversions attempted (proceeds + known consumed lot slices).
    pub legs: u64,
    pub priced: u64,
    pub unpriced: u64,
    /// Exact `priced / legs`; `display_2dp` is presentation only (percent,
    /// rounded half up); `null` without legs.
    pub priced_percent_2dp: Option<String>,
    /// `cex_reference_1m`, `stale_{k}m`, `usdc_par_assumed`, `price_unknown`.
    pub by_label: BTreeMap<String, u64>,
    pub unpriced_by_reason: BTreeMap<String, u64>,
    /// Legs valued at the USDC par assumption (depeg-visible count).
    pub usdc_par_legs: u64,
}

fn percent_2dp(num: u64, den: u64) -> Option<String> {
    if den == 0 {
        return None;
    }
    // num * 10000 / den rounded half up, as a 2-decimal percent.
    let n = u128::from(num).checked_mul(10_000)?;
    let d = u128::from(den);
    let q = n
        .checked_mul(2)?
        .checked_add(d)?
        .div_euclid(d.checked_mul(2)?);
    let whole = q.div_euclid(100);
    let frac = q.rem_euclid(100);
    Some(format!("{whole}.{frac:02}"))
}

impl PriceCoverageDto {
    #[must_use]
    pub fn from_coverage(c: &UsdCoverage) -> Self {
        Self {
            legs: c.legs,
            priced: c.priced,
            unpriced: c.unpriced,
            priced_percent_2dp: percent_2dp(c.priced, c.legs),
            by_label: c.by_label.clone(),
            unpriced_by_reason: c.unpriced_by_reason.clone(),
            usdc_par_legs: c.usdc_par_legs,
        }
    }
}

/// `run_meta.pricing`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PricingMetaDto {
    /// `false` with `--no-usd` (or when no ledger needed pricing sources).
    pub enabled: bool,
    pub source: Option<&'static str>,
    pub policy_version: Option<&'static str>,
    pub products: Vec<&'static str>,
    pub granularity_seconds: Option<u32>,
    pub staleness_limit_minutes: Option<u8>,
    pub price_field: Option<&'static str>,
    pub usdc_assumption: Option<&'static str>,
    /// `SCOUT_COINBASE_ENDPOINT` was set (test/mirror endpoint in use).
    pub endpoint_overridden: bool,
    /// HTTP attempts for prices (retries included); separate from the scan.
    pub requests_made_prices: u64,
    pub max_price_requests: Option<u64>,
    pub prefetch: Option<PrefetchDto>,
    /// Distinct minutes requested per quote asset.
    pub minutes_requested: BTreeMap<&'static str, u64>,
    pub wallets_priced: u64,
    pub wallets_failed: u64,
    /// Run-wide coverage (sum over wallets).
    pub coverage: Option<PriceCoverageDto>,
}

/// Inputs of [`pricing_meta`].
#[derive(Debug, Clone, Copy)]
pub struct PricingMetaInput<'a> {
    pub policy: Option<&'a PricePolicy>,
    pub endpoint_overridden: bool,
    pub requests_made: u64,
    pub max_requests: Option<u64>,
    pub run: Option<&'a UsdPricingRun>,
}

#[must_use]
pub fn pricing_meta(i: &PricingMetaInput<'_>) -> PricingMetaDto {
    let p = i.policy;
    PricingMetaDto {
        enabled: p.is_some(),
        source: p.map(|p| p.source),
        policy_version: p.map(|p| p.policy_version),
        products: p.map(|p| p.products.clone()).unwrap_or_default(),
        granularity_seconds: p.map(|p| p.granularity_seconds),
        staleness_limit_minutes: p.map(|p| p.staleness_limit_minutes),
        price_field: p.map(|p| p.price_field),
        usdc_assumption: p.map(|p| p.usdc_assumption),
        endpoint_overridden: i.endpoint_overridden,
        requests_made_prices: i.requests_made,
        max_price_requests: i.max_requests,
        prefetch: i.run.map(|r| PrefetchDto {
            pages_needed: r.prefetch.pages_needed,
            pages_cached: r.prefetch.pages_cached,
            pages_fetched: r.prefetch.pages_fetched,
            pages_failed: r.prefetch.pages_failed,
            pages_skipped_budget: r.prefetch.pages_skipped_budget,
            pages_skipped_cache_full: r.prefetch.pages_skipped_cache_full,
        }),
        minutes_requested: i
            .run
            .map(|r| {
                r.minutes_requested
                    .iter()
                    .map(|(a, n)| (a.label(), *n))
                    .collect()
            })
            .unwrap_or_default(),
        wallets_priced: i.run.map_or(0, |r| r.wallets_priced),
        wallets_failed: i.run.map_or(0, |r| r.wallets_failed),
        coverage: i.run.map(|r| PriceCoverageDto::from_coverage(&r.coverage)),
    }
}

/// One stderr line summarising the pricing step.
#[must_use]
pub fn pricing_line(bin: &str, i: &PricingMetaInput<'_>) -> String {
    let Some(p) = i.policy else {
        return format!("{bin}: usd pricing: disabled (--no-usd); no USD figures");
    };
    let cov = i.run.map(|r| &r.coverage);
    format!(
        "{bin}: usd pricing: source={} policy={} requests_made_prices={} max_price_requests={} \
         legs={} priced={} unpriced={} usdc_par_legs={}{}",
        p.source,
        p.policy_version,
        i.requests_made,
        i.max_requests
            .map_or_else(|| "unlimited".to_string(), |n| n.to_string()),
        cov.map_or(0, |c| c.legs),
        cov.map_or(0, |c| c.priced),
        cov.map_or(0, |c| c.unpriced),
        cov.map_or(0, |c| c.usdc_par_legs),
        if i.endpoint_overridden {
            " endpoint=override"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_half_up_and_handles_no_legs() {
        assert_eq!(percent_2dp(1, 3).unwrap(), "33.33");
        assert_eq!(percent_2dp(2, 3).unwrap(), "66.67");
        assert_eq!(percent_2dp(1, 1).unwrap(), "100.00");
        assert_eq!(percent_2dp(0, 5).unwrap(), "0.00");
        assert!(percent_2dp(0, 0).is_none());
    }
}
