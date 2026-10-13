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
    pub fn fdv_cents_for_supply(self, supply: u128) -> Option<i64> {
        self.fdv_cents_for_raw_supply(supply, 0)
    }

    /// Floor USD price × a raw token supply, scaled by the mint's decimals,
    /// into integer cents. Overflow returns `None`.
    #[allow(clippy::integer_division)]
    pub fn fdv_cents_for_raw_supply(self, raw_supply: u128, supply_decimals: u8) -> Option<i64> {
        let mut numerator = self.coefficient.checked_mul(raw_supply)?.checked_mul(100)?;
        let effective_scale = self.scale.checked_add(i32::from(supply_decimals))?;
        if effective_scale < 0 {
            numerator =
                numerator.checked_mul(10u128.checked_pow(effective_scale.unsigned_abs())?)?;
        }
        let cents = if effective_scale > 0 {
            // 10^39 exceeds u128::MAX, so every representable numerator is
            // strictly below the denominator at these scales.
            if effective_scale > 38 {
                return Some(0);
            }
            numerator / 10u128.checked_pow(u32::try_from(effective_scale).ok()?)?
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

/// Integer-cent FDV bounds from one provider price and a nonincreasing supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdvCentBounds {
    pub lower_cents: i64,
    pub upper_cents: i64,
}

impl FdvCentBounds {
    /// True only when both exact endpoint calculations floor to the same cent.
    pub fn is_same_cent(self) -> bool {
        self.lower_cents == self.upper_cents
    }
}

/// Bound price × supply FDV cents when supply has only decreased since birth.
///
/// This proves one rounded value only when the initial and current raw supplies
/// are verified, `0 < current_supply <= initial_supply`, decimals are correct,
/// and the current supply snapshot is at or after the provider maximum-price
/// record. It does not prove that the price scope is complete or independently
/// validate the provider price.
pub fn fdv_cent_bounds_for_nonincreasing_supply(
    price: ExactDecimal,
    initial_supply: u128,
    current_supply: u128,
    supply_decimals: u8,
) -> Option<FdvCentBounds> {
    if current_supply == 0 || current_supply > initial_supply {
        return None;
    }
    Some(FdvCentBounds {
        lower_cents: price.fdv_cents_for_raw_supply(current_supply, supply_decimals)?,
        upper_cents: price.fdv_cents_for_raw_supply(initial_supply, supply_decimals)?,
    })
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

    #[test]
    fn robinhood_swap_fx_fixture_preserves_eth_units_and_exact_fdv() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/birdeye_robinhood_swap_fx_validated_fdv_cases.json"
        ))
        .unwrap();
        for row in value["cases"].as_array().unwrap() {
            let supply = row["supply_tokens"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let expected = u128::from(row["expected_fdv_cents"].as_u64().unwrap());
            let price = ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
            assert_eq!(
                price.fdv_cents_for_supply(supply),
                Some(i64::try_from(expected).unwrap())
            );
            let numerator = row["weth_token_ratio_numerator"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let denominator = row["weth_token_ratio_denominator"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            for rate in ["eth_usd_low_cents", "eth_usd_high_cents"] {
                let converted = numerator
                    .checked_mul(supply)
                    .unwrap()
                    .checked_mul(u128::from(row[rate].as_u64().unwrap()))
                    .unwrap()
                    .checked_div(denominator)
                    .unwrap();
                assert!(converted.abs_diff(expected).checked_mul(100).unwrap() <= expected);
            }
            let swap_at = row["swap_at"].as_i64().unwrap();
            let minute_at = row["fx_minute_at"].as_i64().unwrap();
            assert_eq!(swap_at.div_euclid(60) * 60, minute_at);
        }
    }

    #[test]
    fn robinhood_stock_quote_fixture_checks_two_hop_usdg_fx_bracket() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/birdeye_robinhood_stock_quote_swap_fx_validated_fdv_case.json"
        ))
        .unwrap();
        assert_eq!(row["target_token_decimals"].as_u64(), Some(18));
        assert_eq!(row["nvda_decimals"].as_u64(), Some(18));
        assert_eq!(row["usdg_decimals"].as_u64(), Some(6));
        assert_eq!(row["quote_notional_units"], "USDG");
        assert_eq!(row["usdg_usd_kraken"]["pair"], "USDGUSD");

        let supply = row["supply_tokens"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        let expected = row["expected_fdv_cents"].as_u64().unwrap();
        let price = ExactDecimal::parse(row["birdeye_price_usd"].as_str().unwrap()).unwrap();
        assert_eq!(
            price.fdv_cents_for_supply(supply),
            Some(i64::try_from(expected).unwrap())
        );

        let target_block = row["target_block_number"].as_u64().unwrap();
        let target_at = row["target_timestamp"].as_i64().unwrap();
        let swaps = row["stock_swaps"].as_array().unwrap();
        assert_eq!(swaps.len(), 3);
        let target_nvda = row["target_nvda_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let target_token = row["target_token_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        for (index, swap) in swaps.iter().enumerate() {
            let block = swap["block_number"].as_u64().unwrap();
            if index == 0 {
                assert!(block < target_block);
            } else {
                assert!(block > target_block);
            }
            assert!(swap["timestamp"].as_i64().unwrap().abs_diff(target_at) <= 300);

            let usdg = swap["usdg_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let stock_nvda = swap["nvda_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let numerator = target_nvda
                .checked_mul(usdg)
                .unwrap()
                .checked_mul(U256::from(1_000_000_000_000_u64))
                .unwrap()
                .checked_mul(U256::from(supply))
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap();
            let denominator = target_token
                .checked_mul(stock_nvda)
                .unwrap()
                .checked_mul(U256::from(10_000))
                .unwrap();
            for rate in ["low_1e4", "high_1e4"] {
                let converted = numerator
                    .checked_mul(U256::from(row["usdg_usd_kraken"][rate].as_u64().unwrap()))
                    .unwrap()
                    .checked_div(denominator)
                    .unwrap();
                let expected = U256::from(expected);
                let difference = if converted >= expected {
                    converted.checked_sub(expected).unwrap()
                } else {
                    expected.checked_sub(converted).unwrap()
                };
                assert!(difference.checked_mul(U256::from(100)).unwrap() <= expected);
            }
        }
    }

    #[test]
    fn geckoterminal_fx_fixture_preserves_two_quote_conversions() {
        use alloy_primitives::U256;

        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_bsc_swap_fx_validated_fdv_cases.json"
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
            let quote = row["bnb_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let token = row["token_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let denominator = token.checked_mul(U256::from(100_000_000)).unwrap();
            for (bnb, usdt) in [
                ("bnb_usdt_low_milli", "usdt_usd_low_1e5"),
                ("bnb_usdt_high_milli", "usdt_usd_high_1e5"),
            ] {
                let converted = quote
                    .checked_mul(U256::from(supply))
                    .unwrap()
                    .checked_mul(U256::from(100))
                    .unwrap()
                    .checked_mul(U256::from(row[bnb].as_u64().unwrap()))
                    .unwrap()
                    .checked_mul(U256::from(row[usdt].as_u64().unwrap()))
                    .unwrap()
                    .checked_div(denominator)
                    .unwrap();
                let expected = U256::from(expected);
                let difference = if converted >= expected {
                    converted.checked_sub(expected).unwrap()
                } else {
                    expected.checked_sub(converted).unwrap()
                };
                assert!(difference.checked_mul(U256::from(100)).unwrap() <= expected);
            }
        }
    }

    #[test]
    fn geckoterminal_base_fixture_checks_fx_event_day_pool_maxima_and_ties() {
        use alloy_primitives::U256;

        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_base_swap_fx_validated_fdv_cases.json"
        ))
        .unwrap();
        for row in value["cases"].as_array().unwrap() {
            assert_eq!(row["chain_id"].as_u64(), Some(8453));
            assert_eq!(row["token_decimals"].as_u64(), Some(18));
            assert_eq!(row["usdc_decimals"].as_u64(), Some(6));
            assert_eq!(row["supply_tokens"].as_str(), Some("100000000000"));
            assert_eq!(
                row["quote_notional_quality"].as_str(),
                Some("small_quote_notional_observation")
            );
            assert!(row["all_listed_page1_pools_scanned"].as_bool().unwrap());

            let supply = row["supply_tokens"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let expected = row["expected_fdv_cents"].as_u64().unwrap();
            let provider_price =
                ExactDecimal::parse(row["gt_event_day_high_usd"].as_str().unwrap()).unwrap();
            assert_eq!(
                provider_price.fdv_cents_for_supply(supply),
                Some(i64::try_from(expected).unwrap())
            );

            // USDC raw has 6 decimals and the token has 18, so the exact
            // price ratio carries a 10^12 decimal adjustment.
            let quote = row["usdc_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let token = row["token_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let numerator = quote
                .checked_mul(U256::from(1_000_000_000_000_u64))
                .unwrap()
                .checked_mul(U256::from(supply))
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap();
            let denominator = token.checked_mul(U256::from(10_000)).unwrap();
            for rate in ["usdc_usd_low_1e4", "usdc_usd_high_1e4"] {
                let converted = numerator
                    .checked_mul(U256::from(row[rate].as_u64().unwrap()))
                    .unwrap()
                    .checked_div(denominator)
                    .unwrap();
                let expected = U256::from(expected);
                let difference = if converted >= expected {
                    converted.checked_sub(expected).unwrap()
                } else {
                    expected.checked_sub(converted).unwrap()
                };
                assert!(difference.checked_mul(U256::from(100)).unwrap() <= expected);
            }

            let event_at = row["event_timestamp"].as_i64().unwrap();
            let event_day = event_at.div_euclid(86_400) * 86_400;
            assert_eq!(row["event_utc_day_timestamp"].as_i64(), Some(event_day));
            let day_rows = row["gt_event_day_pool_highs"].as_array().unwrap();
            assert!(!day_rows.is_empty());
            let mut highest: Option<ExactDecimal> = None;
            for candle in day_rows {
                assert_eq!(candle["timestamp"].as_i64(), Some(event_day));
                let price = ExactDecimal::parse(candle["high_usd"].as_str().unwrap()).unwrap();
                highest = Some(match highest {
                    Some(current) => {
                        let scale = current.scale.max(price.scale);
                        let scaled = |value: ExactDecimal| {
                            let mut coefficient = U256::from(value.coefficient);
                            for _ in value.scale..scale {
                                coefficient = coefficient.checked_mul(U256::from(10)).unwrap();
                            }
                            coefficient
                        };
                        if scaled(price) > scaled(current) {
                            price
                        } else {
                            current
                        }
                    }
                    None => price,
                });
            }
            assert_eq!(
                highest.unwrap().to_plain_string(),
                ExactDecimal::parse(row["gt_event_day_high_usd"].as_str().unwrap())
                    .unwrap()
                    .to_plain_string()
            );
            let known_pools = row["known_page1_pool_count"].as_u64().unwrap();
            assert_eq!(row["gt_scanned_pool_count"].as_u64(), Some(known_pools));
            assert_eq!(
                u64::try_from(day_rows.len()).unwrap()
                    + u64::try_from(
                        row["gt_event_day_pools_without_candle"]
                            .as_array()
                            .unwrap()
                            .len()
                    )
                    .unwrap(),
                known_pools
            );

            if row["token"].as_str() == Some("0xf3ad70951949ce317144c699fbc3f3dd6af68b07") {
                let tie_rows = row["gt_window_max_tie_rows"].as_array().unwrap();
                assert_eq!(tie_rows.len(), 2);
                let event_day_ties: Vec<_> = tie_rows
                    .iter()
                    .filter(|tie| tie["timestamp"].as_i64() == Some(event_day))
                    .collect();
                assert_eq!(event_day_ties.len(), 1);
                assert_eq!(event_day_ties[0]["high_usd"], row["gt_event_day_high_usd"]);
                assert_ne!(
                    row["birdeye_peak_day"].as_str(),
                    row["event_utc_day"].as_str()
                );
            }
        }
    }

    #[test]
    fn geckoterminal_two_percent_candidate_keeps_scope_and_exact_fx_bounds() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_base_two_percent_price_gate_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["analysis_sha256"].as_str(),
            Some("a018509e9cff9cdf9b5aa24eadf7253534a7e7660714400c18d8bdd5e5f2c206")
        );
        assert_eq!(row["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            row["token"].as_str(),
            Some("0x92d95883e79a695ca4afa40d4885b4ef4ac38b07")
        );
        assert!(row["ath_at"].is_null());
        assert_eq!(row["utc_day"], row["pool_scan"]["max_day"]);
        assert_eq!(row["pool_scan"]["initialized"], row["pool_scan"]["scanned"]);
        assert_eq!(row["pool_scan"]["scanned"].as_u64(), Some(8));
        assert_eq!(
            row["pool_scan"]["usdc_empty_history_pools"].as_u64(),
            Some(4)
        );

        let supply = row["supply_tokens"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        let provider_price =
            ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
        assert_eq!(
            provider_price.fdv_cents_for_supply(supply),
            Some(i64::try_from(row["expected_fdv_cents"].as_u64().unwrap()).unwrap())
        );

        let execution = &row["execution"];
        assert_eq!(execution["pool"], row["pool_scan"]["max_pool"]);
        assert_eq!(execution["quote_token"].as_str(), Some("USDC"));
        assert_eq!(execution["quote_notional_usdc"].as_str(), Some("0.000045"));
        assert_eq!(execution["onchain_execution_max_day"], row["utc_day"]);
        assert_eq!(execution["event_day"], row["utc_day"]);

        let provider = ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
        let provider_num = U256::from(provider.coefficient);
        let provider_den = U256::from(10_u8).pow(U256::from(provider.scale));
        let within_two_percent =
            |candidate_num: U256, candidate_den: U256, reference_num: U256, reference_den: U256| {
                let converted = candidate_num.checked_mul(reference_den).unwrap();
                let reference = reference_num.checked_mul(candidate_den).unwrap();
                let difference = if converted >= reference {
                    converted.checked_sub(reference).unwrap()
                } else {
                    reference.checked_sub(converted).unwrap()
                };
                difference.checked_mul(U256::from(100)).unwrap()
                    <= reference.checked_mul(U256::from(2)).unwrap()
            };

        let quote = execution["quote_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let token = execution["token_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let price_den = token.checked_mul(U256::from(10_000)).unwrap();
        for rate in ["usdc_usd_low_1e4", "usdc_usd_high_1e4"] {
            let price_num = quote
                .checked_mul(U256::from(1_000_000_000_000_u64))
                .unwrap()
                .checked_mul(U256::from(execution[rate].as_u64().unwrap()))
                .unwrap();
            assert!(within_two_percent(
                price_num,
                price_den,
                provider_num,
                provider_den
            ));
        }

        // Both directions: 1.8% and 2% pass; one integer quantum beyond
        // 2% fails. Existing 1% fixture checks remain unchanged.
        let boundary_reference = U256::from(10_000);
        for (amount, expected) in [
            (10_180, true),
            (10_200, true),
            (10_201, false),
            (9_820, true),
            (9_800, true),
            (9_799, false),
        ] {
            assert_eq!(
                within_two_percent(
                    U256::from(amount),
                    boundary_reference,
                    boundary_reference,
                    boundary_reference
                ),
                expected
            );
        }

        let spot = &row["different_price_kind"];
        assert_eq!(spot["kind"].as_str(), Some("post_swap_marginal_spot"));
        assert!(!spot["use_for_execution_corroboration"].as_bool().unwrap());
        let spot_num = spot["usd_per_token"][0]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let spot_den = spot["usd_per_token"][1]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        assert!(!within_two_percent(
            spot_num,
            spot_den,
            provider_num,
            provider_den
        ));
    }

    #[test]
    fn geckoterminal_cross_day_max_tie_uses_same_day_usdc_price_gate() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_base_cross_day_max_tie_price_gate_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["certificate_sha256"].as_str(),
            Some("66b194cd41f530eb112199933fa28cc9d14c3dad095c3721a5af2f12f1fc38c7")
        );
        assert_eq!(
            row["source"].as_str(),
            Some("geckoterminal-window-supply-swap-fx-validated-v2")
        );
        assert_eq!(row["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            row["token"].as_str(),
            Some("0xfec3a80626377bce24c399ef9f4e124afef6cb07")
        );
        assert!(row["ath_at"].is_null());

        let daily = &row["geckoterminal_same_pool_daily_rows"];
        let may3 = &daily["2026-05-03"];
        let may4 = &daily["2026-05-04"];
        assert_eq!(may3["high"], row["provider_price_usd"]);
        assert_eq!(may3["close"], row["provider_price_usd"]);
        assert_eq!(may4["open"], row["provider_price_usd"]);
        assert_eq!(may4["high"], row["provider_price_usd"]);
        assert_ne!(may4["close"], row["provider_price_usd"]);
        let observed_tie_days: Vec<_> = ["2026-05-03", "2026-05-04"]
            .into_iter()
            .filter(|day| daily[*day]["high"] == row["provider_price_usd"])
            .collect();
        assert_eq!(observed_tie_days, ["2026-05-03", "2026-05-04"]);
        assert_eq!(row["max_tie_days"][0].as_str(), Some("2026-05-03"));
        assert_eq!(row["max_tie_days"][1].as_str(), Some("2026-05-04"));

        let supply = &row["supply"];
        assert!(supply["verified"].as_bool().unwrap());
        assert_eq!(supply["decimals"].as_u64(), Some(18));
        let supply_raw = supply["total_supply_raw"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        assert_eq!(supply_raw % 10_u128.pow(18), 0);
        let supply_tokens = supply_raw.checked_div(10_u128.pow(18)).unwrap();
        assert_eq!(
            supply_tokens.to_string(),
            row["supply_tokens"].as_str().unwrap()
        );
        let provider = ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
        assert_eq!(
            provider.fdv_cents_for_supply(supply_tokens),
            Some(i64::try_from(row["expected_fdv_cents"].as_u64().unwrap()).unwrap())
        );

        let scan = &row["pool_scan"];
        assert_eq!(scan["initialized"], scan["scanned"]);
        assert!(
            scan["complete_from_token_creation_to_anchor"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            scan["empty_history_pool"]["complete_swap_logs"].as_u64(),
            Some(0)
        );
        assert_eq!(scan["empty_history_pool"]["ohlc_rows"].as_u64(), Some(0));

        const BASE_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
        assert_eq!(row["pool_identity"]["base_token"], row["token"]);
        assert_eq!(
            row["pool_identity"]["quote_token"].as_str(),
            Some(BASE_USDC)
        );
        assert_eq!(row["pool_identity"]["quote_symbol"].as_str(), Some("USDC"));

        let execution = &row["execution"];
        assert_eq!(execution["pool"], row["pool_identity"]["pool"]);
        assert_eq!(execution["quote_token"].as_str(), Some(BASE_USDC));
        assert_eq!(execution["quote_symbol"].as_str(), Some("USDC"));
        assert_eq!(execution["quote_notional_usdc"].as_str(), Some("0.011818"));
        assert!(
            execution["quality_note"]
                .as_str()
                .unwrap()
                .contains("small")
        );
        assert_eq!(execution["event_day"], row["utc_day"]);
        assert!(
            execution["receipt_and_block_header_verified"]
                .as_bool()
                .unwrap()
        );

        let fraction = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            let scale = u32::try_from(decimal.scale).unwrap();
            (
                U256::from(decimal.coefficient),
                U256::from(10_u8).pow(U256::from(scale)),
            )
        };
        let provider_fraction = fraction(row["provider_price_usd"].as_str().unwrap());
        let (execution_num, execution_den) = (
            execution["execution_price_fraction_after_decimals"][0]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap(),
            execution["execution_price_fraction_after_decimals"][1]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap(),
        );
        assert_eq!(
            execution_num,
            execution["quote_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap()
                .checked_mul(U256::from(1_000_000_000_000_u64))
                .unwrap()
        );
        assert_eq!(
            execution_den,
            execution["token_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap()
        );
        let within_two_percent =
            |candidate_num: U256, candidate_den: U256, reference_num: U256, reference_den: U256| {
                let candidate = candidate_num.checked_mul(reference_den).unwrap();
                let reference = reference_num.checked_mul(candidate_den).unwrap();
                let difference = if candidate >= reference {
                    candidate.checked_sub(reference).unwrap()
                } else {
                    reference.checked_sub(candidate).unwrap()
                };
                difference.checked_mul(U256::from(100)).unwrap()
                    <= reference.checked_mul(U256::from(2)).unwrap()
            };

        for fx_text in execution["fx_usdc_usd_low_high"].as_array().unwrap().iter() {
            let (fx_num, fx_den) = fraction(fx_text.as_str().unwrap());
            let actual = (
                execution_num.checked_mul(fx_num).unwrap(),
                execution_den.checked_mul(fx_den).unwrap(),
            );
            assert!(within_two_percent(
                actual.0,
                actual.1,
                provider_fraction.0,
                provider_fraction.1
            ));
        }

        let spot = &row["different_price_kind"];
        assert_eq!(spot["kind"].as_str(), Some("post_swap_marginal_spot"));
        assert!(!spot["use_for_execution_corroboration"].as_bool().unwrap());

        let birdeye = fraction(
            row["unresolved_birdeye_discrepancy"]["same_day_high_usd"]
                .as_str()
                .unwrap(),
        );
        assert!(!within_two_percent(
            birdeye.0,
            birdeye.1,
            provider_fraction.0,
            provider_fraction.1
        ));
        assert!(
            row["unresolved_birdeye_discrepancy"]["status"]
                .as_str()
                .unwrap()
                .contains("preserved source disagreement")
        );
    }

    #[test]
    fn zora_window_price_uses_exact_route_fx_and_conservative_floor() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/zora_route_fx_window_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["certificate_sha256"].as_str(),
            Some("941113c79a66900a52f079de7cdbcbd3638f2d4fba60c36858e24d8626778b56")
        );
        assert_eq!(
            row["source"].as_str(),
            Some("zora-window-supply-route-fx-validated-v1")
        );
        assert_eq!(row["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            row["token"].as_str(),
            Some("0x058e72b9e99561844582b019ab91cdab6c9ac738")
        );
        assert!(row["ath_at"].is_null());

        let pow10 = |n: u32| {
            (0..n)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let decimal_fraction = |text: &str| {
            let value = ExactDecimal::parse(text).unwrap();
            (
                U256::from(value.coefficient),
                pow10(u32::try_from(value.scale).unwrap()),
                value,
            )
        };
        let raw = |value: &serde_json::Value| {
            U256::from(value.as_str().unwrap().parse::<u128>().unwrap())
        };

        let edge = &row["api_edge"];
        let route = &row["same_transaction_route"];
        assert_eq!(edge["transaction_hash"], route["transaction_hash"]);
        assert_eq!(edge["coin_amount_raw"], route["target_amount_out_raw"]);
        assert_eq!(
            edge["quote_address"].as_str(),
            Some("0x364863eece7fd2e6c5f3dc13a3f043521f77ec49")
        );
        assert_eq!(
            route["weth_address"].as_str(),
            Some("0x4200000000000000000000000000000000000006")
        );
        assert_eq!(edge["quote_decimals"].as_u64(), Some(18));
        assert_eq!(edge["coin_decimals"].as_u64(), Some(18));
        assert_eq!(route["weth_decimals"].as_u64(), Some(18));
        assert_eq!(
            &edge["timestamp_utc"].as_str().unwrap()[..16],
            &route["coinbase_minute_start_utc"].as_str().unwrap()[..16]
        );

        let quote_raw = raw(&edge["onchain_quote_amount_raw"]);
        let api_quote =
            ExactDecimal::parse(edge["api_quote_amount_decimal"].as_str().unwrap()).unwrap();
        let api_quote_scale = u32::try_from(api_quote.scale).unwrap();
        let api_quote_raw = U256::from(api_quote.coefficient)
            .checked_mul(pow10(18_u32.checked_sub(api_quote_scale).unwrap()))
            .unwrap();
        let amount_rounding_difference = quote_raw.checked_sub(api_quote_raw).unwrap();
        let stated_rounding_difference = decimal_fraction(
            edge["api_amount_rounding_difference_quote_units"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(
            amount_rounding_difference,
            stated_rounding_difference
                .0
                .checked_mul(pow10(
                    18_u32
                        .checked_sub(u32::try_from(stated_rounding_difference.2.scale).unwrap())
                        .unwrap()
                ))
                .unwrap()
        );

        let api_price =
            ExactDecimal::parse(edge["api_price_usdc_per_quote_unit"].as_str().unwrap()).unwrap();
        let price_scale = u32::try_from(api_price.scale).unwrap();
        assert!(price_scale >= 24);
        let coin_raw = raw(&edge["coin_amount_raw"]);
        let window_num = quote_raw
            .checked_mul(U256::from(api_price.coefficient))
            .unwrap();
        let window_den = coin_raw
            .checked_mul(pow10(price_scale.checked_sub(24).unwrap()))
            .unwrap();
        let floor_24dp = window_num.checked_div(window_den).unwrap();
        assert!(floor_24dp.checked_mul(window_den).unwrap() <= window_num);
        assert!(
            floor_24dp
                .checked_add(U256::from(1))
                .unwrap()
                .checked_mul(window_den)
                .unwrap()
                > window_num
        );
        let digits = floor_24dp.to_string();
        let floor_text = format!("0.{}{}", "0".repeat(24 - digits.len()), digits);
        assert_eq!(
            floor_text,
            row["canonical_window_price_usd_floor_24dp"]
                .as_str()
                .unwrap()
        );

        let supply = &row["supply"];
        assert!(supply["verified"].as_bool().unwrap());
        let supply_raw = raw(&supply["total_supply_raw"]);
        let supply_tokens = supply_raw
            .checked_div(pow10(
                u32::try_from(supply["decimals"].as_u64().unwrap()).unwrap(),
            ))
            .unwrap();
        assert_eq!(
            supply_tokens.to_string(),
            supply["tokens"].as_str().unwrap()
        );
        let floor_price = ExactDecimal::parse(&floor_text).unwrap();
        assert_eq!(
            floor_price.fdv_cents_for_supply(u128::try_from(supply_tokens).unwrap()),
            Some(i64::try_from(row["expected_fdv_cents"].as_u64().unwrap()).unwrap())
        );

        let weth_raw = raw(&route["weth_amount_in_raw"]);
        for fx_name in ["coinbase_eth_usd_min", "coinbase_eth_usd_max"] {
            let (fx_num, fx_den, _) = decimal_fraction(route[fx_name].as_str().unwrap());
            let candidate_cross = window_num.checked_mul(fx_den).unwrap();
            let route_cross = weth_raw
                .checked_mul(fx_num)
                .unwrap()
                .checked_mul(pow10(price_scale))
                .unwrap();
            let difference = if candidate_cross >= route_cross {
                candidate_cross.checked_sub(route_cross).unwrap()
            } else {
                route_cross.checked_sub(candidate_cross).unwrap()
            };
            assert!(
                difference.checked_mul(U256::from(100)).unwrap()
                    <= candidate_cross.checked_mul(U256::from(2)).unwrap()
            );
        }

        let birdeye = ExactDecimal::parse(
            row["birdeye_disagreement"]["same_day_high_usd"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let birdeye_scale = u32::try_from(birdeye.scale).unwrap();
        for fx_name in ["coinbase_eth_usd_min", "coinbase_eth_usd_max"] {
            let (fx_num, fx_den, _) = decimal_fraction(route[fx_name].as_str().unwrap());
            let route_cross = weth_raw
                .checked_mul(fx_num)
                .unwrap()
                .checked_mul(pow10(birdeye_scale))
                .unwrap();
            let birdeye_cross = U256::from(birdeye.coefficient)
                .checked_mul(coin_raw)
                .unwrap()
                .checked_mul(fx_den)
                .unwrap();
            let difference = if route_cross >= birdeye_cross {
                route_cross.checked_sub(birdeye_cross).unwrap()
            } else {
                birdeye_cross.checked_sub(route_cross).unwrap()
            };
            assert!(
                difference.checked_mul(U256::from(100)).unwrap()
                    > route_cross.checked_mul(U256::from(2)).unwrap()
            );
        }
        assert!(
            row["birdeye_disagreement"]["status"]
                .as_str()
                .unwrap()
                .contains("do not average")
        );

        let coverage = &row["coverage"];
        assert_eq!(coverage["api_reported_edges"].as_u64(), Some(92));
        assert_eq!(coverage["api_observed_price_values"].as_u64(), Some(91));
        assert_eq!(
            coverage["same_pool_chain_swaps_through_anchor"].as_u64(),
            Some(326)
        );
        assert!(coverage["status"].as_str().unwrap().contains("partial"));
    }

    #[test]
    fn geckoterminal_zero_swap_pool_keeps_scoped_fdv_and_unknown_ath() {
        use alloy_primitives::U256;

        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_base_zero_swap_pool_fdv_case.json"
        ))
        .unwrap();
        assert_eq!(
            value["source"].as_str(),
            Some("geckoterminal-window-supply-swap-fx-validated-v1")
        );
        assert_eq!(
            value["analysis_sha256"].as_str(),
            Some("a586689bc26758a2faec06bf66d1b9ce0a118a01c46f0470b0015813404840c5")
        );
        assert_eq!(value["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            value["token"].as_str(),
            Some("0x67d5ab989378bfb2070954989a2f09ca6b19ab07")
        );
        assert!(value["ath_at"].is_null());
        assert_eq!(value["anchor_block"].as_u64(), Some(52_456_236));
        assert_eq!(
            value["geckoterminal_request"]["limit"].as_u64(),
            Some(1_000)
        );
        assert_eq!(
            value["geckoterminal_request"]["before_timestamp"].as_i64(),
            Some(1_791_676_799)
        );
        assert!(value["geckoterminal_request"]["explicit_after_timestamp"].is_null());

        let supply_raw = value["supply_raw"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        let supply_decimals = u8::try_from(value["supply_decimals"].as_u64().unwrap()).unwrap();
        assert_eq!(supply_decimals, 18);
        let scale = 10_u128.pow(u32::from(supply_decimals));
        assert_eq!(supply_raw % scale, 0);
        let supply_tokens = supply_raw.div_euclid(scale);

        let pools = value["pools"].as_array().unwrap();
        assert_eq!(pools.len(), 4);
        let mut pool_ids = Vec::new();
        let mut empty_pool_count = 0;
        let mut highest_pool_price: Option<ExactDecimal> = None;
        for pool in pools {
            pool_ids.push(pool["pool_id"].as_str().unwrap());
            assert!(matches!(
                pool["selector"].as_str(),
                Some("base" | "0x67d5ab989378bfb2070954989a2f09ca6b19ab07")
            ));
            assert_eq!(pool["metadata_base"], value["token"]);
            assert_eq!(pool["metadata_quote"], pool["quote"]);
            assert!(!pool["response_sha256"].as_str().unwrap().is_empty());
            if pool["quote"] == "0x0000000000000000000000000000000000000000" {
                assert_eq!(pool["selector"], value["token"]);
            }
            let rows = pool["ohlc_rows"].as_u64().unwrap();
            if rows == 0 {
                empty_pool_count += 1;
                assert_eq!(
                    pool["pool_id"].as_str(),
                    Some("0x218492c713cd10484093bf261b85c65702627dbb54a01c57ac3dc60dbe92da7a")
                );
                assert!(pool["window_max_usd"].is_null());
                let scan = &pool["zero_swap_scan"];
                assert!(scan["complete"].as_bool().unwrap());
                assert_eq!(scan["swap_logs"].as_u64(), Some(0));
                assert_eq!(
                    scan["from_block"].as_u64(),
                    value["token_created_block"].as_u64()
                );
                assert!(
                    scan["from_block"].as_u64().unwrap()
                        < scan["initialize_block"].as_u64().unwrap()
                );
                assert_eq!(scan["to_block"].as_u64(), value["anchor_block"].as_u64());
                assert_eq!(
                    scan["raw_sha256"].as_str(),
                    Some("2c8d689e37498970918392ae8f68252b1c3f0f3738d79f337f604259c19bc02d")
                );
            } else {
                let price = ExactDecimal::parse(pool["window_max_usd"].as_str().unwrap()).unwrap();
                highest_pool_price = Some(match highest_pool_price {
                    Some(current) => {
                        let scale = current.scale.max(price.scale);
                        let scaled = |value: ExactDecimal| {
                            let mut coefficient = U256::from(value.coefficient);
                            for _ in value.scale..scale {
                                coefficient = coefficient.checked_mul(U256::from(10)).unwrap();
                            }
                            coefficient
                        };
                        if scaled(price) > scaled(current) {
                            price
                        } else {
                            current
                        }
                    }
                    None => price,
                });
            }
        }
        assert_eq!(empty_pool_count, 1);
        pool_ids.sort_unstable();
        assert_eq!(
            pool_ids,
            [
                "0x077721753d0c396d300e223dd86d69a7bdc05f23e68b27e566335dd76f641333",
                "0x218492c713cd10484093bf261b85c65702627dbb54a01c57ac3dc60dbe92da7a",
                "0x256fb38587335b84ecabb8a79986cebd5d7134bbb538fa8be4af9858d48276d4",
                "0x379f3e8f364b101490e01dec8c4a1d98e1483c906c0417ed89a6a0e3ba460389",
            ]
        );
        let provider_price =
            ExactDecimal::parse(value["provider_max"]["price_usd"].as_str().unwrap()).unwrap();
        let expected_cents =
            i64::try_from(value["provider_max"]["fdv_cents_floor"].as_u64().unwrap()).unwrap();
        assert_eq!(
            highest_pool_price.unwrap().to_plain_string(),
            provider_price.to_plain_string()
        );
        assert_eq!(
            provider_price.fdv_cents_for_supply(supply_tokens),
            Some(expected_cents)
        );

        let execution = &value["independent_swap"];
        let timestamp = execution["timestamp"].as_i64().unwrap();
        assert_eq!(
            timestamp.div_euclid(86_400) * 86_400,
            execution["fx_day_start_utc"].as_i64().unwrap()
        );
        assert_eq!(execution["quote_decimals"].as_u64(), Some(6));
        assert_eq!(execution["token_decimals"].as_u64(), Some(18));
        let quote = execution["quote_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let token = execution["token_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let ratio = &execution["ratio_price_fraction"];
        assert_eq!(
            quote
                .checked_mul(U256::from(1_000_000_000_000_u64))
                .unwrap()
                .checked_mul(U256::from(
                    ratio[1].as_str().unwrap().parse::<u128>().unwrap()
                ))
                .unwrap(),
            token
                .checked_mul(U256::from(
                    ratio[0].as_str().unwrap().parse::<u128>().unwrap()
                ))
                .unwrap()
        );
        let numerator = quote
            .checked_mul(U256::from(1_000_000_000_000_u64))
            .unwrap()
            .checked_mul(U256::from(supply_tokens))
            .unwrap()
            .checked_mul(U256::from(100))
            .unwrap();
        let denominator = token.checked_mul(U256::from(10_000)).unwrap();
        for rate in ["fx_low_1e4", "fx_high_1e4"] {
            let converted = numerator
                .checked_mul(U256::from(execution[rate].as_u64().unwrap()))
                .unwrap()
                .checked_div(denominator)
                .unwrap();
            let expected = U256::from(expected_cents);
            let difference = if converted >= expected {
                converted.checked_sub(expected).unwrap()
            } else {
                expected.checked_sub(converted).unwrap()
            };
            assert!(difference.checked_mul(U256::from(100)).unwrap() <= expected);
        }
        assert_eq!(
            value["unresolved_other_source"]["provider"].as_str(),
            Some("Birdeye")
        );
        assert!(
            value["unresolved_other_source"]["status"]
                .as_str()
                .unwrap()
                .contains("unresolved")
        );
        let wrong_token = &value["excluded_wrong_token_selector"];
        assert_eq!(wrong_token["selector"].as_str(), Some("quote"));
        assert_ne!(wrong_token["metadata_base"], value["token"]);
        assert_eq!(wrong_token["metadata_quote"], value["token"]);
        assert_eq!(wrong_token["high_usd"].as_str(), Some("2698.95"));
        assert!(
            value["recording_policy"]
                .as_str()
                .unwrap()
                .contains("Initialize sqrt price is not a trade observation")
        );
    }

    #[test]
    fn usd1_daily_fx_fixture_checks_trade_observations_without_a_peg() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/birdeye_bsc_usd1_daily_fx_validated_fdv_case.json"
        ))
        .unwrap();
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
        assert!(row["daily_volume_1e2"].as_u64().unwrap() > 0);
        assert!(row["daily_turnover_1e5"].as_u64().unwrap() > 0);
        let quote = row["usd1_raw"].as_str().unwrap().parse::<U256>().unwrap();
        let token = row["token_raw"].as_str().unwrap().parse::<U256>().unwrap();
        let denominator = token.checked_mul(U256::from(1_000_000_000)).unwrap();
        for (usd1, usdt) in [
            ("usd1_usdt_low_1e4", "usdt_usd_low_1e5"),
            ("usd1_usdt_high_1e4", "usdt_usd_high_1e5"),
        ] {
            let converted = quote
                .checked_mul(U256::from(supply))
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap()
                .checked_mul(U256::from(row[usd1].as_u64().unwrap()))
                .unwrap()
                .checked_mul(U256::from(row[usdt].as_u64().unwrap()))
                .unwrap()
                .checked_div(denominator)
                .unwrap();
            let expected = U256::from(expected);
            let difference = if converted >= expected {
                converted.checked_sub(expected).unwrap()
            } else {
                expected.checked_sub(converted).unwrap()
            };
            assert!(difference.checked_mul(U256::from(100)).unwrap() <= expected);
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
    fn tracker_price_and_monotonic_raw_supply_golden_cent_bounds() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_supply_fdv_cent_bounds.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["source_evidence_sha256"],
            "68e59f339adf34437c6cb02df283d2f6becb69475a7cbed5cb982ccfd4fe21e1"
        );
        assert_eq!(
            fixture["provider_scope"],
            "provider-recording-window-unknown"
        );
        let snapshot_at_ms = fixture["supply_snapshot_at_unix_seconds"]
            .as_u64()
            .unwrap()
            .checked_mul(1_000)
            .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            assert!(snapshot_at_ms >= case["provider_maximum_timestamp_ms"].as_u64().unwrap());
            let price = ExactDecimal::parse(case["price_usd"].as_str().unwrap()).unwrap();
            let initial = case["initial_raw_supply"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let current = case["current_raw_supply"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let decimals = u8::try_from(fixture["supply_decimals"].as_u64().unwrap()).unwrap();
            let bounds =
                fdv_cent_bounds_for_nonincreasing_supply(price, initial, current, decimals)
                    .unwrap();
            assert_eq!(bounds.lower_cents, case["lower_cents"].as_i64().unwrap());
            assert_eq!(bounds.upper_cents, case["upper_cents"].as_i64().unwrap());
            assert_eq!(bounds.is_same_cent(), case["same_cent"].as_bool().unwrap());
        }
    }

    #[test]
    fn raw_supply_fdv_bounds_reject_invalid_supply_and_overflow() {
        let price = ExactDecimal::parse("1").unwrap();
        assert!(fdv_cent_bounds_for_nonincreasing_supply(price, 1, 0, 6).is_none());
        assert!(fdv_cent_bounds_for_nonincreasing_supply(price, 1, 2, 6).is_none());
        assert!(fdv_cent_bounds_for_nonincreasing_supply(price, 0, 1, 6).is_none());
        assert!(price.fdv_cents_for_raw_supply(u128::MAX, 0).is_none());
        assert!(price.fdv_cents_for_raw_supply(u128::MAX, u8::MAX).is_none());
    }

    #[test]
    fn provider_peak_supply_bounds_collapse_only_when_cent_floors_match() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_peak_supply_fdv_bounds.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["reconstruction_sha256"],
            "419c8f7f1d8064cc67b230d800e011f691257de9cd8853b66f41405fc9e3c143"
        );
        assert_eq!(fixture["provider_window_scope"], "unknown");
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let price = ExactDecimal::parse(case["price_usd"].as_str().unwrap()).unwrap();
            let birth = u128::from(case["birth_raw"].as_u64().unwrap());
            let at_peak = u128::from(case["peak_raw"].as_u64().unwrap());
            let decimals = u8::try_from(case["decimals"].as_u64().unwrap()).unwrap();
            let lower = price.fdv_cents_for_raw_supply(at_peak, decimals).unwrap();
            let upper = price.fdv_cents_for_raw_supply(birth, decimals).unwrap();
            assert_eq!(lower, case["expected_floor_cents"].as_i64().unwrap());
            assert_eq!(lower, upper, "{} has non-collapsing bounds", case["mint"]);
        }

        let negative = &fixture["negative_control"];
        assert_eq!(
            negative["supply_basis"],
            "later finalized current supply; conservative lower bound on supply at the earlier price point, not exact peak-time supply"
        );
        assert_eq!(
            negative["chart_sha256"],
            "e0d4399401ee02baacab63fe217b42c6598b26564492fdb114ee9fafa8865fc0"
        );
        assert_eq!(
            negative["supply_join_sha256"],
            "2f260bbd0af32861f15e2eb3d4edc156642feb4e52abe183a41b60f077a396e3"
        );
        let price = ExactDecimal::parse(negative["price_usd"].as_str().unwrap()).unwrap();
        let decimals = u8::try_from(negative["decimals"].as_u64().unwrap()).unwrap();
        let lower = price
            .fdv_cents_for_raw_supply(
                u128::from(negative["lower_bound_raw"].as_u64().unwrap()),
                decimals,
            )
            .unwrap();
        let upper = price
            .fdv_cents_for_raw_supply(
                u128::from(negative["birth_raw"].as_u64().unwrap()),
                decimals,
            )
            .unwrap();
        assert_eq!(lower, negative["lower_bound_floor_cents"].as_i64().unwrap());
        assert_eq!(upper, negative["birth_floor_cents"].as_i64().unwrap());
        assert_ne!(lower, upper);
    }

    #[test]
    fn two_solana_peak_prices_have_independent_pool_spot_corroboration() {
        use alloy_primitives::U256;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_two_price_gate_cases.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["price_audit_sha256"],
            "7633ace65d9586828bb14077695450e633331fba85fa0602c184cd27ac6794c9"
        );
        assert_eq!(
            fixture["supply_reconstruction_sha256"],
            "419c8f7f1d8064cc67b230d800e011f691257de9cd8853b66f41405fc9e3c143"
        );
        assert_eq!(fixture["ath_at"], serde_json::Value::Null);
        assert_eq!(fixture["provider_window_scope"], "unknown");
        assert_eq!(fixture["market_scope"], "unknown");
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 2);

        for case in cases {
            let price = ExactDecimal::parse(case["provider_price_usd"].as_str().unwrap()).unwrap();
            assert_eq!(
                price.fdv_cents_for_raw_supply(
                    u128::from(case["snapshot_peak_supply_raw"].as_u64().unwrap()),
                    u8::try_from(case["decimals"].as_u64().unwrap()).unwrap(),
                ),
                Some(case["fdv_cents"].as_i64().unwrap())
            );
            let at_ms = case["provider_timestamp_ms"].as_u64().unwrap();
            let block_ms = case["selected_state_block_time_unix_s"]
                .as_u64()
                .unwrap()
                .checked_mul(1_000)
                .unwrap();
            assert!(at_ms.abs_diff(block_ms) <= 2_000);

            let raw = |v: &serde_json::Value| U256::from(v.as_u64().unwrap());
            let cpmm = &case["cpmm"];
            let input_min = raw(&cpmm["input_min_raw"]);
            let input_max = raw(&cpmm["input_max_raw"]);
            let output = raw(&cpmm["output_raw"]);
            let pre_base = raw(&cpmm["pre_base_raw"]);
            let (minimum, maximum) = match cpmm["variant"].as_str().unwrap() {
                "buy_exact_quote_in" => {
                    let min_numerator = input_min.checked_mul(pre_base).unwrap();
                    let max_numerator = input_max.checked_mul(pre_base).unwrap();
                    let minimum = min_numerator
                        .checked_div(output.checked_add(U256::from(1)).unwrap())
                        .unwrap()
                        .checked_sub(input_min)
                        .unwrap()
                        .checked_add(U256::from(1))
                        .unwrap();
                    let maximum = max_numerator
                        .checked_div(output)
                        .unwrap()
                        .checked_sub(input_max)
                        .unwrap();
                    (minimum, maximum)
                }
                "sell" => {
                    let denominator = input_min;
                    let base_after = pre_base.checked_add(input_min).unwrap();
                    let lower_numerator = output.checked_mul(base_after).unwrap();
                    let upper_numerator = output
                        .checked_add(U256::from(1))
                        .unwrap()
                        .checked_mul(base_after)
                        .unwrap();
                    let minimum = lower_numerator
                        .checked_add(denominator.checked_sub(U256::from(1)).unwrap())
                        .unwrap()
                        .checked_div(denominator)
                        .unwrap();
                    let maximum = upper_numerator
                        .checked_add(denominator.checked_sub(U256::from(1)).unwrap())
                        .unwrap()
                        .checked_div(denominator)
                        .unwrap()
                        .checked_sub(U256::from(1))
                        .unwrap();
                    (minimum, maximum)
                }
                _ => (U256::ZERO, U256::ZERO),
            };
            assert_eq!(minimum, raw(&cpmm["effective_quote_min_raw"]));
            assert_eq!(maximum, raw(&cpmm["effective_quote_max_raw"]));
            assert!(minimum <= raw(&cpmm["actual_pre_quote_raw"]));
            assert!(raw(&cpmm["actual_pre_quote_raw"]) <= maximum);
            let quote_vault_delta = raw(&cpmm["quote_vault_delta_raw"]);
            assert_eq!(
                minimum.checked_add(quote_vault_delta).unwrap(),
                raw(&case["spot_quote_min_raw"])
            );
            assert_eq!(
                maximum.checked_add(quote_vault_delta).unwrap(),
                raw(&case["spot_quote_max_raw"])
            );

            let pow10 = |n: u32| {
                (0..n)
                    .try_fold(U256::from(1), |v, _| v.checked_mul(U256::from(10)))
                    .unwrap()
            };
            for quote in [
                raw(&case["spot_quote_min_raw"]),
                raw(&case["spot_quote_max_raw"]),
            ] {
                for fx_field in ["coinbase_sol_usd_low", "coinbase_sol_usd_high"] {
                    let fx = ExactDecimal::parse(case[fx_field].as_str().unwrap()).unwrap();
                    let lhs = quote
                        .checked_mul(U256::from(fx.coefficient))
                        .unwrap()
                        .checked_mul(pow10(u32::try_from(price.scale).unwrap()))
                        .unwrap();
                    let quote_base_scale = case["quote_decimals"]
                        .as_u64()
                        .unwrap()
                        .checked_sub(case["decimals"].as_u64().unwrap())
                        .unwrap();
                    let exponent = u32::try_from(quote_base_scale)
                        .unwrap()
                        .checked_add(u32::try_from(fx.scale).unwrap())
                        .unwrap();
                    let rhs = raw(&case["base_reserve_raw"])
                        .checked_mul(pow10(exponent))
                        .unwrap()
                        .checked_mul(U256::from(price.coefficient))
                        .unwrap();
                    let delta = if lhs >= rhs {
                        lhs.checked_sub(rhs).unwrap()
                    } else {
                        rhs.checked_sub(lhs).unwrap()
                    };
                    assert!(delta.checked_mul(U256::from(100)).unwrap() <= rhs);
                }
            }
        }
    }

    #[test]
    fn solana_tracker_spot_supply_cases_are_correlated() {
        use alloy_primitives::U256;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_scoped_spot_supply_cases.json"
        ))
        .unwrap();
        let evidence_hashes = [
            "24b21422e43dace5ced5416bf7962dba68a8943bee4c0dc42ce123d6e20bf5c0",
            "f91052b814ab7cd02a79a96f256d30a573a2358c8b6f2d0f361826570397c868",
        ];
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), evidence_hashes.len());
        for (case, evidence_hash) in cases.iter().zip(evidence_hashes) {
            assert_eq!(
                case["source"],
                "solana-tracker-window-supply-bounds-pool-fx-validated-v1"
            );
            assert_eq!(case["chain"], "solana-mainnet");
            assert!(case["ath_at"].is_null());
            assert_eq!(case["provider_scope"], "provider-recording-window-unknown");
            assert_eq!(case["market_scope"], "unknown");
            assert_eq!(case["source_evidence_sha256"], evidence_hash);

            let provider = &case["provider"];
            let price = ExactDecimal::parse(provider["price_usd"].as_str().unwrap()).unwrap();
            let supply = &case["supply"];
            assert!(
                supply["snapshot_unix_seconds"].as_u64().unwrap() * 1_000
                    >= provider["timestamp_ms"].as_u64().unwrap()
            );
            let bounds = fdv_cent_bounds_for_nonincreasing_supply(
                price,
                u128::from(supply["birth_raw"].as_u64().unwrap()),
                u128::from(supply["current_raw"].as_u64().unwrap()),
                u8::try_from(supply["decimals"].as_u64().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                (bounds.lower_cents, bounds.upper_cents),
                (
                    case["fdv_cents"].as_i64().unwrap(),
                    case["fdv_cents"].as_i64().unwrap()
                )
            );

            let pool = &case["spot"];
            assert_eq!(pool["pool_id"], provider["pool_id"]);
            assert_eq!(pool["base_mint"], case["token"]);
            assert_eq!(
                pool["quote_mint"],
                "So11111111111111111111111111111111111111112"
            );
            let at_ms = provider["timestamp_ms"].as_u64().unwrap();
            assert!(
                at_ms.abs_diff(pool["block_time_unix_seconds"].as_u64().unwrap() * 1_000) <= 2_000
            );
            assert_eq!(
                at_ms.div_euclid(60_000),
                pool["coinbase_minute_start_unix_seconds"]
                    .as_u64()
                    .unwrap()
                    .div_euclid(60)
            );

            let raw = |value: &serde_json::Value| U256::from(value.as_u64().unwrap());
            let pow10 = |n: u32| {
                (0..n)
                    .try_fold(U256::from(1), |v, _| v.checked_mul(U256::from(10)))
                    .unwrap()
            };
            let within_one_percent = |quote: U256, base: U256, fx_text: &str| {
                let fx = ExactDecimal::parse(fx_text).unwrap();
                let lhs = quote
                    .checked_mul(U256::from(fx.coefficient))
                    .and_then(|value| value.checked_mul(pow10(u32::try_from(price.scale).unwrap())))
                    .unwrap();
                let quote_base_scale = pool["quote_decimals"]
                    .as_u64()
                    .unwrap()
                    .checked_sub(pool["base_decimals"].as_u64().unwrap())
                    .unwrap();
                let scale = u32::try_from(quote_base_scale)
                    .unwrap()
                    .checked_add(u32::try_from(fx.scale).unwrap())
                    .unwrap();
                let rhs = base
                    .checked_mul(pow10(scale))
                    .and_then(|value| value.checked_mul(U256::from(price.coefficient)))
                    .unwrap();
                let delta = if lhs >= rhs {
                    lhs.checked_sub(rhs).unwrap()
                } else {
                    rhs.checked_sub(lhs).unwrap()
                };
                delta.checked_mul(U256::from(100)).unwrap() <= rhs
            };

            let input = raw(&pool["quote_amount_in_raw"]);
            let post_base = raw(&pool["post_base_raw"]);
            let output = raw(&pool["base_amount_out_raw"]);
            let actual_pre = raw(&pool["pre_quote_raw"]).checked_mul(output).unwrap();
            let lower = input
                .checked_sub(U256::from(1))
                .unwrap()
                .checked_mul(post_base)
                .unwrap();
            let upper = input.checked_mul(post_base).unwrap();
            assert!(lower <= actual_pre && actual_pre <= upper);
            for fx in ["coinbase_sol_usd_low", "coinbase_sol_usd_high"] {
                assert!(within_one_percent(
                    raw(&pool["post_quote_raw"]),
                    post_base,
                    pool[fx].as_str().unwrap()
                ));
                if let Some(negative) = case.get("negative_control") {
                    assert!(!within_one_percent(
                        raw(&negative["post_quote_raw"]),
                        raw(&negative["post_base_raw"]),
                        pool[fx].as_str().unwrap()
                    ));
                }
            }
        }
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
