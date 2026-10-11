//! Bounded Birdeye price history adapter. It reports USD candle highs,
//! not token FDV or lifetime ATH.

use std::{cmp::Ordering, time::Duration};

use serde::Deserialize;
use serde_json::value::RawValue;

const DAY_SECONDS: i64 = 86_400;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CANDLES: usize = 5_000;

/// Explicit candle buckets for day and intraday supply reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandleInterval {
    Day,
    Minute,
    Second,
}

impl CandleInterval {
    const fn label(self) -> &'static str {
        match self {
            Self::Day => "1D",
            Self::Minute => "1m",
            Self::Second => "1s",
        }
    }

    const fn seconds(self) -> i64 {
        match self {
            Self::Day => DAY_SECONDS,
            Self::Minute => 60,
            Self::Second => 1,
        }
    }
}

/// Supported Birdeye chain identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BirdeyeChain {
    Solana,
    Bsc,
    Base,
    Robinhood,
}

impl BirdeyeChain {
    fn header_value(self) -> &'static str {
        match self {
            Self::Solana => "solana",
            Self::Bsc => "bsc",
            Self::Base => "base",
            Self::Robinhood => "robinhood",
        }
    }

    fn valid_address(self, address: &str) -> bool {
        match self {
            Self::Solana => bs58::decode(address)
                .into_vec()
                .is_ok_and(|bytes| bytes.len() == 32),
            Self::Bsc | Self::Base | Self::Robinhood => {
                address.len() == 42
                    && address.starts_with("0x")
                    && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
            }
        }
    }

    fn same_address(self, left: &str, right: &str) -> bool {
        match self {
            Self::Solana => left == right,
            Self::Bsc | Self::Base | Self::Robinhood => left.eq_ignore_ascii_case(right),
        }
    }
}

/// Requested inclusive UTC Unix-time range. Candles are checked against the
/// selected interval's UTC buckets containing these endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OhlcvWindow {
    pub chain: BirdeyeChain,
    pub address: String,
    pub time_from: i64,
    pub time_to: i64,
}

/// Requested and observed boundaries for one returned candle window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OhlcvTimeScope {
    pub requested_from: i64,
    pub requested_to: i64,
    pub first_candle_at: i64,
    pub last_candle_at: i64,
}

/// Highest Birdeye USD candle price in the requested range. This is a price,
/// not an ATH FDV; callers need a validated supply before deriving FDV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPriceHigh {
    pub chain: BirdeyeChain,
    pub address: String,
    pub max_price_usd: String,
    pub candle_at: i64,
    pub returned_candles: usize,
    pub time_scope: OhlcvTimeScope,
}

/// Selected, exact fields from Birdeye token overview. Supply and price are
/// observations only; callers must verify supply independently before FDV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOverview {
    pub chain: BirdeyeChain,
    pub address: String,
    pub total_supply: Option<String>,
    pub decimals: Option<u8>,
    pub holders: Option<u64>,
    pub liquidity_usd: Option<String>,
    pub price_usd: Option<String>,
    pub market_cap_usd: Option<String>,
    pub current_fdv_usd: Option<String>,
    pub circulating_supply: Option<String>,
    pub is_scaled_ui_token: Option<bool>,
    pub multiplier: Option<String>,
}

/// Exact nonnegative decimal represented as coefficient × 10^-scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactDecimal {
    coefficient: u128,
    scale: i32,
}

impl ExactDecimal {
    /// Parse a JSON number lexeme without passing through floating point.
    pub fn parse_json_number(value: &RawValue) -> Result<Self, BirdeyeError> {
        Self::parse(value.get())
    }

    /// Parse a plain or scientific nonnegative decimal with at most 38
    /// coefficient digits and a decimal exponent in [-38, 38]. Effective
    /// scale may reach 76 when a 38-digit fraction combines with `e-38`.
    pub fn parse(value: &str) -> Result<Self, BirdeyeError> {
        let (mantissa, exponent_text) = match value.find(['e', 'E']) {
            Some(index) => (&value[..index], Some(&value[index + 1..])),
            None => (value, None),
        };
        let exponent = exponent_text.map_or(Ok(0), |text| {
            let exponent = text
                .parse::<i32>()
                .map_err(|_| BirdeyeError::InvalidDecimal)?;
            if !(-38..=38).contains(&exponent) {
                return Err(BirdeyeError::InvalidDecimal);
            }
            Ok(exponent)
        })?;
        if mantissa.starts_with('-') || mantissa.starts_with('+') {
            return Err(BirdeyeError::InvalidDecimal);
        }
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            || mantissa.contains('.') && fraction.is_empty()
        {
            return Err(BirdeyeError::InvalidDecimal);
        }
        let digits = format!("{whole}{fraction}");
        let significant_digits = digits.trim_start_matches('0');
        if significant_digits.len() > 38 {
            return Err(BirdeyeError::InvalidDecimal);
        }
        let coefficient = if significant_digits.is_empty() {
            "0"
        } else {
            significant_digits
        }
        .parse::<u128>()
        .map_err(|_| BirdeyeError::InvalidDecimal)?;
        let mut scale = i32::try_from(fraction.len())
            .map_err(|_| BirdeyeError::InvalidDecimal)?
            .checked_sub(exponent)
            .ok_or(BirdeyeError::InvalidDecimal)?;
        if !(-38..=76).contains(&scale) {
            return Err(BirdeyeError::InvalidDecimal);
        }
        let mut coefficient = coefficient;
        if coefficient == 0 {
            return Ok(Self {
                coefficient: 0,
                scale: 0,
            });
        }
        while scale > 0 && coefficient != 0 && coefficient % 10 == 0 {
            coefficient /= 10;
            scale -= 1;
        }
        Ok(Self { coefficient, scale })
    }

    /// Exact decimal spelling without exponent notation.
    pub fn to_plain_string(self) -> String {
        let digits = self.coefficient.to_string();
        if self.scale <= 0 {
            let zeroes = usize::try_from(self.scale.unsigned_abs()).unwrap_or_default();
            return format!("{}{}", digits, "0".repeat(zeroes));
        }
        let scale = usize::try_from(self.scale).unwrap_or_default();
        if digits.len() <= scale {
            format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
        } else {
            let split = digits.len() - scale;
            format!("{}.{}", &digits[..split], &digits[split..])
        }
    }

    /// Floor USD price × validated whole-token supply into integer cents.
    /// Overflow returns `None`; no rounding or float conversion is performed.
    #[allow(clippy::integer_division)]
    pub fn fdv_cents_for_supply(self, supply: u128) -> Option<i64> {
        let mut numerator = self.coefficient.checked_mul(supply)?.checked_mul(100)?;
        if self.scale < 0 {
            numerator = numerator.checked_mul(10u128.checked_pow(self.scale.unsigned_abs())?)?;
        }
        let cents = if self.scale > 0 {
            // 10^39 exceeds u128::MAX, so every representable numerator is
            // strictly below the denominator at these scales.
            if self.scale > 38 {
                return Some(0);
            }
            numerator / 10u128.checked_pow(u32::try_from(self.scale).ok()?)?
        } else {
            numerator
        };
        i64::try_from(cents).ok()
    }

    fn compare(self, other: Self) -> Ordering {
        if self.coefficient == 0 || other.coefficient == 0 {
            return self.coefficient.cmp(&other.coefficient);
        }
        let left = self.coefficient.to_string();
        let right = other.coefficient.to_string();
        let left_magnitude = i32::try_from(left.len()).unwrap_or(i32::MAX) - self.scale;
        let right_magnitude = i32::try_from(right.len()).unwrap_or(i32::MAX) - other.scale;
        match left_magnitude.cmp(&right_magnitude) {
            Ordering::Equal => {
                let width = left.len().max(right.len());
                let left_aligned = format!("{left:0<width$}");
                let right_aligned = format!("{right:0<width$}");
                left_aligned.cmp(&right_aligned)
            }
            order => order,
        }
    }
}

/// Birdeye adapter errors. HTTP status bodies are intentionally not retained.
#[derive(Debug, thiserror::Error)]
pub enum BirdeyeError {
    #[error("invalid Birdeye window or address")]
    InvalidRequest,
    #[error("Birdeye transport: {0}")]
    Transport(String),
    #[error("Birdeye denied the request (HTTP {0})")]
    AccessDenied(u16),
    #[error("Birdeye rate limited the request (HTTP {0})")]
    RateLimited(u16),
    #[error("Birdeye returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Birdeye response exceeded 2097152 bytes")]
    ResponseTooLarge,
    #[error("Birdeye response JSON or schema is invalid")]
    InvalidResponse,
    #[error("Birdeye reported success=false")]
    Unsuccessful,
    #[error("Birdeye candle is invalid for the requested window")]
    InvalidCandle,
    #[error("Birdeye response address does not match the requested token")]
    WrongAddress,
    #[error("Birdeye scaled UI prices or nontrivial multipliers are unsupported")]
    UnsupportedScale,
    #[error("Birdeye decimal is invalid or exceeds exact bounds")]
    InvalidDecimal,
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    data: Option<Data>,
}

#[derive(Deserialize)]
struct Data {
    items: Vec<Candle>,
    #[serde(default)]
    is_scaled_ui_token: Option<bool>,
    #[serde(default)]
    multiplier: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct Candle {
    address: String,
    unix_time: i64,
    #[serde(rename = "type")]
    candle_type: String,
    currency: String,
    o: Box<RawValue>,
    h: Box<RawValue>,
    l: Box<RawValue>,
    c: Box<RawValue>,
    v: Box<RawValue>,
    v_usd: Box<RawValue>,
}

#[derive(Deserialize)]
struct OverviewEnvelope {
    success: bool,
    data: Option<OverviewData>,
}

#[derive(Deserialize)]
struct OverviewData {
    address: String,
    #[serde(rename = "totalSupply")]
    total_supply: Option<Box<RawValue>>,
    #[serde(rename = "circulatingSupply")]
    circulating_supply: Option<Box<RawValue>>,
    decimals: Option<Box<RawValue>>,
    holder: Option<Box<RawValue>>,
    liquidity: Option<Box<RawValue>>,
    price: Option<Box<RawValue>>,
    #[serde(rename = "marketCap")]
    market_cap: Option<Box<RawValue>>,
    fdv: Option<Box<RawValue>>,
    #[serde(rename = "isScaledUiToken")]
    is_scaled_ui_token: Option<bool>,
    multiplier: Option<Box<RawValue>>,
}

async fn bounded_response(request: reqwest::RequestBuilder) -> Result<Vec<u8>, BirdeyeError> {
    let mut response = request
        .send()
        .await
        .map_err(|error| BirdeyeError::Transport(error.to_string()))?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(BirdeyeError::AccessDenied(status.as_u16()));
    }
    if status.as_u16() == 429 {
        return Err(BirdeyeError::RateLimited(status.as_u16()));
    }
    if !status.is_success() {
        return Err(BirdeyeError::HttpStatus(status.as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| BirdeyeError::Transport(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(BirdeyeError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn optional_exact_decimal(value: Option<Box<RawValue>>) -> Result<Option<String>, BirdeyeError> {
    value
        .map(|raw| {
            let text = if raw.get().starts_with('"') {
                serde_json::from_str::<String>(raw.get())
                    .map_err(|_| BirdeyeError::InvalidResponse)?
            } else {
                raw.get().to_owned()
            };
            ExactDecimal::parse(&text).map(ExactDecimal::to_plain_string)
        })
        .transpose()
}

fn optional_multiplier(value: Option<Box<RawValue>>) -> Result<Option<String>, BirdeyeError> {
    let value = optional_exact_decimal(value)?;
    if value.as_deref().is_some_and(|multiplier| multiplier != "1") {
        return Err(BirdeyeError::UnsupportedScale);
    }
    Ok(value)
}

fn optional_unsigned_integer<T>(value: Option<Box<RawValue>>) -> Result<Option<T>, BirdeyeError>
where
    T: std::str::FromStr,
{
    value
        .map(|raw| {
            raw.get()
                .parse::<T>()
                .map_err(|_| BirdeyeError::InvalidResponse)
        })
        .transpose()
}

/// Fetch a bounded daily OHLCV window and return its highest USD candle high.
/// The caller supplies the API key, client, and endpoint explicitly.
/// Empty `items` returns `Ok(None)`.
///
/// # Errors
/// Invalid request, transport/status failure, oversized or malformed response,
/// or an invalid candle.
pub async fn fetch_window_price_high(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    request: &OhlcvWindow,
) -> Result<Option<WindowPriceHigh>, BirdeyeError> {
    fetch_candle_price_high(http, endpoint, api_key, request, CandleInterval::Day).await
}

/// Fetch a bounded USD price high at an explicit candle interval.
/// Second-level requests are supported only on Solana. A candle bucket still
/// does not establish transaction order or supply at an intrabucket trade.
///
/// # Errors
/// Invalid request/interval, transport/status failure, oversized or malformed
/// response, or an invalid candle.
pub async fn fetch_candle_price_high(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    request: &OhlcvWindow,
    interval: CandleInterval,
) -> Result<Option<WindowPriceHigh>, BirdeyeError> {
    if api_key.is_empty()
        || !request.chain.valid_address(&request.address)
        || request.time_from < 0
        || request.time_to < request.time_from
        || interval == CandleInterval::Second && request.chain != BirdeyeChain::Solana
    {
        return Err(BirdeyeError::InvalidRequest);
    }
    let request_builder = http
        .get(endpoint)
        .header("X-API-KEY", api_key)
        .header("x-chain", request.chain.header_value())
        .query(&[
            ("address", request.address.as_str()),
            ("type", interval.label()),
            ("currency", "usd"),
            ("time_from", &request.time_from.to_string()),
            ("time_to", &request.time_to.to_string()),
            ("mode", "range"),
            ("padding", "false"),
            ("outlier", "true"),
        ])
        .timeout(Duration::from_secs(25));
    let body = bounded_response(request_builder).await?;
    let envelope: Envelope =
        serde_json::from_slice(&body).map_err(|_| BirdeyeError::InvalidResponse)?;
    if !envelope.success {
        return Err(BirdeyeError::Unsuccessful);
    }
    let data = envelope.data.ok_or(BirdeyeError::InvalidResponse)?;
    if data.is_scaled_ui_token == Some(true) {
        return Err(BirdeyeError::UnsupportedScale);
    }
    optional_multiplier(data.multiplier)?;
    let items = data.items;
    if items.len() > MAX_CANDLES {
        return Err(BirdeyeError::InvalidResponse);
    }
    if items.is_empty() {
        return Ok(None);
    }
    let seconds = interval.seconds();
    let from_bucket = request.time_from.div_euclid(seconds) * seconds;
    let to_bucket = request.time_to.div_euclid(seconds) * seconds;
    let mut previous = None;
    let mut maximum: Option<(ExactDecimal, i64)> = None;
    for candle in &items {
        if !request
            .chain
            .same_address(&candle.address, &request.address)
            || candle.candle_type != interval.label()
            || candle.currency != "usd"
            || candle.unix_time < from_bucket
            || candle.unix_time > to_bucket
            || candle.unix_time.rem_euclid(seconds) != 0
            || previous.is_some_and(|time| candle.unix_time <= time)
        {
            return Err(BirdeyeError::InvalidCandle);
        }
        previous = Some(candle.unix_time);
        let open = ExactDecimal::parse_json_number(&candle.o)?;
        let high = ExactDecimal::parse_json_number(&candle.h)?;
        let low = ExactDecimal::parse_json_number(&candle.l)?;
        let close = ExactDecimal::parse_json_number(&candle.c)?;
        let volume = ExactDecimal::parse_json_number(&candle.v)?;
        let volume_usd = ExactDecimal::parse_json_number(&candle.v_usd)?;
        if low.compare(open) == Ordering::Greater
            || low.compare(close) == Ordering::Greater
            || high.compare(open) == Ordering::Less
            || high.compare(close) == Ordering::Less
        {
            return Err(BirdeyeError::InvalidCandle);
        }
        let _ = (volume, volume_usd);
        if maximum.is_none_or(|(current, _)| high.compare(current) == Ordering::Greater) {
            maximum = Some((high, candle.unix_time));
        }
    }
    let (price, candle_at) = maximum.ok_or(BirdeyeError::InvalidResponse)?;
    Ok(Some(WindowPriceHigh {
        chain: request.chain,
        address: request.address.clone(),
        max_price_usd: price.to_plain_string(),
        candle_at,
        returned_candles: items.len(),
        time_scope: OhlcvTimeScope {
            requested_from: request.time_from,
            requested_to: request.time_to,
            first_candle_at: items
                .first()
                .ok_or(BirdeyeError::InvalidResponse)?
                .unix_time,
            last_candle_at: items.last().ok_or(BirdeyeError::InvalidResponse)?.unix_time,
        },
    }))
}

/// Fetch selected token-overview fields without performing hidden setup.
/// Supply and price remain provider observations and are not promoted to FDV.
///
/// # Errors
/// Invalid address, transport/status failure, oversized/malformed response, or
/// mismatched response identity.
pub async fn fetch_token_overview(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    chain: BirdeyeChain,
    address: &str,
) -> Result<TokenOverview, BirdeyeError> {
    if api_key.is_empty() || !chain.valid_address(address) {
        return Err(BirdeyeError::InvalidRequest);
    }
    let request = http
        .get(endpoint)
        .header("X-API-KEY", api_key)
        .header("x-chain", chain.header_value())
        .query(&[("address", address)])
        .timeout(Duration::from_secs(25));
    let body = bounded_response(request).await?;
    let envelope: OverviewEnvelope =
        serde_json::from_slice(&body).map_err(|_| BirdeyeError::InvalidResponse)?;
    if !envelope.success {
        return Err(BirdeyeError::Unsuccessful);
    }
    let data = envelope.data.ok_or(BirdeyeError::InvalidResponse)?;
    if !chain.same_address(&data.address, address) {
        return Err(BirdeyeError::WrongAddress);
    }
    let decimals: Option<u8> = optional_unsigned_integer(data.decimals)?;
    let holders: Option<u64> = optional_unsigned_integer(data.holder)?;
    Ok(TokenOverview {
        chain,
        address: address.to_owned(),
        total_supply: optional_exact_decimal(data.total_supply)?,
        decimals,
        holders,
        liquidity_usd: optional_exact_decimal(data.liquidity)?,
        price_usd: optional_exact_decimal(data.price)?,
        market_cap_usd: optional_exact_decimal(data.market_cap)?,
        current_fdv_usd: optional_exact_decimal(data.fdv)?,
        circulating_supply: optional_exact_decimal(data.circulating_supply)?,
        is_scaled_ui_token: data.is_scaled_ui_token,
        multiplier: optional_exact_decimal(data.multiplier)?,
    })
}

/// Build a reusable HTTP client with bounded connection and request timeouts.
///
/// # Errors
/// Client builder error.
pub fn http_client() -> Result<reqwest::Client, BirdeyeError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|error| BirdeyeError::Transport(error.to_string()))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const BASE_ADDRESS: &str = "0xa187403aDCc6FCDF64fE9C850e0d9fAf4bA3837f";

    fn request(chain: BirdeyeChain, address: &str) -> OhlcvWindow {
        OhlcvWindow {
            chain,
            address: address.to_string(),
            time_from: 1_760_054_400,
            time_to: 1_791_681_286,
        }
    }

    fn response(items: serde_json::Value) -> serde_json::Value {
        json!({"success": true, "data": {"items": items}})
    }

    #[test]
    fn swap_fx_fixture_corroborates_exact_fdv_without_assuming_a_usdc_peg() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/birdeye_swap_fx_validated_fdv_cases.json"
        ))
        .unwrap();
        for row in value["cases"].as_array().unwrap() {
            let supply = row["supply_tokens"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let expected = row["expected_fdv_cents"].as_u64().unwrap();
            let price = ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
            assert_eq!(
                price.fdv_cents_for_supply(supply),
                Some(i64::try_from(expected).unwrap())
            );
            let quote = row["usdc_raw"].as_str().unwrap().parse::<u128>().unwrap();
            let token = row["token_raw"].as_str().unwrap().parse::<u128>().unwrap();
            let numerator = quote
                .checked_mul(1_000_000_000_000)
                .unwrap()
                .checked_mul(supply)
                .unwrap()
                .checked_mul(100)
                .unwrap();
            let denominator = token.checked_mul(10_000).unwrap();
            for rate in ["usdc_usd_low_1e4", "usdc_usd_high_1e4"] {
                let converted = numerator
                    .checked_mul(u128::from(row[rate].as_u64().unwrap()))
                    .unwrap()
                    .checked_div(denominator)
                    .unwrap();
                assert!(
                    converted
                        .abs_diff(u128::from(expected))
                        .checked_mul(100)
                        .unwrap()
                        <= u128::from(expected)
                );
            }
        }
    }

    #[tokio::test]
    async fn intraday_high_checks_interval_identity_and_bucket_alignment() {
        const SOL: &str = "48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump";
        let fixture = include_str!("../tests/fixtures/birdeye_solana_minute_ohlcv.json");
        let server = MockServer::start().await;
        Mock::given(matchers::query_param("type", "1m"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
            .mount(&server)
            .await;
        let client = http_client().unwrap();
        let endpoint = server.uri();
        let mut window = request(BirdeyeChain::Solana, SOL);
        window.time_to = 1_760_054_581;
        let high =
            fetch_candle_price_high(&client, &endpoint, "test", &window, CandleInterval::Minute)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(high.max_price_usd, "0.00012179988071982598");
        assert_eq!(high.candle_at, 1_760_054_460);
        assert_eq!(high.time_scope.last_candle_at, 1_760_054_520);
        for (kind, time, expected) in [
            ("1s", 1_760_054_461, true),
            ("1m", 1_760_054_461, false),
            ("1D", 1_760_054_460, false),
        ] {
            let server = MockServer::start().await;
            let value = fixture
                .replace("\"type\":\"1m\"", &format!("\"type\":\"{kind}\""))
                .replace("1760054460", &time.to_string());
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(value, "application/json"))
                .mount(&server)
                .await;
            let interval = if kind == "1s" {
                CandleInterval::Second
            } else {
                CandleInterval::Minute
            };
            let result =
                fetch_candle_price_high(&client, &server.uri(), "test", &window, interval).await;
            if expected {
                assert_eq!(result.unwrap().unwrap().candle_at, time);
            } else {
                assert!(matches!(result, Err(BirdeyeError::InvalidCandle)));
            }
        }
        assert!(matches!(
            fetch_candle_price_high(
                &client,
                &endpoint,
                "test",
                &request(BirdeyeChain::Base, BASE_ADDRESS),
                CandleInterval::Second
            )
            .await,
            Err(BirdeyeError::InvalidRequest)
        ));
    }

    #[tokio::test]
    async fn base_fixture_returns_price_high_and_exact_scope() {
        let server = MockServer::start().await;
        let fixture = include_str!("../tests/fixtures/birdeye_base_daily_ohlcv.json");
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/defi/v3/ohlcv"))
            .and(matchers::header("x-chain", "base"))
            .and(matchers::header("X-API-KEY", "test-explicit-key"))
            .and(matchers::query_param("type", "1D"))
            .and(matchers::query_param("currency", "usd"))
            .and(matchers::query_param("mode", "range"))
            .and(matchers::query_param("padding", "false"))
            .and(matchers::query_param("outlier", "true"))
            .and(matchers::query_param("time_from", "1760054400"))
            .and(matchers::query_param("time_to", "1791681286"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
            .mount(&server)
            .await;
        let result = fetch_window_price_high(
            &http_client().unwrap(),
            &format!("{}/defi/v3/ohlcv", server.uri()),
            "test-explicit-key",
            &request(BirdeyeChain::Base, BASE_ADDRESS),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.chain, BirdeyeChain::Base);
        assert_eq!(result.address, BASE_ADDRESS);
        assert_eq!(result.max_price_usd, "0.00005642741802520651");
        assert_eq!(result.candle_at, 1_760_054_400);
        assert_eq!(result.returned_candles, 3);
        assert_eq!(result.time_scope.first_candle_at, 1_760_054_400);
        assert_eq!(result.time_scope.last_candle_at, 1_791_590_400);
    }

    #[test]
    fn exact_decimal_handles_scientific_notation_and_checked_fdv() {
        let tiny = ExactDecimal::parse("1.25e-20").unwrap();
        assert_eq!(tiny.to_plain_string(), "0.0000000000000000000125");
        assert_eq!(
            tiny.fdv_cents_for_supply(1_000_000_000_000_000_000),
            Some(1)
        );
        let ordinary = ExactDecimal::parse("1.234e2").unwrap();
        assert_eq!(ordinary.to_plain_string(), "123.4");
        assert_eq!(ordinary.fdv_cents_for_supply(2), Some(24_680));
        assert_eq!(
            ExactDecimal::parse("0.9")
                .unwrap()
                .compare(ExactDecimal::parse("0.89").unwrap()),
            Ordering::Greater
        );
        assert_eq!(
            ExactDecimal::parse("0")
                .unwrap()
                .compare(ExactDecimal::parse("0.1").unwrap()),
            Ordering::Less
        );
        assert_eq!(
            ExactDecimal::parse("0.000e-20").unwrap().to_plain_string(),
            "0"
        );
        let max_scale = ExactDecimal::parse(&format!("0.{}e-38", "0".repeat(37) + "1")).unwrap();
        assert_eq!(
            max_scale.to_plain_string(),
            format!("0.{}1", "0".repeat(75))
        );
        assert_eq!(max_scale.fdv_cents_for_supply(1), Some(0));
        assert!(ExactDecimal::parse("-1").is_err());
        assert!(ExactDecimal::parse("1e39").is_err());
        assert!(ExactDecimal::parse("1e-39").is_err());
        assert!(ExactDecimal::parse(&format!("0.{}e-39", "0".repeat(37) + "1")).is_err());
        assert!(ExactDecimal::parse("1.").is_err());
        assert!(ExactDecimal::parse("123456789012345678901234567890123456789").is_err());
        assert_eq!(
            ExactDecimal::parse("99999999999999999999999999999999999999")
                .unwrap()
                .fdv_cents_for_supply(u128::MAX),
            None
        );
    }

    #[test]
    fn validated_supply_case_has_exact_fdv_without_claiming_ath_time() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/birdeye_validated_fdv_case.json"
        ))
        .unwrap();
        assert_eq!(fixture["chain"], "base");
        assert_eq!(
            fixture["token"],
            "0xe44861a39b8f73040455fa618aa921907d450b07"
        );
        assert_eq!(fixture["ath_at"], serde_json::Value::Null);
        let price = ExactDecimal::parse(fixture["max_price_usd"].as_str().unwrap()).unwrap();
        let supply = fixture["verified_total_supply"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        assert_eq!(
            price.fdv_cents_for_supply(supply),
            Some(fixture["fdv_cents"].as_i64().unwrap())
        );
        assert_eq!(price.fdv_cents_for_supply(supply), Some(51_104_374_107));
    }

    #[tokio::test]
    async fn very_small_scientific_candle_prices_preserve_exact_maximum() {
        let server = MockServer::start().await;
        let fixture = include_str!("../tests/fixtures/birdeye_precision_extreme_scale.json");
        Mock::given(matchers::method("GET"))
            .and(matchers::header("x-chain", "base"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
            .mount(&server)
            .await;
        let request = OhlcvWindow {
            chain: BirdeyeChain::Base,
            address: "0x109585ED25e91720f97C188E4de023F770C9E474".into(),
            time_from: 1_790_208_000,
            time_to: 1_790_294_400,
        };
        let result =
            fetch_window_price_high(&http_client().unwrap(), &server.uri(), "test-key", &request)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(result.returned_candles, 2);
        assert_eq!(result.max_price_usd, "0.0000008111506335367991");
        assert_eq!(result.candle_at, 1_790_208_000);
        assert_eq!(
            ExactDecimal::parse(&result.max_price_usd)
                .unwrap()
                .fdv_cents_for_supply(1_000_000_000),
            Some(81_115)
        );
        assert_eq!(
            ExactDecimal::parse("1.7204350703333583e-30")
                .unwrap()
                .fdv_cents_for_supply(1_000_000_000),
            Some(0)
        );
    }

    #[tokio::test]
    async fn all_chain_headers_are_explicit_and_empty_is_unknown() {
        for (chain, address, header) in [
            (BirdeyeChain::Base, BASE_ADDRESS, "base"),
            (
                BirdeyeChain::Bsc,
                "0xa187403aDCc6FCDF64fE9C850e0d9fAf4bA3837f",
                "bsc",
            ),
            (
                BirdeyeChain::Robinhood,
                "0xa187403aDCc6FCDF64fE9C850e0d9fAf4bA3837f",
                "robinhood",
            ),
            (
                BirdeyeChain::Solana,
                "So11111111111111111111111111111111111111112",
                "solana",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .and(matchers::header("x-chain", header))
                .respond_with(ResponseTemplate::new(200).set_body_json(response(json!([]))))
                .mount(&server)
                .await;
            assert!(
                fetch_window_price_high(
                    &http_client().unwrap(),
                    &server.uri(),
                    "explicit",
                    &request(chain, address),
                )
                .await
                .unwrap()
                .is_none()
            );
        }
    }

    #[tokio::test]
    async fn overview_preserves_exact_optional_fields_and_chain_identity() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::header("x-chain", "base"))
            .and(matchers::query_param("address", BASE_ADDRESS))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"success":true,"data":{{"address":"{}","totalSupply":"1000000.000000000000000001","decimals":18,"holder":1234,"liquidity":1.25e-20,"price":null,"name":"ignored"}}}}"#,
                    BASE_ADDRESS.to_ascii_lowercase()
                ),
                "application/json",
            ))
            .mount(&server)
            .await;
        let overview = fetch_token_overview(
            &http_client().unwrap(),
            &server.uri(),
            "explicit",
            BirdeyeChain::Base,
            BASE_ADDRESS,
        )
        .await
        .unwrap();
        assert_eq!(overview.chain, BirdeyeChain::Base);
        assert_eq!(overview.address, BASE_ADDRESS);
        assert_eq!(
            overview.total_supply.as_deref(),
            Some("1000000.000000000000000001")
        );
        assert_eq!(overview.decimals, Some(18));
        assert_eq!(overview.holders, Some(1234));
        assert_eq!(
            overview.liquidity_usd.as_deref(),
            Some("0.0000000000000000000125")
        );
        assert_eq!(overview.price_usd, None);
    }

    #[tokio::test]
    async fn archived_overview_fixtures_preserve_optional_supply_and_market_fields() {
        let cases = [
            (
                BirdeyeChain::Bsc,
                "0x5B02F9454c368CAf86869880d5db819809B9fFfF",
                include_str!("../tests/fixtures/birdeye_overview_bsc.json"),
                Some("1000000000"),
                None,
                Some("2245.7377020135796"),
                Some("2245.7377020135796"),
                Some("0.0000022457377020135798"),
                Some("1000000000"),
            ),
            (
                BirdeyeChain::Base,
                "0x6801E54BFd612319A9109a10b574657cE36d7fD3",
                include_str!("../tests/fixtures/birdeye_overview_base.json"),
                Some("99999999999.99998"),
                None,
                None,
                None,
                None,
                Some("99999999999.99998"),
            ),
            (
                BirdeyeChain::Robinhood,
                "0x622A9F696bF75c53AbAe9315C8EF4560eaf61E18",
                include_str!("../tests/fixtures/birdeye_overview_robinhood.json"),
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                BirdeyeChain::Solana,
                "4evPrwwvZq8qzigkq3Q6zQN7fcYFwoDT7CstCRNEpump",
                include_str!("../tests/fixtures/birdeye_overview_solana.json"),
                Some("1121758849.983755"),
                Some(5),
                Some("235.64391068826725"),
                Some("235.64391068826725"),
                Some("0.00000021006646008781635"),
                Some("1121758849.983755"),
            ),
        ];
        for (chain, address, fixture, supply, holders, market_cap, fdv, price, circulating) in cases
        {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .and(matchers::header("x-chain", chain.header_value()))
                .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
                .mount(&server)
                .await;
            let overview = fetch_token_overview(
                &http_client().unwrap(),
                &server.uri(),
                "explicit",
                chain,
                address,
            )
            .await
            .unwrap();
            assert_eq!(overview.total_supply.as_deref(), supply);
            assert_eq!(overview.holders, holders);
            assert_eq!(overview.market_cap_usd.as_deref(), market_cap);
            assert_eq!(overview.current_fdv_usd.as_deref(), fdv);
            assert_eq!(overview.price_usd.as_deref(), price);
            assert_eq!(overview.circulating_supply.as_deref(), circulating);
        }
    }

    #[tokio::test]
    async fn overview_rejects_mismatched_address_and_malformed_numeric_fields() {
        let http = http_client().unwrap();
        for data in [
            json!({"address": "0x0000000000000000000000000000000000000000"}),
            json!({"address": BASE_ADDRESS, "totalSupply": -1}),
            json!({"address": BASE_ADDRESS, "holder": -1}),
            json!({"address": BASE_ADDRESS, "decimals": 256}),
            json!({"address": BASE_ADDRESS, "liquidity": "NaN"}),
        ] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "success": true, "data": data
                })))
                .mount(&server)
                .await;
            let result = fetch_token_overview(
                &http,
                &server.uri(),
                "explicit",
                BirdeyeChain::Base,
                BASE_ADDRESS,
            )
            .await;
            assert!(matches!(
                result,
                Err(BirdeyeError::WrongAddress
                    | BirdeyeError::InvalidResponse
                    | BirdeyeError::InvalidDecimal)
            ));
        }
        let sol_address = "So11111111111111111111111111111111111111112";
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true, "data": {"address": sol_address.to_ascii_lowercase()}
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_token_overview(
                &http,
                &server.uri(),
                "explicit",
                BirdeyeChain::Solana,
                sol_address,
            )
            .await,
            Err(BirdeyeError::WrongAddress)
        ));
    }

    #[tokio::test]
    async fn response_cap_status_and_invalid_schema_are_typed() {
        let http = http_client().unwrap();
        let req = request(BirdeyeChain::Base, BASE_ADDRESS);
        for (status, expected) in [(403, 0_u8), (429, 1_u8)] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string("redacted"))
                .mount(&server)
                .await;
            let result = fetch_window_price_high(&http, &server.uri(), "explicit", &req).await;
            assert!(matches!(
                (expected, result),
                (0, Err(BirdeyeError::AccessDenied(403)))
                    | (1, Err(BirdeyeError::RateLimited(429)))
            ));
        }
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(" ".repeat(MAX_RESPONSE_BYTES + 1)),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_window_price_high(&http, &server.uri(), "explicit", &req).await,
            Err(BirdeyeError::ResponseTooLarge)
        ));
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": false})))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_window_price_high(&http, &server.uri(), "explicit", &req).await,
            Err(BirdeyeError::Unsuccessful)
        ));
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true, "data": {"items": [{"address": BASE_ADDRESS}]}
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_window_price_high(&http, &server.uri(), "explicit", &req).await,
            Err(BirdeyeError::InvalidResponse)
        ));
        for data in [
            json!({"items": [], "is_scaled_ui_token": true, "multiplier": null}),
            json!({"items": [], "is_scaled_ui_token": false, "multiplier": 1.25}),
        ] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "success": true, "data": data
                })))
                .mount(&server)
                .await;
            assert!(matches!(
                fetch_window_price_high(&http, &server.uri(), "explicit", &req).await,
                Err(BirdeyeError::UnsupportedScale)
            ));
        }
        let items = vec![
            json!({
                "address": BASE_ADDRESS, "unix_time": 1_760_054_400,
                "o": 1, "h": 2, "l": 1, "c": 2, "v": 1, "v_usd": 1,
                "type": "1D", "currency": "usd"
            });
            MAX_CANDLES + 1
        ];
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response(json!(items))))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_window_price_high(&http, &server.uri(), "explicit", &req).await,
            Err(BirdeyeError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn rejects_bad_addresses_network_identity_out_of_window_and_candle_math() {
        let http = http_client().unwrap();
        let bad_address = request(BirdeyeChain::Base, "bad");
        assert!(matches!(
            fetch_window_price_high(&http, "http://127.0.0.1:1", "explicit", &bad_address).await,
            Err(BirdeyeError::InvalidRequest)
        ));
        let valid_item = || {
            json!({
                "address": BASE_ADDRESS,
                "unix_time": 1_760_054_400,
                "o": 1e-8, "h": 2e-8, "l": 1e-8, "c": 1.5e-8,
                "v": 10, "v_usd": 1,
                "type": "1D", "currency": "usd"
            })
        };
        for item in [
            {
                let mut item = valid_item();
                item["address"] = json!("0xa187403aDCc6FCDF64fE9C850e0d9fAf4bA3830");
                item
            },
            {
                let mut item = valid_item();
                item["unix_time"] = json!(1_759_968_000);
                item
            },
            {
                let mut item = valid_item();
                item["l"] = json!(3e-8);
                item
            },
            {
                let mut item = valid_item();
                item["h"] = json!(-2e-8);
                item
            },
        ] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(response(json!([item]))))
                .mount(&server)
                .await;
            assert!(matches!(
                fetch_window_price_high(
                    &http,
                    &server.uri(),
                    "explicit",
                    &request(BirdeyeChain::Base, BASE_ADDRESS)
                )
                .await,
                Err(BirdeyeError::InvalidCandle | BirdeyeError::InvalidDecimal)
            ));
        }
    }
}
