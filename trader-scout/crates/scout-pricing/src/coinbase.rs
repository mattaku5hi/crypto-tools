//! Coinbase Exchange public candles adapter (ADR-018).
//!
//! `GET {endpoint}/products/{SOL-USD|USDT-USD}/candles?granularity=60&start&end`
//! (no key; a `User-Agent` header is required by the provider). One request
//! is one PAGE of up to 300 one-minute candles aligned to a 300-minute
//! boundary; pages are cached per `(product, page)` for the run. A price
//! at minute `m` may need the page of `m` and the page of `m - 5 min`
//! (staleness window).
//!
//! Bounds (invariant #13): response body cap, request timeout, retry with
//! backoff on 429/5xx/transport, a TOTAL HTTP-attempt budget shared by all
//! clones of the source, and a cap on cached pages. Pages are fetched
//! sequentially (deterministic budget use, polite to the provider's public
//! rate limit). No secrets are involved or sent.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use scout_rpc::{CounterJitter, JitterSource, RetryPolicy, Sleeper, TokioSleeper};

use crate::candle::{Candle, parse_candles};
use crate::observation::{
    PRICE_POLICY_VERSION, PriceErrorClass, PriceObservation, PricePolicy, QuoteAsset,
    STALENESS_LIMIT_MINUTES, UnknownPriceReason, minute_start,
};
use crate::source::{MinuteLookup, PrefetchSummary, PriceSource, resolve};

pub const COINBASE_DEFAULT_ENDPOINT: &str = "https://api.exchange.coinbase.com";
/// Source id recorded on every observation and in `run_meta`.
pub const COINBASE_SOURCE_ID: &str = "coinbase-exchange-candles-1m";
/// Candles per page (the provider maximum per request).
pub const PAGE_MINUTES: i64 = 300;
const PAGE_SECONDS: i64 = PAGE_MINUTES * 60;
const USER_AGENT: &str = "trader-scout-pricing/0.1 (public market data; no credentials)";
/// Longest `Retry-After` honoured inside one call.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Start (unix seconds) of the 300-minute page containing `t`.
#[must_use]
pub fn page_start_of(t: i64) -> i64 {
    t.div_euclid(PAGE_SECONDS).saturating_mul(PAGE_SECONDS)
}

/// Adapter configuration.
#[derive(Debug, Clone)]
pub struct CoinbaseConfig {
    /// Base URL without trailing slash (tests point this at a mock server).
    pub endpoint: String,
    pub timeout_ms: u64,
    /// HTTP attempts per page (first try included), >= 1.
    pub max_attempts: u32,
    /// Total HTTP attempts for the whole source (`None` = unlimited, still counted).
    pub max_requests: Option<u64>,
    pub max_response_bytes: usize,
    /// Maximum cached pages (loaded or failed).
    pub max_cached_pages: usize,
    /// First backoff step (ms); doubles per attempt.
    pub retry_base_delay_ms: u64,
}

impl Default for CoinbaseConfig {
    fn default() -> Self {
        Self {
            endpoint: COINBASE_DEFAULT_ENDPOINT.to_string(),
            timeout_ms: 15_000,
            max_attempts: 3,
            max_requests: None,
            max_response_bytes: 1024 * 1024,
            max_cached_pages: 1_024,
            retry_base_delay_ms: 250,
        }
    }
}

#[derive(Debug)]
enum PageState {
    Loaded(BTreeMap<i64, Candle>),
    Failed(PriceErrorClass),
    SkippedBudget,
    SkippedCacheFull,
}

enum FetchOutcome {
    Loaded(BTreeMap<i64, Candle>),
    Failed(PriceErrorClass),
    BudgetExhausted,
}

enum Attempt {
    Done(BTreeMap<i64, Candle>),
    Retry(PriceErrorClass, Option<Duration>),
    Terminal(PriceErrorClass),
}

/// Coinbase Exchange candle source. Cheap to share behind an `Arc`; the
/// budget and the page cache are shared by every user.
pub struct CoinbasePriceSource {
    http: reqwest::Client,
    cfg: CoinbaseConfig,
    retry: RetryPolicy,
    sleeper: Arc<dyn Sleeper>,
    jitter: Arc<dyn JitterSource>,
    used: AtomicU64,
    pages: RwLock<BTreeMap<(QuoteAsset, i64), PageState>>,
}

impl std::fmt::Debug for CoinbasePriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoinbasePriceSource")
            .field("endpoint", &self.cfg.endpoint)
            .field("requests_made", &self.used.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

fn product_of(asset: QuoteAsset) -> Option<&'static str> {
    match asset {
        QuoteAsset::Sol => Some("SOL-USD"),
        QuoteAsset::Usdt => Some("USDT-USD"),
        QuoteAsset::Usdc => None,
    }
}

/// Proleptic Gregorian civil date of days since 1970-01-01 (Hinnant).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe.div_euclid(1_460) + doe.div_euclid(36_524) - doe.div_euclid(146_096))
        .div_euclid(365);
    let doy = doe - (365 * yoe + yoe.div_euclid(4) - yoe.div_euclid(100));
    let mp = (5 * doy + 2).div_euclid(153);
    let d = doy - (153 * mp + 2).div_euclid(5) + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `2026-10-02T12:00:00Z` for unix seconds.
fn iso_utc(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod.div_euclid(3_600),
        sod.rem_euclid(3_600).div_euclid(60),
        sod.rem_euclid(60)
    )
}

impl CoinbasePriceSource {
    /// Build the source (does no I/O).
    pub fn new(cfg: CoinbaseConfig) -> Result<Self, PriceErrorClass> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| PriceErrorClass::Transport)?;
        let mut retry = RetryPolicy::from_config(cfg.max_attempts.max(1), cfg.timeout_ms);
        retry.base_delay_ms = cfg.retry_base_delay_ms;
        Ok(Self {
            http,
            cfg,
            retry,
            sleeper: Arc::new(TokioSleeper),
            jitter: Arc::new(CounterJitter::default()),
            used: AtomicU64::new(0),
            pages: RwLock::new(BTreeMap::new()),
        })
    }

    /// Replace the backoff sleeper (tests record delays instead of waiting).
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    fn budget_exhausted(&self) -> bool {
        self.cfg
            .max_requests
            .is_some_and(|l| self.used.load(Ordering::Acquire) >= l)
    }

    /// Reserve one HTTP attempt; `false` when the budget is spent.
    fn try_reserve(&self) -> bool {
        match self.cfg.max_requests {
            None => {
                self.used.fetch_add(1, Ordering::AcqRel);
                true
            }
            Some(limit) => self
                .used
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                    (u < limit).then_some(u + 1)
                })
                .is_ok(),
        }
    }

    async fn read_capped(&self, mut resp: reqwest::Response) -> Result<Vec<u8>, Attempt> {
        let cap = self.cfg.max_response_bytes;
        let cap_u64 = u64::try_from(cap).unwrap_or(u64::MAX);
        if resp.content_length().is_some_and(|l| l > cap_u64) {
            return Err(Attempt::Terminal(PriceErrorClass::ResponseTooLarge));
        }
        let mut buf: Vec<u8> = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if buf.len().saturating_add(chunk.len()) > cap {
                        return Err(Attempt::Terminal(PriceErrorClass::ResponseTooLarge));
                    }
                    buf.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(buf),
                Err(_) => return Err(Attempt::Retry(PriceErrorClass::Transport, None)),
            }
        }
    }

    async fn attempt(&self, product: &str, start: i64) -> Attempt {
        let url = format!(
            "{}/products/{product}/candles?granularity=60&start={}&end={}",
            self.cfg.endpoint.trim_end_matches('/'),
            iso_utc(start),
            iso_utc(start + (PAGE_MINUTES - 1) * 60)
        );
        let resp = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return Attempt::Retry(PriceErrorClass::Transport, None),
        };
        let status = resp.status();
        let code = status.as_u16();
        if status.is_success() {
            let body = match self.read_capped(resp).await {
                Ok(b) => b,
                Err(a) => return a,
            };
            return match parse_candles(&body) {
                Ok(candles) => Attempt::Done(
                    candles
                        .into_iter()
                        .filter(|c| c.time >= start && c.time < start + PAGE_SECONDS)
                        .map(|c| (c.time, c))
                        .collect(),
                ),
                Err(_) => Attempt::Terminal(PriceErrorClass::MalformedBody),
            };
        }
        match code {
            404 => Attempt::Terminal(PriceErrorClass::NotFound),
            401 | 403 => Attempt::Terminal(PriceErrorClass::Unauthorized),
            429 => {
                let ra = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(RetryPolicy::parse_retry_after);
                if ra.is_some_and(|d| d > MAX_RETRY_AFTER) {
                    Attempt::Terminal(PriceErrorClass::RateLimited)
                } else {
                    Attempt::Retry(PriceErrorClass::RateLimited, ra)
                }
            }
            500..=599 => Attempt::Retry(PriceErrorClass::HttpServer(code), None),
            _ => Attempt::Terminal(PriceErrorClass::HttpClient(code)),
        }
    }

    async fn fetch_page(&self, product: &str, start: i64) -> FetchOutcome {
        let mut last: Option<PriceErrorClass> = None;
        for attempt in 0..self.retry.max_attempts.max(1) {
            if attempt > 0 {
                if self.budget_exhausted() {
                    break;
                }
                let delay = self.retry.delay_for_attempt(attempt, self.jitter.as_ref());
                self.sleeper.sleep(delay).await;
            }
            if !self.try_reserve() {
                return match last {
                    Some(c) => FetchOutcome::Failed(c),
                    None => FetchOutcome::BudgetExhausted,
                };
            }
            match self.attempt(product, start).await {
                Attempt::Done(m) => return FetchOutcome::Loaded(m),
                Attempt::Terminal(c) => return FetchOutcome::Failed(c),
                Attempt::Retry(c, ra) => {
                    last = Some(c);
                    if let Some(ra) = ra {
                        self.sleeper.sleep(ra).await;
                    }
                }
            }
        }
        FetchOutcome::Failed(last.unwrap_or(PriceErrorClass::Transport))
    }

    fn pages_read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<(QuoteAsset, i64), PageState>> {
        self.pages
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pages_write(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, BTreeMap<(QuoteAsset, i64), PageState>> {
        self.pages
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl PriceSource for CoinbasePriceSource {
    fn policy(&self) -> PricePolicy {
        PricePolicy {
            policy_version: PRICE_POLICY_VERSION,
            source: COINBASE_SOURCE_ID,
            products: vec!["SOL-USD", "USDT-USD"],
            granularity_seconds: 60,
            staleness_limit_minutes: STALENESS_LIMIT_MINUTES,
            usdc_assumption: "usdc_par_assumed: USDC valued at exactly 1 USD (no keyless historical USDC/USD source verified)",
            price_field: "close",
        }
    }

    async fn prefetch(&self, needs: &BTreeMap<QuoteAsset, BTreeSet<i64>>) -> PrefetchSummary {
        let mut wanted: BTreeSet<(QuoteAsset, i64)> = BTreeSet::new();
        for (asset, times) in needs {
            if product_of(*asset).is_none() {
                continue;
            }
            for t in times {
                let m = minute_start(*t);
                wanted.insert((*asset, page_start_of(m)));
                wanted.insert((
                    *asset,
                    page_start_of(m.saturating_sub(i64::from(STALENESS_LIMIT_MINUTES) * 60)),
                ));
            }
        }
        let mut sum = PrefetchSummary {
            pages_needed: u64::try_from(wanted.len()).unwrap_or(u64::MAX),
            ..PrefetchSummary::default()
        };
        for (asset, start) in wanted {
            let Some(product) = product_of(asset) else {
                continue;
            };
            let cached_pages = {
                let pages = self.pages_read();
                if matches!(
                    pages.get(&(asset, start)),
                    Some(PageState::Loaded(_) | PageState::Failed(_))
                ) {
                    sum.pages_cached += 1;
                    continue;
                }
                pages
                    .values()
                    .filter(|p| matches!(p, PageState::Loaded(_) | PageState::Failed(_)))
                    .count()
            };
            if cached_pages >= self.cfg.max_cached_pages {
                self.pages_write()
                    .insert((asset, start), PageState::SkippedCacheFull);
                sum.pages_skipped_cache_full += 1;
                continue;
            }
            let state = match self.fetch_page(product, start).await {
                FetchOutcome::Loaded(m) => {
                    sum.pages_fetched += 1;
                    PageState::Loaded(m)
                }
                FetchOutcome::Failed(c) => {
                    sum.pages_failed += 1;
                    PageState::Failed(c)
                }
                FetchOutcome::BudgetExhausted => {
                    sum.pages_skipped_budget += 1;
                    PageState::SkippedBudget
                }
            };
            self.pages_write().insert((asset, start), state);
        }
        sum
    }

    fn usd_price(&self, asset: QuoteAsset, t: i64) -> PriceObservation {
        if asset == QuoteAsset::Usdc {
            return PriceObservation::usdc_par(minute_start(t), COINBASE_SOURCE_ID);
        }
        let pages = self.pages_read();
        resolve(asset, t, COINBASE_SOURCE_ID, |m| {
            match pages.get(&(asset, page_start_of(m))) {
                Some(PageState::Loaded(map)) => match map.get(&m) {
                    Some(c) => MinuteLookup::Candle(*c),
                    None => MinuteLookup::Absent,
                },
                Some(PageState::Failed(c)) => {
                    MinuteLookup::Unavailable(UnknownPriceReason::Provider(*c))
                }
                Some(PageState::SkippedBudget) => {
                    MinuteLookup::Unavailable(UnknownPriceReason::BudgetExhausted)
                }
                Some(PageState::SkippedCacheFull) => {
                    MinuteLookup::Unavailable(UnknownPriceReason::PageCacheFull)
                }
                None => MinuteLookup::Unavailable(UnknownPriceReason::NotPrefetched),
            }
        })
    }

    fn requests_made(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::PriceLabel;
    use wiremock::matchers::{header_exists, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BODY: &str = "[[1790942700,121.73,121.84,121.75,121.75,102.7895735],\
        [1790942640,121.67,121.77,121.67,121.74,358.14388924],\
        [1790942400,121.72,121.92,121.79,121.89,226.4044408]]";

    fn cfg(server: &MockServer) -> CoinbaseConfig {
        CoinbaseConfig {
            endpoint: server.uri(),
            timeout_ms: 5_000,
            retry_base_delay_ms: 1,
            ..CoinbaseConfig::default()
        }
    }

    fn needs(asset: QuoteAsset, t: i64) -> BTreeMap<QuoteAsset, BTreeSet<i64>> {
        BTreeMap::from([(asset, BTreeSet::from([t]))])
    }

    #[test]
    fn iso_and_pages() {
        assert_eq!(iso_utc(1_790_942_400), "2026-10-02T12:00:00Z");
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(page_start_of(1_790_942_400) % PAGE_SECONDS, 0);
        assert_eq!(page_start_of(-1), -PAGE_SECONDS);
    }

    #[tokio::test]
    async fn prices_from_the_recorded_body_and_caches_the_page() {
        let server = MockServer::start().await;
        let page = page_start_of(1_790_942_400);
        Mock::given(method("GET"))
            .and(path("/products/SOL-USD/candles"))
            .and(query_param("granularity", "60"))
            .and(query_param("start", iso_utc(page)))
            .and(header_exists("user-agent"))
            .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
            .mount(&server)
            .await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        let n = needs(QuoteAsset::Sol, 1_790_942_400 + 5);
        let s1 = src.prefetch(&n).await;
        assert_eq!(s1.pages_fetched, s1.pages_needed);
        let n_req = src.requests_made();
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_405);
        assert_eq!(o.label, PriceLabel::CexReference1m);
        assert_eq!(o.value.unwrap().to_decimal_string(), "121.89");
        assert_eq!(o.low.unwrap().to_decimal_string(), "121.72");
        assert_eq!(o.high.unwrap().to_decimal_string(), "121.92");
        assert_eq!(o.source, COINBASE_SOURCE_ID);
        // 12:01 is missing -> stale_1m of 12:00; 12:04 missing -> stale_4m.
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_400 + 60);
        assert_eq!(o.label, PriceLabel::Stale { minutes: 1 });
        assert_eq!(o.value.unwrap().to_decimal_string(), "121.89");
        // 12:05 is present.
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_700);
        assert_eq!(o.value.unwrap().to_decimal_string(), "121.75");
        // gap > 5 minutes -> unknown (12:11, nothing since 12:05).
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_700 + 360);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::NoCandleWithinStaleLimit
            }
        );
        // Second prefetch hits the cache: no new request.
        let s2 = src.prefetch(&n).await;
        assert_eq!(s2.pages_fetched, 0);
        assert_eq!(s2.pages_cached, s2.pages_needed);
        assert_eq!(src.requests_made(), n_req);
    }

    #[tokio::test]
    async fn usdc_is_par_without_any_request() {
        let server = MockServer::start().await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        let s = src.prefetch(&needs(QuoteAsset::Usdc, 1_790_942_400)).await;
        assert_eq!(s.pages_needed, 0);
        let o = src.usd_price(QuoteAsset::Usdc, 1_790_942_400);
        assert_eq!(o.label, PriceLabel::UsdcParAssumed);
        assert_eq!(src.requests_made(), 0);
    }

    #[tokio::test]
    async fn not_found_is_unknown_with_class_and_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{\"message\":\"NotFound\"}"))
            .expect(2)
            .mount(&server)
            .await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        // Two pages (m and m-5min straddle a page boundary) are needed.
        let t = page_start_of(1_790_942_400) + 60;
        src.prefetch(&needs(QuoteAsset::Usdt, t)).await;
        let o = src.usd_price(QuoteAsset::Usdt, t);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::Provider(PriceErrorClass::NotFound)
            }
        );
        assert!(o.value.is_none());
        assert_eq!(src.requests_made(), 2);
    }

    #[tokio::test]
    async fn server_errors_are_retried_then_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        let t = page_start_of(1_790_942_400) + 600;
        let s = src.prefetch(&needs(QuoteAsset::Sol, t)).await;
        assert_eq!(s.pages_failed, 1);
        assert_eq!(src.requests_made(), 3, "3 attempts for the one page");
        let o = src.usd_price(QuoteAsset::Sol, t);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::Provider(PriceErrorClass::HttpServer(503))
            }
        );
    }

    #[tokio::test]
    async fn transient_error_then_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
            .mount(&server)
            .await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        src.prefetch(&needs(QuoteAsset::Sol, 1_790_942_400)).await;
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_400);
        assert_eq!(o.label, PriceLabel::CexReference1m);
        assert!(src.requests_made() >= 2);
    }

    #[tokio::test]
    async fn malformed_and_oversized_bodies_are_classified() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("start", iso_utc(page_start_of(1_790_942_400))))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"x\":1}"))
            .mount(&server)
            .await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        src.prefetch(&needs(QuoteAsset::Sol, 1_790_942_400)).await;
        assert_eq!(
            src.usd_price(QuoteAsset::Sol, 1_790_942_400).label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::Provider(PriceErrorClass::MalformedBody)
            }
        );
        let server2 = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
            .mount(&server2)
            .await;
        let mut c = cfg(&server2);
        c.max_response_bytes = 20;
        let src = CoinbasePriceSource::new(c).unwrap();
        src.prefetch(&needs(QuoteAsset::Sol, 1_790_942_400)).await;
        assert_eq!(
            src.usd_price(QuoteAsset::Sol, 1_790_942_400).label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::Provider(PriceErrorClass::ResponseTooLarge)
            }
        );
    }

    #[tokio::test]
    async fn request_budget_bounds_total_attempts_and_labels_the_rest() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
            .mount(&server)
            .await;
        let mut c = cfg(&server);
        c.max_requests = Some(1);
        let src = CoinbasePriceSource::new(c).unwrap();
        // Two distant minutes -> at least 2 distinct pages; only 1 request allowed.
        let t1 = 1_790_942_400;
        let t2 = t1 + 10 * PAGE_SECONDS;
        let n = BTreeMap::from([(QuoteAsset::Sol, BTreeSet::from([t1, t2]))]);
        let s = src.prefetch(&n).await;
        assert_eq!(src.requests_made(), 1);
        assert!(s.pages_skipped_budget >= 1);
        let o = src.usd_price(QuoteAsset::Sol, t2);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::BudgetExhausted
            }
        );
    }

    #[tokio::test]
    async fn unprefetched_minute_is_unknown_not_zero() {
        let server = MockServer::start().await;
        let src = CoinbasePriceSource::new(cfg(&server)).unwrap();
        let o = src.usd_price(QuoteAsset::Sol, 1_790_942_400);
        assert_eq!(
            o.label,
            PriceLabel::Unknown {
                reason: UnknownPriceReason::NotPrefetched
            }
        );
    }

    #[tokio::test]
    async fn page_cache_cap_is_enforced() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .mount(&server)
            .await;
        let mut c = cfg(&server);
        c.max_cached_pages = 1;
        let src = CoinbasePriceSource::new(c).unwrap();
        let t1 = 1_790_942_400;
        let t2 = t1 + 10 * PAGE_SECONDS;
        let n = BTreeMap::from([(QuoteAsset::Sol, BTreeSet::from([t1, t2]))]);
        let s = src.prefetch(&n).await;
        assert_eq!(s.pages_fetched, 1);
        assert!(s.pages_skipped_cache_full >= 1);
    }
}
