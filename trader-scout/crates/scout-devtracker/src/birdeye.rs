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
#[allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const BASE_ADDRESS: &str = "0xa187403aDCc6FCDF64fE9C850e0d9fAf4bA3837f";

    fn bytes_from_hex(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn pubkey_from_hex(hex: &str) -> [u8; 32] {
        bytes_from_hex(hex).try_into().unwrap()
    }

    fn raw_instruction_from_hex(hex: &str) -> scout_core::RawSolanaInstruction {
        scout_core::RawSolanaInstruction {
            program_id: bs58::decode("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")
                .into_vec()
                .unwrap()
                .try_into()
                .unwrap(),
            accounts: Vec::new(),
            data: bytes_from_hex(hex),
            instruction_index: 0,
        }
    }

    #[test]
    fn solana_tracker_8ih_maximum_fdv_is_bounded_within_one_cent() {
        use alloy_primitives::{U256, U512};

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_8ih_legacy_cent_bound_case.json"
        ))
        .unwrap();
        let parse = |text: &str| {
            let decimal = ExactDecimal::parse(text).unwrap();
            let denominator = (0..u32::try_from(decimal.scale).unwrap())
                .fold(U256::from(1), |value, _| value * U256::from(10));
            (U256::from(decimal.coefficient), denominator)
        };
        let price = parse(row["provider"]["price_usd"].as_str().unwrap());
        let raw = |key: &str| {
            row["supply"][key]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap()
        };
        let burn_sum =
            row["supply"]["burns"]
                .as_array()
                .unwrap()
                .iter()
                .fold(U256::ZERO, |sum, burn| {
                    assert!(
                        burn["block_time_unix_s"].as_u64().unwrap()
                            < row["provider"]["timestamp_ms"].as_u64().unwrap() / 1_000
                    );
                    sum + U256::from(burn["amount_raw"].as_u64().unwrap())
                });
        assert_eq!(raw("birth_raw") - burn_sum, raw("peak_raw"));
        let denominator = price.1 * U256::from(1_000_000);
        let numerator = |supply| price.0 * supply * U256::from(100);
        let lower = numerator(raw("peak_raw")) / denominator;
        let upper = numerator(raw("birth_raw")) / denominator;
        assert_eq!(lower, U256::from(10_435_467));
        assert_eq!(upper, U256::from(10_435_468));
        assert!(numerator(burn_sum) < denominator);
        let acceptable = |low: U256, high: U256| {
            high.checked_sub(low)
                .is_some_and(|width| low > U256::ZERO && width <= U256::from(1))
        };
        assert!(acceptable(lower, upper));
        assert!(!acceptable(lower - U256::from(1), upper));
        assert!(!acceptable(U256::ZERO, U256::from(1)));
        assert!(!acceptable(upper, lower));
        for state in row["events"].as_array().unwrap() {
            let instruction =
                raw_instruction_from_hex(state["event_cpi_data_hex"].as_str().unwrap());
            let outcome = scout_dex_solana::classify_pump_amm_event(&instruction);
            let (base, quote, virtual_reserves, pool) = match outcome {
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Buy(event),
                ) => (
                    event.pool_base_token_reserves,
                    event.pool_quote_token_reserves,
                    event.virtual_quote_reserves,
                    event.pool,
                ),
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Sell(event),
                ) => (
                    event.pool_base_token_reserves,
                    event.pool_quote_token_reserves,
                    event.virtual_quote_reserves,
                    event.pool,
                ),
                _ => panic!("captured event-CPI must decode as a trade"),
            };
            assert_eq!(virtual_reserves, None);
            assert_eq!(bs58::encode(pool).into_string(), row["provider"]["pool"]);
            assert_eq!(base.to_string(), state["base_raw"]);
            assert_eq!(quote.to_string(), state["quote_raw"]);
            for endpoint in ["fx_low_usd", "fx_high_usd"] {
                let fx = parse(state[endpoint].as_str().unwrap());
                let left = U512::from(quote) * U512::from(fx.0) * U512::from(price.1);
                let right =
                    U512::from(base) * U512::from(1_000) * U512::from(fx.1) * U512::from(price.0);
                let difference = if left >= right {
                    left - right
                } else {
                    right - left
                };
                assert!(difference * U512::from(100) <= right * U512::from(2));
            }
        }
        assert_eq!(row["supply"]["successful_history_transactions"], 838);
        assert_eq!(row["supply"]["successful_signatures_without_birth"], 837);
        assert!(row["ath_at"].is_null());
    }

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
    fn conservative_provider_window_fdv_bounds_replay_exactly() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_conservative_fdv_bound_cases.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["schema"],
            "validated-provider-window-conservative-fdv-bounds-v1"
        );
        assert_eq!(
            fixture["source"],
            "solana-tracker-window-supply-lower-bound-2pct-pool-fx-validated-v1"
        );
        assert_eq!(fixture["automatic_adapter_rule_added"], false);
        assert_eq!(fixture["intervals_are_exact_peak_supply"], false);
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 14);
        let fht_fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_fht_conservative_fdv_bound_case.json"
        ))
        .unwrap();
        let mut cases_to_replay = cases.iter().collect::<Vec<_>>();
        cases_to_replay.push(&fht_fixture["case"]);
        assert_eq!(cases_to_replay.len(), 15);

        let fraction = |value: &str| {
            let (n, d) = value.split_once('/').unwrap();
            (
                U512::from_str_radix(n, 10).unwrap(),
                U512::from_str_radix(d, 10).unwrap(),
            )
        };
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U512::from(1), |v, _| v.checked_mul(U512::from(10)))
                .unwrap()
        };
        let decimal_ratio = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            (
                U512::from(decimal.coefficient),
                pow10(u32::try_from(decimal.scale).unwrap()),
            )
        };
        let matches_two_percent = |reference: (U512, U512), cents: i64| {
            (reference.0 * U512::from(100))
                .abs_diff(U512::from(u64::try_from(cents).unwrap()) * reference.1)
                <= reference.0 * U512::from(2)
        };
        let mut eligible = Vec::new();
        let mut rejected = Vec::new();

        for case in cases_to_replay {
            let mint = case["mint"].as_str().unwrap();
            let raw_b64 = case["current_mint"]["raw_account_base64"].as_str().unwrap();
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut raw = Vec::new();
            let (mut accumulator, mut bits) = (0_u32, 0_u32);
            for byte in raw_b64.bytes().filter(|byte| *byte != b'=') {
                accumulator = (accumulator << 6)
                    | u32::try_from(
                        alphabet
                            .iter()
                            .position(|candidate| *candidate == byte)
                            .unwrap(),
                    )
                    .unwrap();
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    raw.push(u8::try_from((accumulator >> bits) & 0xff).unwrap());
                }
            }
            let current = &case["current_mint"];
            let owner = current["owner"].as_str().unwrap();
            assert_eq!(
                raw.len(),
                usize::try_from(current["raw_account_length"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(&raw)),
                current["raw_account_sha256"].as_str().unwrap()
            );
            assert_eq!(raw[0..4], [0, 0, 0, 0]);
            assert_eq!(
                u64::from_le_bytes(raw[36..44].try_into().unwrap()).to_string(),
                current["supply_raw"].as_str().unwrap()
            );
            assert_eq!(
                raw[44],
                u8::try_from(case["decimals"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(current["supply_raw"], case["current_supply_raw"]);
            assert_eq!(current["snapshot_is_finalized"], true);
            if owner == "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" {
                assert_eq!(raw.len(), 82);
                assert!(current["extensions"].as_array().unwrap().is_empty());
            } else {
                assert_eq!(owner, "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
                assert_eq!(raw[165], 1);
                assert!(raw[82..165].iter().all(|byte| *byte == 0));
                let mut offset = 166;
                let mut types = Vec::new();
                for extension in current["extensions"].as_array().unwrap() {
                    let kind = u16::from_le_bytes(raw[offset..offset + 2].try_into().unwrap());
                    let len = usize::from(u16::from_le_bytes(
                        raw[offset + 2..offset + 4].try_into().unwrap(),
                    ));
                    let value = &raw[offset + 4..offset + 4 + len];
                    assert_eq!(u64::from(kind), extension["type"].as_u64().unwrap());
                    assert_eq!(
                        u64::try_from(len).unwrap(),
                        extension["length"].as_u64().unwrap()
                    );
                    assert_eq!(
                        format!("{:x}", Sha256::digest(value)),
                        extension["value_sha256"].as_str().unwrap()
                    );
                    types.push(kind);
                    offset += 4 + len;
                }
                assert_eq!(offset, raw.len());
                assert_eq!(types, [18, 19]);
                assert_eq!(current["extension_set_exact"], true);
            }

            let audit = &case["birth_instruction_audit"];
            assert_eq!(audit["strict_allowlist_pass"], true);
            assert_eq!(audit["no_unrecognized_target_token_instruction"], true);
            let instructions = audit["instructions"].as_array().unwrap();
            let init: Vec<_> = instructions
                .iter()
                .filter(|ix| ix["kind"] == "initialize_mint2")
                .collect();
            let mints: Vec<_> = instructions
                .iter()
                .filter(|ix| ix["kind"] == "mint_to")
                .collect();
            let revokes: Vec<_> = instructions
                .iter()
                .filter(|ix| ix["kind"] == "set_mint_authority_none")
                .collect();
            assert_eq!(init.len(), 1);
            assert!(!mints.is_empty());
            assert_eq!(revokes.len(), 1);
            let init_data = bytes_from_hex(init[0]["data_hex"].as_str().unwrap());
            assert_eq!((init_data[0], init_data[1]), (20, raw[44]));
            assert_eq!(audit["initialize_mint2"].as_array().unwrap().len(), 1);
            assert_eq!(
                bs58::encode(&init_data[2..34]).into_string(),
                audit["initialize_mint2"][0]["mint_authority"]
                    .as_str()
                    .unwrap()
            );
            let mut birth = 0_u128;
            let mut last_mint = (-1_i64, -1_i64);
            for mint_ix in &mints {
                let data = bytes_from_hex(mint_ix["data_hex"].as_str().unwrap());
                assert_eq!(data.len(), 9);
                assert_eq!(data[0], 7);
                let amount = u64::from_le_bytes(data[1..9].try_into().unwrap());
                birth += u128::from(amount);
                assert_eq!(mint_ix["accounts"][0].as_str(), Some(mint));
                last_mint = last_mint.max((
                    mint_ix["outer_index"].as_i64().unwrap(),
                    mint_ix["inner_index"].as_i64().unwrap(),
                ));
            }
            let revoke = &revokes[0];
            assert_eq!(
                bytes_from_hex(revoke["data_hex"].as_str().unwrap()),
                [6, 0, 0]
            );
            assert_eq!(
                audit["authority_revocation"]["kind"],
                "set_mint_authority_none"
            );
            assert_eq!(
                audit["authority_revocation"]["authority"],
                audit["initialize_mint2"][0]["mint_authority"]
            );
            assert_eq!(revoke["accounts"][0].as_str(), Some(mint));
            assert!(
                (
                    init[0]["outer_index"].as_i64().unwrap(),
                    init[0]["inner_index"].as_i64().unwrap()
                ) < last_mint
            );
            assert!(
                last_mint
                    < (
                        revoke["outer_index"].as_i64().unwrap(),
                        revoke["inner_index"].as_i64().unwrap()
                    )
            );
            assert_eq!(
                birth.to_string(),
                case["birth_supply_raw"].as_str().unwrap()
            );
            assert_eq!(birth.to_string(), audit["mint_sum_raw"].as_str().unwrap());
            assert_eq!(
                audit["mint_instructions"].as_array().unwrap().len(),
                mints.len()
            );

            let price =
                ExactDecimal::parse(case["provider_price_usd_decimal"].as_str().unwrap()).unwrap();
            let birth = case["birth_supply_raw"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let current = case["current_supply_raw"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let decimals = u8::try_from(case["decimals"].as_u64().unwrap()).unwrap();
            assert!(case["both_snapshots_after_price_peak"].as_bool().unwrap());
            let bounds =
                fdv_cent_bounds_for_nonincreasing_supply(price, birth, current, decimals).unwrap();
            let stored = case["stored_conservative_floor_cents"].as_i64().unwrap();
            assert_eq!(
                [bounds.lower_cents, bounds.upper_cents],
                [
                    case["fdv_cent_floors"][0].as_i64().unwrap(),
                    case["fdv_cent_floors"][1].as_i64().unwrap()
                ]
            );
            assert_eq!(bounds.lower_cents, stored);
            assert!(!bounds.is_same_cent());
            assert_eq!(case["cent_bounds_collapse"], false);
            assert_eq!(case["peak_supply_exact"], false);
            assert_eq!(case["full_history_complete"], false);
            assert!(
                case["source_scope"]
                    .as_str()
                    .unwrap()
                    .contains("not lifetime/all-market ATH")
            );

            let scale = price.scale.checked_add(i32::from(decimals)).unwrap();
            let upper_num = U512::from(price.coefficient) * U512::from(birth);
            let upper_den = pow10(u32::try_from(scale).unwrap());
            let floor_gate = U512::from(u64::try_from(stored).unwrap()) * upper_den
                >= upper_num * U512::from(98);
            assert_eq!(
                floor_gate,
                case["floor_aware_interval_within_2pct"].as_bool().unwrap()
            );
            let low_gate = matches_two_percent(
                fraction(case["reference_lower_fdv_fraction"].as_str().unwrap()),
                stored,
            );
            let high_gate = matches_two_percent(
                fraction(case["reference_upper_fdv_fraction"].as_str().unwrap()),
                stored,
            );
            let spot_gate = low_gate && high_gate;
            assert_eq!(
                spot_gate,
                case["independent_spot_supply_endpoints_within_2pct_of_stored"]
                    .as_bool()
                    .unwrap()
            );
            let accepted = floor_gate && spot_gate;
            assert_eq!(
                accepted,
                case["eligible_manual_lower_bound_candidate"]
                    .as_bool()
                    .unwrap()
            );
            if accepted {
                eligible.push(mint.to_owned());
            } else {
                rejected.push(case["prefix"].as_str().unwrap().to_owned());
            }
        }

        let fht_case = &fht_fixture["case"];
        let hash = |raw: &str| format!("{:x}", Sha256::digest(raw.as_bytes()));
        let same = |a: (U512, U512), b: (U512, U512)| a.0 * b.1 == b.0 * a.1;
        assert_eq!(
            fht_fixture["certificate_sha256"],
            "abdb98ca0674c9f2b04fc3eabb3c0d1d95f3b3341a0f8ee3dd64218b792bcd33"
        );
        let current = &fht_case["current_mint"];
        let capture = &fht_fixture["current_mint_capture"];
        let (request_raw, response_raw) = (
            capture["request_json_raw"].as_str().unwrap(),
            capture["response_json_raw"].as_str().unwrap(),
        );
        let (request_sha, response_sha) = (hash(request_raw), hash(response_raw));
        assert_eq!(
            (request_sha.as_str(), response_sha.as_str()),
            (
                "60dedd3c6b5c410ac12e55897987ffdfae486caa838d8ce6cdb563748eb53d27",
                "def69007a71b4a64aed415157a215a17d9e57939357ebb7093883637d2089996"
            )
        );
        assert_eq!(
            (request_sha.as_str(), response_sha.as_str()),
            (
                current["snapshot_request_sha256"].as_str().unwrap(),
                current["snapshot_response_sha256"].as_str().unwrap()
            )
        );
        let (request, response): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(request_raw).unwrap(),
            serde_json::from_str(response_raw).unwrap(),
        );
        let addresses = request["params"][0].as_array().unwrap();
        assert_eq!(
            addresses.iter().filter(|a| *a == &fht_case["mint"]).count(),
            1
        );
        assert_eq!(
            addresses.iter().position(|a| a == &fht_case["mint"]),
            Some(5)
        );
        assert_eq!(
            (
                current["snapshot_selected_original_index"].as_u64(),
                current["singleton_selected_view_of_original_capture"].as_bool()
            ),
            (Some(5), Some(true))
        );
        assert_eq!(request["params"][1]["commitment"], "finalized");
        assert_eq!(
            response["result"]["context"]["slot"],
            current["snapshot_context_slot"]
        );
        assert_eq!(
            (
                response["result"]["value"][5]["owner"].as_str(),
                response["result"]["value"][5]["data"][0].as_str()
            ),
            (
                current["owner"].as_str(),
                current["raw_account_base64"].as_str()
            )
        );

        let wire_raw = fht_fixture["wire_proof_json_raw"].as_str().unwrap();
        assert_eq!(
            hash(wire_raw),
            "b3e6bc95aaeae0a350b383ef0b8353ecf77b810c0d58dfcd822d9a6a967d84d9"
        );
        let wire: serde_json::Value = serde_json::from_str(wire_raw).unwrap();
        assert_eq!(
            (
                wire["state_count"].as_u64(),
                wire["trade_count"].as_u64(),
                wire["dated_official_idl"]["version"].as_str()
            ),
            (Some(4), Some(2), Some("feb17"))
        );
        assert_eq!(
            wire["dated_official_idl"]["sha256"],
            "5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6"
        );
        let manifest_raw = fht_fixture["canonical_manifest_json_raw"].as_str().unwrap();
        assert_eq!(
            hash(manifest_raw),
            "aa5e97995587ae83a97ccdeeda9b225a3c8169c42a9c6cd5984043c466596030"
        );
        let manifest: serde_json::Value = serde_json::from_str(manifest_raw).unwrap();
        assert_eq!(
            (
                manifest["status"].as_str(),
                manifest["authorized_method_cap"].as_u64(),
                manifest["attempts"].as_array().map(Vec::len)
            ),
            (Some("passed"), Some(2), Some(2))
        );
        assert_eq!(
            (
                manifest["attempts"][0]["signature"].as_str(),
                manifest["attempts"][1]["signature"].as_str()
            ),
            (
                fht_case["birth_signature"].as_str(),
                wire["rows"][1]["signature"].as_str()
            )
        );
        assert!(
            manifest["comparison"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["whole_transaction_equal"] == true && r["entire_meta_equal"] == true)
        );

        let provider = decimal_ratio(fht_case["provider_price_usd_decimal"].as_str().unwrap());
        for state in fht_fixture["global_states"].as_array().unwrap() {
            let native = fraction(state["spot_sol_per_token"].as_str().unwrap());
            let quote =
                U512::from_str_radix(state["quote_lamports"].as_str().unwrap(), 10).unwrap();
            let base = U512::from_str_radix(state["base_raw"].as_str().unwrap(), 10).unwrap();
            assert_eq!(native.0 * base * U512::from(1_000), quote * native.1);
            let candle = &state["sol_usd_candle"];
            for (usd, fx) in [
                (
                    fraction(state["usd_low"].as_str().unwrap()),
                    decimal_ratio(candle[3].as_str().unwrap()),
                ),
                (
                    fraction(state["usd_high"].as_str().unwrap()),
                    decimal_ratio(candle[2].as_str().unwrap()),
                ),
            ] {
                assert!(same(usd, (native.0 * fx.0, native.1 * fx.1)));
                let (a, b) = (usd.0 * provider.1, provider.0 * usd.1);
                assert!(a.abs_diff(b) * U512::from(100) <= b * U512::from(2));
            }
        }
        assert!(
            fht_fixture["global_states"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["offset_ms_from_tracker_peak"] == -708)
        );
        assert_eq!(
            fht_case["provider_peak_ms_original"].as_u64().unwrap()
                - fht_case["provider_peak_ms"].as_u64().unwrap(),
            708
        );
        let combined = fraction(fht_fixture["combined_uncertainty_max"].as_str().unwrap());
        assert!(combined.0 * U512::from(100) <= combined.1 * U512::from(2));
        for tx in wire["rows"].as_array().unwrap() {
            let trade = &tx["trade_instruction"];
            let data = bytes_from_hex(trade["data_hex"].as_str().unwrap());
            assert_eq!(data.len(), 25);
            assert_eq!(
                &data[..8],
                &[0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea]
            );
            assert_eq!(
                (
                    trade["account_count"].as_u64(),
                    trade["dated_idl_expected_bytes"].as_u64(),
                    trade["dated_idl_account_count"].as_u64()
                ),
                (Some(23), Some(25), Some(23))
            );
            assert_eq!(
                (
                    tx["event_cpi"]["authority_present"].as_bool(),
                    tx["event_cpi"]["stack_height"].as_u64(),
                    tx["event"]["name"].as_str()
                ),
                (Some(true), Some(2), Some("BuyEvent"))
            );
            let event = bytes_from_hex(tx["event"]["payload_hex"].as_str().unwrap());
            let cert_tx = fht_fixture["target_transactions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["signature"] == tx["signature"])
                .unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&event)),
                cert_tx["event_sha256"]
            );
        }

        assert_eq!(eligible.len(), 13);
        assert_eq!(rejected, ["4evPrwwv", "vC95xGRq"]);
        let price = ExactDecimal::parse("1").unwrap();
        let rounded = fdv_cent_bounds_for_nonincreasing_supply(price, 505, 495, 3).unwrap();
        assert_eq!((rounded.lower_cents, rounded.upper_cents), (49, 50));
        assert!(u128::from(505_u64 - 495) * 100 <= 505 * 2); // Raw width alone passes.
        assert!(U512::from(49) * U512::from(1_000) < U512::from(505) * U512::from(98)); // Floor-aware gate rejects.
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

    #[test]
    fn geckoterminal_base_3607_marginal_spot_window_uses_all_valid_states() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/geckoterminal_base_3607_marginal_spot_window_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["source"].as_str(),
            Some("geckoterminal-window-supply-pool-state-fx-validated-v1")
        );
        assert_eq!(
            row["candidate_certificate_sha256"].as_str(),
            Some("231fb11c15f6d517d5eeaa694e538d4d62fbcf2de948ad9a55d41569dea747f1")
        );
        assert_eq!(
            row["all_state_recheck_sha256"].as_str(),
            Some("f2747b81e124831a0b0478af14c96907d6e686e9fed0ae570c819d5e86697452")
        );
        assert_eq!(row["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            row["token"].as_str(),
            Some("0x3607f8f1d16c5ae717b61d28eec1d624df763b07")
        );
        assert!(row["ath_at"].is_null());

        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let parse = |value: &serde_json::Value| value.as_str().unwrap().parse::<U256>().unwrap();
        let gcd = |mut left: U256, mut right: U256| {
            while !right.is_zero() {
                let remainder = left.checked_rem(right).unwrap();
                left = right;
                right = remainder;
            }
            left
        };

        let provider =
            ExactDecimal::parse(row["provider_observation"]["price_usd"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            row["provider_observation"]["kind"].as_str(),
            Some("geckoterminal_daily_ohlcv_high_usd")
        );
        assert_eq!(
            row["provider_observation"]["days_utc"],
            json!(["2026-05-04", "2026-05-05"])
        );
        assert!(
            row["provider_observation"]["same_high_on_both_days"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            row["all_state_recheck"]["raw_may4_swap_logs"].as_u64(),
            Some(15)
        );
        assert_eq!(
            row["all_state_recheck"]["eligible_positive_liquidity_post_swap_states"].as_u64(),
            Some(15)
        );
        assert_eq!(
            row["all_state_recheck"]["one_sided_positive_liquidity_states_included"].as_u64(),
            Some(1)
        );
        assert_eq!(
            row["all_state_recheck"]["prior_manifest_state_count_not_raw_event_count"].as_u64(),
            Some(31)
        );
        assert_eq!(row["all_state_recheck"]["pool_count"].as_u64(), Some(4));

        let maximum = &row["all_state_recheck"]["maximum"];
        assert_eq!(maximum["currency0"], row["token"]);
        assert_eq!(
            maximum["currency1"].as_str(),
            Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
        );
        assert_eq!(maximum["currency0_role"].as_str(), Some("target"));
        assert_eq!(maximum["currency1_role"].as_str(), Some("USDC"));
        assert_eq!(maximum["currency1_decimals"].as_u64(), Some(6));
        assert!(maximum["receipt_success"].as_bool().unwrap());
        assert!(
            maximum["header_hash_matches_receipt_and_log"]
                .as_bool()
                .unwrap()
        );

        let sqrt_price_x96 = parse(&maximum["sqrt_price_x96"]);
        let spot_num = sqrt_price_x96
            .checked_mul(sqrt_price_x96)
            .unwrap()
            .checked_mul(U256::from(1_000_000_000_000_u64))
            .unwrap();
        let spot_den = U256::from(2).checked_pow(U256::from(192)).unwrap();
        let divisor = gcd(spot_num, spot_den);
        assert_eq!(
            spot_num.checked_div(divisor).unwrap(),
            parse(&maximum["spot_fraction_numerator"])
        );
        assert_eq!(
            spot_den.checked_div(divisor).unwrap(),
            parse(&maximum["spot_fraction_denominator"])
        );

        let provider_num = U256::from(provider.coefficient);
        let provider_scale = u32::try_from(provider.scale).unwrap();
        let provider_den = pow10(provider_scale);
        let fx = &row["fx_and_gate"];
        for rate in ["kraken_usdc_usd_daily_low", "kraken_usdc_usd_daily_high"] {
            let fx_price = ExactDecimal::parse(fx[rate].as_str().unwrap()).unwrap();
            let spot_usd_num = spot_num
                .checked_mul(U256::from(fx_price.coefficient))
                .unwrap();
            let spot_usd_den = spot_den
                .checked_mul(pow10(u32::try_from(fx_price.scale).unwrap()))
                .unwrap();
            let candidate_cross = spot_usd_num.checked_mul(provider_den).unwrap();
            let provider_cross = provider_num.checked_mul(spot_usd_den).unwrap();
            let difference = if candidate_cross >= provider_cross {
                candidate_cross.checked_sub(provider_cross).unwrap()
            } else {
                provider_cross.checked_sub(candidate_cross).unwrap()
            };
            assert!(
                difference.checked_mul(U256::from(100)).unwrap()
                    <= provider_cross.checked_mul(U256::from(2)).unwrap()
            );
        }
        assert!(
            fx["both_fx_endpoints_within_two_percent_of_provider_price"]
                .as_bool()
                .unwrap()
        );

        let supply = &row["supply"];
        assert_eq!(supply["verified_tokens"].as_str(), Some("100000000000"));
        assert_eq!(
            supply["total_supply_raw"].as_str(),
            Some("100000000000000000000000000000")
        );
        assert_eq!(supply["mint_count_creation_to_anchor"].as_u64(), Some(1));
        assert_eq!(supply["burn_count_creation_to_anchor"].as_u64(), Some(0));
        let supply_raw = parse(&supply["total_supply_raw"]);
        let supply_num = provider_num
            .checked_mul(supply_raw)
            .unwrap()
            .checked_mul(U256::from(100))
            .unwrap();
        let supply_den = pow10(provider_scale + 18);
        assert_eq!(
            supply_num.checked_div(supply_den).unwrap(),
            U256::from(
                supply["supply_floor_cents_from_provider_price"]
                    .as_u64()
                    .unwrap()
            )
        );
        assert!(
            supply["bridge_mint_burn_capability_is_not_an_observed_event"]
                .as_bool()
                .unwrap()
        );

        let one_sided = &row["all_state_recheck"]["one_sided_eligible_state"];
        assert!(
            one_sided["eligible_positive_liquidity_post_swap_state"]
                .as_bool()
                .unwrap()
        );
        assert!(one_sided["one_sided_swap"].as_bool().unwrap());
        assert!(!one_sided["execution_average_defined"].as_bool().unwrap());
        assert_eq!(one_sided["amount1_raw_signed"].as_str(), Some("0"));
        assert!(!parse(&one_sided["sqrt_price_x96"]).is_zero());
        assert!(!parse(&one_sided["liquidity_after_raw"]).is_zero());
        assert!(sqrt_price_x96 > parse(&one_sided["sqrt_price_x96"]));

        let execution = &row["same_event_execution_average"];
        assert_eq!(
            execution["kind"].as_str(),
            Some("uniswap_v4_swap_execution_average")
        );
        assert!(execution["same_swap_event"].as_bool().unwrap());
        assert!(
            execution["treatment"]
                .as_str()
                .unwrap()
                .contains("Keep distinct")
        );
        let execution_numerator = parse(&execution["usdc_raw_output"])
            .checked_mul(U256::from(1_000_000_000_000_u64))
            .unwrap();
        let execution_denominator = parse(&execution["target_raw_input"]);
        assert!(
            execution_numerator.checked_mul(provider_den).unwrap()
                > provider_num
                    .checked_mul(execution_denominator)
                    .unwrap()
                    .checked_mul(U256::from(7))
                    .unwrap()
        );
    }

    #[test]
    fn robinhood_scoped_reference_maxima_weight_every_event_before_selecting() {
        use alloy_primitives::{U256, U512};
        use std::cmp::Ordering;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_scoped_reference_maxima_case.json"
        ))
        .unwrap();
        assert_eq!(row["chain_id"].as_u64(), Some(4663));
        assert_eq!(
            row["source_review_sha256"].as_str(),
            Some("06e56f774c800cb8e7539159ddc1b6cdc9a2e82d6b33ca495e98a001abed3399")
        );
        assert_eq!(
            row["source_kinds"]["weth"].as_str(),
            Some("onchain-pool-window-supply-cex-reference-v1")
        );
        assert_eq!(
            row["source_kinds"]["aapl"].as_str(),
            Some("onchain-pool-window-supply-stock-trade-fx-reference-v1")
        );

        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let gcd = |mut left: U256, mut right: U256| {
            while !right.is_zero() {
                let remainder = left.checked_rem(right).unwrap();
                left = right;
                right = remainder;
            }
            left
        };
        let normalize = |numerator: U256, denominator: U256| {
            let divisor = gcd(numerator, denominator);
            (
                numerator.checked_div(divisor).unwrap(),
                denominator.checked_div(divisor).unwrap(),
            )
        };
        let fraction = |raw: &str| {
            let (numerator, denominator) = raw.split_once('/').unwrap();
            normalize(
                numerator.parse::<U256>().unwrap(),
                denominator.parse::<U256>().unwrap(),
            )
        };
        let decimal_fraction = |raw: &str| {
            let value = ExactDecimal::parse(raw).unwrap();
            normalize(
                U256::from(value.coefficient),
                pow10(u32::try_from(value.scale).unwrap()),
            )
        };
        let multiply = |left: (U256, U256), right: (U256, U256)| {
            let left_cancel = gcd(left.0, right.1);
            let right_cancel = gcd(right.0, left.1);
            normalize(
                left.0
                    .checked_div(left_cancel)
                    .unwrap()
                    .checked_mul(right.0.checked_div(right_cancel).unwrap())
                    .unwrap(),
                left.1
                    .checked_div(right_cancel)
                    .unwrap()
                    .checked_mul(right.1.checked_div(left_cancel).unwrap())
                    .unwrap(),
            )
        };
        let compare = |left: (U256, U256), right: (U256, U256)| {
            (U512::from(left.0) * U512::from(right.1))
                .cmp(&(U512::from(right.0) * U512::from(left.1)))
        };
        let positive_magnitude = |raw: &str| {
            raw.strip_prefix('-')
                .unwrap_or(raw)
                .parse::<U256>()
                .unwrap()
        };
        let floor_fdv_cents = |price: (U256, U256)| {
            price
                .0
                .checked_mul(U256::from(1_000_000_000_u64))
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap()
                .checked_div(price.1)
                .unwrap()
        };

        let weth = &row["weth_case"];
        assert_eq!(
            weth["model_kind"].as_str(),
            Some("direct_initial_pool_swap_times_coinbase_ethusd_utc_minute_close")
        );
        assert_eq!(
            weth["complete_pool_swap_scan"]["post_creation_event_count"],
            3
        );
        assert!(
            weth["complete_pool_swap_scan"]["all_creation_tx_events_excluded"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            weth["supply"]["total_supply_raw"].as_str(),
            Some("1000000000000000000000000000")
        );
        assert_eq!(weth["supply"]["decimals"].as_u64(), Some(18));
        assert!(
            weth["supply"]["total_supply_confirmed_at_event"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(weth["supply"]["burns_through_head"].as_u64(), Some(0));

        let mut weth_prices = Vec::new();
        for event in weth["events"].as_array().unwrap() {
            let quote_ratio = normalize(
                positive_magnitude(event["raw_quote_delta"].as_str().unwrap()),
                positive_magnitude(event["raw_token_delta"].as_str().unwrap()),
            );
            assert_eq!(
                quote_ratio,
                fraction(event["quote_per_token_fraction"].as_str().unwrap())
            );
            let close = decimal_fraction(event["coinbase_close_usd_per_eth"].as_str().unwrap());
            let reference_price = multiply(quote_ratio, close);
            assert_eq!(
                reference_price,
                fraction(event["reference_price_fraction"].as_str().unwrap())
            );
            let low = multiply(
                quote_ratio,
                decimal_fraction(event["coinbase_low"].as_str().unwrap()),
            );
            let high = multiply(
                quote_ratio,
                decimal_fraction(event["coinbase_high"].as_str().unwrap()),
            );
            assert_eq!(
                low,
                fraction(event["uncertainty_low_fraction"].as_str().unwrap())
            );
            assert_eq!(
                high,
                fraction(event["uncertainty_high_fraction"].as_str().unwrap())
            );
            assert!(compare(low, reference_price) != Ordering::Greater);
            assert!(compare(reference_price, high) != Ordering::Greater);
            weth_prices.push((event, reference_price));
        }
        let (weth_winner, weth_maximum) = weth_prices
            .iter()
            .max_by(|left, right| compare(left.1, right.1))
            .unwrap();
        let weth_selected = &weth["selected_maximum"];
        assert_eq!(weth_winner["block"], weth_selected["block"]);
        assert_eq!(weth_winner["log_index"], weth_selected["log_index"]);
        assert_eq!(
            *weth_maximum,
            fraction(weth_selected["reference_price_fraction"].as_str().unwrap())
        );
        assert_eq!(weth_selected["block"].as_u64(), Some(46_586_365));
        assert_eq!(weth_selected["log_index"].as_u64(), Some(11));
        assert_eq!(
            floor_fdv_cents(*weth_maximum),
            U256::from(weth_selected["fdv_cents_floor"].as_u64().unwrap())
        );

        let aapl = &row["aapl_case"];
        assert_eq!(
            aapl["model_kind"].as_str(),
            Some(
                "direct_initial_pool_swap_times_last_preceding_usdg_aapl_v3_trade_times_kraken_usdgusd_utc_day_close"
            )
        );
        assert_eq!(
            aapl["complete_pool_swap_scan"]["event_count"].as_u64(),
            Some(6)
        );
        assert_eq!(
            aapl["supply"]["total_supply_raw"].as_str(),
            Some("1000000000000000000000000000")
        );
        assert_eq!(aapl["supply"]["decimals"].as_u64(), Some(18));
        assert!(
            aapl["supply"]["total_supply_confirmed_at_event"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(aapl["supply"]["burns_through_head"].as_u64(), Some(0));

        let mut aapl_prices = Vec::new();
        for event in aapl["events"].as_array().unwrap() {
            let ratio = normalize(
                positive_magnitude(event["raw_quote_delta"].as_str().unwrap()),
                positive_magnitude(event["raw_token_delta"].as_str().unwrap()),
            );
            assert_eq!(
                ratio,
                fraction(event["quote_per_token_fraction"].as_str().unwrap())
            );
            let group = &aapl["quote_fx_groups"][event["fx_bracket_group"].as_str().unwrap()];
            let reference_quote = fraction(event["last_trade_before_fraction"].as_str().unwrap());
            assert_eq!(
                reference_quote,
                fraction(group["before_fraction"].as_str().unwrap())
            );
            let kraken_close = decimal_fraction(group["daily_usdg_usd_close"].as_str().unwrap());
            let reference_price = multiply(multiply(ratio, reference_quote), kraken_close);
            assert_eq!(
                reference_price,
                fraction(event["reference_price_fraction"].as_str().unwrap())
            );

            let before = fraction(group["before_fraction"].as_str().unwrap());
            let after = fraction(group["after_fraction"].as_str().unwrap());
            let (lower_quote, upper_quote) = if compare(before, after) == Ordering::Greater {
                (after, before)
            } else {
                (before, after)
            };
            let low = multiply(
                multiply(ratio, lower_quote),
                decimal_fraction(group["daily_usdg_usd_low"].as_str().unwrap()),
            );
            let high = multiply(
                multiply(ratio, upper_quote),
                decimal_fraction(group["daily_usdg_usd_high"].as_str().unwrap()),
            );
            assert_eq!(
                low,
                fraction(event["uncertainty_low_fraction"].as_str().unwrap())
            );
            assert_eq!(
                high,
                fraction(event["uncertainty_high_fraction"].as_str().unwrap())
            );
            aapl_prices.push((event, reference_price));
        }
        let (aapl_winner, aapl_maximum) = aapl_prices
            .iter()
            .max_by(|left, right| compare(left.1, right.1))
            .unwrap();
        let aapl_selected = &aapl["selected_maximum"];
        assert_eq!(aapl_winner["block"], aapl_selected["block"]);
        assert_eq!(aapl_winner["log_index"], aapl_selected["log_index"]);
        assert_eq!(
            *aapl_maximum,
            fraction(aapl_selected["reference_price_fraction"].as_str().unwrap())
        );
        assert_eq!(aapl_selected["block"].as_u64(), Some(27_216_237));
        assert_eq!(aapl_selected["log_index"].as_u64(), Some(22));
        assert_eq!(
            floor_fdv_cents(*aapl_maximum),
            U256::from(aapl_selected["fdv_cents_floor"].as_u64().unwrap())
        );
        for group_name in ["first", "second"] {
            let group = &aapl["quote_fx_groups"][group_name];
            assert_eq!(group["bracket_seconds"].as_array().unwrap().len(), 2);
            assert!(group["bracket_seconds"][0].as_u64().unwrap() > 0);
            assert!(group["bracket_seconds"][1].as_u64().unwrap() > 0);
            assert!(
                group["before_timestamp_utc"].as_str().unwrap()
                    < group["after_timestamp_utc"].as_str().unwrap()
            );
        }

        let negative = &row["unbracketed_negative_control"];
        assert_eq!(
            negative["token"].as_str(),
            Some("0x48c01e934d149b54af69398a3f767b7e399fb4aa")
        );
        assert_eq!(
            negative["status"].as_str(),
            Some("not_price_candidate_quote_bracket_incomplete")
        );
        assert!(
            !negative["source_model_max_claim_allowed"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            negative["post_event_neighbor_headers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        for header in negative["post_event_neighbor_headers"].as_array().unwrap() {
            assert!(
                header["timestamp_utc"].as_str().unwrap()
                    > negative["event_max_utc"].as_str().unwrap()
            );
        }

        let followup: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_48c_usdg_aapl_followup_case.json"
        ))
        .unwrap();
        assert_eq!(
            followup["candidate_sha256"].as_str(),
            Some("988799f1fd6b5a105eacbad67e5698109378467c3519f617fa08147989c080c4")
        );
        assert_eq!(
            followup["source"].as_str(),
            Some("onchain-pool-window-supply-stock-trade-fx-reference-v1")
        );
        assert_eq!(
            followup["token"].as_str(),
            Some("0x48c01e934d149b54af69398a3f767b7e399fb4aa")
        );
        assert_eq!(
            followup["pool"].as_str(),
            Some("0x591948ec42765aa9f190e62fd3cec3fa8a5ea13d593c9e8d58858debe960a1ab")
        );
        assert_eq!(followup["target_and_aapl_decimals"].as_u64(), Some(18));
        assert!(followup["scope"]["ath_at"].is_null());
        assert_eq!(followup["supply"]["decimals"].as_u64(), Some(18));
        assert_eq!(
            followup["supply"]["raw"].as_str(),
            Some("1000000000000000000000000000")
        );
        assert_eq!(followup["supply"]["mints_after_creation"].as_u64(), Some(0));
        assert_eq!(followup["supply"]["burns_after_creation"].as_u64(), Some(0));
        assert_eq!(followup["events"].as_array().unwrap().len(), 15);
        assert_eq!(followup["scope"]["groups"]["22791875"].as_u64(), Some(10));
        assert_eq!(followup["scope"]["groups"]["22792033"].as_u64(), Some(5));
        assert_eq!(
            followup["fx"]["raw_sha256"].as_str(),
            Some("2c3222370fb29cf678eb33bc08fd0112fc1a175a2e5ec78e38c069987b460517")
        );

        let usd_close = decimal_fraction(followup["fx"]["usdG_usd_close"].as_str().unwrap());
        let usd_low = decimal_fraction(followup["fx"]["usdG_usd_low"].as_str().unwrap());
        let usd_high = decimal_fraction(followup["fx"]["usdG_usd_high"].as_str().unwrap());
        let mut followup_prices = Vec::new();
        let mut observed_groups = std::collections::BTreeMap::<String, u64>::new();
        for event in followup["events"].as_array().unwrap() {
            let target = positive_magnitude(event["raw_target_delta"].as_str().unwrap());
            let aapl_raw = positive_magnitude(event["raw_aapl_delta"].as_str().unwrap());
            let event_ratio = normalize(aapl_raw, target);
            let group = &followup["quote_groups"][event["quote_group"].as_str().unwrap()];
            let group_name = event["quote_group"].as_str().unwrap();
            assert_eq!(event["block"].as_u64().unwrap().to_string(), group_name);
            *observed_groups.entry(group_name.to_string()).or_default() += 1;
            assert!(
                group["before_timestamp_unix"].as_u64().unwrap()
                    < event["timestamp_unix"].as_u64().unwrap()
            );
            assert!(
                event["timestamp_unix"].as_u64().unwrap()
                    < group["after_timestamp_unix"].as_u64().unwrap()
            );
            let quote_before = fraction(group["before_fraction"].as_str().unwrap());
            let quote_after = fraction(group["after_fraction"].as_str().unwrap());
            let reference = multiply(multiply(event_ratio, quote_before), usd_close);
            let lower_quote = if compare(quote_before, quote_after) == Ordering::Greater {
                quote_after
            } else {
                quote_before
            };
            let upper_quote = if compare(quote_before, quote_after) == Ordering::Greater {
                quote_before
            } else {
                quote_after
            };
            let low = multiply(multiply(event_ratio, lower_quote), usd_low);
            let high = multiply(multiply(event_ratio, upper_quote), usd_high);
            assert!(compare(low, reference) != Ordering::Greater);
            assert!(compare(reference, high) != Ordering::Greater);
            followup_prices.push((event, reference));
        }
        assert_eq!(observed_groups["22791875"], 10);
        assert_eq!(observed_groups["22792033"], 5);
        let (winner, maximum) = followup_prices
            .iter()
            .max_by(|left, right| compare(left.1, right.1))
            .unwrap();
        let expected = &followup["expected_maximum"];
        assert_eq!(winner["block"], expected["block"]);
        assert_eq!(winner["log_index"], expected["log_index"]);
        assert_eq!(
            *maximum,
            fraction(expected["price_fraction"].as_str().unwrap())
        );
        assert_eq!(
            floor_fdv_cents(*maximum),
            U256::from(expected["fdv_cents_floor"].as_u64().unwrap())
        );
        assert_eq!(expected["block"].as_u64(), Some(22_791_875));
        assert_eq!(expected["log_index"].as_u64(), Some(129));
        assert_eq!(expected["fdv_cents_floor"].as_u64(), Some(6_143_395));
        assert!(
            !followup["supersedes_historical_negative_snapshot"]["source_model_max_claim_allowed"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !followup["limitations"]["all_market_or_lifetime_max_claim"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !followup["limitations"]["automatic_adapter_or_promotion"]
                .as_bool()
                .unwrap()
        );

        let nvda: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_nvda_reference_followup_case.json"
        ))
        .unwrap();
        assert_eq!(
            nvda["candidate_sha256"].as_str(),
            Some("77d19011f3c103ae03c90bde88264cb5744f2fb64ee8aaead04a82c600bf7e97")
        );
        assert_eq!(
            nvda["source"].as_str(),
            Some("onchain-pool-window-supply-stock-trade-fx-reference-v1")
        );
        assert_eq!(
            nvda["token"].as_str(),
            Some("0x1e6489ef6d75c6f785843669b01262a7562b1e18")
        );
        assert_eq!(nvda["target_and_quote_decimals"].as_u64(), Some(18));
        assert_eq!(nvda["events"].as_array().unwrap().len(), 3);
        assert_eq!(
            nvda["supply"]["raw"].as_str(),
            Some("1000000000000000000000000000")
        );
        assert_eq!(nvda["supply"]["mints"].as_u64(), Some(1));
        assert_eq!(nvda["supply"]["burns"].as_u64(), Some(0));
        assert_eq!(nvda["supply"]["through_block"].as_u64(), Some(17_404_579));
        assert_eq!(
            nvda["supply"]["manifest_sha256"].as_str(),
            Some("870cfbf0d02cd39788e4e15229782ff9dbd70131d89bfaa8a735f0664994a65c")
        );

        let usd_close = decimal_fraction(nvda["fx"]["close"].as_str().unwrap());
        let usd_low = decimal_fraction(nvda["fx"]["low"].as_str().unwrap());
        let usd_high = decimal_fraction(nvda["fx"]["high"].as_str().unwrap());
        let mut nvda_prices = Vec::new();
        for event in nvda["events"].as_array().unwrap() {
            let target_delta = positive_magnitude(event["raw_target_delta"].as_str().unwrap());
            let quote_delta = positive_magnitude(event["raw_quote_delta"].as_str().unwrap());
            let target_quote_ratio = normalize(quote_delta, target_delta);
            let reference = &event["quote_reference"];
            assert_eq!(
                reference["quote_pool"].as_str(),
                Some("0xd4eb21209c4d6093f80b5b84f5c45cc093ea14a3")
            );
            let same_timestamp =
                reference["timestamp_unix"].as_u64() == event["timestamp_unix"].as_u64();
            assert!(
                reference["timestamp_unix"].as_u64().unwrap()
                    <= event["timestamp_unix"].as_u64().unwrap()
            );
            if same_timestamp {
                assert_eq!(reference["tx"], event["tx"]);
                assert!(
                    reference["log_index"].as_u64().unwrap() < event["log_index"].as_u64().unwrap()
                );
            }
            let usdg_raw = positive_magnitude(reference["raw_usdg_delta"].as_str().unwrap());
            let nvda_raw = positive_magnitude(reference["raw_nvda_delta"].as_str().unwrap());
            let usdg_per_nvda = normalize(
                usdg_raw
                    .checked_mul(U256::from(1_000_000_000_000_u64))
                    .unwrap(),
                nvda_raw,
            );
            assert_eq!(
                usdg_per_nvda,
                fraction(reference["quote_per_nvda_fraction"].as_str().unwrap())
            );
            let price = multiply(multiply(target_quote_ratio, usdg_per_nvda), usd_close);
            let low = multiply(multiply(target_quote_ratio, usdg_per_nvda), usd_low);
            let high = multiply(multiply(target_quote_ratio, usdg_per_nvda), usd_high);
            assert!(compare(low, price) != Ordering::Greater);
            assert!(compare(price, high) != Ordering::Greater);
            assert!(event["receipt_header_verified"].as_bool().unwrap());
            nvda_prices.push((event, price));
        }

        let route = &nvda["same_transaction_quote_route"];
        assert!(route["exact_native_amount_match"].as_bool().unwrap());
        assert_eq!(route["quote_log_index"].as_u64(), Some(9));
        assert_eq!(route["target_log_index"].as_u64(), Some(12));
        assert_eq!(
            route["quote_raw_nvda_output"],
            route["target_raw_nvda_input"]
        );
        assert_eq!(route["target_tx"], nvda_prices[0].0["tx"]);
        assert_eq!(
            route["receipt_sha256"].as_str(),
            Some("71192a742a9a76357ed4d7949d1255ad95c3e9456697f309f4e8a4382fbb3f3b")
        );

        let (nvda_winner, nvda_maximum) = nvda_prices
            .iter()
            .max_by(|left, right| compare(left.1, right.1))
            .unwrap();
        let expected = &nvda["expected_maximum"];
        assert_eq!(nvda_winner["block"], expected["block"]);
        assert_eq!(nvda_winner["log_index"], expected["log_index"]);
        assert_eq!(
            *nvda_maximum,
            fraction(expected["price_fraction"].as_str().unwrap())
        );
        assert_eq!(floor_fdv_cents(*nvda_maximum), U256::from(2_020_370));
        assert_eq!(expected["fdv_cents_floor"].as_u64(), Some(2_020_370));
        assert!(nvda["limits"]["ath_at"].is_null());
        assert!(!nvda["limits"]["all_market_or_lifetime"].as_bool().unwrap());
        assert!(!nvda["limits"]["automatic_promotion"].as_bool().unwrap());
    }

    #[test]
    fn robinhood_doppler_candidates_maximize_all_minute_weighted_events() {
        use alloy_primitives::{U256, U512};
        use std::cmp::Ordering;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_doppler_no_v4_reference_maxima_case.json"
        ))
        .unwrap();
        assert_eq!(fixture["chain_id"].as_u64(), Some(4663));
        assert_eq!(fixture["anchor_block"].as_u64(), Some(87_808_917));
        assert_eq!(
            fixture["anchor_hash"].as_str(),
            Some("0x1d6082d2ec642c43409a394a2fa56e382fdf7cfb63d8dffc5addc050224f46e0")
        );
        assert_eq!(
            fixture["source"].as_str(),
            Some("onchain-pool-window-supply-cex-reference-v1")
        );
        assert_eq!(
            fixture["event_delta_semantics"].as_str(),
            Some(
                "quote_delta_raw is PoolManager Swap amount0 and token_delta_raw is amount1; initialized currencies are [WETH, target], both 18 decimals."
            )
        );
        assert_eq!(
            fixture["summary_sha256"].as_str(),
            Some("f82812ccee80ce7ffcd974fb1343fe40155da83447484cc5e551263f5261c8a5")
        );
        assert_eq!(
            fixture["raw_capture_sha256"].as_str(),
            Some("667c00d5b5ae020d93735556947e6cb2d0d003ee6d8a7f0ccdc68ff5386d3ffe")
        );
        assert_eq!(
            fixture["coinbase_evidence_sha256"].as_str(),
            Some("cd2b70bf6ea7cba494b4634dd5303ab35fd92c38754355009a482c86e8bc8113")
        );
        assert_eq!(fixture["cases"].as_array().unwrap().len(), 3);

        let gcd = |mut left: U256, mut right: U256| {
            while !right.is_zero() {
                let remainder = left.checked_rem(right).unwrap();
                left = right;
                right = remainder;
            }
            left
        };
        let normalize = |numerator: U256, denominator: U256| {
            let divisor = gcd(numerator, denominator);
            (
                numerator.checked_div(divisor).unwrap(),
                denominator.checked_div(divisor).unwrap(),
            )
        };
        let decimal_fraction = |raw: &str| {
            let value = ExactDecimal::parse(raw).unwrap();
            let denominator = (0..u32::try_from(value.scale).unwrap())
                .try_fold(U256::from(1), |acc, _| acc.checked_mul(U256::from(10)))
                .unwrap();
            normalize(U256::from(value.coefficient), denominator)
        };
        let multiply = |left: (U256, U256), right: (U256, U256)| {
            let mut a = left.0;
            let mut b = left.1;
            let mut c = right.0;
            let mut d = right.1;
            let cross_left = gcd(a, d);
            let cross_right = gcd(c, b);
            a = a.checked_div(cross_left).unwrap();
            d = d.checked_div(cross_left).unwrap();
            c = c.checked_div(cross_right).unwrap();
            b = b.checked_div(cross_right).unwrap();
            normalize(a.checked_mul(c).unwrap(), b.checked_mul(d).unwrap())
        };
        let compare = |left: (U256, U256), right: (U256, U256)| {
            U512::from(left.0)
                .checked_mul(U512::from(right.1))
                .unwrap()
                .cmp(&U512::from(right.0).checked_mul(U512::from(left.1)).unwrap())
        };

        let expected_cases = [
            (
                "0x3d5733d2ac4d8f572aa22531e84bf620cc98fee1",
                46_091_348,
                60,
                46_091_504,
                34,
                3_248_630,
            ),
            (
                "0xcc799821eba30921d0f387f846d36cb6a0cafee1",
                46_870_851,
                10,
                46_871_356,
                32,
                1_226_265,
            ),
            (
                "0xd34b6aa31674b581ac37766198ce3a9350a3fee1",
                46_140_507,
                10,
                46_140_551,
                94,
                3_476_092,
            ),
        ];
        for (
            case,
            (
                expected_token,
                expected_creation,
                expected_count,
                expected_block,
                expected_log,
                expected_cents,
            ),
        ) in fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .zip(expected_cases)
        {
            assert_eq!(case["token"].as_str(), Some(expected_token));
            assert_eq!(case["creation_block"].as_u64(), Some(expected_creation));
            assert_eq!(
                case["postlaunch_event_count"].as_u64(),
                Some(expected_count)
            );
            assert_eq!(case["token_decimals"].as_u64(), Some(18));
            assert_eq!(case["weth_decimals"].as_u64(), Some(18));
            assert_eq!(
                case["total_supply_raw"].as_str(),
                Some("1000000000000000000000000000")
            );
            assert_eq!(case["supply_tokens"].as_str(), Some("1000000000"));
            assert_eq!(case["initialize_currencies"][1], case["token"]);
            assert_eq!(
                case["initialize_currencies"][0],
                "0x0bd7d308f8e1639fab988df18a8011f41eacad73"
            );

            let mut prices = Vec::new();
            for event in case["events"].as_array().unwrap() {
                let block = event[0].as_u64().unwrap();
                let log_index = event[1].as_u64().unwrap();
                let timestamp = event[2].as_u64().unwrap();
                assert!(block > case["creation_block"].as_u64().unwrap());
                assert!(block <= fixture["anchor_block"].as_u64().unwrap());

                let quote_delta = event[3].as_str().unwrap();
                let token_delta = event[4].as_str().unwrap();
                assert_ne!(quote_delta.starts_with('-'), token_delta.starts_with('-'));
                let quote_raw = quote_delta
                    .strip_prefix('-')
                    .unwrap_or(quote_delta)
                    .parse::<U256>()
                    .unwrap();
                let token_raw = token_delta
                    .strip_prefix('-')
                    .unwrap_or(token_delta)
                    .parse::<U256>()
                    .unwrap();
                assert!(!quote_raw.is_zero() && !token_raw.is_zero());
                let ratio = normalize(quote_raw, token_raw);

                let minute_start = timestamp.div_euclid(60) * 60;
                let candle = case["coinbase_minutes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|minute| minute[0].as_u64() == Some(minute_start))
                    .unwrap();
                let low = decimal_fraction(candle[1].as_str().unwrap());
                let high = decimal_fraction(candle[2].as_str().unwrap());
                let close = decimal_fraction(candle[3].as_str().unwrap());
                assert!(compare(low, close) != Ordering::Greater);
                assert!(compare(close, high) != Ordering::Greater);
                let price = multiply(ratio, close);
                prices.push((block, log_index, price));
            }
            assert_eq!(
                u64::try_from(prices.len()).unwrap(),
                case["postlaunch_event_count"].as_u64().unwrap()
            );
            assert_eq!(u64::try_from(prices.len()).unwrap(), expected_count);
            let maximum = prices
                .iter()
                .max_by(|left, right| compare(left.2, right.2))
                .unwrap();
            let expected = &case["maximum"];
            let (expected_numerator, expected_denominator) = expected["price_fraction"]
                .as_str()
                .unwrap()
                .split_once('/')
                .unwrap();
            assert_eq!(
                maximum.2,
                normalize(
                    expected_numerator.parse::<U256>().unwrap(),
                    expected_denominator.parse::<U256>().unwrap()
                )
            );
            assert_eq!(maximum.0, expected["block"].as_u64().unwrap());
            assert_eq!(maximum.1, expected["log_index"].as_u64().unwrap());
            assert_eq!(maximum.0, expected_block);
            assert_eq!(maximum.1, expected_log);
            assert_eq!(expected["fdv_cents_floor"].as_u64(), Some(expected_cents));
            assert_eq!(
                maximum
                    .2
                    .0
                    .checked_mul(U256::from(100_000_000_000_u64))
                    .unwrap()
                    .checked_div(maximum.2.1)
                    .unwrap(),
                U256::from(expected_cents)
            );
        }

        let followup: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_weth_reference_followup_case.json"
        ))
        .unwrap();
        assert_eq!(
            followup["candidate_sha256"].as_str(),
            Some("3eb7ff79c93912eff0c03676589d359891e81d54e7d24f08ec04b519ce8b224e")
        );
        assert_eq!(
            followup["source"].as_str(),
            Some("onchain-pool-window-supply-cex-reference-v1")
        );
        assert_eq!(followup["anchor_block"].as_u64(), Some(87_808_917));
        assert!(
            followup["creation_transaction_swap_logs_excluded"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(followup["scan_counts"]["aa539cf7"].as_u64(), Some(5));
        assert_eq!(followup["scan_counts"]["c3a0e1a6"].as_u64(), Some(4));
        assert_eq!(
            followup["raw_swap_capture_sha256"].as_str(),
            Some("667c00d5b5ae020d93735556947e6cb2d0d003ee6d8a7f0ccdc68ff5386d3ffe")
        );
        assert_eq!(followup["cases"].as_array().unwrap().len(), 2);
        let expected = [
            (
                "0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1",
                44_528_731,
                3,
                45_582_061,
                107,
                1_037_304,
            ),
            (
                "0xc3a0e1a6c10b97c3e124ff8a2c291e5677a6fee1",
                49_434_763,
                2,
                49_435_080,
                41,
                1_021_169,
            ),
        ];
        for (case, (token, creation, count, winner_block, winner_log, cents)) in
            followup["cases"].as_array().unwrap().iter().zip(expected)
        {
            assert_eq!(case["token"].as_str(), Some(token));
            assert_eq!(case["creation_block"].as_u64(), Some(creation));
            assert_eq!(case["postcreation_event_count"].as_u64(), Some(count));
            assert_eq!(
                case["events"].as_array().unwrap().len(),
                usize::try_from(count).unwrap()
            );
            assert_eq!(case["token_decimals"].as_u64(), Some(18));
            assert_eq!(case["weth_decimals"].as_u64(), Some(18));
            assert_eq!(
                case["total_supply_raw"].as_str(),
                Some("1000000000000000000000000000")
            );
            assert_eq!(case["supply_tokens"].as_str(), Some("1000000000"));
            assert_eq!(case["creation_receipt_initial_mints"].as_u64(), Some(1));
            assert_eq!(case["winner_block_decimals"].as_u64(), Some(18));
            if token == "0xaa539cf78ff43e4b9fd9db2715435ff57d09fee1" {
                assert_eq!(case["supply_observation_block"].as_u64(), Some(45_582_406));
                assert_eq!(
                    case["decimals_observation_block"].as_u64(),
                    Some(45_582_406)
                );
                assert_eq!(
                    case["mint_logs_birth_through_last_included_block"].as_u64(),
                    Some(1)
                );
                assert_eq!(
                    case["burn_logs_birth_through_last_included_block"].as_u64(),
                    Some(0)
                );
                assert!(
                    case["selected_event_supply_proven_unchanged_through_observation_block"]
                        .as_bool()
                        .unwrap()
                );
                assert_eq!(case["winner_block_decimals"].as_u64(), Some(18));
                assert_eq!(
                    case["winner_block_decimals_evidence_sha256"].as_str(),
                    Some("942eb00e02101dd8673395e77f951530862ae177255e90bbbb71a3c42dd6fba6")
                );
                assert_eq!(
                    case["full_window_supply_manifest_sha256"].as_str(),
                    Some("6b89d834968823f75913f11524ada44a35ac3150a021b18c2b83d3b7a5793958")
                );
            } else {
                assert_eq!(
                    case["mint_logs_birth_through_selected_block"].as_u64(),
                    Some(1)
                );
                assert_eq!(
                    case["burn_logs_birth_through_selected_block"].as_u64(),
                    Some(0)
                );
                assert_eq!(
                    case["winner_block_decimals_observation_block"].as_u64(),
                    Some(49_435_080)
                );
                assert_eq!(
                    case["winner_block_decimals_evidence_sha256"].as_str(),
                    Some("5994a23ca29269e0dc0cf2f50e0e3589b1233b66da1ea14b058f93d9d10ef199")
                );
            }

            let mut prices = Vec::new();
            for event in case["events"].as_array().unwrap() {
                let block = event[0].as_u64().unwrap();
                let log_index = event[1].as_u64().unwrap();
                let timestamp = event[2].as_u64().unwrap();
                assert!(block > creation && block <= followup["anchor_block"].as_u64().unwrap());
                let quote_delta = event[3].as_str().unwrap();
                let token_delta = event[4].as_str().unwrap();
                assert_ne!(quote_delta.starts_with('-'), token_delta.starts_with('-'));
                let ratio = normalize(
                    quote_delta
                        .strip_prefix('-')
                        .unwrap_or(quote_delta)
                        .parse::<U256>()
                        .unwrap(),
                    token_delta
                        .strip_prefix('-')
                        .unwrap_or(token_delta)
                        .parse::<U256>()
                        .unwrap(),
                );
                let minute_start = timestamp.div_euclid(60) * 60;
                assert_eq!(
                    minute_start,
                    case["coinbase_minutes"][0][0].as_u64().unwrap()
                );
                let candle = case["coinbase_minutes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|minute| minute[0].as_u64() == Some(minute_start))
                    .unwrap();
                let low = decimal_fraction(candle[1].as_str().unwrap());
                let high = decimal_fraction(candle[2].as_str().unwrap());
                let close = decimal_fraction(candle[3].as_str().unwrap());
                assert!(compare(low, close) != Ordering::Greater);
                assert!(compare(close, high) != Ordering::Greater);
                prices.push((block, log_index, multiply(ratio, close)));
            }
            let maximum = prices
                .iter()
                .max_by(|left, right| compare(left.2, right.2))
                .unwrap();
            let maximum_fixture = &case["maximum"];
            assert_eq!(maximum.0, winner_block);
            assert_eq!(maximum.1, winner_log);
            assert_eq!(maximum.0, maximum_fixture["block"].as_u64().unwrap());
            assert_eq!(maximum.1, maximum_fixture["log_index"].as_u64().unwrap());
            let (numerator, denominator) = maximum_fixture["price_fraction"]
                .as_str()
                .unwrap()
                .split_once('/')
                .unwrap();
            assert_eq!(
                maximum.2,
                normalize(
                    numerator.parse::<U256>().unwrap(),
                    denominator.parse::<U256>().unwrap()
                )
            );
            assert_eq!(
                maximum
                    .2
                    .0
                    .checked_mul(U256::from(100_000_000_000_u64))
                    .unwrap()
                    .checked_div(maximum.2.1)
                    .unwrap(),
                U256::from(cents)
            );
            assert_eq!(maximum_fixture["fdv_cents_floor"].as_u64(), Some(cents));
            assert_eq!(maximum_fixture["receipt_status"].as_str(), Some("0x1"));
            assert!(
                maximum_fixture["receipt_log_index_match"]
                    .as_bool()
                    .unwrap()
            );
            assert!(
                maximum_fixture["header_hash_timestamp_match"]
                    .as_bool()
                    .unwrap()
            );
        }
        assert!(
            !followup["scope"]["all_market_lifetime_complete"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !followup["scope"]["alternate_migrations_or_pools_proven"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !followup["automatic_adapter_or_promotion"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            followup["validation"]["root_independent_verification_sha256"].as_str(),
            Some("7f55bcbc64b630602bd0448b6ee88a02a15485f18ac52e1843714f1afa9653e5")
        );
    }

    #[test]
    fn robinhood_amd_oracle_reference_uses_fresh_multiplier_adjusted_rounds() {
        use alloy_primitives::{U256, U512};
        use std::cmp::Ordering;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_amd_oracle_reference_case.json"
        ))
        .unwrap();
        assert_eq!(row["chain_id"].as_u64(), Some(4663));
        assert_eq!(
            row["source"].as_str(),
            Some("onchain-pool-window-supply-stock-oracle-reference-v1")
        );
        assert_eq!(
            row["candidate_sha256"].as_str(),
            Some("332df5d9d53f05722a2f899c9a6be3e97effbf9a060a4608aa16b281151c7f2b")
        );
        assert_eq!(
            row["brackets_sidecar_sha256"].as_str(),
            Some("0e98414594d9ece1bde4bc8b08d3e8b6d1e480e1af302a8f4c50baaf7cf0e9d2")
        );
        assert_eq!(row["target_decimals"].as_u64(), Some(18));
        assert_eq!(row["quote"]["decimals"].as_u64(), Some(18));
        assert_eq!(row["oracle"]["feed_decimals"].as_u64(), Some(8));
        assert!(
            row["oracle"]["answer_is_already_multiplier_adjusted"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["oracle"]["apply_ui_multiplier_again"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            row["oracle"]["historical_ui_multiplier_raw"].as_str(),
            Some("1000000000000000000")
        );
        assert_eq!(
            row["oracle"]["historical_ui_multiplier_blocks"],
            serde_json::json!([17438576, 17438776])
        );
        assert_eq!(row["events"].as_array().unwrap().len(), 3);

        let fraction = |value: &serde_json::Value| {
            (
                value[0].as_str().unwrap().parse::<U256>().unwrap(),
                value[1].as_str().unwrap().parse::<U256>().unwrap(),
            )
        };
        let fraction_string = |value: &str| {
            let (numerator, denominator) = value.split_once('/').unwrap();
            (
                numerator.parse::<U256>().unwrap(),
                denominator.parse::<U256>().unwrap(),
            )
        };
        let normalize = |numerator: U256, denominator: U256| (numerator, denominator);
        let multiply = |left: (U256, U256), right: (U256, U256)| {
            (
                left.0.checked_mul(right.0).unwrap(),
                left.1.checked_mul(right.1).unwrap(),
            )
        };
        let compare = |left: (U256, U256), right: (U256, U256)| {
            (U512::from(left.0) * U512::from(right.1))
                .cmp(&(U512::from(right.0) * U512::from(left.1)))
        };
        let magnitude = |raw: &str| {
            raw.strip_prefix('-')
                .unwrap_or(raw)
                .parse::<U256>()
                .unwrap()
        };
        let feed_scale = U256::from(100_000_000_u64);
        let low_usd = normalize(U256::from(54_487_500_000_u64), feed_scale);
        let selected_usd = normalize(U256::from(54_871_805_000_u64), feed_scale);
        assert!(compare(low_usd, selected_usd) == Ordering::Less);
        assert!(
            U512::from(selected_usd.0) * U512::from(low_usd.1) * U512::from(100)
                <= U512::from(low_usd.0) * U512::from(selected_usd.1) * U512::from(102)
        );
        let spread_percent = normalize(
            selected_usd
                .0
                .checked_sub(low_usd.0)
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap(),
            low_usd.0,
        );
        assert_eq!(
            compare(
                spread_percent,
                fraction_string(
                    row["feed_range"]["spread_percent_of_low_fraction"]
                        .as_str()
                        .unwrap(),
                )
            ),
            Ordering::Equal
        );

        let mut prices = Vec::new();
        for event in row["events"].as_array().unwrap() {
            let timestamp = event["timestamp_unix"].as_u64().unwrap();
            assert_eq!(event["receipt_status"].as_str(), Some("0x1"));
            assert!(event["receipt_log_match"].as_bool().unwrap());
            let before = &event["oracle_round_before"];
            let after = &event["oracle_round_after"];
            let before_id = before["round_id"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let after_id = after["round_id"].as_str().unwrap().parse::<u128>().unwrap();
            assert_eq!(before_id, 18_446_744_073_709_552_912);
            assert_eq!(after_id, 18_446_744_073_709_552_913);
            assert!(before_id > 0 && after_id > before_id);
            assert_eq!(before["answered_in_round"], before["round_id"]);
            assert_eq!(after["answered_in_round"], after["round_id"]);
            let before_time = before["updated_at"].as_u64().unwrap();
            let after_time = after["updated_at"].as_u64().unwrap();
            assert!(before_time <= timestamp && timestamp <= after_time);
            assert!(timestamp - before_time <= 300);
            assert!(after_time - timestamp <= 300);
            assert_eq!(
                before["age_seconds"].as_u64(),
                Some(timestamp - before_time)
            );
            assert_eq!(after["age_seconds"].as_u64(), Some(after_time - timestamp));
            assert_eq!(before["answer_raw"].as_str(), Some("54871805000"));
            assert_eq!(after["answer_raw"].as_str(), Some("54487500000"));

            let target_raw = magnitude(event["target_delta_raw"].as_str().unwrap());
            let amd_raw = magnitude(event["amd_delta_raw"].as_str().unwrap());
            assert_ne!(
                event["target_delta_raw"].as_str().unwrap().starts_with('-'),
                event["amd_delta_raw"].as_str().unwrap().starts_with('-')
            );
            let amd_per_target = normalize(amd_raw, target_raw);
            let price = multiply(amd_per_target, selected_usd);
            let low_price = multiply(amd_per_target, low_usd);
            assert!(compare(low_price, price) == Ordering::Less);
            assert!(
                U512::from(price.0) * U512::from(low_price.1) * U512::from(100)
                    <= U512::from(low_price.0) * U512::from(price.1) * U512::from(102)
            );
            assert_eq!(
                compare(price, fraction(&event["expected_price_fraction"])),
                Ordering::Equal
            );
            let fdv_cents = price
                .0
                .checked_mul(U256::from(100_000_000_000_u64))
                .unwrap()
                .checked_div(price.1)
                .unwrap();
            assert_eq!(
                fdv_cents,
                U256::from(event["expected_fdv_cents"].as_u64().unwrap())
            );
            prices.push((event, price));
        }

        let (winner, maximum) = prices
            .iter()
            .max_by(|left, right| compare(left.1, right.1))
            .unwrap();
        assert_eq!(winner["block"], row["expected_maximum"]["block"]);
        assert_eq!(winner["log_index"], row["expected_maximum"]["log_index"]);
        assert_eq!(
            compare(
                *maximum,
                fraction(&row["expected_maximum"]["price_fraction"])
            ),
            Ordering::Equal
        );
        assert_eq!(
            maximum
                .0
                .checked_mul(U256::from(100_000_000_000_u64))
                .unwrap()
                .checked_div(maximum.1)
                .unwrap(),
            U256::from(2_029_518)
        );
        assert_eq!(
            row["supply"]["raw"].as_str(),
            Some("1000000000000000000000000000")
        );
        assert_eq!(
            row["supply"]["observation_block"].as_u64(),
            Some(17_438_576)
        );
        assert_eq!(
            row["supply"]["decimals_observation_block"].as_u64(),
            Some(17_438_576)
        );
        assert_eq!(
            row["supply"]["birth_mint_count_through_last_event"].as_u64(),
            Some(1)
        );
        assert_eq!(
            row["supply"]["burn_count_through_last_event"].as_u64(),
            Some(0)
        );
        assert_eq!(row["supply"]["last_event_block"].as_u64(), Some(17_438_776));
        assert!(
            row["feed_range"]["all_target_event_round_updates_within_300s"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            row["stale_negative_control"]["target_round_age_seconds"].as_u64(),
            Some(7_054)
        );
        assert!(
            !row["stale_negative_control"]["within_300_seconds"]
                .as_bool()
                .unwrap()
        );
        assert!(row["limitations"]["ath_at"].is_null());
        assert!(
            !row["limitations"]["all_market_or_lifetime_ath"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["direct_amd_usdg_execution_price"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["automatic_adapter_or_promotion"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn robinhood_60d63_and_tsla_quote_trade_windows_are_case_scoped() {
        use alloy_primitives::{U256, U512};
        use std::cmp::Ordering;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/robinhood_60d63_tsla_quote_trade_followup_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["candidate_sha256"].as_str(),
            Some("f321bf77d62c8a7969c9169c407567b36e85b65523069819eb427d1059eacde0")
        );
        let fraction = |raw: &str| {
            let (n, d) = raw.split_once('/').unwrap();
            (n.parse::<U256>().unwrap(), d.parse::<U256>().unwrap())
        };
        let multiply = |a: (U256, U256), b: (U256, U256)| {
            (a.0.checked_mul(b.0).unwrap(), a.1.checked_mul(b.1).unwrap())
        };
        let compare = |a: (U256, U256), b: (U256, U256)| {
            (U512::from(a.0) * U512::from(b.1)).cmp(&(U512::from(b.0) * U512::from(a.1)))
        };
        let magnitude = |raw: &str| {
            raw.strip_prefix('-')
                .unwrap_or(raw)
                .parse::<U256>()
                .unwrap()
        };
        let quote_usd = |quote_raw: U256, usdg_raw: U256| {
            (
                usdg_raw
                    .checked_mul(U256::from(1_000_000_000_000_u64))
                    .unwrap(),
                quote_raw,
            )
        };
        let neighbor_spread = |a: (U256, U256), b: (U256, U256)| {
            let (low, high) = if compare(a, b) == Ordering::Greater {
                (b, a)
            } else {
                (a, b)
            };
            let delta =
                U512::from(high.0) * U512::from(low.1) - U512::from(low.0) * U512::from(high.1);
            (
                delta * U512::from(100),
                U512::from(high.1) * U512::from(low.0),
            )
        };
        let within_two_percent = |a: (U256, U256), b: (U256, U256)| {
            let cross_a = U512::from(a.0) * U512::from(b.1);
            let cross_b = U512::from(b.0) * U512::from(a.1);
            let delta = if cross_a >= cross_b {
                cross_a - cross_b
            } else {
                cross_b - cross_a
            };
            delta * U512::from(50) <= U512::from(b.0) * U512::from(a.1)
        };
        let within_age = |age: u64, limit: u64| age <= limit;

        let expected = [
            (
                "0x60d63f32f0520178eabf4045cfb6e78941901e18",
                "onchain-pool-window-supply-quote-trade-fx-reference-v1",
                37_usize,
                300_u64,
                false,
                65505526_u64,
                107_u64,
                3_000_729_u64,
            ),
            (
                "0x9aadb292cd6e17cdf98ad16d1a1caf1704041e18",
                "onchain-pool-window-supply-stock-trade-fx-reference-600s-v1",
                6_usize,
                600_u64,
                true,
                21054458_u64,
                43_u64,
                2_027_889_u64,
            ),
        ];
        let mut total_events = 0;
        for (case, (token, source, count, age_limit, exception, winner_block, winner_log, cents)) in
            row["cases"].as_array().unwrap().iter().zip(expected)
        {
            assert_eq!(case["token"].as_str(), Some(token));
            assert_eq!(case["source"].as_str(), Some(source));
            assert_eq!(case["selected_age_limit_seconds"].as_u64(), Some(age_limit));
            assert_eq!(
                case["selected_600s_case_exception"].as_bool(),
                Some(exception)
            );
            assert_eq!(case["events"].as_array().unwrap().len(), count);
            assert_eq!(
                case["fully_priced_count"].as_u64(),
                Some(u64::try_from(count).unwrap())
            );
            assert_eq!(case["supply"]["decimals"].as_u64(), Some(18));
            assert_eq!(case["quote_decimals"].as_u64(), Some(18));
            assert_eq!(case["usdg_decimals"].as_u64(), Some(6));
            assert_eq!(
                case["supply"]["total_supply_raw_at_end"].as_str(),
                Some("1000000000000000000000000000")
            );
            assert_eq!(
                case["supply"]["birth_mint_count_through_end"].as_u64(),
                Some(1)
            );
            assert_eq!(case["supply"]["burn_count_through_end"].as_u64(), Some(0));
            assert_eq!(
                case["supply"]["observed_at_block"],
                case["window"]["last_event_block"]
            );
            assert!(case["scope"]["ath_at"].is_null());
            assert!(!case["scope"]["all_markets_or_lifetime"].as_bool().unwrap());

            let daily_close = fraction(case["daily_usdg_usd_close_fraction"].as_str().unwrap());
            let mut prices = Vec::new();
            let mut pass_300 = true;
            for event in case["events"].as_array().unwrap() {
                let timestamp = event["timestamp"].as_u64().unwrap();
                let target_key = (
                    event["block"].as_u64().unwrap(),
                    event["tx_index"].as_u64().unwrap(),
                    event["log_index"].as_u64().unwrap(),
                );
                let before = &event["quote_trade_before"];
                let after = &event["quote_trade_after"];
                let key = |trade: &serde_json::Value| {
                    (
                        trade["block"].as_u64().unwrap(),
                        trade["tx_index"].as_u64().unwrap(),
                        trade["log_index"].as_u64().unwrap(),
                    )
                };
                assert!(key(before) < target_key && target_key < key(after));
                assert!(before["timestamp"].as_u64().unwrap() <= timestamp);
                assert!(timestamp <= after["timestamp"].as_u64().unwrap());
                let age_before = timestamp - before["timestamp"].as_u64().unwrap();
                let age_after = after["timestamp"].as_u64().unwrap() - timestamp;
                assert_eq!(before["seconds_before"].as_u64(), Some(age_before));
                assert_eq!(after["seconds_after"].as_u64(), Some(age_after));
                assert!(within_age(age_before, age_limit) && within_age(age_after, age_limit));
                pass_300 &= within_age(age_before, 300) && within_age(age_after, 300);

                let target_quote = (
                    magnitude(event["target_quote_delta_raw"].as_str().unwrap()),
                    magnitude(event["target_token_delta_raw"].as_str().unwrap()),
                );
                let usd_before = quote_usd(
                    magnitude(before["quote_delta_raw"].as_str().unwrap()),
                    magnitude(before["usdg_delta_raw"].as_str().unwrap()),
                );
                let usd_after = quote_usd(
                    magnitude(after["quote_delta_raw"].as_str().unwrap()),
                    magnitude(after["usdg_delta_raw"].as_str().unwrap()),
                );
                let spread = neighbor_spread(usd_before, usd_after);
                assert!(spread.0 <= spread.1 * U512::from(2));
                if exception {
                    let expected_spread = fraction(
                        case["expected_quote_neighbor_spread_percent_fraction"]
                            .as_str()
                            .unwrap(),
                    );
                    assert_eq!(
                        spread.0 * U512::from(expected_spread.1),
                        U512::from(expected_spread.0) * spread.1
                    );
                }
                let price = multiply(multiply(target_quote, usd_before), daily_close);
                assert_eq!(
                    compare(
                        price,
                        fraction(event["candidate_usd_per_token_fraction"].as_str().unwrap())
                    ),
                    Ordering::Equal
                );
                let low_price = multiply(
                    multiply(target_quote, usd_before),
                    fraction(case["daily_usdg_usd_low_fraction"].as_str().unwrap()),
                );
                let high_price = multiply(
                    multiply(target_quote, usd_before),
                    fraction(case["daily_usdg_usd_high_fraction"].as_str().unwrap()),
                );
                assert!(within_two_percent(low_price, price));
                assert!(within_two_percent(high_price, price));
                prices.push((event, price));
            }
            assert_eq!(case["all_events_pass_300s"].as_bool(), Some(pass_300));
            assert_eq!(pass_300, !exception);
            assert!(
                case["all_events_pass_selected_age_and_two_percent_spread"]
                    .as_bool()
                    .unwrap()
            );
            let (winner, maximum) = prices.iter().max_by(|a, b| compare(a.1, b.1)).unwrap();
            assert_eq!(winner["block"].as_u64(), Some(winner_block));
            assert_eq!(winner["log_index"].as_u64(), Some(winner_log));
            assert_eq!(
                compare(
                    *maximum,
                    fraction(case["max_event"]["price_fraction"].as_str().unwrap())
                ),
                Ordering::Equal
            );
            let fdv_cents = maximum
                .0
                .checked_mul(U256::from(100_000_000_000_u64))
                .unwrap()
                .checked_div(maximum.1)
                .unwrap();
            assert_eq!(fdv_cents, U256::from(cents));
            assert_eq!(case["max_event"]["fdv_cents_floor"].as_u64(), Some(cents));
            total_events += count;
        }
        assert_eq!(total_events, 43);
        assert!(within_age(600, 600));
        assert!(!within_age(601, 600));
        assert_eq!(
            row["stale_oracle_negative_control"]["tsla_chainlink_round_age_seconds"].as_u64(),
            Some(7_054)
        );
        assert!(
            !row["stale_oracle_negative_control"]["within_300s"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn base_e465_indirect_spot_reference_matches_composed_routes_and_supply() {
        use alloy_primitives::{U512, aliases::U1024};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/base_e465_indirect_spot_reference_case.json"
        ))
        .unwrap();
        assert_eq!(fixture["chain_id"].as_u64(), Some(8453));
        assert_eq!(
            fixture["source"].as_str(),
            Some("onchain-pool-window-supply-indirect-spot-cex-reference-v1")
        );
        assert!(fixture["ath_at"].is_null());
        assert_eq!(
            fixture["source_hashes"]["candidate_sha256"].as_str(),
            Some("888e884399a2d0c09e390f3c9710245595c729706666ac7ac6472c4fc4cee03a")
        );
        assert_eq!(
            fixture["source_hashes"]["target_receipt_sha256"].as_str(),
            Some("6233f14b75e01bfb109fe944e9691b5b9df4730e9f1977db28558de3a765b74f")
        );

        let gcd = |mut left: U512, mut right: U512| {
            while !right.is_zero() {
                let remainder = left.checked_rem(right).unwrap();
                left = right;
                right = remainder;
            }
            left
        };
        let normalize = |numerator: U512, denominator: U512| {
            let divisor = gcd(numerator, denominator);
            (
                numerator.checked_div(divisor).unwrap(),
                denominator.checked_div(divisor).unwrap(),
            )
        };
        let fraction = |value: &serde_json::Value| {
            let numerator = value[0].as_str().unwrap().parse::<U512>().unwrap();
            let denominator = value[1].as_str().unwrap().parse::<U512>().unwrap();
            normalize(numerator, denominator)
        };
        let decimal_fraction = |raw: &str| {
            let decimal = ExactDecimal::parse(raw).unwrap();
            let denominator = (0..u32::try_from(decimal.scale).unwrap())
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap();
            normalize(U512::from(decimal.coefficient), denominator)
        };
        let multiply = |left: (U512, U512), right: (U512, U512)| {
            let left_cross = gcd(left.0, right.1);
            let right_cross = gcd(right.0, left.1);
            normalize(
                left.0
                    .checked_div(left_cross)
                    .unwrap()
                    .checked_mul(right.0.checked_div(right_cross).unwrap())
                    .unwrap(),
                left.1
                    .checked_div(right_cross)
                    .unwrap()
                    .checked_mul(right.1.checked_div(left_cross).unwrap())
                    .unwrap(),
            )
        };
        let sqrt_ratio = |sqrt_price: &str| {
            let sqrt = sqrt_price.parse::<U512>().unwrap();
            let denominator = (0..192)
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(2)))
                .unwrap();
            normalize(sqrt.checked_mul(sqrt).unwrap(), denominator)
        };
        let floor_fdv_cents = |price: (U512, U512), supply_tokens: U512| {
            price
                .0
                .checked_mul(supply_tokens)
                .unwrap()
                .checked_mul(U512::from(100))
                .unwrap()
                .checked_div(price.1)
                .unwrap()
        };

        let scope = &fixture["scope"];
        let target = &fixture["target_event"];
        let quote = &fixture["same_transaction_quote_swap"];
        let supply = &fixture["supply"];
        assert_eq!(
            fixture["token"].as_str(),
            Some("0xe46504da7f7f9d9233d6118e27bdfbf27964a215")
        );
        assert_eq!(
            scope["creation_block"].as_u64(),
            Some(supply["creation_block"].as_u64().unwrap())
        );
        assert_eq!(scope["query_from_block"], scope["creation_block"]);
        assert_eq!(scope["query_to_block"].as_u64(), Some(52_470_375));
        assert!(scope["creation_in_scan"].as_bool().unwrap());
        assert!(scope["complete_through_cutoff"].as_bool().unwrap());
        assert_eq!(scope["post_creation_swap_count"].as_u64(), Some(1));
        assert_eq!(target["block"].as_u64(), Some(51_792_272));
        assert!(target["block"].as_u64().unwrap() > scope["creation_block"].as_u64().unwrap());
        assert!(target["block"].as_u64().unwrap() <= scope["query_to_block"].as_u64().unwrap());
        assert_eq!(target["log_index"].as_u64(), Some(366));
        assert_eq!(quote["log_index"].as_u64(), Some(353));
        assert_eq!(quote["transaction_hash"], target["transaction_hash"]);
        assert_eq!(quote["block"], target["block"]);
        assert!(quote["log_index"].as_u64().unwrap() < target["log_index"].as_u64().unwrap());
        assert_eq!(target["currency0"], quote["currency0"]);
        assert_eq!(target["currency1"], fixture["token"]);
        assert_eq!(
            quote["currency1"].as_str(),
            Some("0x1111111111166b7fe7bd91427724b487980afc69")
        );
        assert_eq!(target["timestamp_unix"].as_u64(), Some(1_790_373_891));
        assert_eq!(
            target["timestamp_unix"].as_u64().unwrap().div_euclid(60) * 60,
            1_790_373_840
        );

        assert_eq!(supply["raw"].as_str(), Some("1000000000000000000000000000"));
        assert_eq!(supply["tokens"].as_str(), Some("1000000000"));
        assert_eq!(supply["decimals"].as_u64(), Some(18));
        assert_eq!(supply["mint_count_creation_to_event"].as_u64(), Some(1));
        assert_eq!(supply["burn_count_creation_to_event"].as_u64(), Some(0));
        assert!(supply["constant_through_event"].as_bool().unwrap());
        assert!(
            !target["target_pool_liquidity_post_raw"]
                .as_str()
                .unwrap()
                .parse::<U512>()
                .unwrap()
                .is_zero()
        );
        assert_eq!(target["receipt_status"].as_str(), Some("0x1"));
        assert_eq!(target["receipt_logs_count"].as_u64(), Some(37));
        assert!(target["receipt_swap_log_matches_scan"].as_bool().unwrap());

        let two_to_192 = (0..192)
            .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(2)))
            .unwrap();
        let target_sqrt = target["target_pool_sqrt_price_x96_post"]
            .as_str()
            .unwrap()
            .parse::<U512>()
            .unwrap();
        let target_quote_per_token =
            normalize(two_to_192, target_sqrt.checked_mul(target_sqrt).unwrap());
        assert_eq!(
            target_quote_per_token,
            fraction(&fixture["derived"]["target_quote_per_token_fraction"])
        );
        let zora_per_quote = sqrt_ratio(quote["sqrt_price_x96"].as_str().unwrap());
        assert_eq!(
            zora_per_quote,
            fraction(&fixture["derived"]["zora_per_quote_fraction"])
        );

        let fx = &fixture["fx"];
        let supply_tokens = supply["tokens"].as_str().unwrap().parse::<U512>().unwrap();
        let mut fee_close_prices = Vec::new();
        for venue in fixture["parent_venues"].as_array().unwrap() {
            assert_eq!(venue["token0"], quote["currency1"]);
            assert_eq!(
                venue["token1"].as_str(),
                Some("0x4200000000000000000000000000000000000006")
            );
            let liquidity = venue["liquidity_raw"]
                .as_str()
                .unwrap()
                .parse::<U512>()
                .unwrap();
            assert!(!liquidity.is_zero());
            let weth_per_zora = sqrt_ratio(venue["sqrt_price_x96"].as_str().unwrap());
            assert_eq!(weth_per_zora, fraction(&venue["weth_per_zora_fraction"]));
            let mut close_price = None;
            for key in ["low", "high", "open", "close"] {
                let price = multiply(
                    multiply(
                        multiply(target_quote_per_token, zora_per_quote),
                        weth_per_zora,
                    ),
                    decimal_fraction(fx[key].as_str().unwrap()),
                );
                assert_eq!(
                    price,
                    fraction(&venue["price_usd_per_target_token_fraction"][key])
                );
                let cents = floor_fdv_cents(price, supply_tokens);
                assert_eq!(
                    cents.to_string(),
                    venue["fdv_cents_floor"][key].as_u64().unwrap().to_string()
                );
                if key == "close" {
                    close_price = Some(price);
                }
            }
            fee_close_prices.push((venue["fee"].as_u64().unwrap(), close_price.unwrap()));
        }
        assert_eq!(fee_close_prices[0].0, 3000);
        assert_eq!(fee_close_prices[1].0, 10000);
        assert_eq!(
            fee_close_prices[0].1,
            fraction(&fixture["derived"]["source_price_close_fraction"])
        );
        assert_eq!(
            fixture["derived"]["source_fdv_cents_floor"].as_u64(),
            Some(11_260)
        );
        assert_eq!(
            fixture["derived"]["source_fdv_low_cents_floor"].as_u64(),
            Some(11_257)
        );
        assert_eq!(
            fixture["derived"]["source_fdv_high_cents_floor"].as_u64(),
            Some(11_264)
        );

        let primary_ratio = fraction(&fixture["parent_venues"][0]["weth_per_zora_fraction"]);
        let secondary_ratio = fraction(&fixture["parent_venues"][1]["weth_per_zora_fraction"]);
        let primary_cross = U1024::from(primary_ratio.0)
            .checked_mul(U1024::from(secondary_ratio.1))
            .unwrap();
        let secondary_cross = U1024::from(secondary_ratio.0)
            .checked_mul(U1024::from(primary_ratio.1))
            .unwrap();
        let diff = if primary_cross > secondary_cross {
            primary_cross - secondary_cross
        } else {
            secondary_cross - primary_cross
        };
        assert!(
            diff.checked_mul(U1024::from(100)).unwrap()
                <= primary_cross.checked_mul(U1024::from(2)).unwrap()
        );
        assert_eq!(
            fixture["parent_venues"][1]["fdv_cents_floor"]["close"].as_u64(),
            Some(11_296)
        );
        assert!(
            fixture["limits"]["configured_target_pool_only"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            fixture["limits"]["geckoterminal_daily_candle_count"].as_u64(),
            Some(0)
        );
        assert!(
            !fixture["limits"]["all_market_or_lifetime_ath_claim"]
                .as_bool()
                .unwrap()
        );
        assert!(!fixture["limits"]["database_promotion"].as_bool().unwrap());
        assert!(
            !fixture["limits"]["wallet_cost_or_pnl_claim"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn solana_tracker_brkf_legacy_pool_spot_has_scoped_supply_bound() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_brkf_legacy_pool_spot_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["source"].as_str(),
            Some("solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1")
        );
        assert!(row["ath_at"].is_null());
        assert_eq!(
            row["mint"].as_str(),
            Some("BRkf1C38Y8ZKt1sApadJDP1EAGEFrbtqZbQ1kwSupump")
        );
        assert_eq!(
            row["pool"].as_str(),
            Some("BVTGRCFsThHWrjsjCHWwy1AaCApwjqrNJHPkKmbMBLuF")
        );

        let provider =
            ExactDecimal::parse(row["provider_observation"]["price_usd"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            row["provider_observation"]["kind"].as_str(),
            Some("provider_reported_maximum_spot_price")
        );
        let selected = &row["selected_pool_state"];
        assert_eq!(selected["event"].as_str(), Some("successful PumpSwap Buy"));
        assert_eq!(selected["state"].as_str(), Some("pre-event vault balances"));
        assert_eq!(selected["pool_owner"], row["pool"]);
        assert_eq!(selected["base_mint"], row["mint"]);
        assert_eq!(selected["base_decimals"].as_u64(), Some(6));
        assert_eq!(selected["quote_decimals"].as_u64(), Some(9));
        assert_eq!(
            selected["quote_mint"].as_str(),
            Some("So11111111111111111111111111111111111111112")
        );
        assert_eq!(
            selected["marginal_spot_formula"].as_str(),
            Some("quote_raw * SOL_USD / (base_raw * 1000)")
        );
        assert_eq!(selected["timestamp_unix"].as_u64(), Some(1_760_342_676));
        assert_eq!(
            selected["milliseconds_before_provider_timestamp"].as_u64(),
            Some(1_288)
        );
        assert_eq!(
            selected["timestamp_unix"].as_u64().unwrap().div_euclid(60) * 60,
            row["fx"]["minute_open_unix"].as_u64().unwrap()
        );
        assert_eq!(
            row["source_hashes"]["candidate_sha256"].as_str(),
            Some("b394638cf3d4bcce90551ef13b61f9b8a10107f7af3c3a1ac2ba30e9d391004e")
        );
        assert_eq!(
            row["source_hashes"]["selected_transactions_sha256"].as_str(),
            Some("ce9b72bd1392dd8103942700fb4066c3d80857e933e30652c0530138b3a8d792")
        );

        let base_raw = selected["base_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let quote_raw = selected["quote_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let provider_num = U256::from(provider.coefficient);
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let gcd = |mut left: U256, mut right: U256| {
            while !right.is_zero() {
                let remainder = left.checked_rem(right).unwrap();
                left = right;
                right = remainder;
            }
            left
        };
        let normalize = |numerator: U256, denominator: U256| {
            let divisor = gcd(numerator, denominator);
            (
                numerator.checked_div(divisor).unwrap(),
                denominator.checked_div(divisor).unwrap(),
            )
        };
        let provider_den = pow10(u32::try_from(provider.scale).unwrap());
        let supply = &row["supply_at_provider_timestamp"];
        let supply_raw = supply["raw"].as_str().unwrap().parse::<U256>().unwrap();
        let supply_tokens = supply["tokens"].as_str().unwrap().parse::<U256>().unwrap();
        assert_eq!(supply["raw"].as_str(), Some("1000000000000000"));
        assert_eq!(supply["tokens"].as_str(), Some("1000000000"));
        assert_eq!(supply["decimals"].as_u64(), Some(6));

        let provider_fdv_cents = provider_num
            .checked_mul(U256::from(100_000_000_000_u64))
            .unwrap()
            .checked_div(provider_den)
            .unwrap();
        assert_eq!(provider_fdv_cents, U256::from(12_703_138));
        assert_eq!(
            provider_fdv_cents,
            U256::from(
                row["max_fdv"]["provider_price_times_peak_supply_fdv_cents_floor"]
                    .as_u64()
                    .unwrap()
            )
        );
        assert_eq!(supply_raw, supply_tokens.checked_mul(pow10(6)).unwrap());
        assert_eq!(
            supply["mint_instructions_birth_to_snapshot"].as_u64(),
            Some(1)
        );
        assert_eq!(supply["mint_sum_raw"], supply["raw"]);
        assert_eq!(
            supply["burns_before_or_through_peak_after_birth"].as_u64(),
            Some(0)
        );
        let burn_sum = supply["burn_sum_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let snapshot_supply = supply["finalized_snapshot_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        assert_eq!(supply_raw.checked_sub(burn_sum).unwrap(), snapshot_supply);
        assert!(
            supply["mint_authority_absent_at_snapshot"]
                .as_bool()
                .unwrap()
        );

        let fx = &row["fx"];
        let mut spot_cents = Vec::new();
        for key in ["low", "close", "high"] {
            let fx_price = ExactDecimal::parse(fx[key].as_str().unwrap()).unwrap();
            let spot_num = quote_raw
                .checked_mul(U256::from(fx_price.coefficient))
                .unwrap();
            let spot_den = base_raw
                .checked_mul(U256::from(1_000_u64))
                .unwrap()
                .checked_mul(pow10(u32::try_from(fx_price.scale).unwrap()))
                .unwrap();
            let provider_cross = provider_num.checked_mul(spot_den).unwrap();
            let spot_cross = spot_num.checked_mul(provider_den).unwrap();
            let diff = if provider_cross > spot_cross {
                provider_cross - spot_cross
            } else {
                spot_cross - provider_cross
            };
            assert!(
                diff.checked_mul(U256::from(100)).unwrap()
                    <= provider_cross.checked_mul(U256::from(2)).unwrap()
            );

            let fdv_cents = spot_num
                .checked_mul(supply_tokens)
                .unwrap()
                .checked_mul(U256::from(100))
                .unwrap()
                .checked_div(spot_den)
                .unwrap();
            assert_eq!(
                fdv_cents.to_string(),
                row["derived"][format!("pool_spot_fdv_cents_{key}_floor")]
                    .as_u64()
                    .unwrap()
                    .to_string()
            );
            if key == "close" {
                assert_eq!(
                    normalize(spot_num, spot_den),
                    normalize(
                        quote_raw.checked_mul(U256::from(19_650_u64)).unwrap(),
                        base_raw.checked_mul(U256::from(100_000_u64)).unwrap(),
                    )
                );
            }
            spot_cents.push(fdv_cents);
        }
        assert!(spot_cents[0] < spot_cents[1] && spot_cents[1] < spot_cents[2]);

        let following = &row["following_pool_state"];
        assert_eq!(following["timestamp_unix"].as_u64(), Some(1_760_342_678));
        assert_eq!(
            following["milliseconds_after_provider_timestamp"].as_u64(),
            Some(712)
        );
        assert_eq!(following["pool_owner"], row["pool"]);
        assert_eq!(
            following["timestamp_unix"].as_u64().unwrap().div_euclid(60) * 60,
            fx["minute_open_unix"].as_u64().unwrap()
        );
        let following_base_raw = following["base_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        let following_quote_raw = following["quote_raw"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        for key in ["low", "high"] {
            let fx_price = ExactDecimal::parse(fx[key].as_str().unwrap()).unwrap();
            let spot_num = following_quote_raw
                .checked_mul(U256::from(fx_price.coefficient))
                .unwrap();
            let spot_den = following_base_raw
                .checked_mul(U256::from(1_000_u64))
                .unwrap()
                .checked_mul(pow10(u32::try_from(fx_price.scale).unwrap()))
                .unwrap();
            let provider_cross = provider_num.checked_mul(spot_den).unwrap();
            let spot_cross = spot_num.checked_mul(provider_den).unwrap();
            let diff = if provider_cross > spot_cross {
                provider_cross - spot_cross
            } else {
                spot_cross - provider_cross
            };
            assert!(
                diff.checked_mul(U256::from(100)).unwrap()
                    <= provider_cross.checked_mul(U256::from(2)).unwrap()
            );
        }
        assert_eq!(
            row["time_bracketing_policy"].as_str(),
            Some(
                "nearest preceding successful pool swap state and earliest following successful pool swap state around the provider millisecond timestamp"
            )
        );
        assert!(
            row["max_fdv"]["claimable_as_provider_window_maximum"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["max_fdv"]["all_market_or_lifetime_max_claim"]
                .as_bool()
                .unwrap()
        );

        assert!(
            row["supply_at_provider_timestamp"]["signature_set_matches_full_transaction_set"]
                .as_bool()
                .unwrap()
        );
        assert!(
            row["supply_at_provider_timestamp"]["supply_history_complete_through_snapshot"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            row["supply_at_provider_timestamp"]["signature_count_birth_to_snapshot"].as_u64(),
            Some(764)
        );
        assert_eq!(
            row["supply_at_provider_timestamp"]["full_transaction_count_birth_to_snapshot"]
                .as_u64(),
            Some(764)
        );
        assert_eq!(
            row["supply_at_provider_timestamp"]["mints_birth_to_peak"].as_u64(),
            Some(0)
        );
        assert_eq!(
            row["supply_at_provider_timestamp"]["burns_before_or_through_peak_after_birth"]
                .as_u64(),
            Some(0)
        );
        assert!(
            row["max_fdv"]["supply_nonincreasing_from_birth_through_finalized_snapshot"]
                .as_bool()
                .unwrap()
        );
        assert!(
            row["max_fdv"]["provider_max_price_attained_at_peak_supply"]
                .as_bool()
                .unwrap()
        );
        assert!(
            row["protocol_evidence"]["reserve_fields_match_pool_owned_pre_token_balances"]
                .as_bool()
                .unwrap()
        );
        assert!(row["protocol_evidence"]["legacy_real_reserve_curve_and_fees_reconciled_on_sample_buy_and_sell"].as_bool().unwrap());
        let idls = row["protocol_evidence"]["official_idl_snapshots"]
            .as_array()
            .unwrap();
        assert_eq!(idls.len(), 3);
        assert_eq!(idls[0]["commit"].as_str(), Some("7645c16c"));
        assert_eq!(idls[1]["commit"].as_str(), Some("33590255"));
        assert_eq!(idls[2]["commit"].as_str(), Some("ff9cbf82"));
        assert_eq!(
            idls[0]["sha256"].as_str(),
            Some("f356cbdadf2eb36b4464b8a3112ac2517f6719898d30c7a9622ef0df12a67f75")
        );
        assert_eq!(
            idls[1]["sha256"].as_str(),
            Some("69ace43fd5a2f1ca78c6f330831146fd7d6abade0d70a3264073b1f63c320f46")
        );
        assert_eq!(
            idls[2]["sha256"].as_str(),
            Some("09f2dfa15de59804383d8610fcb6a052feddabc70717705a4d5d7cd07ac304ac")
        );
        assert!(
            !row["protocol_evidence"]["deployed_binary_proof"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["protocol_evidence"]["general_missing_virtual_reserve_equals_zero_rule"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn solana_tracker_avx_legacy_pool_spot_has_scoped_supply_bound() {
        use alloy_primitives::U256;

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_avx_legacy_pool_spot_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["source"].as_str(),
            Some("solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1")
        );
        assert!(row["ath_at"].is_null());
        assert_eq!(
            row["mint"].as_str(),
            Some("Avx71ncfuZBMJxxmVqHLsy34RYzVoHTrwD26HjQVpump")
        );
        assert_eq!(
            row["pool"].as_str(),
            Some("CMG29JVFKnAoBVjPeBPDvoPPGMzwEqrNLVNYXqdw6Ltm")
        );
        assert_eq!(
            row["source_hashes"]["candidate_sha256"].as_str(),
            Some("837a8826283825c011dc62cc97244e25c5428c691c35e31a0cb8485dd2635578")
        );

        let provider =
            ExactDecimal::parse(row["provider_observation"]["price_usd"].as_str().unwrap())
                .unwrap();
        let provider_num = U256::from(provider.coefficient);
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let provider_den = pow10(u32::try_from(provider.scale).unwrap());
        let fx = &row["fx"];
        assert!(fx["provider_observation_share_minute"].as_bool().unwrap());
        assert_eq!(
            row["provider_observation"]["timestamp_ms"]
                .as_u64()
                .unwrap()
                .div_euclid(1_000)
                .div_euclid(60),
            fx["minute_open_unix"].as_u64().unwrap().div_euclid(60)
        );

        let check_spot = |state: &serde_json::Value, fx_key: &str| {
            let base = state["base_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let quote = state["quote_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap();
            if !state["pool_owner"].is_null() {
                assert_eq!(state["pool_owner"], row["pool"]);
                assert_eq!(state["base_decimals"].as_u64(), Some(6));
                assert_eq!(state["quote_decimals"].as_u64(), Some(9));
            }
            let usd = ExactDecimal::parse(fx[fx_key].as_str().unwrap()).unwrap();
            let spot_num = quote.checked_mul(U256::from(usd.coefficient)).unwrap();
            let spot_den = base
                .checked_mul(U256::from(1_000_u64))
                .unwrap()
                .checked_mul(pow10(u32::try_from(usd.scale).unwrap()))
                .unwrap();
            let provider_cross = provider_num.checked_mul(spot_den).unwrap();
            let spot_cross = spot_num.checked_mul(provider_den).unwrap();
            (spot_num, spot_den, provider_cross, spot_cross)
        };

        let anchors = row["selected_pool_states"].as_array().unwrap();
        assert_eq!(anchors.len(), 2);
        for anchor in anchors {
            let state_minute = anchor["timestamp_unix"].as_u64().unwrap().div_euclid(60) * 60;
            assert_eq!(state_minute, fx["minute_open_unix"].as_u64().unwrap());
            for key in ["low", "high"] {
                let (_, _, provider_cross, spot_cross) = check_spot(anchor, key);
                let difference = provider_cross.abs_diff(spot_cross);
                assert!(
                    difference.checked_mul(U256::from(100)).unwrap()
                        <= provider_cross.checked_mul(U256::from(2)).unwrap()
                );
            }
        }

        let same_second = row["same_second_pre_states"].as_array().unwrap();
        assert_eq!(same_second.len(), 6);
        for state in same_second {
            assert_eq!(state["timestamp_unix"].as_u64(), Some(1_765_803_131));
            let (_, _, provider_cross, spot_cross) = check_spot(state, "close");
            assert!(spot_cross < provider_cross);
        }

        let supply = &row["supply_bound"];
        assert_eq!(supply["decimals"].as_u64(), Some(6));
        assert_eq!(supply["birth_raw"].as_str(), Some("1000000000000000"));
        assert_eq!(supply["peak_raw"].as_str(), Some("999999999999999"));
        assert!(supply["cent_bound_collapses"].as_bool().unwrap());
        assert!(supply["provider_scope_max_fdv_proven"].as_bool().unwrap());
        assert_eq!(supply["signature_index_success_rows"].as_u64(), Some(1_736));
        assert_eq!(supply["ascending_pages"].as_u64(), Some(10));
        assert_eq!(supply["descending_pages"].as_u64(), Some(8));
        assert_eq!(supply["overlap_rows"].as_u64(), Some(64));
        assert!(
            supply["ascending_descending_union_matches_signature_index_set"]
                .as_bool()
                .unwrap()
        );
        assert!(
            supply["later_mint_to_or_mint_checked_absent"]
                .as_bool()
                .unwrap()
        );
        assert!(
            supply["mint_close_authority_extension_absent"]
                .as_bool()
                .unwrap()
        );
        assert!(
            supply["mint_close_authority_init_opcode25_absent"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            supply["birth_supply_upper_bound_fdv_cents_floor"].as_u64(),
            Some(7_372_003)
        );
        assert_eq!(
            supply["exact_peak_supply_fdv_cents_floor"].as_u64(),
            Some(7_372_003)
        );

        let fdv = provider_num
            .checked_mul(U256::from(100_000_000_000_u64))
            .unwrap()
            .checked_div(provider_den)
            .unwrap();
        assert_eq!(fdv, U256::from(7_372_003));
        let protocol = &row["protocol"];
        assert!(!protocol["deployed_binary_proof"].as_bool().unwrap());
        assert!(
            !protocol["universal_missing_virtual_reserve_equals_zero_rule"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["lifetime_or_all_pool_ath_claim"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["automatic_adapter_or_promotion"]
                .as_bool()
                .unwrap()
        );

        let control = &row["buy_exact_quote_in_control"];
        assert_eq!(
            control["instruction_name"].as_str(),
            Some("buy_exact_quote_in")
        );
        assert_eq!(
            control["signature"].as_str(),
            Some(
                "2Fh5YwQKB3jRVq63CccNSMFwzXCd5rxsxW9FSiTV5tCCpaSL13ap51Y8W43oCnRXmXW1h4TKgQG7frd4pWBTyKAW"
            )
        );
        assert_eq!(
            control["capture_sha256"].as_str(),
            Some("aa27e0f41c2ba744c7cee89865d7a675ccbb088073ac552b98d221ce86f15587")
        );
        let parse_control = |key: &str| control[key].as_str().unwrap().parse::<U256>().unwrap();
        let base_reserve = parse_control("pool_base_raw_before");
        let quote_reserve = parse_control("pool_quote_raw_before");
        let gross_quote = parse_control("quote_amount_in_gross_raw");
        let net_quote = parse_control("user_quote_amount_in_net_raw");
        let observed_out = parse_control("base_amount_out_raw");
        let denominator = quote_reserve
            .checked_add(net_quote)
            .unwrap()
            .checked_sub(U256::from(1))
            .unwrap();
        let calculated_out = net_quote
            .checked_sub(U256::from(1))
            .unwrap()
            .checked_mul(base_reserve)
            .unwrap()
            .checked_div(denominator)
            .unwrap();
        assert_eq!(calculated_out, U256::from(143_666_486_066_u64));
        assert_eq!(calculated_out, observed_out);

        let mut total_fee = U256::ZERO;
        for fee in ["lp", "protocol", "creator"] {
            let bps = U256::from(control["fee_bps"][fee].as_u64().unwrap());
            let fee_raw = net_quote
                .checked_mul(bps)
                .unwrap()
                .checked_add(U256::from(9_999))
                .unwrap()
                .checked_div(U256::from(10_000))
                .unwrap();
            assert_eq!(
                fee_raw,
                U256::from(control["fee_raw"][fee].as_u64().unwrap())
            );
            total_fee = total_fee.checked_add(fee_raw).unwrap();
        }
        assert_eq!(net_quote.checked_add(total_fee).unwrap(), gross_quote);
    }

    #[test]
    fn solana_tracker_5b2bs_legacy_idl_price_and_supply_golden() {
        use alloy_primitives::{U256, U512};

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_5b2bs_legacy_pool_spot_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["source"].as_str(),
            Some("solana-tracker-window-historical-supply-bounds-pool-fx-validated-v1")
        );
        assert_eq!(
            row["mint"].as_str(),
            Some("5b2BsBEVvsjBegVaKik9vByFJ8YGcDbk8R3A96RTpump")
        );
        assert!(row["scope"]["ath_at"].is_null());
        assert!(!row["scope"]["all_market_or_lifetime"].as_bool().unwrap());
        assert!(
            !row["limitations"]["binary_activation_date_proven"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["generic_missing_virtual_reserve_zero_rule"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(row["dated_idl"]["commit"].as_str(), Some("82dacacf15"));
        assert_eq!(
            row["dated_idl"]["sha256"].as_str(),
            Some("5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6")
        );
        assert_eq!(
            row["independent_recheck_sha256"].as_str(),
            Some("bf6a6a2200cc3846154eebbb13e6e12d73dd0f85f9853d6ad5929b4969581fa1")
        );
        assert!(
            row["dated_idl"]["cashback_fields_present"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["dated_idl"]["virtual_quote_reserves_field_present"]
                .as_bool()
                .unwrap()
        );

        let provider = ExactDecimal::parse(row["provider"]["price_usd"].as_str().unwrap()).unwrap();
        let provider_num = U256::from(provider.coefficient);
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let provider_den = pow10(u32::try_from(provider.scale).unwrap());
        let fx = &row["fx"];
        assert_eq!(fx["minute_open_unix"].as_u64(), Some(1_773_522_420));
        assert_eq!(row["states"].as_array().unwrap().len(), 2);
        for state in row["states"].as_array().unwrap() {
            let instruction =
                raw_instruction_from_hex(state["event_cpi_data_hex"].as_str().unwrap());
            assert_eq!(
                instruction.data.len(),
                usize::try_from(state["event_cpi_data_bytes"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(
                state["inner_instruction_program_id"].as_str(),
                Some("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")
            );
            match scout_dex_solana::classify_pump_amm_event(&instruction) {
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Buy(event),
                ) => {
                    assert_eq!(state["event"].as_str(), Some("BuyEvent"));
                    assert_eq!(
                        event.pool,
                        pubkey_from_hex(state["event_fields"]["pool"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user,
                        pubkey_from_hex(state["event_fields"]["user"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user_base_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_base_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(
                        event.user_quote_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_quote_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(event.cashback_fee_basis_points, Some(0));
                    assert_eq!(event.cashback, Some(0));
                    assert_eq!(event.virtual_quote_reserves, None);
                    assert_eq!(event.ix_name.as_deref(), Some("buy_exact_quote_in"));
                    assert_eq!(event.quote_cost(), Some(227_907_305));
                }
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Sell(event),
                ) => {
                    assert_eq!(state["event"].as_str(), Some("SellEvent"));
                    assert_eq!(
                        event.pool,
                        pubkey_from_hex(state["event_fields"]["pool"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user,
                        pubkey_from_hex(state["event_fields"]["user"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user_base_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_base_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(
                        event.user_quote_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_quote_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(event.cashback_fee_basis_points, Some(0));
                    assert_eq!(event.cashback, Some(0));
                    assert_eq!(event.virtual_quote_reserves, None);
                    assert_eq!(event.quote_proceeds(), Some(181_751_708));
                }
                other => panic!("selected recorded event was not classified as a trade: {other:?}"),
            }
            assert_eq!(
                state["event_payload_bytes"].as_u64(),
                Some(if state["event"] == "BuyEvent" {
                    439
                } else {
                    376
                })
            );
            assert_eq!(
                state["decoded_under_dated_idl_commit"].as_str(),
                Some("82dacacf15")
            );
            assert_eq!(
                state["event_payload_sha256"].as_str(),
                Some(if state["event"] == "BuyEvent" {
                    "ebce7f0d361f988f2756e776704f17b057c71de0c1a61395bfafbd55ea030860"
                } else {
                    "0dd891e6f67ae4eb4bd0a0bfc531ac9a62acfdb905bd4d6144f773c3fe0e939a"
                })
            );
            assert!(state["curve_fee_vault_checks"].as_str() == Some("PASS"));
            assert!(
                state["reserve_fields_match_pool_owned_prebalances"]
                    .as_bool()
                    .unwrap()
            );
            let fields = &state["event_fields"];
            assert_eq!(fields["cashback_fee_basis_points"].as_u64(), Some(0));
            assert_eq!(fields["cashback"].as_u64(), Some(0));
            assert_eq!(
                i128::from(state["timestamp_unix"].as_u64().unwrap()) * 1_000
                    - i128::from(row["provider"]["timestamp_ms"].as_u64().unwrap()),
                i128::from(state["milliseconds_from_provider"].as_i64().unwrap())
            );

            let base =
                U256::from_str_radix(state["base_reserve_raw"].as_str().unwrap(), 10).unwrap();
            let quote =
                U256::from_str_radix(state["quote_reserve_raw"].as_str().unwrap(), 10).unwrap();
            let post_base =
                U256::from_str_radix(state["pool_vault_post_base_raw"].as_str().unwrap(), 10)
                    .unwrap();
            let post_quote =
                U256::from_str_radix(state["pool_vault_post_quote_raw"].as_str().unwrap(), 10)
                    .unwrap();
            let amount = |key: &str| U256::from(fields[key].as_u64().unwrap());
            let ceil_fee = |gross: U256, bps: U256| {
                gross
                    .checked_mul(bps)
                    .unwrap()
                    .checked_add(U256::from(9_999))
                    .unwrap()
                    .checked_div(U256::from(10_000))
                    .unwrap()
            };
            if state["event"] == "BuyEvent" {
                assert_eq!(state["ix_name"].as_str(), Some("buy_exact_quote_in"));
                let net = amount("user_quote_amount_in");
                let net_minus_one = net.checked_sub(U256::from(1)).unwrap();
                let expected_out = net_minus_one
                    .checked_mul(base)
                    .unwrap()
                    .checked_div(quote.checked_add(net_minus_one).unwrap())
                    .unwrap();
                assert_eq!(expected_out, amount("base_amount_out"));
                let gross = amount("quote_amount_in");
                let lp = ceil_fee(net, amount("lp_fee_basis_points"));
                let protocol = ceil_fee(net, amount("protocol_fee_basis_points"));
                let creator = ceil_fee(net, amount("coin_creator_fee_basis_points"));
                assert_eq!(lp, amount("lp_fee"));
                assert_eq!(protocol, amount("protocol_fee"));
                assert_eq!(creator, amount("coin_creator_fee"));
                assert_eq!(
                    net.checked_add(lp).unwrap(),
                    amount("quote_amount_in_with_lp_fee")
                );
                assert_eq!(
                    amount("quote_amount_in_with_lp_fee")
                        .checked_add(protocol)
                        .unwrap()
                        .checked_add(creator)
                        .unwrap(),
                    gross
                );
                assert_eq!(base.checked_sub(post_base).unwrap(), expected_out);
                assert_eq!(
                    post_quote.checked_sub(quote).unwrap(),
                    amount("quote_amount_in_with_lp_fee")
                );
            } else {
                assert_eq!(state["event"].as_str(), Some("SellEvent"));
                let base_in = amount("base_amount_in");
                let gross = base_in
                    .checked_mul(quote)
                    .unwrap()
                    .checked_div(base.checked_add(base_in).unwrap())
                    .unwrap();
                assert_eq!(gross, amount("quote_amount_out"));
                let lp = ceil_fee(gross, amount("lp_fee_basis_points"));
                let protocol = ceil_fee(gross, amount("protocol_fee_basis_points"));
                let creator = ceil_fee(gross, amount("coin_creator_fee_basis_points"));
                assert_eq!(lp, amount("lp_fee"));
                assert_eq!(protocol, amount("protocol_fee"));
                assert_eq!(creator, amount("coin_creator_fee"));
                let net = gross.checked_sub(lp).unwrap();
                assert_eq!(net, amount("quote_amount_out_without_lp_fee"));
                assert_eq!(
                    amount("user_quote_amount_out")
                        .checked_add(protocol)
                        .unwrap()
                        .checked_add(creator)
                        .unwrap(),
                    net
                );
                assert_eq!(post_base.checked_sub(base).unwrap(), base_in);
                assert_eq!(quote.checked_sub(post_quote).unwrap(), net);
            }

            let spot_n = quote.checked_mul(U256::from(1_000_000_u64)).unwrap();
            let spot_d = base.checked_mul(U256::from(1_000_000_000_u64)).unwrap();
            let stored = state["sol_per_token_fraction"].as_str().unwrap();
            let (stored_n, stored_d) = stored.split_once('/').unwrap();
            assert_eq!(
                U512::from(spot_n) * U512::from_str_radix(stored_d, 10).unwrap(),
                U512::from(spot_d) * U512::from_str_radix(stored_n, 10).unwrap()
            );
            for key in ["low", "high"] {
                let usd = ExactDecimal::parse(fx[key].as_str().unwrap()).unwrap();
                let candidate_n = spot_n.checked_mul(U256::from(usd.coefficient)).unwrap();
                let candidate_d = spot_d
                    .checked_mul(pow10(u32::try_from(usd.scale).unwrap()))
                    .unwrap();
                let diff = (U512::from(candidate_n) * U512::from(provider_den))
                    .abs_diff(U512::from(provider_num) * U512::from(candidate_d));
                assert!(
                    diff * U512::from(50) <= U512::from(provider_num) * U512::from(candidate_d)
                );
            }
        }

        let supply = &row["supply"];
        assert_eq!(supply["birth_raw"].as_str(), Some("1000000000000000"));
        assert_eq!(supply["peak_raw"].as_str(), Some("1000000000000000"));
        assert_eq!(
            supply["current_snapshot_raw"].as_str(),
            Some("999999912053342")
        );
        assert_eq!(supply["current_snapshot_slot"].as_u64(), Some(456_603_633));
        assert_eq!(supply["decimals"].as_u64(), Some(6));
        assert_eq!(supply["successful_transactions"].as_u64(), Some(429));
        assert_eq!(
            supply["successful_signature_index_rows"].as_u64(),
            Some(428)
        );
        assert!(supply["signature_sets_match_plus_birth"].as_bool().unwrap());
        assert_eq!(supply["mint_count"].as_u64(), Some(1));
        assert_eq!(supply["burn_count"].as_u64(), Some(0));
        assert_eq!(supply["unknown_target_opcodes"].as_u64(), Some(0));
        let upper_fdv = provider_num
            .checked_mul(U256::from_str_radix(supply["birth_raw"].as_str().unwrap(), 10).unwrap())
            .unwrap()
            .checked_mul(U256::from(100))
            .unwrap()
            .checked_div(provider_den.checked_mul(pow10(6)).unwrap())
            .unwrap();
        let current_fdv = provider_num
            .checked_mul(
                U256::from_str_radix(supply["current_snapshot_raw"].as_str().unwrap(), 10).unwrap(),
            )
            .unwrap()
            .checked_mul(U256::from(100))
            .unwrap()
            .checked_div(provider_den.checked_mul(pow10(6)).unwrap())
            .unwrap();
        assert_eq!(upper_fdv, U256::from(12_133_750));
        assert_eq!(current_fdv, U256::from(12_133_749));
        assert_eq!(
            row["provider"]["fdv_floor_cents"].as_u64(),
            Some(12_133_750)
        );
        assert_eq!(
            row["provider"]["current_supply_fdv_floor_cents"].as_u64(),
            Some(12_133_749)
        );
    }

    #[test]
    fn solana_tracker_88pqb_uses_dated_legacy_event_layout() {
        use alloy_primitives::{U256, U512};

        let row: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_88pqb_pool_spot_case.json"
        ))
        .unwrap();
        assert_eq!(
            row["source"].as_str(),
            Some("solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1")
        );
        assert!(row["ath_at"].is_null());
        assert_eq!(
            row["mint"].as_str(),
            Some("88pqbGiXSTb9UpNuSKegCBBEa23vaneN8R9EkKJhpump")
        );
        assert_eq!(
            row["pool"].as_str(),
            Some("HSKHmd8U4UrKm1i2TSVPD59T6H8Bhg4Mmx98PEEfzUBC")
        );
        assert_eq!(
            row["historical_layout_correction"]["dated_official_idl_commit"].as_str(),
            Some("82dacacf15")
        );
        assert_eq!(
            row["historical_layout_correction"]["dated_official_idl_sha256"].as_str(),
            Some("5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6")
        );
        assert_eq!(
            row["evidence_hashes"]["independent_dated_layout_correction_sha256"].as_str(),
            Some("9248a86e615262e62b70959514d19f206cf9bdc07d3d0f744a0d77e48d8edfb7")
        );
        assert_eq!(
            row["evidence_hashes"]["prior_independent_proof_with_withdrawn_vq_interpretation_sha256"]
                .as_str(),
            Some("4ce029419fceed8b87c1694da3a6cdb59179e6e7a021987f6eb012c22d8998cc")
        );
        assert_eq!(
            row["historical_layout_correction"]["prior_suffix_as_virtual_quote_interpretation"]
                .as_str(),
            Some("withdrawn; retained in original diagnostic artifacts only")
        );
        assert!(
            !row["historical_layout_correction"]["deployed_binary_activation_proven"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["historical_layout_correction"]["virtual_quote_reserves_field_present"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["protocol"]["general_missing_virtual_reserve_equals_zero_rule"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !row["limitations"]["all_pool_or_lifetime_ath"]
                .as_bool()
                .unwrap()
        );
        assert!(!row["limitations"]["database_promotion"].as_bool().unwrap());

        let provider = ExactDecimal::parse(row["provider"]["price_usd"].as_str().unwrap()).unwrap();
        let provider_num = U256::from(provider.coefficient);
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let provider_den = pow10(u32::try_from(provider.scale).unwrap());
        let fx = &row["fx"];
        let provider_minute = row["provider"]["timestamp_ms"].as_u64().unwrap() / 1_000 / 60 * 60;
        assert_eq!(provider_minute, fx["minute_open_unix"].as_u64().unwrap());

        for state in row["states"].as_array().unwrap() {
            let instruction =
                raw_instruction_from_hex(state["event_cpi_data_hex"].as_str().unwrap());
            assert_eq!(
                instruction.data.len(),
                usize::try_from(state["event_cpi_data_bytes"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(
                state["inner_instruction_program_id"].as_str(),
                Some("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")
            );
            match scout_dex_solana::classify_pump_amm_event(&instruction) {
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Buy(event),
                ) => {
                    assert_eq!(state["event"].as_str(), Some("BuyEvent"));
                    assert_eq!(
                        event.pool,
                        pubkey_from_hex(state["event_fields"]["pool"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user,
                        pubkey_from_hex(state["event_fields"]["user"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user_base_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_base_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(
                        event.user_quote_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_quote_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(event.cashback_fee_basis_points, Some(0));
                    assert_eq!(event.cashback, Some(0));
                    assert_eq!(event.virtual_quote_reserves, None);
                    assert_eq!(event.ix_name.as_deref(), Some("buy_exact_quote_in"));
                    assert_eq!(event.quote_cost(), Some(100_000_000));
                }
                scout_dex_solana::PumpAmmEventOutcome::Trade(
                    scout_dex_solana::PumpAmmEvent::Sell(event),
                ) => {
                    assert_eq!(state["event"].as_str(), Some("SellEvent"));
                    assert_eq!(
                        event.pool,
                        pubkey_from_hex(state["event_fields"]["pool"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user,
                        pubkey_from_hex(state["event_fields"]["user"].as_str().unwrap())
                    );
                    assert_eq!(
                        event.user_base_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_base_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(
                        event.user_quote_token_account,
                        pubkey_from_hex(
                            state["event_fields"]["user_quote_token_account"]
                                .as_str()
                                .unwrap()
                        )
                    );
                    assert_eq!(event.cashback_fee_basis_points, Some(0));
                    assert_eq!(event.cashback, Some(0));
                    assert_eq!(event.virtual_quote_reserves, None);
                    assert_eq!(event.quote_proceeds(), Some(11_123_524_554));
                }
                other => panic!("selected recorded event was not classified as a trade: {other:?}"),
            }
            assert_eq!(
                state["event_payload_bytes"].as_u64(),
                Some(if state["event"] == "BuyEvent" {
                    439
                } else {
                    376
                })
            );
            assert_eq!(
                state["decoded_under_dated_idl_commit"].as_str(),
                Some("82dacacf15")
            );
            assert_eq!(
                state["effective_quote_model"].as_str(),
                Some("real pre-swap quote-vault balance")
            );
            assert!(state.get("virtual_quote_raw_signed_i128").is_none());
            assert!(state.get("virtual_quote_raw_suffix_hex").is_none());
            assert!(
                state["reserve_fields_match_pool_owned_prebalances"]
                    .as_bool()
                    .unwrap()
            );
            assert!(
                state["event_amount_fee_balance_reconciliation"]
                    .as_bool()
                    .unwrap()
            );
            let fields = &state["event_fields"];
            assert_eq!(fields["cashback_fee_basis_points"].as_str(), Some("0"));
            assert_eq!(fields["cashback"].as_str(), Some("0"));

            let base = state["base_raw"].as_str().unwrap().parse::<U256>().unwrap();
            let quote = state["quote_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap();
            let post_base = state["pool_vault_post_base_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap();
            let post_quote = state["pool_vault_post_quote_raw"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap();
            let amount = |key: &str| fields[key].as_str().unwrap().parse::<U256>().unwrap();
            let ceil_fee = |gross: U256, bps: U256| {
                gross
                    .checked_mul(bps)
                    .unwrap()
                    .checked_add(U256::from(9_999))
                    .unwrap()
                    .checked_div(U256::from(10_000))
                    .unwrap()
            };
            if state["event"] == "BuyEvent" {
                assert_eq!(fields["ix_name"].as_str(), Some("buy_exact_quote_in"));
                let net = amount("user_quote_amount_in");
                let net_minus_one = net.checked_sub(U256::from(1)).unwrap();
                let output = net_minus_one
                    .checked_mul(base)
                    .unwrap()
                    .checked_div(quote.checked_add(net_minus_one).unwrap())
                    .unwrap();
                assert_eq!(output, amount("base_amount_out"));
                let gross = amount("quote_amount_in");
                let lp = ceil_fee(net, amount("lp_fee_basis_points"));
                let protocol = ceil_fee(net, amount("protocol_fee_basis_points"));
                let creator = ceil_fee(net, amount("coin_creator_fee_basis_points"));
                assert_eq!(lp, amount("lp_fee"));
                assert_eq!(protocol, amount("protocol_fee"));
                assert_eq!(creator, amount("coin_creator_fee"));
                let pool_quote_in = amount("quote_amount_in_with_lp_fee");
                assert_eq!(net.checked_add(lp).unwrap(), pool_quote_in);
                assert_eq!(
                    pool_quote_in
                        .checked_add(protocol)
                        .unwrap()
                        .checked_add(creator)
                        .unwrap(),
                    gross
                );
                assert_eq!(base.checked_sub(post_base).unwrap(), output);
                assert_eq!(post_quote.checked_sub(quote).unwrap(), pool_quote_in);
            } else {
                assert_eq!(state["event"].as_str(), Some("SellEvent"));
                let base_in = amount("base_amount_in");
                let gross = base_in
                    .checked_mul(quote)
                    .unwrap()
                    .checked_div(base.checked_add(base_in).unwrap())
                    .unwrap();
                assert_eq!(gross, amount("quote_amount_out"));
                let lp = ceil_fee(gross, amount("lp_fee_basis_points"));
                let protocol = ceil_fee(gross, amount("protocol_fee_basis_points"));
                let creator = ceil_fee(gross, amount("coin_creator_fee_basis_points"));
                assert_eq!(lp, amount("lp_fee"));
                assert_eq!(protocol, amount("protocol_fee"));
                assert_eq!(creator, amount("coin_creator_fee"));
                let pool_quote_out = amount("quote_amount_out_without_lp_fee");
                assert_eq!(gross.checked_sub(lp).unwrap(), pool_quote_out);
                assert_eq!(
                    amount("user_quote_amount_out")
                        .checked_add(protocol)
                        .unwrap()
                        .checked_add(creator)
                        .unwrap(),
                    pool_quote_out
                );
                assert_eq!(post_base.checked_sub(base).unwrap(), base_in);
                assert_eq!(quote.checked_sub(post_quote).unwrap(), pool_quote_out);
            }

            let timestamp = state["timestamp_unix"].as_u64().unwrap();
            assert_eq!(
                timestamp.div_euclid(60) * 60,
                fx["minute_open_unix"].as_u64().unwrap()
            );
            for key in ["low", "close", "high"] {
                let fx_value = ExactDecimal::parse(fx[key].as_str().unwrap()).unwrap();
                let spot_num = quote.checked_mul(U256::from(fx_value.coefficient)).unwrap();
                let spot_den = base
                    .checked_mul(U256::from(1_000_u64))
                    .unwrap()
                    .checked_mul(pow10(u32::try_from(fx_value.scale).unwrap()))
                    .unwrap();
                let provider_cross = provider_num.checked_mul(spot_den).unwrap();
                let spot_cross = spot_num.checked_mul(provider_den).unwrap();
                let difference = U512::from(provider_cross).abs_diff(U512::from(spot_cross));
                assert!(difference * U512::from(100) <= U512::from(provider_cross) * U512::from(2));
            }
        }

        let supply = &row["supply"];
        assert_eq!(supply["raw"].as_str(), Some("2000000000000000"));
        assert_eq!(supply["tokens"].as_str(), Some("2000000000"));
        assert_eq!(supply["decimals"].as_u64(), Some(6));
        assert_eq!(supply["successful_transactions"].as_u64(), Some(191));
        assert!(
            supply["signature_index_matches_window_set"]
                .as_bool()
                .unwrap()
        );
        assert!(supply["birth_authority_revoked"].as_bool().unwrap());
        assert!(supply["no_close_authority"].as_bool().unwrap());
        assert_eq!(
            supply["mint_to_after_birth_through_peak_second"].as_str(),
            Some("0")
        );
        assert_eq!(
            supply["burn_before_or_during_peak_second"].as_str(),
            Some("0")
        );
        let fdv = provider_num
            .checked_mul(U256::from(200_000_000_000_u64))
            .unwrap()
            .checked_div(provider_den)
            .unwrap();
        assert_eq!(fdv, U256::from(1_921_249));
        assert_eq!(fdv.to_string(), supply["fdv_cents_floor"].as_str().unwrap());
        assert_eq!(
            row["provider"]["market_cap_usd_reported"].as_str(),
            Some("9606.25")
        );
    }

    #[test]
    fn solana_tracker_pumpswap_prebalance_binding_and_full_window_gates() {
        use alloy_primitives::U512;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_pumpswap_prebalance_window_cases.json"
        ))
        .unwrap();
        let recovered_fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_pumpswap_prebalance_batch14_cases.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["provenance"]["window_proof_v5_sha256"].as_str(),
            Some("127348ce325353679112d28958e7009dba29d9c25ce8b41a5047979274074976")
        );
        assert_eq!(
            recovered_fixture["provenance"]["window_proof_v5_sha256"].as_str(),
            Some("33d8e5b4f841fb6f147dccbf294951ee20b4bec732c1250425c5e0fe225d5895")
        );
        assert_eq!(
            recovered_fixture["provenance"]["root_four_bounds_sha256"].as_str(),
            Some("b101c28dc5e41cce6636a3dc509e65491b44a9fe8973b75b04394910df18a0f5")
        );
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap()
        };
        let decimal_ratio = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            (
                U512::from(decimal.coefficient),
                pow10(u32::try_from(decimal.scale).unwrap()),
            )
        };
        let compare_ratio = |a: (U512, U512), b: (U512, U512)| (a.0 * b.1).cmp(&(b.0 * a.1));
        let within_two = |a: (U512, U512), b: (U512, U512)| {
            let left = a.0 * b.1;
            let right = b.0 * a.1;
            let delta = if left >= right {
                left - right
            } else {
                right - left
            };
            delta * U512::from(100) <= right * U512::from(2)
        };
        let is_bound = |row: &serde_json::Value| {
            let create = &row["create_pool"];
            let buy = &row["buy"];
            let event = &row["buy_event_cpi"];
            let create_pos = (
                create["outer"].as_u64().unwrap(),
                create["inner"].as_i64().unwrap(),
            );
            let buy_pos = (
                buy["outer"].as_u64().unwrap(),
                buy["inner"].as_i64().unwrap(),
            );
            let event_pos = (
                event["outer"].as_u64().unwrap(),
                event["inner"].as_i64().unwrap(),
            );
            let boundary = (
                row["same_outer_parent_boundary"][0].as_u64().unwrap(),
                row["same_outer_parent_boundary"][1].as_i64().unwrap(),
            );
            let accounts = event["accounts"].as_array().unwrap();
            if create_pos >= buy_pos
                || event_pos.0 != buy_pos.0
                || event_pos <= buy_pos
                || event_pos >= boundary
                || event["stack_height"].as_u64().unwrap()
                    != buy["stack_height"].as_u64().unwrap() + 1
                || accounts.len() != 1
                || accounts[0].as_str() != row["buy_event_authority"].as_str()
                || buy["roles"]["event_authority"].as_str() != row["buy_event_authority"].as_str()
                || buy["roles"]["program"].as_str()
                    != Some("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")
                || row["binding_expectations"]["create_pool_tail_bytes"].as_u64() != Some(0)
                || row["binding_expectations"]["buyevent_tail_bytes"].as_u64() != Some(0)
                || row["ata_initializers"]
                    .as_array()
                    .is_none_or(|a| a.len() != 2)
                || row["vault_transfer_checked_instructions"]
                    .as_array()
                    .is_none_or(|a| a.len() != 4)
                || row["unrecognized_vault_token_operations"]
                    .as_array()
                    .is_none_or(|a| !a.is_empty())
            {
                return false;
            }
            let inits = row["token_account_initializers"].as_array().unwrap();
            row["ata_initializers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|ata| {
                    let vault = ata["accounts"][1].as_str().unwrap();
                    let is_base = vault == row["vaults"]["base"].as_str().unwrap();
                    let mint = if is_base {
                        row["mint"].as_str().unwrap()
                    } else {
                        row["quote_mint"].as_str().unwrap()
                    };
                    let ata_pos = (
                        ata["outer"].as_u64().unwrap(),
                        ata["inner"].as_i64().unwrap(),
                    );
                    ata["program"].as_str() == Some("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
                        && ata["data_hex"].as_str() == Some("01")
                        && ata_pos < create_pos
                        && ata["accounts"][2].as_str() == row["pool"].as_str()
                        && ata["accounts"][3].as_str() == Some(mint)
                        && ata["accounts"][5].as_str()
                            == row["create_pool"]["roles"][if is_base {
                                "base_token_program"
                            } else {
                                "quote_token_program"
                            }]
                            .as_str()
                        && inits
                            .iter()
                            .filter(|ix| {
                                let data = bytes_from_hex(ix["data_hex"].as_str().unwrap());
                                let pos =
                                    (ix["outer"].as_u64().unwrap(), ix["inner"].as_i64().unwrap());
                                ix["program"].as_str()
                                    == row["create_pool"]["roles"][if is_base {
                                        "base_token_program"
                                    } else {
                                        "quote_token_program"
                                    }]
                                    .as_str()
                                    && ix["accounts"][0].as_str() == Some(vault)
                                    && pos < create_pos
                                    && data.len() == 33
                                    && data[0] == 18
                                    && bs58::encode(&data[1..]).into_string()
                                        == row["pool"].as_str().unwrap()
                            })
                            .count()
                            == 1
                })
        };

        let mut cases = fixture["cases"].as_array().unwrap().clone();
        cases.extend(
            recovered_fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
        let mut outcomes = Vec::new();
        for row in &cases {
            assert!(row["full_window_complete"].as_bool().unwrap());
            assert_eq!(
                usize::try_from(row["verified_state_count"].as_u64().unwrap()).unwrap(),
                row["states"].as_array().unwrap().len()
            );
            assert!(is_bound(row));
            let create_data = bytes_from_hex(row["create_pool"]["data_hex"].as_str().unwrap());
            assert_eq!(&create_data[..8], &[233, 146, 209, 142, 207, 104, 64, 188]);
            assert_eq!(
                u16::from_le_bytes(create_data[8..10].try_into().unwrap()),
                0
            );
            assert_eq!(
                u64::from_le_bytes(create_data[10..18].try_into().unwrap()),
                206_900_000_000_000
            );
            let create_quote = u64::from_le_bytes(create_data[18..26].try_into().unwrap());
            assert_eq!(create_data[58], 0, "captured pool is not in mayhem mode");
            if row["binding_expectations"]["create_pool_option_bool"].as_bool() == Some(true) {
                assert_eq!(create_data.len(), 60);
                assert_eq!(create_data[59], 0, "dated IDL cashback OptionBool is false");
            } else {
                assert_eq!(create_data.len(), 59);
            }
            let create_roles = &row["create_pool"]["roles"];
            assert_eq!(create_roles["pool"].as_str(), row["pool"].as_str());
            assert_eq!(create_roles["base_mint"].as_str(), row["mint"].as_str());
            assert_eq!(
                create_roles["quote_mint"].as_str(),
                row["quote_mint"].as_str()
            );
            assert_eq!(
                create_roles["pool_base_token_account"].as_str(),
                row["vaults"]["base"].as_str()
            );
            assert_eq!(
                create_roles["pool_quote_token_account"].as_str(),
                row["vaults"]["quote"].as_str()
            );
            assert_eq!(
                create_quote.to_string(),
                row["reserve_transition"]["pre"]["quote"].as_str().unwrap()
            );
            let event_ix =
                raw_instruction_from_hex(row["buy_event_cpi"]["data_hex"].as_str().unwrap());
            let (base_pre, quote_pre, base_out, quote_in, event_time) =
                match scout_dex_solana::classify_pump_amm_event(&event_ix) {
                    scout_dex_solana::PumpAmmEventOutcome::Trade(
                        scout_dex_solana::PumpAmmEvent::Buy(event),
                    ) => {
                        assert_eq!(bs58::encode(event.pool).into_string(), row["pool"]);
                        assert_eq!(event.virtual_quote_reserves, None);
                        (
                            event.pool_base_token_reserves,
                            event.pool_quote_token_reserves,
                            event.base_amount_out,
                            event.quote_amount_in_with_lp_fee,
                            event.timestamp,
                        )
                    }
                    other => panic!("bound event did not decode as BuyEvent: {other:?}"),
                };
            assert_eq!(
                event_time,
                i64::try_from(row["buy_event_cpi"]["block_time_unix_s"].as_u64().unwrap()).unwrap()
            );
            let state = &row["reserve_transition"];
            assert_eq!(base_pre.to_string(), state["pre"]["base"].as_str().unwrap());
            assert_eq!(
                quote_pre.to_string(),
                state["pre"]["quote"].as_str().unwrap()
            );
            assert_eq!(
                base_pre - base_out,
                state["post"]["base"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            );
            assert_eq!(
                quote_pre + quote_in,
                state["post"]["quote"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            );
            assert_eq!(row["post_vault_balances"].as_array().unwrap().len(), 2);
            let transfers = row["vault_transfer_checked_instructions"]
                .as_array()
                .unwrap();
            let create_pos = (
                row["create_pool"]["outer"].as_u64().unwrap(),
                row["create_pool"]["inner"].as_i64().unwrap(),
            );
            let buy_pos = (
                row["buy"]["outer"].as_u64().unwrap(),
                row["buy"]["inner"].as_i64().unwrap(),
            );
            let mut funding_count = 0;
            let mut buy_count = 0;
            for ix in transfers {
                let bytes = bytes_from_hex(ix["data_hex"].as_str().unwrap());
                assert_eq!(bytes.len(), 10);
                assert_eq!(bytes[0], 12);
                let amount = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                let accounts = ix["accounts"].as_array().unwrap();
                let source = accounts[0].as_str().unwrap();
                let token_mint = accounts[1].as_str().unwrap();
                let destination = accounts[2].as_str().unwrap();
                let vault = if source == row["vaults"]["base"].as_str().unwrap()
                    || destination == row["vaults"]["base"].as_str().unwrap()
                {
                    row["vaults"]["base"].as_str().unwrap()
                } else {
                    row["vaults"]["quote"].as_str().unwrap()
                };
                let transfer_pos = (ix["outer"].as_u64().unwrap(), ix["inner"].as_i64().unwrap());
                if transfer_pos > create_pos && transfer_pos < buy_pos {
                    funding_count += 1;
                    assert_eq!(destination, vault);
                    assert_eq!(
                        accounts[3].as_str(),
                        row["create_pool"]["roles"]["creator"].as_str()
                    );
                    assert_eq!(
                        source,
                        row["create_pool"]["roles"][if vault
                            == row["vaults"]["base"].as_str().unwrap()
                        {
                            "user_base_token_account"
                        } else {
                            "user_quote_token_account"
                        }]
                        .as_str()
                        .unwrap()
                    );
                    assert_eq!(
                        amount.to_string(),
                        row["reserve_transition"]["pre"][if vault
                            == row["vaults"]["base"].as_str().unwrap()
                        {
                            "base"
                        } else {
                            "quote"
                        }]
                        .as_str()
                        .unwrap()
                    );
                    let token_role = if vault == row["vaults"]["base"].as_str().unwrap() {
                        "base_token_program"
                    } else {
                        "quote_token_program"
                    };
                    assert_eq!(
                        ix["program"].as_str(),
                        row["create_pool"]["roles"][token_role].as_str()
                    );
                } else if transfer_pos > buy_pos {
                    buy_count += 1;
                    assert_eq!(
                        ix["stack_height"].as_u64().unwrap(),
                        row["buy"]["stack_height"].as_u64().unwrap() + 1
                    );
                    if vault == row["vaults"]["base"].as_str().unwrap() {
                        assert_eq!(accounts[3].as_str(), row["pool"].as_str());
                        assert_eq!(source, vault);
                        assert_eq!(
                            destination,
                            row["buy"]["roles"]["user_base_token_account"]
                                .as_str()
                                .unwrap()
                        );
                        assert_eq!(amount, base_out);
                    } else {
                        assert_eq!(accounts[3].as_str(), row["buy"]["roles"]["user"].as_str());
                        assert_eq!(
                            source,
                            row["buy"]["roles"]["user_quote_token_account"]
                                .as_str()
                                .unwrap()
                        );
                        assert_eq!(destination, vault);
                        assert_eq!(amount, quote_in);
                    }
                    let token_role = if vault == row["vaults"]["base"].as_str().unwrap() {
                        "base_token_program"
                    } else {
                        "quote_token_program"
                    };
                    assert_eq!(
                        ix["program"].as_str(),
                        row["create_pool"]["roles"][token_role].as_str()
                    );
                } else {
                    panic!("vault transfer is outside CreatePool funding or target buy scope");
                }
                assert_eq!(
                    bytes[9],
                    if token_mint == row["mint"].as_str().unwrap() {
                        6
                    } else {
                        9
                    }
                );
                let token_role = if token_mint == row["mint"].as_str().unwrap() {
                    "base_token_program"
                } else {
                    assert_eq!(token_mint, row["quote_mint"].as_str().unwrap());
                    "quote_token_program"
                };
                assert_eq!(
                    ix["program"].as_str(),
                    row["create_pool"]["roles"][token_role].as_str()
                );
            }
            assert_eq!((funding_count, buy_count), (2, 2));
            for balance in row["post_vault_balances"].as_array().unwrap() {
                let base_vault = balance["account"].as_str() == row["vaults"]["base"].as_str();
                let expected = if base_vault {
                    state["post"]["base"].as_str().unwrap()
                } else {
                    state["post"]["quote"].as_str().unwrap()
                };
                assert_eq!(balance["raw_amount"].as_str(), Some(expected));
                assert_eq!(balance["owner"].as_str(), row["pool"].as_str());
                assert_eq!(
                    balance["mint"].as_str(),
                    if base_vault {
                        row["mint"].as_str()
                    } else {
                        row["quote_mint"].as_str()
                    }
                );
            }
            let provider = decimal_ratio(row["provider_price_usd"].as_str().unwrap());
            type Mark = ((U512, U512), i64, String);
            let mut marks: [Vec<Mark>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for s in row["states"].as_array().unwrap() {
                assert!(s["offset_from_peak_s"].as_i64().unwrap().abs() <= 3);
                assert_eq!(
                    s["coinbase_minute_unix_s"].as_u64().unwrap(),
                    s["block_time_unix_s"].as_u64().unwrap() / 60 * 60
                );
                let base = U512::from(
                    s["base_vault_raw"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                );
                let quote = U512::from(
                    s["quote_vault_raw"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                );
                let native = (quote * pow10(6), base * U512::from(1_000_000_000_u64));
                for (i, key) in ["sol_usd_low", "sol_usd_close", "sol_usd_high"]
                    .iter()
                    .enumerate()
                {
                    let fx = decimal_ratio(s[*key].as_str().unwrap());
                    marks[i].push((
                        (native.0 * fx.0, native.1 * fx.1),
                        s["offset_from_peak_s"].as_i64().unwrap(),
                        s["signature"].as_str().unwrap().to_owned(),
                    ));
                }
            }
            for endpoint in &mut marks {
                endpoint.sort_by(|a, b| compare_ratio(a.0, b.0));
            }
            let price_pass = marks
                .iter()
                .all(|states| within_two(states.last().unwrap().0, provider));
            let label = row["label"].as_str().unwrap();
            match label {
                "69KX" => {
                    assert_eq!(marks[1].last().unwrap().1, 1);
                    assert!(marks[1].iter().any(|mark| {
                        mark.2.starts_with("2FCxxFF2")
                            && compare_ratio(mark.0, marks[1].last().unwrap().0).is_eq()
                    }));
                    assert!(!price_pass);
                }
                "CtN5" => {
                    assert!(marks[0].iter().any(|mark| {
                        mark.1 == -3 && compare_ratio(mark.0, marks[0].last().unwrap().0).is_eq()
                    }));
                    assert!(price_pass);
                }
                "G3Hy" => {
                    assert_eq!(marks[0].last().unwrap().1, -1);
                    assert!(price_pass);
                }
                "D8uy" | "gLTR" | "A5Ewn" | "NDt3" => {
                    assert!(price_pass);
                    assert!(
                        marks
                            .iter()
                            .all(|states| (-3..=0).contains(&states.last().unwrap().1))
                    );
                }
                _ => panic!("unexpected mint case"),
            }
            let supply = &row["supply"];
            let birth = supply["birth_raw"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let current = supply["current_raw"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            assert_eq!(birth, 1_000_000_000_000_000);
            assert!(
                (birth - current) * 50 <= birth,
                "current-to-birth supply interval exceeds 2%"
            );
            assert_eq!(supply["decimals"].as_u64(), Some(6));
            assert!(
                supply["mint_authority_none"].as_bool().unwrap()
                    && supply["extension_set_exact"].as_bool().unwrap()
                    && supply["birth_allowlist_pass"].as_bool().unwrap()
            );
            let price = ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
            let cents = price.fdv_cents_for_raw_supply(current, 6).unwrap();
            if supply["stored_candidate_source"].is_string() {
                assert_eq!(
                    supply["stored_candidate_source"].as_str(),
                    Some("provider_peak_price_times_current_supply_lower_bound")
                );
                assert_eq!(supply["stored_candidate_floor_cents"].as_i64(), Some(cents));
                assert!(
                    supply["floor_aware_interval_within_two_percent"]
                        .as_bool()
                        .unwrap()
                        && supply["independent_spot_supply_endpoints_within_two_percent"]
                            .as_bool()
                            .unwrap()
                        && supply["all_global_price_endpoints_within_two_percent"]
                            .as_bool()
                            .unwrap()
                        && supply["eligible_manual_lower_bound_candidate"]
                            .as_bool()
                            .unwrap()
                );
                assert!(!supply["historical_peak_supply_exact"].as_bool().unwrap());
            }
            let provider_upper = (provider.0 * U512::from(birth), provider.1 * pow10(6));
            assert!(U512::from(98) * provider_upper.0 <= U512::from(cents) * provider_upper.1);
            let stored = (provider.0 * U512::from(current), provider.1 * pow10(6));
            let independent_low = marks[0].last().unwrap().0;
            let independent_high = marks[2].last().unwrap().0;
            let low_fdv = (
                independent_low.0 * U512::from(current),
                independent_low.1 * pow10(6),
            );
            let high_fdv = (
                independent_high.0 * U512::from(birth),
                independent_high.1 * pow10(6),
            );
            let supply_price_pass = within_two(stored, low_fdv) && within_two(stored, high_fdv);
            assert_eq!(supply_price_pass, label != "69KX");
            outcomes.push((label, cents, price_pass, supply_price_pass));
        }
        assert_eq!(
            outcomes,
            vec![
                ("69KX", 7_963_221, false, false),
                ("CtN5", 7_547_302, true, true),
                ("G3Hy", 5_854_639, true, true),
                ("D8uy", 4_422_273, true, true),
                ("gLTR", 4_406_919, true, true),
                ("A5Ewn", 4_096_359, true, true),
                ("NDt3", 3_453_617, true, true)
            ]
        );

        let mut rogue = fixture["cases"][1].clone();
        rogue["buy_event_cpi"]["accounts"][0] =
            serde_json::Value::String("11111111111111111111111111111111".into());
        assert!(!is_bound(&rogue));
        let mut outside = fixture["cases"][1].clone();
        outside["buy_event_cpi"]["inner"] = outside["same_outer_parent_boundary"][1].clone();
        assert!(!is_bound(&outside));
        let mut missing_init = fixture["cases"][1].clone();
        missing_init["token_account_initializers"] = serde_json::Value::Array(Vec::new());
        assert!(!is_bound(&missing_init));
        let mut unknown_op = fixture["cases"][1].clone();
        unknown_op["unrecognized_vault_token_operations"] =
            serde_json::json!([{"opcode":3,"touches_pool_vault":true}]);
        assert!(!is_bound(&unknown_op));
        let mut malformed = raw_instruction_from_hex(
            fixture["cases"][1]["buy_event_cpi"]["data_hex"]
                .as_str()
                .unwrap(),
        );
        malformed.data.truncate(20);
        assert!(!matches!(
            scout_dex_solana::classify_pump_amm_event(&malformed),
            scout_dex_solana::PumpAmmEventOutcome::Trade(scout_dex_solana::PumpAmmEvent::Buy(_))
        ));
    }

    #[test]
    fn solana_tracker_chronological_fdv_window_exact_bounds_and_binding() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_chronological_fdv_window_cases.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["provenance"]["artifact_sha256"]["full_fdv_replay_v25_sha256"].as_str(),
            Some("414d4555fda934b0a8eab435430ff7efcabc746ca9ae02d134214cf4d2ef2d2d")
        );
        assert_eq!(
            fixture["provenance"]["artifact_sha256"]["root_full_fdv_certificate_v4_sha256"]
                .as_str(),
            Some("e85185acd8d78c9c348808e7fb0d286754a4a2ba4563911fac34bb8347d69251")
        );
        assert_eq!(
            fixture["provenance"]["artifact_sha256"]["root_curve_token_transfers_v1_sha256"]
                .as_str(),
            Some("fc16c015c3b9d2bdf9c1218eef8d30fa849f2636251ea2386dc08c8b6463a323")
        );
        assert_eq!(
            fixture["provenance"]["artifact_sha256"]["cpm_pre_q_inversion_interval_audit_v2_sha256"]
                .as_str(),
            Some("54d1aee7ef3f1727010f27f4a98a307019214a08a5b4a10cc65ae519671ce2f8")
        );
        assert_eq!(fixture["scope"]["state_count"].as_u64(), Some(1_715));
        assert_eq!(
            fixture["scope"]["curve_pre_state_count"].as_u64(),
            Some(684)
        );
        assert_eq!(fixture["fx_minutes"].as_array().unwrap().len(), 84);
        let request_raw = fixture["current_mint_capture"]["request_json_raw"]
            .as_str()
            .unwrap();
        let response_raw = fixture["current_mint_capture"]["response_json_raw"]
            .as_str()
            .unwrap();
        let request_sha256 = format!("{:x}", Sha256::digest(request_raw.as_bytes()));
        let response_sha256 = format!("{:x}", Sha256::digest(response_raw.as_bytes()));
        assert_eq!(
            request_sha256,
            fixture["provenance"]["artifact_sha256"]["current_mint_capture_request_sha256"]
        );
        assert_eq!(
            response_sha256,
            fixture["provenance"]["artifact_sha256"]["current_mint_capture_response_sha256"]
        );
        let request: serde_json::Value = serde_json::from_str(request_raw).unwrap();
        let response: serde_json::Value = serde_json::from_str(response_raw).unwrap();
        assert_eq!(request["method"], "getMultipleAccounts");
        assert_eq!(request["params"][1]["commitment"], "finalized");
        assert_eq!(request["params"][1]["encoding"], "base64");
        assert_eq!(
            response["result"]["context"]["slot"].as_u64(),
            Some(456_670_470)
        );
        assert_eq!(request["params"][0].as_array().unwrap().len(), 6);
        assert_eq!(response["result"]["value"].as_array().unwrap().len(), 6);
        assert_eq!(
            fixture["scope"]["limitations"]["exact_deployed_mayhem_wrapper_pre_q"],
            false
        );
        assert_eq!(fixture["scope"]["limitations"]["all_venues"], false);
        assert_eq!(fixture["scope"]["limitations"]["lifetime_history"], false);
        assert_eq!(fixture["scope"]["limitations"]["annual_history"], false);

        let ratio = |value: &str| {
            let (numerator, denominator) = value.split_once('/').unwrap();
            (
                U512::from_str_radix(numerator, 10).unwrap(),
                U512::from_str_radix(denominator, 10).unwrap(),
            )
        };
        let compare =
            |left: (U512, U512), right: (U512, U512)| (left.0 * right.1).cmp(&(right.0 * left.1));
        let ceil_div = |numerator: U512, denominator: U512| {
            (numerator + denominator - U512::from(1)) / denominator
        };
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap()
        };
        let decimal_ratio = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            (
                U512::from(decimal.coefficient),
                pow10(u32::try_from(decimal.scale).unwrap()),
            )
        };
        let fdv = |price: (U512, U512), supply_raw: U512| {
            (
                price.0 * supply_raw * U512::from(100),
                price.1 * U512::from(1_000_000),
            )
        };
        let transfer_is_bound = |row: &serde_json::Value| {
            let transfers = row["scoped_transfers"].as_array().unwrap();
            if !row["matches"].as_bool().unwrap()
                || !row["unknown"].as_array().unwrap().is_empty()
                || transfers.len() != 1
            {
                return false;
            }
            let transfer = &transfers[0];
            let roles = &row["trade_roles"];
            let instruction = &row["matched_trade_instruction"];
            let bytes = bytes_from_hex(transfer["raw_hex"].as_str().unwrap());
            let amount = U512::from(transfer["amount_raw"].as_u64().unwrap());
            let event_amount = U512::from(row["event_token_raw"].as_u64().unwrap());
            let inner = transfer["inner_index"].as_u64().unwrap();
            let end = row["subtree_end"].as_u64().unwrap();
            let trade_inner = row["inner_index"].as_i64().unwrap();
            let trade_height = row["trade_stack_height"].as_u64().unwrap();
            let is_buy = matches!(
                row["instruction"].as_str(),
                Some("buy" | "buy_exact_sol_in")
            );
            let expected_source = if is_buy {
                roles["associated_bonding_curve"].as_str().unwrap()
            } else {
                roles["associated_user"].as_str().unwrap()
            };
            let expected_destination = if is_buy {
                roles["associated_user"].as_str().unwrap()
            } else {
                roles["associated_bonding_curve"].as_str().unwrap()
            };
            let expected_authority = if is_buy {
                roles["bonding_curve"].as_str().unwrap()
            } else {
                roles["user"].as_str().unwrap()
            };
            let trade_data = bytes_from_hex(instruction["data_hex"].as_str().unwrap());
            let expected_discriminator: &[u8] = match row["instruction"].as_str() {
                Some("buy") => &[0x66, 0x06, 0x3d, 0x12, 0x01, 0xda, 0xeb, 0xea],
                Some("buy_exact_sol_in") => &[0x38, 0xfc, 0x74, 0x08, 0x9e, 0xdf, 0xcd, 0x5f],
                Some("sell") => &[0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad],
                _ => return false,
            };
            let expected_start = if trade_inner < 0 {
                0
            } else {
                u64::try_from(trade_inner).unwrap() + 1
            };
            bytes.len() == 10
                && bytes[0] == 12
                && U512::from(u64::from_le_bytes(bytes[1..9].try_into().unwrap())) == amount
                && amount == event_amount
                && bytes[9] == 6
                && transfer["opcode"].as_u64() == Some(12)
                && transfer["mint"] == roles["mint"]
                && transfer["program"] == roles["token_program"]
                && transfer["authority"] == expected_authority
                && transfer["source"] == expected_source
                && transfer["destination"] == expected_destination
                && instruction["program"].as_str()
                    == Some("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
                && trade_data.starts_with(expected_discriminator)
                && instruction["inner_index"] == row["inner_index"]
                && instruction["outer_index"] == row["outer_index"]
                && instruction["stack_height"] == row["trade_stack_height"]
                && instruction["subtree_end"] == row["subtree_end"]
                && transfer["stack_height"].as_u64().unwrap() > trade_height
                && inner > expected_start
                && inner < end
        };

        let expected = [
            ("EWbabspX", 469, 198, 613_081, 613_803),
            ("FLkQktRL", 455, 152, 632_673, 632_892),
            ("2b4p8ien", 791, 334, 6_025_465, 6_031_181),
        ];
        let mut state_total = 0_usize;
        let mut pre_total = 0_usize;
        for (case, (prefix, state_count, pre_count, low_cents, high_cents)) in
            fixture["cases"].as_array().unwrap().iter().zip(expected)
        {
            assert_eq!(case["prefix"].as_str(), Some(prefix));
            let states = case["states"].as_array().unwrap();
            let curve_pre = case["curve_pre_states"].as_array().unwrap();
            let bindings = case["curve_trade_bindings"].as_array().unwrap();
            assert_eq!(states.len(), state_count);
            assert_eq!(curve_pre.len(), pre_count);
            assert_eq!(bindings.len(), pre_count);
            assert_eq!(case["decimals"].as_u64(), Some(6));
            state_total += states.len();
            pre_total += curve_pre.len();

            let mut max_low: Option<(U512, U512)> = None;
            let mut max_high: Option<(U512, U512)> = None;
            let mut chronological = Vec::with_capacity(states.len());
            for state in states {
                let minute = state["fx_minute_unix_s"]
                    .as_u64()
                    .or_else(|| state["minute"].as_u64())
                    .unwrap();
                let fx = fixture["fx_minutes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["minute_unix_s"].as_u64() == Some(minute))
                    .unwrap();
                let (native_low, native_high) = if state["venue"].as_str() == Some("pumpfun_curve")
                    && state["side"].as_str() == Some("pre")
                {
                    (
                        ratio(state["price_native_sol_per_token_low"].as_str().unwrap()),
                        ratio(state["price_native_sol_per_token_high"].as_str().unwrap()),
                    )
                } else {
                    let native = ratio(state["price_native"].as_str().unwrap());
                    (native, native)
                };
                let fx_low = decimal_ratio(fx["low_usd_per_sol"].as_str().unwrap());
                let fx_high = decimal_ratio(fx["high_usd_per_sol"].as_str().unwrap());
                assert_eq!(
                    compare(
                        (native_low.0 * fx_low.0, native_low.1 * fx_low.1),
                        ratio(state["price_usd_low"].as_str().unwrap())
                    ),
                    Ordering::Equal
                );
                assert_eq!(
                    compare(
                        (native_high.0 * fx_high.0, native_high.1 * fx_high.1),
                        ratio(state["price_usd_high"].as_str().unwrap())
                    ),
                    Ordering::Equal
                );
                let low = fdv(
                    ratio(state["price_usd_low"].as_str().unwrap()),
                    U512::from(state["supply_low_raw"].as_u64().unwrap()),
                );
                let high = fdv(
                    ratio(state["price_usd_high"].as_str().unwrap()),
                    U512::from(state["supply_high_raw"].as_u64().unwrap()),
                );
                let low_floor = low.0 / low.1;
                let high_ceil = ceil_div(high.0, high.1);
                assert_eq!(
                    low_floor,
                    U512::from(state["fdv_low_cents"].as_u64().unwrap())
                );
                assert_eq!(
                    high_ceil,
                    U512::from(state["fdv_high_cents"].as_u64().unwrap())
                );
                if state["venue"].as_str() == Some("pumpfun_curve")
                    && state["side"].as_str() == Some("pre")
                {
                    assert_eq!(
                        state["supply_low_raw"],
                        state["supply_immediately_before_trade_raw"]
                    );
                    assert_eq!(
                        state["supply_high_raw"],
                        state["supply_immediately_before_trade_raw"]
                    );
                }
                if max_low.is_none_or(|current| compare(low, current).is_gt()) {
                    max_low = Some(low);
                }
                if max_high.is_none_or(|current| compare(high, current).is_gt()) {
                    max_high = Some(high);
                }
                let side_order = match state["side"].as_str().unwrap() {
                    "pre" => 0_u8,
                    "post" => 1_u8,
                    _ => panic!("unexpected chronological mark side"),
                };
                chronological.push((
                    state["slot"].as_u64().unwrap(),
                    state["transaction_index"].as_u64().unwrap(),
                    state["log_index"].as_u64().unwrap(),
                    side_order,
                ));
            }
            assert!(chronological.windows(2).all(|pair| pair[0] <= pair[1]));

            for (bound, maximum, expected_cents) in [
                (
                    "full_window_global_max_fdv_low",
                    max_low.unwrap(),
                    low_cents,
                ),
                (
                    "full_window_global_max_fdv_high",
                    max_high.unwrap(),
                    high_cents,
                ),
            ] {
                let winner = &case["global_maxima"][bound];
                assert_eq!(
                    compare(
                        maximum,
                        ratio(winner["fdv_cents_unrounded"].as_str().unwrap())
                    ),
                    Ordering::Equal
                );
                assert_eq!(winner["fdv_cents"].as_u64(), Some(expected_cents));
                assert!(states.iter().any(|state| {
                    state["signature"].as_str() == winner["signature"].as_str()
                        && state["side"] == winner["side"]
                        && state["log_index"] == winner["log_index"]
                }));
            }
            assert_eq!(
                case["certificate"]["stored_lower_fdv_cents"].as_u64(),
                Some(low_cents)
            );
            let current_mint = &case["certificate"]["current_mint"];
            assert_eq!(current_mint["snapshot_request_sha256"], request_sha256);
            assert_eq!(current_mint["snapshot_response_sha256"], response_sha256);
            assert_eq!(
                current_mint["snapshot_context_slot"],
                response["result"]["context"]["slot"]
            );
            assert_eq!(current_mint["snapshot_value_count"].as_u64(), Some(6));
            assert_eq!(current_mint["snapshot_is_finalized"], true);
            let mint_index = request["params"][0]
                .as_array()
                .unwrap()
                .iter()
                .position(|mint| mint.as_str() == case["mint"].as_str())
                .unwrap();
            let account = &response["result"]["value"][mint_index];
            assert_eq!(account["owner"], current_mint["owner"]);
            assert_eq!(account["data"][0], current_mint["raw_account_base64"]);
            assert_eq!(account["data"][1], "base64");
            let certified_upper = ratio(
                case["certificate"]["fdv_upper_unrounded_cents"]
                    .as_str()
                    .unwrap(),
            );
            let certified_uncertainty = ratio(
                case["certificate"]["combined_relative_uncertainty_including_cent_rounding"]
                    .as_str()
                    .unwrap(),
            );
            assert_eq!(
                compare(
                    certified_upper,
                    ratio(
                        case["global_maxima"]["full_window_global_max_fdv_high"]
                            ["fdv_cents_unrounded"]
                            .as_str()
                            .unwrap()
                    )
                ),
                Ordering::Equal
            );
            let recomputed_uncertainty = (
                certified_upper.0 - U512::from(low_cents) * certified_upper.1,
                certified_upper.0,
            );
            assert_eq!(
                compare(recomputed_uncertainty, certified_uncertainty),
                Ordering::Equal
            );
            assert!(
                certified_uncertainty.0 * U512::from(100)
                    <= certified_uncertainty.1 * U512::from(2)
            );

            for (pre, binding) in curve_pre.iter().zip(bindings) {
                assert_eq!(pre["signature"], binding["signature"]);
                assert_eq!(pre["event_sha256"], binding["event_sha256"]);
                assert_eq!(binding["trade_roles"]["mint"], case["mint"]);
                assert!(
                    transfer_is_bound(binding),
                    "transfer role binding failed for {}",
                    binding["signature"]
                );
                let t = U512::from(pre["pre_virtual_token_raw"].as_u64().unwrap());
                let a = U512::from(pre["event_token_amount_raw"].as_u64().unwrap());
                let signed_delta = pre["actual_curve_native_delta_lamports"].as_i64().unwrap();
                let d = U512::from(signed_delta.unsigned_abs());
                let event_sol_amount =
                    U512::from(pre["event_sol_amount_lamports"].as_u64().unwrap());
                assert_eq!(d, event_sol_amount);
                assert_eq!(
                    signed_delta.is_positive(),
                    pre["side_of_trade"].as_str() == Some("buy")
                );
                let candidate = U512::from(
                    pre["candidate_previous_post_virtual_sol_lamports"]
                        .as_u64()
                        .unwrap(),
                );
                let low = U512::from(
                    pre["pre_virtual_sol_interval_low_lamports"]
                        .as_u64()
                        .unwrap(),
                );
                let high = U512::from(
                    pre["pre_virtual_sol_interval_high_lamports"]
                        .as_u64()
                        .unwrap(),
                );
                assert!(low <= candidate && candidate <= high);
                assert_eq!(
                    high - low + U512::from(1),
                    U512::from(
                        pre["pre_virtual_sol_interval_integer_count"]
                            .as_u64()
                            .unwrap()
                    )
                );
                let formula = pre["pre_virtual_sol_interval_formula"].as_str().unwrap();
                let (derived_low, derived_high) = match formula {
                    "buy_exact_token_out_cost_formula" => {
                        assert!(signed_delta > 0 && t > a);
                        (
                            ceil_div((d - U512::from(1)) * (t - a), a),
                            ceil_div(d * (t - a), a) - U512::from(1),
                        )
                    }
                    "sell_sdk" => {
                        assert!(signed_delta < 0);
                        (
                            ceil_div(d * (t + a), a),
                            ceil_div((d + U512::from(1)) * (t + a), a) - U512::from(1),
                        )
                    }
                    "exact_sol_in_net_minus_1" => {
                        assert!(signed_delta > 0);
                        (
                            ((d - U512::from(1)) * t) / (a + U512::from(1)) + U512::from(2) - d,
                            ((d - U512::from(1)) * t) / a + U512::from(1) - d,
                        )
                    }
                    _ => panic!("unexpected curve quote interval formula"),
                };
                assert_eq!((low, high), (derived_low, derived_high));
                let sdk_output = |q: U512| match formula {
                    "buy_exact_token_out_cost_formula" => a * q / (t - a) + U512::from(1),
                    "sell_sdk" => a * q / (t + a),
                    "exact_sol_in_net_minus_1" => (d - U512::from(1)) * t / (q + d - U512::from(1)),
                    _ => unreachable!(),
                };
                let expected_output = if formula == "exact_sol_in_net_minus_1" {
                    a
                } else {
                    d
                };
                assert_eq!(sdk_output(low), expected_output);
                assert_eq!(sdk_output(high), expected_output);
                assert_ne!(sdk_output(low - U512::from(1)), expected_output);
                assert_ne!(sdk_output(high + U512::from(1)), expected_output);
                assert!(pre["pre_virtual_sol_lamports"].is_null());
            }

            let sample = &case["raw_birth_create_and_first_trade_sample"];
            assert_eq!(sample["signature"], case["sample_binding_signature"]);
            assert!(
                sample["log_messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|line| { line.as_str().unwrap().contains("Instruction: CreateV2") })
            );
            assert!(
                sample["log_messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|line| { line.as_str().unwrap().contains("Instruction: Buy") })
            );
            assert!(
                sample["log_messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|line| { line.as_str().unwrap().contains("Program data:") })
            );
            for event_name in ["create_event", "trade_event"] {
                let event = &sample["event_payloads"][event_name];
                let event_bytes = bytes_from_hex(event["data_hex"].as_str().unwrap());
                assert_eq!(
                    format!("{:x}", Sha256::digest(&event_bytes)),
                    event["sha256"].as_str().unwrap()
                );
            }
            assert_eq!(
                sample["event_payloads"]["trade_event"]["sha256"],
                case["sample_binding"]["event_sha256"]
            );
            assert_eq!(
                &bytes_from_hex(
                    sample["event_payloads"]["create_event"]["data_hex"]
                        .as_str()
                        .unwrap()
                )[..8],
                &[27, 114, 169, 77, 222, 235, 99, 118]
            );
            assert!(
                !sample["pre_balances_lamports"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(
                !sample["post_balances_lamports"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(sample["pre_token_balances"].as_array().unwrap().is_empty());
            assert!(
                sample["post_token_balances"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|balance| {
                        balance["mint"].as_str() == case["mint"].as_str()
                            && balance["uiTokenAmount"]["decimals"].as_u64() == Some(6)
                    })
            );
        }
        assert_eq!(state_total, 1_715);
        assert_eq!(pre_total, 684);

        let mut bad_authority = fixture["cases"][0]["sample_binding"].clone();
        bad_authority["scoped_transfers"][0]["authority"] =
            serde_json::json!("11111111111111111111111111111111");
        assert!(!transfer_is_bound(&bad_authority));
        let mut bad_mint = fixture["cases"][0]["sample_binding"].clone();
        bad_mint["scoped_transfers"][0]["mint"] =
            serde_json::json!("So11111111111111111111111111111111111111112");
        assert!(!transfer_is_bound(&bad_mint));
        let mut bad_amount = fixture["cases"][0]["sample_binding"].clone();
        bad_amount["scoped_transfers"][0]["amount_raw"] = serde_json::json!(
            bad_amount["scoped_transfers"][0]["amount_raw"]
                .as_u64()
                .unwrap()
                + 1
        );
        assert!(!transfer_is_bound(&bad_amount));
        let mut out_of_scope = fixture["cases"][0]["sample_binding"].clone();
        out_of_scope["scoped_transfers"][0]["inner_index"] = out_of_scope["subtree_end"].clone();
        assert!(!transfer_is_bound(&out_of_scope));
        let mut wrong_cpi_height = fixture["cases"][0]["sample_binding"].clone();
        wrong_cpi_height["scoped_transfers"][0]["stack_height"] =
            wrong_cpi_height["trade_stack_height"].clone();
        assert!(!transfer_is_bound(&wrong_cpi_height));
        let mut bad_opcode = fixture["cases"][0]["sample_binding"].clone();
        bad_opcode["scoped_transfers"][0]["raw_hex"] = serde_json::Value::String(format!(
            "03{}",
            &bad_opcode["scoped_transfers"][0]["raw_hex"]
                .as_str()
                .unwrap()[2..]
        ));
        bad_opcode["scoped_transfers"][0]["opcode"] = serde_json::json!(3);
        assert!(!transfer_is_bound(&bad_opcode));

        let reset = fixture["cases"][0]["curve_pre_states"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["signature"].as_str().unwrap().starts_with("8NqJJo"))
            .unwrap();
        assert!(
            reset["candidate_previous_post_virtual_sol_lamports"]
                .as_u64()
                .unwrap()
                >= reset["pre_virtual_sol_interval_low_lamports"]
                    .as_u64()
                    .unwrap()
        );
        assert!(
            reset["candidate_previous_post_virtual_sol_lamports"]
                .as_u64()
                .unwrap()
                <= reset["pre_virtual_sol_interval_high_lamports"]
                    .as_u64()
                    .unwrap()
        );
        assert!(
            reset["pre_virtual_sol_interval_integer_count"]
                .as_u64()
                .unwrap()
                > 1
        );
        assert!(reset["pre_virtual_sol_lamports"].is_null());

        let flk = &fixture["cases"][1];
        let peak_price_state = flk["states"]
            .as_array()
            .unwrap()
            .iter()
            .max_by(|left, right| {
                compare(
                    ratio(left["price_usd_high"].as_str().unwrap()),
                    ratio(right["price_usd_high"].as_str().unwrap()),
                )
            })
            .unwrap();
        assert_eq!(peak_price_state["fdv_low_cents"].as_u64(), Some(373_427));
        assert_eq!(peak_price_state["fdv_high_cents"].as_u64(), Some(373_562));
        assert!(
            peak_price_state["fdv_high_cents"].as_u64().unwrap()
                < flk["global_maxima"]["full_window_global_max_fdv_high"]["fdv_cents"]
                    .as_u64()
                    .unwrap()
        );

        let vc95: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_vc95_chronological_fdv_window_case.json"
        ))
        .unwrap();
        assert_eq!(
            vc95["provenance"]["root_certificate_sha256"].as_str(),
            Some("bb8991d7c5d1687a3bc0feb374a33259a71c1e70b65c9bf0c1b946cba0e56c26")
        );
        assert_eq!(
            vc95["provenance"]["pump_sdk_1_31_tarball_sha256"].as_str(),
            Some("b12fdf292139f16bb2ff209a0bd57ac5e2b2168d687dc3d51a131cf45575ad18")
        );
        let idl_raw = vc95["pump_sdk_1_31_idl_json_raw"].as_str().unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(idl_raw.as_bytes())),
            "0ba934dd633279d22c33b8ed247e0c06accf81f5c91537d68c2d20b592aa0f57"
        );
        assert_eq!(
            vc95["provenance"]["pump_sdk_1_31_idl_sha256"].as_str(),
            Some("0ba934dd633279d22c33b8ed247e0c06accf81f5c91537d68c2d20b592aa0f57")
        );
        let idl: serde_json::Value = serde_json::from_str(idl_raw).unwrap();
        let option_bool = idl["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ty| ty["name"] == "OptionBool")
            .unwrap();
        assert_eq!(option_bool["type"]["kind"], "struct");
        assert_eq!(option_bool["type"]["fields"][0], "bool");
        for (name, argument_count, byte_count) in
            [("buy", 3, 25), ("buy_exact_sol_in", 3, 25), ("sell", 2, 24)]
        {
            let instruction = idl["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|instruction| instruction["name"] == name)
                .unwrap();
            assert_eq!(
                instruction["args"].as_array().unwrap().len(),
                argument_count
            );
            assert_eq!(instruction["args"][0]["type"], "u64");
            assert_eq!(instruction["args"][1]["type"], "u64");
            assert_eq!(
                instruction["args"][2]["name"].as_str(),
                (argument_count == 3).then_some("track_volume")
            );
            assert_eq!(8 + (2 * 8) + usize::from(argument_count == 3), byte_count);
        }
        let wire_audit_raw = vc95["wire_consistency_audit_raw"].as_str().unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(wire_audit_raw.as_bytes())),
            "9ae25d1eeeaf46e0ffda5acf38f37d858b196cb754559523b1b17914e62bd784"
        );
        let wire_audit: serde_json::Value = serde_json::from_str(wire_audit_raw).unwrap();
        assert_eq!(wire_audit["network_calls"], 0);
        assert_eq!(wire_audit["database_writes"], 0);
        assert_eq!(
            wire_audit["vc95"]["complete_dated_idl_argument_decodes"],
            390
        );
        assert_eq!(wire_audit["vc95"]["complete_event_cpi_trade_bindings"], 442);
        assert_eq!(wire_audit["vc95"]["appended_account_event_count"], 121);
        assert_eq!(wire_audit["vc95"]["appended_account_slot_count"], 172);
        assert_eq!(
            vc95["provenance"]["native_flow_audit_v2_sha256"].as_str(),
            Some("b73417d3d66a64ab8c5f512a3bac61d4c7fda792a9637cd1ad4eecd4f9e1f485")
        );
        assert_eq!(
            vc95["provenance"]["source_hashes"][".scout-data/diagnostics/solana-vC95-provenance-audit-2026-10-14/raw-provenance-audit-v2.json"],
            "b73417d3d66a64ab8c5f512a3bac61d4c7fda792a9637cd1ad4eecd4f9e1f485"
        );
        assert_eq!(vc95["scope"]["unresolved_extra_account_slots"], 172);
        let native_flow_scope = &vc95["native_flow_scope"];
        assert_eq!(native_flow_scope["buy_event_input_count"], 233);
        assert_eq!(native_flow_scope["sell_whole_transaction_delta_count"], 209);
        assert_eq!(native_flow_scope["buy_extra_curve_inflow_count"], 1);
        assert_eq!(native_flow_scope["sell_unrelated_curve_transfer_count"], 0);
        assert_eq!(
            native_flow_scope["buy_creation_rent_example"]["kind"],
            "create_account_rent"
        );
        assert_eq!(
            native_flow_scope["buy_creation_rent_example"]["lamports"],
            1_468_560
        );
        assert_eq!(
            native_flow_scope["buy_unknown_extra_inflow_example"]["kind"],
            "unresolved_extra_system_transfer_to_curve"
        );
        assert_eq!(
            native_flow_scope["buy_unknown_extra_inflow_example"]["signature"],
            "2FvT5wBwzjWwAnHduzF8BRyhptHqHVP21hgtXgpYvPNp1zko66NdwhKPzcido2EC4LtVw9zoL7yCzGnFH2EFQ636"
        );
        assert_eq!(
            native_flow_scope["buy_unknown_extra_inflow_example"]["event_input_lamports"],
            33_977_059
        );
        assert_eq!(
            native_flow_scope["buy_unknown_extra_inflow_example"]["lamports"],
            473_280
        );
        assert_eq!(
            native_flow_scope["buy_unknown_extra_inflow_example"]["whole_curve_balance_delta_lamports"],
            34_450_339
        );
        assert_eq!(vc95["scope"]["endpoint_rows"].as_u64(), Some(1_661));
        assert_eq!(vc95["scope"]["physical_price_marks"].as_u64(), Some(1_219));
        assert_eq!(vc95["scope"]["curve_pre_intervals"].as_u64(), Some(442));
        assert_eq!(vc95["fx_minutes"].as_array().unwrap().len(), 35);
        let vc95_case = &vc95["case"];
        assert_eq!(
            vc95_case["mint"].as_str(),
            Some("vC95xGRqaLWpYPkYrgMGtMqyFe8utiCjDoiMSKxpump")
        );
        assert_eq!(vc95_case["states"].as_array().unwrap().len(), 1_661);
        assert_eq!(vc95_case["curve_pre_states"].as_array().unwrap().len(), 442);
        assert_eq!(
            vc95_case["curve_trade_bindings"].as_array().unwrap().len(),
            442
        );
        assert_eq!(
            vc95_case["certificate"]["stored_lower_fdv_cents"].as_u64(),
            Some(49_712_596)
        );
        assert_eq!(
            vc95_case["certificate"]["upper_fdv_cents_ceiling"].as_u64(),
            Some(49_790_592)
        );
        assert_eq!(
            vc95_case["certificate"]["strict_birth"]["strict_allowlist_pass"],
            true
        );
        assert_eq!(
            vc95_case["certificate"]["current_mint"]["extension_set_exact"],
            true
        );
        assert_eq!(
            vc95_case["certificate"]["current_mint"]["snapshot_is_finalized"],
            true
        );
        assert_eq!(
            vc95_case["certificate"]["current_mint"]["raw_account_length"].as_u64(),
            Some(399)
        );

        let vc95_request_raw = vc95["current_mint_capture"]["request_json_raw"]
            .as_str()
            .unwrap();
        let vc95_response_raw = vc95["current_mint_capture"]["response_json_raw"]
            .as_str()
            .unwrap();
        let vc95_request_sha = format!("{:x}", Sha256::digest(vc95_request_raw.as_bytes()));
        let vc95_response_sha = format!("{:x}", Sha256::digest(vc95_response_raw.as_bytes()));
        let vc95_current = &vc95_case["certificate"]["current_mint"];
        assert_eq!(vc95_request_sha, vc95_current["snapshot_request_sha256"]);
        assert_eq!(vc95_response_sha, vc95_current["snapshot_response_sha256"]);
        let vc95_request: serde_json::Value = serde_json::from_str(vc95_request_raw).unwrap();
        let vc95_response: serde_json::Value = serde_json::from_str(vc95_response_raw).unwrap();
        assert_eq!(vc95_request["method"], "getMultipleAccounts");
        assert_eq!(vc95_request["params"][1]["commitment"], "finalized");
        assert_eq!(vc95_request["params"][1]["encoding"], "base64");
        assert_eq!(vc95_request["params"][0].as_array().unwrap().len(), 20);
        assert_eq!(
            vc95_response["result"]["context"]["slot"],
            vc95_current["snapshot_context_slot"]
        );
        assert_eq!(
            vc95_response["result"]["value"].as_array().unwrap().len(),
            20
        );
        let vc95_mint_index = vc95_request["params"][0]
            .as_array()
            .unwrap()
            .iter()
            .position(|mint| mint.as_str() == vc95_case["mint"].as_str())
            .unwrap();
        let vc95_account = &vc95_response["result"]["value"][vc95_mint_index];
        assert_eq!(vc95_account["owner"], vc95_current["owner"]);
        assert_eq!(vc95_account["data"][0], vc95_current["raw_account_base64"]);
        assert_eq!(vc95_account["data"][1], "base64");

        let vc95_manifest_raw = vc95["canonical_manifest_raw"].as_str().unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(vc95_manifest_raw.as_bytes())),
            vc95["canonical_manifest_sha256"]
        );
        let vc95_manifest: serde_json::Value = serde_json::from_str(vc95_manifest_raw).unwrap();
        assert_eq!(vc95_manifest["status"], "complete");
        assert_eq!(vc95_manifest["calls_used"].as_u64(), Some(3));
        assert!(
            vc95_manifest["calls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|call| {
                    call["http_status"].as_u64() == Some(200)
                        && call["matches_frozen_tfa_object"] == true
                })
        );

        let vc95_birth_raw = vc95_case["birth_transaction_response_raw"]
            .as_str()
            .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(vc95_birth_raw.as_bytes())),
            vc95_case["birth_transaction_response_sha256"]
        );
        let vc95_birth: serde_json::Value = serde_json::from_str(vc95_birth_raw).unwrap();
        let vc95_birth = &vc95_birth["result"];
        assert_eq!(
            vc95_birth["transaction"]["signatures"][0],
            vc95_case["birth_sample"]["signature"]
        );
        let vc95_birth_sample = &vc95_case["birth_sample"];
        assert!(
            vc95_birth_sample["log_messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("Instruction: CreateV2"))
        );
        assert!(
            vc95_birth_sample["log_messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("Instruction: Buy"))
        );
        for event_name in ["create_event", "trade_event"] {
            let payload = &vc95_birth_sample["event_payloads"][event_name];
            let bytes = bytes_from_hex(payload["data_hex"].as_str().unwrap());
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), payload["sha256"]);
        }
        assert_eq!(
            vc95_birth_sample["event_payloads"]["create_event"]["sha256"],
            "a3ef7c70ece0cf7bf1db4fe51cd8a25d681bb89d0b0d290dc8470a78f7b20f1f"
        );
        assert_eq!(
            vc95_birth_sample["event_payloads"]["trade_event"]["sha256"],
            "abc32de3644ca904d7a5abe77981067ed3e481574fb7b1f221cb539fb7ccbabb"
        );
        assert!(
            vc95_birth_sample["pre_token_balances"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            !vc95_birth_sample["post_token_balances"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let recovered = &vc95_case["recovered_event_cpi_gap"];
        let recovered_raw = recovered["response_raw"].as_str().unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(recovered_raw.as_bytes())),
            recovered["response_sha256"]
        );
        assert_eq!(recovered["audit"]["child_before_parent_boundary"], true);
        assert_eq!(recovered["audit"]["event_authority_match"], true);
        assert_eq!(
            recovered["audit"]["event_sha256"],
            "a269c53b999b56044bb42be2ef18929761a5ad9da5058e781f0fcd0d996f7666"
        );

        let mut vc95_state_total = 0_usize;
        let mut vc95_max_low: Option<(U512, U512)> = None;
        let mut vc95_max_high: Option<(U512, U512)> = None;
        let mut vc95_chronological = Vec::with_capacity(1_661);
        for state in vc95_case["states"].as_array().unwrap() {
            let minute = state["minute"].as_u64().unwrap();
            let fx = vc95["fx_minutes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["minute_unix_s"].as_u64() == Some(minute))
                .unwrap();
            let native = ratio(state["price_native"].as_str().unwrap());
            let fx_low = decimal_ratio(fx["low_usd_per_sol"].as_str().unwrap());
            let fx_high = decimal_ratio(fx["high_usd_per_sol"].as_str().unwrap());
            assert_eq!(
                compare(
                    (native.0 * fx_low.0, native.1 * fx_low.1),
                    ratio(state["price_usd_low"].as_str().unwrap())
                ),
                Ordering::Equal
            );
            assert_eq!(
                compare(
                    (native.0 * fx_high.0, native.1 * fx_high.1),
                    ratio(state["price_usd_high"].as_str().unwrap())
                ),
                Ordering::Equal
            );
            let low = fdv(
                ratio(state["price_usd_low"].as_str().unwrap()),
                U512::from(state["supply_low_raw"].as_u64().unwrap()),
            );
            let high = fdv(
                ratio(state["price_usd_high"].as_str().unwrap()),
                U512::from(state["supply_high_raw"].as_u64().unwrap()),
            );
            assert_eq!(
                low.0 / low.1,
                U512::from(state["fdv_low_cents"].as_u64().unwrap())
            );
            assert_eq!(
                ceil_div(high.0, high.1),
                U512::from(state["fdv_high_cents"].as_u64().unwrap())
            );
            if vc95_max_low.is_none_or(|current| compare(low, current).is_gt()) {
                vc95_max_low = Some(low);
            }
            if vc95_max_high.is_none_or(|current| compare(high, current).is_gt()) {
                vc95_max_high = Some(high);
            }
            let side_order = match state["side"].as_str().unwrap() {
                "create" => 0_u8,
                "pre" => 1_u8,
                "post" => 2_u8,
                _ => panic!("unexpected vC95 state side"),
            };
            vc95_chronological.push((
                state["slot"].as_u64().unwrap(),
                state["transaction_index"].as_u64().unwrap(),
                state["log_index"].as_u64().unwrap(),
                side_order,
            ));
            vc95_state_total += 1;
        }
        assert_eq!(vc95_state_total, 1_661);
        assert!(vc95_chronological.windows(2).all(|pair| pair[0] <= pair[1]));
        for (key, maximum, expected_cents) in [
            (
                "full_window_global_max_fdv_low",
                vc95_max_low.unwrap(),
                49_712_596,
            ),
            (
                "full_window_global_max_fdv_high",
                vc95_max_high.unwrap(),
                49_790_592,
            ),
        ] {
            let winner = &vc95_case["global_maxima"][key];
            assert_eq!(
                compare(
                    maximum,
                    ratio(winner["fdv_cents_unrounded"].as_str().unwrap())
                ),
                Ordering::Equal
            );
            assert_eq!(winner["fdv_cents"].as_u64(), Some(expected_cents));
            assert!(vc95_case["states"].as_array().unwrap().iter().any(|state| {
                state["signature"] == winner["signature"]
                    && state["side"] == winner["side"]
                    && state["log_index"] == winner["log_index"]
            }));
        }
        assert_eq!(
            compare(
                vc95_max_low.unwrap(),
                ratio(
                    vc95_case["certificate"]["fdv_lower_unrounded_cents"]
                        .as_str()
                        .unwrap()
                )
            ),
            Ordering::Equal
        );
        let vc95_upper = ratio(
            vc95_case["certificate"]["fdv_upper_unrounded_cents"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(compare(vc95_max_high.unwrap(), vc95_upper), Ordering::Equal);
        let vc95_uncertainty = ratio(
            vc95_case["certificate"]["combined_relative_uncertainty_including_cent_rounding"]
                .as_str()
                .unwrap(),
        );
        assert!(vc95_uncertainty.0 * U512::from(100) <= vc95_uncertainty.1 * U512::from(2));
        assert!(vc95_uncertainty.0 * U512::from(100_000) < vc95_uncertainty.1 * U512::from(157));

        let provider_price = decimal_ratio(
            vc95_case["provider_peak"]["provider_price_usd"]
                .as_str()
                .unwrap(),
        );
        let max_price_low = ratio(
            vc95_case["provider_peak"]["maximum_price_winner_low"]["usd_price_low_fraction"]
                .as_str()
                .unwrap(),
        );
        let max_price_high = ratio(
            vc95_case["provider_peak"]["maximum_price_winner_high"]["usd_price_high_fraction"]
                .as_str()
                .unwrap(),
        );
        let within_two_percent = |candidate: (U512, U512)| {
            let candidate_scaled = candidate.0 * provider_price.1;
            let provider_scaled = provider_price.0 * candidate.1;
            let delta = if candidate_scaled >= provider_scaled {
                candidate_scaled - provider_scaled
            } else {
                provider_scaled - candidate_scaled
            };
            delta * U512::from(100) <= provider_scaled * U512::from(2)
        };
        assert!(within_two_percent(max_price_low));
        assert!(within_two_percent(max_price_high));

        for (pre, binding) in vc95_case["curve_pre_states"]
            .as_array()
            .unwrap()
            .iter()
            .zip(vc95_case["curve_trade_bindings"].as_array().unwrap())
        {
            assert_eq!(pre["signature"], binding["signature"]);
            assert_eq!(pre["event_sha256"], binding["event_sha256"]);
            assert_eq!(binding["trade_roles"]["mint"], vc95_case["mint"]);
            assert!(
                transfer_is_bound(binding),
                "vC95 transfer binding failed for {}",
                binding["signature"]
            );
            let t = U512::from(pre["pre_virtual_token_raw"].as_u64().unwrap());
            let a = U512::from(pre["event_token_amount_raw"].as_u64().unwrap());
            let signed_delta = pre["actual_curve_native_delta_lamports"].as_i64().unwrap();
            let d = U512::from(signed_delta.unsigned_abs());
            assert_eq!(
                pre["native_movement_measurement_kind"],
                if pre["side_of_trade"] == "buy" {
                    "scoped_system_event_input"
                } else {
                    "whole_transaction_curve_balance_delta"
                }
            );
            assert_eq!(
                d,
                U512::from(pre["event_sol_amount_lamports"].as_u64().unwrap())
            );
            let candidate = U512::from(
                pre["candidate_previous_post_virtual_sol_lamports"]
                    .as_u64()
                    .unwrap(),
            );
            let low = U512::from(
                pre["pre_virtual_sol_interval_low_lamports"]
                    .as_u64()
                    .unwrap(),
            );
            let high = U512::from(
                pre["pre_virtual_sol_interval_high_lamports"]
                    .as_u64()
                    .unwrap(),
            );
            assert!(low <= candidate && candidate <= high);
            assert_eq!(
                high - low + U512::from(1),
                U512::from(
                    pre["pre_virtual_sol_interval_integer_count"]
                        .as_u64()
                        .unwrap()
                )
            );
            let formula = pre["pre_virtual_sol_interval_formula"].as_str().unwrap();
            let (derived_low, derived_high) = match formula {
                "buy_exact_token_out_cost_formula" => {
                    assert!(signed_delta > 0 && t > a);
                    (
                        ceil_div((d - U512::from(1)) * (t - a), a),
                        ceil_div(d * (t - a), a) - U512::from(1),
                    )
                }
                "sell_sdk" => {
                    assert!(signed_delta < 0);
                    (
                        ceil_div(d * (t + a), a),
                        ceil_div((d + U512::from(1)) * (t + a), a) - U512::from(1),
                    )
                }
                "exact_sol_in_net_minus_1" => {
                    assert!(signed_delta > 0);
                    (
                        ((d - U512::from(1)) * t) / (a + U512::from(1)) + U512::from(2) - d,
                        ((d - U512::from(1)) * t) / a + U512::from(1) - d,
                    )
                }
                _ => panic!("unexpected vC95 curve interval formula"),
            };
            assert_eq!((low, high), (derived_low, derived_high));
            let sdk_output = |q: U512| match formula {
                "buy_exact_token_out_cost_formula" => a * q / (t - a) + U512::from(1),
                "sell_sdk" => a * q / (t + a),
                "exact_sol_in_net_minus_1" => (d - U512::from(1)) * t / (q + d - U512::from(1)),
                _ => unreachable!(),
            };
            let output = if formula == "exact_sol_in_net_minus_1" {
                a
            } else {
                d
            };
            assert_eq!(sdk_output(low), output);
            assert_eq!(sdk_output(high), output);
            assert_ne!(sdk_output(low - U512::from(1)), output);
            assert_ne!(sdk_output(high + U512::from(1)), output);
            assert!(pre["pre_virtual_sol_lamports"].is_null());

            let shape = &pre["instruction_shape"];
            let actual_data = bytes_from_hex(
                binding["matched_trade_instruction"]["data_hex"]
                    .as_str()
                    .unwrap(),
            );
            assert_eq!(
                actual_data.len(),
                usize::try_from(shape["data_bytes"].as_u64().unwrap()).unwrap()
            );
            let instruction_name = pre["instruction_name"].as_str().unwrap();
            let has_track_volume = matches!(instruction_name, "buy" | "buy_exact_sol_in");
            if has_track_volume && actual_data.len() == 25 {
                assert!(matches!(actual_data[24], 0 | 1));
                assert_eq!(shape["track_volume_bool"], actual_data[24] == 1);
                assert_eq!(shape["legacy_wire_gap_missing_track_volume"], false);
            } else if has_track_volume {
                assert_eq!(actual_data.len(), 24);
                assert_eq!(shape["track_volume_bool"], serde_json::Value::Null);
                assert_eq!(shape["legacy_wire_gap_missing_track_volume"], true);
            } else {
                assert_eq!(instruction_name, "sell");
                assert_eq!(actual_data.len(), 24);
                assert_eq!(shape["track_volume_bool"], serde_json::Value::Null);
                assert_eq!(shape["legacy_wire_gap_missing_track_volume"], false);
            }
            let accounts = binding["matched_trade_instruction"]["accounts"]
                .as_array()
                .unwrap();
            let account_count = u64::try_from(accounts.len()).unwrap();
            assert_eq!(account_count, shape["account_count"].as_u64().unwrap());
            let idl_accounts = shape["dated_idl_account_count"].as_u64().unwrap();
            assert_eq!(
                shape["unresolved_extra_account_count"].as_u64().unwrap(),
                account_count.saturating_sub(idl_accounts)
            );
        }
        let mut vc95_bad_authority = vc95_case["curve_trade_bindings"][0].clone();
        vc95_bad_authority["scoped_transfers"][0]["authority"] =
            serde_json::json!("11111111111111111111111111111111");
        assert!(!transfer_is_bound(&vc95_bad_authority));
        let mut vc95_bad_mint = vc95_case["curve_trade_bindings"][0].clone();
        vc95_bad_mint["scoped_transfers"][0]["mint"] =
            serde_json::json!("So11111111111111111111111111111111111111112");
        assert!(!transfer_is_bound(&vc95_bad_mint));
        let mut vc95_out_of_scope = vc95_case["curve_trade_bindings"][0].clone();
        vc95_out_of_scope["scoped_transfers"][0]["inner_index"] =
            vc95_out_of_scope["subtree_end"].clone();
        assert!(!transfer_is_bound(&vc95_out_of_scope));
    }

    #[test]
    fn solana_tracker_qbh_nontrade_and_huvd_truncated_cpi_gaps_replay() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_qbh_huvd_metadata_cpi_cases_v1.json"
        ))
        .unwrap();
        let hash = |raw: &str| format!("{:x}", Sha256::digest(raw.as_bytes()));
        macro_rules! check {
            ($actual:expr, $expected:expr $(,)?) => {
                assert_eq!($actual, $expected)
            };
        }
        let qbh_raw = fixture["qbh_proof_json_raw"].as_str().unwrap();
        let huvd_raw = fixture["huvd_proof_json_raw"].as_str().unwrap();
        let audit_raw = fixture["rawgate_audit_json_raw"].as_str().unwrap();
        check!(hash(qbh_raw), fixture["source_sha256"]["qbh_proof"]);
        check!(hash(huvd_raw), fixture["source_sha256"]["huvd_proof"]);
        check!(hash(audit_raw), fixture["source_sha256"]["rawgate_audit"]);
        let cert_raw = fixture["root_certificate_json_raw"].as_str().unwrap();
        let all_qbh_raw = fixture["qbh_all_114_json_raw"].as_str().unwrap();
        let correction_raw = fixture["provenance_correction_json_raw"].as_str().unwrap();
        check!(hash(cert_raw), fixture["source_sha256"]["root_certificate"]);
        check!(hash(all_qbh_raw), fixture["source_sha256"]["qbh_all_114"]);
        check!(
            hash(correction_raw),
            fixture["source_sha256"]["provenance_correction"]
        );
        check!(
            fixture["source_sha256"]["root_certificate"],
            "14f855e2c82e11c75d1455d93ac9b2d554d6e2109f3c0ea22439e596e185e785"
        );
        check!(
            fixture["source_sha256"]["qbh_all_114"],
            "0247fd7edd68b3649b7ab06677ec65d38d934b371f827dd5f88dc4e17a5a01ac"
        );
        check!(
            fixture["source_sha256"]["provenance_correction"],
            "ae14409bb6de34fb71e3c1a42bb36aec4aaf9e1a1fa6f5467897b870433020b2"
        );
        let certificate: serde_json::Value = serde_json::from_str(cert_raw).unwrap();
        let qbh_all: serde_json::Value = serde_json::from_str(all_qbh_raw).unwrap();
        let correction: serde_json::Value = serde_json::from_str(correction_raw).unwrap();
        check!(
            serde_json::json!([
                correction["original_v1_announced_sha256"],
                correction["original_body_available"],
                correction["v1_overwritten_before_root_capture"],
                correction["final_body_sha256"],
                correction["new_numeric_claims"],
            ]),
            serde_json::json!([
                "359c0fa0ebf2fdc9c76b5c02f732a49dfd51aaf33f3bfa6dd8b1775d7dc5616c",
                false,
                true,
                "435ba3d19e87d932a222b8c0b9cf49b8d8ddeac4f82686e57627ca6daa05b62e",
                false,
            ])
        );
        let (qbh, huvd, audit): (serde_json::Value, serde_json::Value, serde_json::Value) = (
            serde_json::from_str(qbh_raw).unwrap(),
            serde_json::from_str(huvd_raw).unwrap(),
            serde_json::from_str(audit_raw).unwrap(),
        );
        check!(
            serde_json::json!([
                fixture["source_kinds"]["qbh"],
                fixture["source_kinds"]["huvd"]
            ]),
            serde_json::json!([
                "onchain-pool-window-bounded-supply-cex-reference-v1",
                "onchain-pool-window-supply-cex-reference-v1",
            ]),
        );
        let nontrade = &qbh["evidence"]["nontrade_event_log"];
        let event_cpi = &qbh["evidence"]["event_cpi"];
        let close_ix = &qbh["evidence"]["pump_swap_close_instruction"];
        check!(
            serde_json::json!([
                qbh["schema"],
                qbh["target_mint"],
                qbh["event_transaction"]["meta_err"],
                nontrade["bytes"],
                nontrade["field_bytes_consumed"],
                nontrade["discriminator_hex"],
                nontrade["sha256"],
                nontrade["decoded_fields"]["timestamp"],
                nontrade["decoded_fields"]["user"],
                event_cpi["full_data_bytes"],
                event_cpi["program"],
                event_cpi["outer_index"],
                event_cpi["inner_index"],
                event_cpi["wrapper_discriminator_hex"],
                event_cpi["matches_log_payload"],
                close_ix["discriminator_hex"],
                close_ix["inner_index"],
                close_ix["outer_index"],
                qbh["evidence"]["log_context"]["trade_buy_event_log_index"],
                qbh["evidence"]["log_context"]["nontrade_payload_log_index"],
                qbh["evidence"]["trade_event_log"]["type"],
                qbh["price_gate_reuse"]["existing_decoded_price_states"],
                qbh["price_gate_reuse"]["existing_decoded_selected_pool_events"],
                qbh["price_gate_reuse"]["existing_gap_event_is_separate_from_pool_buy_event"],
                qbh["price_gate_reuse"]["supply_gate"],
            ]),
            serde_json::json!([
                "qbh-nontrade-event-resolution-v1",
                "qbhDFrtEsfpn3sWapy3S8RnVKdH2cBKv4Yr2fMRpump",
                null,
                80,
                80,
                "929fbdac925838f4",
                "f4ecaecd0c823c8fe416996ae572f6607fc77e8a1a1b14f67873366782079342",
                1770535306,
                "AgmLJBMDCqWynYnQiPCuj9ewsNNsBJXyzoUhD9LJzN51",
                88,
                "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
                3,
                23,
                "e445a52e51cb9a1d",
                true,
                "f945a4da9667548a",
                22,
                3,
                95,
                103,
                "BuyEvent",
                114,
                57,
                true,
                "not closed by this audit; no FDV claim",
            ]),
        );
        check!(
            serde_json::json!([
                qbh["evidence"]["dated_idl"]["sha256"],
                qbh["evidence"]["dated_idl"]["event_name"],
                qbh["evidence"]["dated_idl"]["event_discriminator_hex"],
                qbh["evidence"]["log_context"]["close_instruction_log"],
            ]),
            serde_json::json!([
                "773a430d6727446721c0f5028710e65484643fbf9c0cac50f0ba117594e5514b",
                "CloseUserVolumeAccumulatorEvent",
                "929fbdac925838f4",
                "Program log: Instruction: CloseUserVolumeAccumulator",
            ]),
        );
        check!(
            serde_json::json!([
                huvd["schema"],
                huvd["status"],
                huvd["scope"],
                huvd["combined_state_count"],
                huvd["saved_states_count"],
                huvd["new_states"].as_array().unwrap().len(),
                huvd["comparison_status"],
                huvd["low_within_2pct"],
                huvd["high_within_2pct"],
                huvd["canonical_recapture"]["canonical_equals_TFA_full_object"],
                huvd["canonical_recapture"]["log_truncated_marker"],
                huvd["canonical_recapture"]["response_sha256"],
                huvd["event_recovery"]["event_name"],
                huvd["event_recovery"]["event_payload_bytes"],
                huvd["event_recovery"]["exact_payload_cpi_matches"],
                huvd["event_recovery"]["cpi_outer_index"],
                huvd["event_recovery"]["cpi_inner_index"],
                huvd["event_recovery"]["trade_outer_index"],
                huvd["event_recovery"]["trade_inner_index"],
                huvd["event_recovery"]["trade_instruction_count_in_tx"],
                huvd["event_recovery"]["trade_accounts_pool_base_quote_vault_match"],
                huvd["vault_binding"]["event_pre_reserves_match"],
                huvd["vault_binding"]["pre_base_raw"],
                huvd["vault_binding"]["pre_quote_raw"],
                huvd["vault_binding"]["post_base_raw"],
                huvd["vault_binding"]["post_quote_raw"],
                huvd["global_maxima"]["usd_low_fraction"]["fraction"],
                huvd["global_maxima"]["usd_high_fraction"]["fraction"],
            ]),
            serde_json::json!([
                "huvd-canonical-event-cpi-log-truncation-recovery-price-proof-v1",
                "recovered_selected_event_context_candidate",
                "Selected pool, all 45 previously verified events plus this single recovered event, exact captured ±3-second window only; not all-pools lifetime ATH.",
                92,
                90,
                2,
                "both_endpoints_within_2pct",
                true,
                true,
                true,
                true,
                "3d02c10ecd46889452cdff87762ce50b9f909837c91f5e017c18691862fb63ef",
                "BuyEvent",
                439,
                1,
                6,
                20,
                6,
                14,
                1,
                true,
                true,
                "196656575562011",
                "89514001595",
                "196293514766293",
                "89679896138",
                "59476247547849/1358344002846350000",
                "119203672847067/2716688005692700000",
            ]),
        );
        let abi_gap = &audit["unparsed_abi_gaps"][0];
        check!(
            serde_json::json!([
                audit["schema"],
                audit["rawgate_pass"],
                audit["all_pass"],
                audit["huvd"]["historical_virtual_quote_reserve"]["status"],
                audit["huvd"]["historical_virtual_quote_reserve"]["used_in_recovered_marks"],
                audit["huvd"]["trade_instruction_abi_observation"]["wire_bytes"],
                audit["huvd"]["trade_instruction_abi_observation"]["dated_idl_declared_extra_option_bool_bytes"],
                audit["checks"]["huvd_idl_declared_trade_args_wire_length_consistent"],
                abi_gap["captured_wire_bytes"],
                abi_gap["idl_declared_bytes"],
                abi_gap["fatal_to_observed_vault_marks"],
                abi_gap["status"],
                audit["source_sha256"]["qbh_proof"],
                audit["source_sha256"]["huvd_proof"],
            ]),
            serde_json::json!([
                "solana-qbh-huvd-independent-rawgate-audit-v1",
                true,
                false,
                "unknown",
                false,
                24,
                1,
                false,
                24,
                25,
                false,
                "unparsed ABI gap; no bool/default inferred",
                fixture["source_sha256"]["qbh_proof"],
                fixture["source_sha256"]["huvd_proof"],
            ]),
        );
        assert!(
            audit["checks"]
                .as_object()
                .unwrap()
                .iter()
                .all(|(name, value)| {
                    if name == "huvd_idl_declared_trade_args_wire_length_consistent" {
                        value == false
                    } else {
                        value == true
                    }
                })
        );
        assert!(
            audit["checks"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(name, _)| name.starts_with("qbh_"))
                .all(|(_, value)| value == true)
        );
        assert!(
            qbh["price_gate_reuse"]["supply_gate"]
                .as_str()
                .unwrap()
                .contains("no FDV claim")
        );
        {
            let raw = fixture["qbh_bounded_fdv_json_raw"].as_str().unwrap();
            check!(hash(raw), fixture["source_sha256"]["qbh_bounded_fdv"]);
            check!(
                fixture["source_sha256"]["qbh_bounded_fdv"],
                "e545116e191cfcd3bd369d3d57ab0439470578b1610ede766753ed7d6b7e9b0a"
            );
            let bound: serde_json::Value = serde_json::from_str(raw).unwrap();
            let fraction = |value: &str| {
                let (numerator, denominator) = value.split_once('/').unwrap();
                (
                    U512::from_str_radix(numerator, 10).unwrap(),
                    U512::from_str_radix(denominator, 10).unwrap(),
                )
            };
            let compare = |left: (U512, U512), right: (U512, U512)| {
                (left.0 * right.1).cmp(&(right.0 * left.1))
            };
            let ceil = |n: U512, d: U512| (n + d - U512::from(1)) / d;
            let supply = &bound["supply_bounds"];
            let fdv = &bound["fdv_bound"];
            let low_price = fraction(
                bound["price_window"]["extrema"]["low"]["usd_fraction"]
                    .as_str()
                    .unwrap(),
            );
            let high_price = fraction(
                bound["price_window"]["extrema"]["high"]["usd_fraction"]
                    .as_str()
                    .unwrap(),
            );
            let scale = U512::from(1_000_000_u64);
            let low = (
                low_price.0
                    * U512::from_str_radix(
                        supply["current_later_snapshot_supply_raw"]
                            .as_str()
                            .unwrap(),
                        10,
                    )
                    .unwrap(),
                low_price.1 * scale,
            );
            let high = (
                high_price.0
                    * U512::from_str_radix(supply["birth_supply_raw"].as_str().unwrap(), 10)
                        .unwrap(),
                high_price.1 * scale,
            );
            assert_eq!(
                compare(low, fraction(fdv["lower_usd_fraction"].as_str().unwrap())),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                compare(high, fraction(fdv["upper_usd_fraction"].as_str().unwrap())),
                std::cmp::Ordering::Equal
            );
            let width = (high.0 * low.1 - low.0 * high.1, high.0 * low.1);
            assert_eq!(
                compare(
                    width,
                    fraction(fdv["width_fraction_of_upper"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            assert!(compare(width, (U512::from(1), U512::from(50))).is_le());
            check!(
                serde_json::json!([
                    bound["schema"],
                    bound["mint"],
                    bound["status"],
                    bound["price_window"]["states_count"],
                    bound["price_window"]["selected_pool_event_count"],
                    bound["price_window"]["scope"],
                    supply["historical_supply_exact"],
                    supply["birth_supply_raw"],
                    supply["current_later_snapshot_supply_raw"],
                    supply["birth"]["successful_receipt_in_raw_batch"],
                    supply["birth"]["birth_mint_to_sum_raw"],
                    supply["birth"]["mint_authority_revocation"]["new_authority_option"],
                    supply["current_snapshot"]["requested_address_index"],
                    supply["current_snapshot"]["context_slot"],
                    supply["current_snapshot"]["extension_ids_and_lengths"],
                    fdv["lower_cents_floor"],
                    fdv["upper_cents_ceil"],
                    fdv["within_two_percent"],
                    fdv["width_percent_of_upper"],
                ]),
                serde_json::json!([
                    "solana-qbh-conservative-observed-window-fdv-supply-bound-v1",
                    "qbhDFrtEsfpn3sWapy3S8RnVKdH2cBKv4Yr2fMRpump",
                    "bounded_observed_selected_pool_window",
                    114,
                    57,
                    "Verified selected-pool pre/post states in the captured provider-peak ±3-second window only; not lifetime ATH or all-pool coverage. The formerly unsupported 80-byte event is separately classified as nontrade.",
                    false,
                    "1000000000000000",
                    "998812646101067",
                    true,
                    "1000000000000000",
                    0,
                    98,
                    455686275,
                    [[18, 64], [19, 169]],
                    3815036,
                    3826187,
                    true,
                    "0.291400939",
                ]),
            );
            assert_eq!(
                low.0 * U512::from(100) / low.1,
                U512::from(fdv["lower_cents_floor"].as_u64().unwrap())
            );
            assert_eq!(
                ceil(high.0 * U512::from(100), high.1),
                U512::from(fdv["upper_cents_ceil"].as_u64().unwrap())
            );
            check!(
                serde_json::json!([
                    qbh_all["schema"],
                    qbh_all["mint"],
                    qbh_all["source"],
                    qbh_all["marks"].as_array().unwrap().len(),
                    qbh_all["supply_lower_raw"],
                    qbh_all["supply_upper_raw"],
                    qbh_all["historical_supply_exact"],
                    qbh_all["unknown_historical_virtual_quote_reserves"],
                    qbh_all["annual_allmarket_lifetime_complete"],
                ]),
                serde_json::json!([
                    "root-qbh-all-114-observed-marks-bounded-supply-v1",
                    "qbhDFrtEsfpn3sWapy3S8RnVKdH2cBKv4Yr2fMRpump",
                    "onchain-pool-window-bounded-supply-cex-reference-v1",
                    114,
                    "998812646101067",
                    "1000000000000000",
                    false,
                    true,
                    false,
                ])
            );
            let certificate_qbh = &certificate["rows"][1];
            check!(
                serde_json::json!([
                    certificate["schema"],
                    certificate["rawgate_pass"],
                    certificate["observed_marks"],
                    certificate_qbh["source"],
                    certificate_qbh["observed_marks"],
                    certificate_qbh["lower_fdv_cents"],
                    certificate_qbh["upper_fdv_cents_ceiling"],
                    certificate_qbh["historical_supply_exact"],
                    certificate_qbh["conditional_vault_ratio_model"],
                    certificate_qbh["historical_virtual_quote_reserves"],
                ]),
                serde_json::json!([
                    "root-qbh-huvd-206-observed-marks-certificate-v1",
                    true,
                    206,
                    qbh_all["source"],
                    114,
                    3815036,
                    3826187,
                    false,
                    true,
                    "unknown",
                ])
            );
            for (path, expected_hash) in [
                (
                    ".scout-data/diagnostics/solana-qbh-conservative-fdv-bound-2026-10-14/root-all-114-state-replay-v1.json",
                    "0247fd7edd68b3649b7ab06677ec65d38d934b371f827dd5f88dc4e17a5a01ac",
                ),
                (
                    ".scout-data/diagnostics/solana-qbh-conservative-fdv-bound-2026-10-14/independent-raw-bound-audit-v3.json",
                    "b5a2550daacae56d18a74fd97b9aaf68cac54f8c7af735a448de98d212d5bd6f",
                ),
            ] {
                check!(certificate["source_sha256"][path], expected_hash);
            }
            let lower_supply =
                U512::from_str_radix(qbh_all["supply_lower_raw"].as_str().unwrap(), 10).unwrap();
            let upper_supply =
                U512::from_str_radix(qbh_all["supply_upper_raw"].as_str().unwrap(), 10).unwrap();
            let scale = U512::from(1_000_000_u64);
            let mut max_low: Option<(U512, U512)> = None;
            let mut max_high: Option<(U512, U512)> = None;
            for mark in qbh_all["marks"].as_array().unwrap() {
                assert_eq!(mark["event_cpi_matched"], true);
                let low_price = fraction(mark["price_low_fraction"].as_str().unwrap());
                let high_price = fraction(mark["price_high_fraction"].as_str().unwrap());
                let low_fdv = (low_price.0 * lower_supply, low_price.1 * scale);
                let high_fdv = (high_price.0 * upper_supply, high_price.1 * scale);
                assert_eq!(
                    compare(
                        low_fdv,
                        fraction(mark["fdv_lower_usd_fraction"].as_str().unwrap())
                    ),
                    std::cmp::Ordering::Equal
                );
                assert_eq!(
                    compare(
                        high_fdv,
                        fraction(mark["fdv_upper_usd_fraction"].as_str().unwrap())
                    ),
                    std::cmp::Ordering::Equal
                );
                if max_low.is_none_or(|previous| compare(low_fdv, previous).is_gt()) {
                    max_low = Some(low_fdv);
                }
                if max_high.is_none_or(|previous| compare(high_fdv, previous).is_gt()) {
                    max_high = Some(high_fdv);
                }
            }
            let max_low = max_low.unwrap();
            let max_high = max_high.unwrap();
            assert_eq!(
                compare(
                    max_low,
                    fraction(qbh_all["global_lower_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                compare(
                    max_high,
                    fraction(qbh_all["global_upper_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                max_low.0 * U512::from(100) / max_low.1,
                U512::from(qbh_all["lower_fdv_cents"].as_u64().unwrap())
            );
            assert_eq!(
                ceil(max_high.0 * U512::from(100), max_high.1),
                U512::from(qbh_all["upper_fdv_cents_ceiling"].as_u64().unwrap())
            );
            let width = (
                max_high.0 * max_low.1 - max_low.0 * max_high.1,
                max_high.0 * max_low.1,
            );
            assert!(compare(width, (U512::from(1), U512::from(50))).is_le());
            assert_eq!(
                compare(
                    max_low,
                    fraction(certificate_qbh["lower_usd_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                compare(
                    max_high,
                    fraction(certificate_qbh["upper_usd_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
        }
        assert!(
            huvd["remaining_caveats"]
                .as_array()
                .unwrap()
                .iter()
                .any(|caveat| {
                    caveat
                        .as_str()
                        .unwrap()
                        .contains("No supply history or FDV")
                })
        );
        let huvd_supply_raw = fixture["huvd_supply_v3_json_raw"].as_str().unwrap();
        let supply_case: serde_json::Value = serde_json::from_str(huvd_supply_raw).unwrap();
        check!(
            supply_case["source_sha256"],
            fixture["source_sha256"]["huvd_supply_v3"]
        );
        check!(
            fixture["source_sha256"]["huvd_supply_v3"],
            "987c2f979a65d832ddf89efa25b70d53c5cfa0ae4c3cceb24a2a33bc57a68a2b"
        );
        let supply = &supply_case["supply"];
        let raw_u512 =
            |value: &serde_json::Value| U512::from_str_radix(value.as_str().unwrap(), 10).unwrap();
        let fraction = |value: &str| {
            let (numerator, denominator) = value.split_once('/').unwrap();
            (
                U512::from_str_radix(numerator, 10).unwrap(),
                U512::from_str_radix(denominator, 10).unwrap(),
            )
        };
        let compare_fraction =
            |left: (U512, U512), right: (U512, U512)| (left.0 * right.1).cmp(&(right.0 * left.1));
        check!(
            serde_json::json!([
                supply_case["schema"],
                supply_case["mint"],
                supply_case["observed_price_mark_count"],
                supply["decimals"],
                supply["initial_supply_raw"],
                supply["mint_sum_raw_in_window"],
                supply["burn_sum_raw_in_window"],
                supply["window_end_supply_raw"],
                supply["birth_supply_reconciles_to_window_end"],
                supply["current_snapshot_supply_raw"],
                supply["current_supply_after_later_time_differs_from_window_end_raw"],
                supply_case["quality_gaps"]["unknown_token_opcode_count"],
                supply_case["quality_gaps"]["malformed_token_instruction_count"],
                supply_case["quality_gaps"]["same_transaction_supply_change_mark_count"],
                supply["history_completeness"]["complete_for_supply_at_each_observed_window_transaction"],
                supply["history_completeness"]["not_full_history_through_current_account_snapshot"],
                supply_case["price_window"]["scope"],
            ]),
            serde_json::json!([
                "solana-huvd-window-supply-and-fdv-replay-v3",
                "HUvdhx4exbZK9Rp2rfwGvcL6bTPTuYeQqKFER6b5X3aF",
                92,
                6,
                "1000000000000000",
                "1000000000000000",
                "6175438944166",
                "993824561055834",
                true,
                "861553990334431",
                "-132270570721403",
                0,
                0,
                0,
                true,
                true,
                "all 45 previous decoded selected-pool events plus one canonical-recovered event; all 46 events and their 92 pre/post states are a captured ±3s neighborhood only, not full life / all pools",
            ]),
        );
        let mint = &supply["mint_instructions"][0];
        let initialize = &supply["initialization_instructions_in_birth_tx"][0];
        let revoke = &supply["mint_authority_changes"][0];
        check!(
            serde_json::json!([
                supply["mint_instructions"].as_array().unwrap().len(),
                mint["opcode"],
                mint["outer_index"],
                mint["inner_index"],
                mint["amount_raw"],
                initialize["opcode"],
                initialize["decimals_in_instruction"],
                revoke["new_authority_option"],
                revoke["outer_index"],
                revoke["inner_index"],
                supply["current_account_raw_binding"]["raw_supply_raw"],
                supply["current_account_raw_binding"]["response_context_slot"],
            ]),
            serde_json::json!([
                1,
                7,
                3,
                12,
                "1000000000000000",
                20,
                6,
                0,
                3,
                13,
                "861553990334431",
                455686275,
            ]),
        );
        let burn_total = supply["burn_instructions"]
            .as_array()
            .unwrap()
            .iter()
            .fold(U512::from(0), |sum, burn| {
                sum + raw_u512(&burn["amount_raw"])
            });
        assert_eq!(burn_total, raw_u512(&supply["burn_sum_raw_in_window"]));
        assert_eq!(
            raw_u512(&supply["initial_supply_raw"]) - burn_total,
            raw_u512(&supply["window_end_supply_raw"])
        );

        let state_supply = raw_u512(&supply["window_end_supply_raw"]);
        let one_million = U512::from(1_000_000_u64);
        let endpoints = ["low", "close", "high"];
        type FdvMaximum = ((U512, U512), String, String);
        let mut maxima: Vec<Option<FdvMaximum>> = vec![None; 3];
        let marks = supply_case["marks"].as_array().unwrap();
        assert_eq!(marks.len(), 92);
        let mut recovered = 0;
        for mark in marks {
            check!(
                mark["supply_raw_at_transaction_boundary"],
                "993824561055834"
            );
            assert!(
                mark["supply_operations_in_transaction"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            if mark["recovered_canonical_event"] == true {
                recovered += 1;
            }
            for (index, endpoint) in endpoints.iter().enumerate() {
                let price = fraction(
                    mark[&format!("price_{endpoint}_fraction")]
                        .as_str()
                        .unwrap(),
                );
                let fdv = (price.0 * state_supply, price.1 * one_million);
                let stored = fraction(
                    mark[&format!("fdv_{endpoint}_usd_fraction")]
                        .as_str()
                        .unwrap(),
                );
                assert_eq!(compare_fraction(fdv, stored), std::cmp::Ordering::Equal);
                assert_eq!(
                    fdv.0 * U512::from(100) / fdv.1,
                    U512::from(
                        mark[&format!("fdv_{endpoint}_cents_floor")]
                            .as_u64()
                            .unwrap()
                    )
                );
                if maxima[index]
                    .as_ref()
                    .is_none_or(|(previous, _, _)| compare_fraction(fdv, *previous).is_gt())
                {
                    maxima[index] = Some((
                        fdv,
                        mark["signature"].as_str().unwrap().to_owned(),
                        mark["state_side"].as_str().unwrap().to_owned(),
                    ));
                }
            }
        }
        assert_eq!(recovered, 2);
        for (index, endpoint) in endpoints.iter().enumerate() {
            let (actual, signature, side) = maxima[index].as_ref().unwrap();
            let expected = &supply_case["global_window_fdv_maxima"][endpoint];
            assert_eq!(
                compare_fraction(
                    *actual,
                    fraction(expected["fdv_usd_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            check!(
                serde_json::json!([signature, side]),
                serde_json::json!([expected["signature"], expected["state_side"]]),
            );
            assert_eq!(
                actual.0 * U512::from(100) / actual.1,
                U512::from(expected["fdv_cents_floor"].as_u64().unwrap())
            );
        }
        let certificate_huvd = &certificate["rows"][0];
        let huvd_high = maxima[2].as_ref().unwrap().0;
        check!(
            serde_json::json!([
                certificate_huvd["mint"],
                certificate_huvd["source"],
                certificate_huvd["observed_marks"],
                certificate_huvd["lower_fdv_cents"],
                certificate_huvd["upper_fdv_cents_ceiling"],
                certificate_huvd["historical_supply_exact"],
                certificate_huvd["conditional_vault_ratio_model"],
                certificate_huvd["historical_virtual_quote_reserves"],
                certificate["rawgate_pass"],
            ]),
            serde_json::json!([
                "HUvdhx4exbZK9Rp2rfwGvcL6bTPTuYeQqKFER6b5X3aF",
                "onchain-pool-window-supply-cex-reference-v1",
                92,
                4351545,
                4360735,
                true,
                true,
                "unknown",
                true,
            ])
        );
        assert_eq!(
            compare_fraction(
                huvd_high,
                fraction(certificate_huvd["upper_usd_fraction"].as_str().unwrap())
            ),
            std::cmp::Ordering::Equal
        );
        let huvd_high_ceil =
            (huvd_high.0 * U512::from(100) + huvd_high.1 - U512::from(1)) / huvd_high.1;
        check!(
            huvd_high_ceil,
            U512::from(
                certificate_huvd["upper_fdv_cents_ceiling"]
                    .as_u64()
                    .unwrap()
            )
        );
    }

    #[test]
    fn solana_tracker_h4_and_9sef_historical_supply_fdv_windows_replay_exactly() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};
        use std::collections::BTreeMap;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_h4_9sef_historical_supply_fdv_window_cases_v2.json"
        ))
        .unwrap();
        let hash = |raw: &str| format!("{:x}", Sha256::digest(raw.as_bytes()));
        macro_rules! check {
            ($actual:expr, $expected:expr $(,)?) => {
                assert_eq!($actual, $expected)
            };
        }
        let raw = [
            ("h4_window", "h4_window_json_raw"),
            ("nine_sef_window", "nine_sef_window_json_raw"),
            (
                "historical_supply_replay",
                "historical_supply_replay_json_raw",
            ),
            (
                "nine_sef_supply_reconciliation",
                "nine_sef_supply_reconciliation_json_raw",
            ),
        ];
        for (source, key) in raw {
            check!(
                hash(fixture[key].as_str().unwrap()),
                fixture["source_sha256"][source]
            );
        }
        check!(
            serde_json::json!([
                fixture["schema"],
                fixture["root_certificate_pending"],
                fixture["source_kind"],
                fixture["source_sha256"]["h4_window"],
                fixture["source_sha256"]["nine_sef_window"],
                fixture["source_sha256"]["historical_supply_replay"],
                fixture["source_sha256"]["nine_sef_supply_reconciliation"],
                fixture["root_certificate_sha256"],
            ]),
            serde_json::json!([
                "solana-tracker-h4-9sef-historical-supply-fdv-window-cases-v1",
                false,
                "onchain-pool-window-supply-cex-reference-v1",
                "b6e0c010070df64480b5b94a0253cffae3788f8c42f2630d0d73cdb23eb88544",
                "c0e527cac3c896cb7d34744bc1cc37fdf1604590a97a6314ba3accdd1ac962ad",
                "c114bf8fc3868192f956af0d77dee8dbc1686ccae2657790f9465ea5544bfe7f",
                "09018eedbe90f2b8d51957a4f4f7d17acf986734a184d0a7ce087ec057ff3ad5",
                "02ad7707bac026e066c1495d992cd18805c52776e6e0b0f262757eadecea3e34",
            ]),
        );
        let certificate_raw = fixture["root_certificate_json_raw"].as_str().unwrap();
        check!(hash(certificate_raw), fixture["root_certificate_sha256"]);
        let certificate: serde_json::Value = serde_json::from_str(certificate_raw).unwrap();
        check!(fixture["source_kind"], certificate["source"]);
        check!(
            serde_json::json!([
                certificate["schema"],
                certificate["source"],
                certificate["all_pass"],
                certificate["observed_states"],
                certificate["supply_ops_checked"],
                certificate["window_supply_changes"],
                certificate["conditional_vault_ratio_model"],
                certificate["historical_virtual_quote_reserves"],
                certificate["current_unparsed_pool_tail_not_extrapolated"],
                certificate["annual_allmarket_lifetime_complete"],
                certificate["ath_at_policy"],
                certificate["network_calls"],
                certificate["database_writes"],
                certificate["source_sha256"][".scout-data/diagnostics/solana-h4-observed-window-fdv-2026-10-14/h4-observed-window-fdv-v1.json"],
                certificate["source_sha256"][".scout-data/diagnostics/solana-9sef-observed-window-fdv-2026-10-14/9sef-observed-window-fdv-v2.json"],
                certificate["source_sha256"][".scout-data/diagnostics/solana-9sef-history-continuation-2026-10-14/9sef-supply-reconciliation-v2.json"],
                certificate["source_sha256"][".scout-data/diagnostics/solana-h4-9sef-independent-audit-2026-10-14/raw-data-audit-v2.json"],
            ]),
            serde_json::json!([
                "root-h4-9sef-observed-pool-window-certificate-v1",
                "onchain-pool-window-supply-cex-reference-v1",
                true,
                46,
                10,
                0,
                true,
                "unknown",
                true,
                false,
                "null",
                0,
                0,
                "b6e0c010070df64480b5b94a0253cffae3788f8c42f2630d0d73cdb23eb88544",
                "c0e527cac3c896cb7d34744bc1cc37fdf1604590a97a6314ba3accdd1ac962ad",
                "09018eedbe90f2b8d51957a4f4f7d17acf986734a184d0a7ce087ec057ff3ad5",
                "523ea53ab2d50702f99afca6d3cf7de2fa6480c156ff27d692971dd062f7b388",
            ]),
        );
        check!(
            serde_json::json!(
                certificate["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| serde_json::json!([
                        row["mint"],
                        row["observed_states"],
                        row["supply_raw"],
                        row["current_supply_raw"],
                        row["lower_fdv_cents"],
                        row["upper_fdv_cents_ceiling"],
                        row["price_peak_gate_pass"],
                        row["global_low_mark"],
                        row["global_high_mark"],
                    ]))
                    .collect::<Vec<_>>()
            ),
            serde_json::json!([
                [
                    "H4knWZeC8zPMbtn4MR8uWZDXLZsAkjosWJgL1XVopump",
                    12,
                    "1136099869009195",
                    "1117339762843949",
                    1784671,
                    1785348,
                    true,
                    "5bhuBSNcdhuM9wgeKduxfvyjvPBht96Tny7AygeNhTvPJZoyLniu1YKD8u27sz6CxAvv8jG7fEUkPDSpS1sPjdrg",
                    "5bhuBSNcdhuM9wgeKduxfvyjvPBht96Tny7AygeNhTvPJZoyLniu1YKD8u27sz6CxAvv8jG7fEUkPDSpS1sPjdrg",
                ],
                [
                    "9sEF8kqzYexjqxYNvV3bBb5kRihxdXcsWSVLSFeypump",
                    34,
                    "1712502929816770",
                    "1699458511258170",
                    2907531,
                    2909291,
                    true,
                    "75arBXWDPwYgJPXDjT6tmZwZ5gorhkZxxnNLhQLXctSd5omM3951u2HmGf8HmzDEkVbxaGnK3Zu4K3KEnqSJWP5",
                    "75arBXWDPwYgJPXDjT6tmZwZ5gorhkZxxnNLhQLXctSd5omM3951u2HmGf8HmzDEkVbxaGnK3Zu4K3KEnqSJWP5",
                ],
            ]),
        );
        let (h4, nine, supply_replay, reconciliation): (
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
        ) = (
            serde_json::from_str(fixture["h4_window_json_raw"].as_str().unwrap()).unwrap(),
            serde_json::from_str(fixture["nine_sef_window_json_raw"].as_str().unwrap()).unwrap(),
            serde_json::from_str(
                fixture["historical_supply_replay_json_raw"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
            serde_json::from_str(
                fixture["nine_sef_supply_reconciliation_json_raw"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        );
        let h4_token = supply_replay["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|token| token["mint"] == h4["mint"])
            .unwrap();
        let nine_token = supply_replay["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|token| token["mint"] == nine["mint"])
            .unwrap();
        let nine_supply = &reconciliation["supply_ops"];
        check!(
            serde_json::json!([
                h4["schema"],
                h4["state_count"],
                h4["swap_count"],
                h4["decimals"],
                h4["quote_decimals"],
                h4["input_sha256"]["price_proof_v4"],
                h4["input_sha256"]["supply_replay_v1"],
                nine["schema"],
                nine["state_count"],
                nine["event_count"],
                nine["base_decimals"],
                nine["quote_decimals"],
                nine["source_hashes"]["v4"],
                nine["source_hashes"]["reconciliation"],
                reconciliation["status"],
                reconciliation["signature_set"]["exact_match"],
                reconciliation["signature_set"]["expected_union_rows"],
                reconciliation["signature_set"]["unique_tfa_rows"],
                reconciliation["signature_set"]["successful_census_rows"],
                reconciliation["signature_set"]["window_rows"],
            ]),
            serde_json::json!([
                "h4-observed-window-fdv-state-envelope-v1",
                12,
                6,
                6,
                9,
                "9784d2b5136404d5a6517857c7d0ca64c733651a1eb2a9bf26d7a85bedf75c4e",
                fixture["source_sha256"]["historical_supply_replay"],
                "solana-9sef-observed-window-fdv-v2",
                34,
                17,
                6,
                9,
                "9784d2b5136404d5a6517857c7d0ca64c733651a1eb2a9bf26d7a85bedf75c4e",
                fixture["source_sha256"]["nine_sef_supply_reconciliation"],
                "complete_success_set_reconciled_raw_locators_verified",
                true,
                391,
                391,
                374,
                17,
            ]),
        );
        check!(
            nine["source_hashes"]["continuation_manifest"],
            reconciliation["source_sha256"]["continuation_manifest"]
        );
        check!(
            serde_json::json!([
                h4["supply_raw_at_all_states"],
                nine["supply_raw_each_state"],
                reconciliation["selected_peak"]["supply_raw"],
                reconciliation["provider_window_supply_rows"]
                    .as_array()
                    .unwrap()
                    .len(),
                nine["comparison_to_existing_provider_peak"]["both_within_2_percent"],
            ]),
            serde_json::json!([
                "1136099869009195",
                "1712502929816770",
                "1712502929816770",
                17,
                true,
            ]),
        );
        check!(
            reconciliation["provider_window_supply_rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["ops"].as_array().unwrap().is_empty()
                    && row["supply_before_raw"] == row["supply_after_raw"]
                    && row["supply_after_raw"] == "1712502929816770"),
            true
        );

        let fraction = |text: &str| {
            let (n, d) = text.split_once('/').unwrap();
            (
                U512::from_str_radix(n, 10).unwrap(),
                U512::from_str_radix(d, 10).unwrap(),
            )
        };
        let compare = |a: (U512, U512), b: (U512, U512)| (a.0 * b.1).cmp(&(b.0 * a.1));
        let same =
            |a: (U512, U512), b: (U512, U512)| assert_eq!(compare(a, b), std::cmp::Ordering::Equal);
        let decimal = |text: &str| {
            let value = ExactDecimal::parse(text).unwrap();
            let denominator = (0..u32::try_from(value.scale).unwrap())
                .try_fold(U512::from(1), |n, _| n.checked_mul(U512::from(10)))
                .unwrap();
            (U512::from(value.coefficient), denominator)
        };
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U512::from(1), |n, _| n.checked_mul(U512::from(10)))
                .unwrap()
        };
        let ceil = |n: U512, d: U512| (n + d - U512::from(1)) / d;
        let replay_window = |proof: &serde_json::Value, endpoints: &[(&str, usize)]| {
            let states = proof["states"].as_array().unwrap();
            let base_decimals = proof["decimals"]
                .as_u64()
                .or_else(|| proof["base_decimals"].as_u64())
                .unwrap();
            let quote_decimals = proof["quote_decimals"].as_u64().unwrap();
            let supply = U512::from_str_radix(
                proof["supply_raw_at_all_states"]
                    .as_str()
                    .or_else(|| proof["supply_raw_each_state"].as_str())
                    .unwrap(),
                10,
            )
            .unwrap();
            let mut maxima: BTreeMap<String, (String, String, (U512, U512))> = BTreeMap::new();
            for state in states {
                let base = U512::from_str_radix(state["base_raw"].as_str().unwrap(), 10).unwrap();
                let quote = U512::from_str_radix(state["quote_raw"].as_str().unwrap(), 10).unwrap();
                let native_price = (
                    quote * pow10(u32::try_from(base_decimals).unwrap()),
                    base * pow10(u32::try_from(quote_decimals).unwrap()),
                );
                if let Some(native) = state
                    .get("spot_native_fraction")
                    .or_else(|| state.get("price_native_fraction"))
                {
                    same(native_price, fraction(native.as_str().unwrap()));
                }
                let candle = state["coinbase_candle"]
                    .as_array()
                    .or_else(|| state["coinbase_candle_low_high_open_close_volume"].as_array())
                    .unwrap();
                for (end, fx_index) in endpoints {
                    let fx = decimal(candle[*fx_index].as_str().unwrap());
                    let price = (native_price.0 * fx.0, native_price.1 * fx.1);
                    let fdv = (
                        price.0 * supply,
                        price.1 * pow10(u32::try_from(base_decimals).unwrap()),
                    );
                    let fraction_key = format!("fdv_{end}_fraction");
                    let usd_fraction_key = format!("fdv_{end}_usd_fraction");
                    let stored = state[if state.get(fraction_key.as_str()).is_some() {
                        fraction_key.as_str()
                    } else {
                        usd_fraction_key.as_str()
                    }]
                    .as_str()
                    .unwrap();
                    same(fdv, fraction(stored));
                    let side = state["side"]
                        .as_str()
                        .or_else(|| state["state_side"].as_str())
                        .unwrap();
                    let signature = state["signature"].as_str().unwrap();
                    if maxima
                        .get(*end)
                        .is_none_or(|(_, _, old)| compare(fdv, *old).is_gt())
                    {
                        maxima.insert(
                            (*end).to_owned(),
                            (signature.to_owned(), side.to_owned(), fdv),
                        );
                    }
                }
            }
            maxima
        };
        let h4_max = replay_window(&h4, &[("low", 3), ("high", 2)]);
        let nine_max = replay_window(&nine, &[("low", 1), ("high", 2), ("close", 4)]);
        for (proof, maxima, keys) in [
            (&h4, &h4_max, [("low", "low"), ("high", "high")]),
            (&nine, &nine_max, [("low", "fdv_low"), ("high", "fdv_high")]),
        ] {
            let expected = if proof["mint"] == h4["mint"] {
                &proof["maxima"]
            } else {
                &proof["global_maxima"]
            };
            for (endpoint, expected_key) in keys {
                let (signature, side, actual) = &maxima[endpoint];
                let record = &expected[expected_key];
                same(
                    *actual,
                    fraction(
                        record["fdv_fraction"]
                            .as_str()
                            .unwrap_or_else(|| record["fraction"].as_str().unwrap()),
                    ),
                );
                check!(
                    serde_json::json!([signature, side]),
                    serde_json::json!([
                        record["signature"],
                        record
                            .get("side")
                            .or_else(|| record.get("state_side"))
                            .unwrap()
                    ])
                );
                let cents = if endpoint == "low" {
                    actual.0 * U512::from(100) / actual.1
                } else {
                    ceil(actual.0 * U512::from(100), actual.1)
                };
                let rounded = if endpoint == "low" {
                    record["floor_cents"].as_u64().unwrap()
                } else {
                    record["ceil_cents"].as_u64().unwrap()
                };
                assert_eq!(cents, U512::from(rounded));
            }
        }
        let (close_signature, close_side, close_fdv) = &nine_max["close"];
        same(
            *close_fdv,
            fraction(
                nine["global_maxima"]["fdv_close"]["fraction"]
                    .as_str()
                    .unwrap(),
            ),
        );
        check!(
            serde_json::json!([close_signature, close_side]),
            serde_json::json!([
                nine["global_maxima"]["fdv_close"]["signature"],
                nine["global_maxima"]["fdv_close"]["state_side"],
            ]),
        );
        check!(
            serde_json::json!([
                h4_max["low"].0,
                h4_max["high"].0,
                h4_max["low"].1.to_string(),
                h4_max["high"].1.to_string(),
                nine_max["low"].0,
                nine_max["high"].0,
                nine_max["close"].0,
                nine_max["low"].1.to_string(),
                nine_max["high"].1.to_string(),
            ]),
            serde_json::json!([
                "5bhuBSNcdhuM9wgeKduxfvyjvPBht96Tny7AygeNhTvPJZoyLniu1YKD8u27sz6CxAvv8jG7fEUkPDSpS1sPjdrg",
                "5bhuBSNcdhuM9wgeKduxfvyjvPBht96Tny7AygeNhTvPJZoyLniu1YKD8u27sz6CxAvv8jG7fEUkPDSpS1sPjdrg",
                "post",
                "post",
                "75arBXWDPwYgJPXDjT6tmZwZ5gorhkZxxnNLhQLXctSd5omM3951u2HmGf8HmzDEkVbxaGnK3Zu4K3KEnqSJWP5",
                "75arBXWDPwYgJPXDjT6tmZwZ5gorhkZxxnNLhQLXctSd5omM3951u2HmGf8HmzDEkVbxaGnK3Zu4K3KEnqSJWP5",
                "75arBXWDPwYgJPXDjT6tmZwZ5gorhkZxxnNLhQLXctSd5omM3951u2HmGf8HmzDEkVbxaGnK3Zu4K3KEnqSJWP5",
                "post",
                "post",
            ]),
        );

        let h4_birth = &h4_token["birth"];
        let h4_sequence = &h4_birth["birth_program_sequence"];
        let h4_ops = h4_token["supply_ops"]["all_ordered_ops"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>();
        let h4_mints = h4_ops
            .iter()
            .copied()
            .filter(|op| op["kind"] == "mint_to")
            .collect::<Vec<_>>();
        let nine_birth = &reconciliation["birth"];
        let nine_ops = nine_supply.as_array().unwrap().iter().collect::<Vec<_>>();
        let nine_mints = nine_ops
            .iter()
            .copied()
            .filter(|op| op["kind"] == "mint_to")
            .collect::<Vec<_>>();
        check!(
            serde_json::json!([
                nine_mints[0]["outer_index"],
                nine_mints[0]["inner_index"],
                nine_mints[1]["outer_index"],
                nine_mints[1]["inner_index"],
            ]),
            serde_json::json!([2, 19, 2, 20]),
        );
        let raw_amount = |value: &serde_json::Value| {
            value.as_str().map_or_else(
                || U512::from(value.as_u64().unwrap()),
                |text| U512::from_str_radix(text, 10).unwrap(),
            )
        };
        for (birth, mints, ops, revoked, historic_supply, window_start) in [
            (
                h4_birth,
                &h4_mints,
                &h4_ops,
                &h4_sequence["mint_authority_revoked_at_birth"],
                &h4["supply_raw_at_all_states"],
                1_775_162_881_u64,
            ),
            (
                nine_birth,
                &nine_mints,
                &nine_ops,
                &nine_birth["mint_authority_revoked_at_birth"],
                &nine["supply_raw_each_state"],
                1_774_650_073_u64,
            ),
        ] {
            let birth_value = birth
                .get("raw_supply")
                .or_else(|| birth.get("supply_raw"))
                .unwrap();
            let birth_raw = raw_amount(birth_value);
            let mint_total = mints.iter().fold(U512::from(0), |sum, mint| {
                sum + raw_amount(&mint["amount_raw"])
            });
            assert_eq!(mints.len(), 2);
            assert_eq!(birth_raw, U512::from(2_000_000_000_000_000_u64));
            assert_eq!(mint_total, birth_raw);
            check!(revoked, true);
            let burns: Vec<_> = ops.iter().filter(|op| op["kind"] == "burn").collect();
            assert_eq!(burns.len(), 3);
            let burned = burns
                .iter()
                .fold(U512::from(0), |sum, op| sum + raw_amount(&op["amount_raw"]));
            assert_eq!(
                birth_raw - burned,
                U512::from_str_radix(historic_supply.as_str().unwrap(), 10).unwrap()
            );
            assert!(
                burns
                    .iter()
                    .all(|op| op["block_time"].as_u64().unwrap() < window_start)
            );
        }
        check!(
            serde_json::json!([
                h4_sequence["initialize_mint2"]["decimals"],
                h4_sequence["initialize_mint2"]["inner_index"],
                h4_sequence["mint_to_instructions"][0]["inner_index"],
                h4_sequence["mint_to_instructions"][1]["inner_index"],
                h4_sequence["set_authority_revoke"]["inner_index"],
                h4_sequence["set_authority_revoke"]["new_authority_option"],
                h4_birth["unix_s"],
                reconciliation["birth"]["unix_s"],
                reconciliation["supply_ops"].as_array().unwrap().len(),
                reconciliation["selected_peak"]["same_transaction_supply_ops"]
                    .as_array()
                    .unwrap()
                    .len(),
            ]),
            serde_json::json!([6, 2, 19, 20, 21, 0, 1773438537, 1773868141, 5, 0]),
        );
        let h4_current = &h4_token["current_snapshot"];
        let nine_current = &nine_token["current_snapshot"];
        check!(
            serde_json::json!([
                h4_current["supply_raw"],
                h4_current["context_slot"],
                h4_current["account_raw_sha256"],
                nine_current["supply_raw"],
                nine_current["context_slot"],
                nine_current["account_raw_sha256"],
                reconciliation["current_snapshot_after_window"]["supply_raw"],
                reconciliation["current_snapshot_after_window"]["sha256"],
            ]),
            serde_json::json!([
                "1117339762843949",
                456650939,
                "b864ca0ef25e8a5a97f2e814bedf82714b23670548551e698f5137ecc060bbc4",
                "1699458511258170",
                456650939,
                "180f06aa8d6d4e4f5e998258fddb933eaa042938c3d07c7910dc6fc431c06d5f",
                "1699458511258170",
                "180f06aa8d6d4e4f5e998258fddb933eaa042938c3d07c7910dc6fc431c06d5f",
            ]),
        );
        assert!(
            U512::from_str_radix(h4_current["supply_raw"].as_str().unwrap(), 10).unwrap()
                < U512::from_str_radix(h4["supply_raw_at_all_states"].as_str().unwrap(), 10)
                    .unwrap()
        );
        assert!(
            U512::from_str_radix(nine_current["supply_raw"].as_str().unwrap(), 10).unwrap()
                < U512::from_str_radix(nine["supply_raw_each_state"].as_str().unwrap(), 10)
                    .unwrap()
        );
        assert!(h4_current["context_slot"].as_u64().unwrap() > 410_607_988);
        assert!(nine_current["context_slot"].as_u64().unwrap() > 409_297_681);
        assert_eq!(h4["limitations"].as_array().unwrap().len(), 3);
        assert!(
            h4["scope"]
                .as_str()
                .unwrap()
                .contains("not lifetime ATH or all-market coverage")
        );
        assert_eq!(nine["model_caveats"].as_array().unwrap().len(), 3);
        assert!(
            nine["model_caveats"][0]
                .as_str()
                .unwrap()
                .contains("30-byte unparsed tail")
        );
        assert!(nine["states"].as_array().unwrap().iter().all(|state| {
            state["event_layout"] == "historical_legacy_no_virtual_quote_field"
                && state["supply_raw_at_state"] == "1712502929816770"
        }));
        for (token, sequence, expected_mint) in [
            (
                h4_token,
                &h4_birth["birth_program_sequence"],
                h4["mint"].as_str().unwrap(),
            ),
            (
                nine_token,
                &nine_token["birth"]["birth_program_sequence"],
                nine["mint"].as_str().unwrap(),
            ),
        ] {
            let mint_to = sequence["mint_to_instructions"].as_array().unwrap();
            check!(
                serde_json::json!([
                    token["mint"],
                    token["current_snapshot"]["account_owner"],
                    token["current_snapshot"]["mint_authority_absent"],
                    token["current_snapshot"]["freeze_authority_absent"],
                    sequence["initialize_mint2"]["mint_authority"],
                    sequence["initialize_mint2"]["decimals"],
                    sequence["set_authority_revoke"]["mint"],
                    sequence["set_authority_revoke"]["new_authority_option"],
                    mint_to[0]["mint"],
                    mint_to[1]["mint"],
                    mint_to[0]["authority"],
                    mint_to[1]["authority"],
                    mint_to[0]["destination"],
                    mint_to[1]["destination"],
                ]),
                serde_json::json!([
                    expected_mint,
                    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
                    true,
                    true,
                    "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM",
                    6,
                    expected_mint,
                    0,
                    expected_mint,
                    expected_mint,
                    "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM",
                    "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM",
                    mint_to[0]["destination"],
                    mint_to[1]["destination"],
                ]),
            );
            assert_ne!(mint_to[0]["destination"], mint_to[1]["destination"]);
        }
        assert!(
            nine["coverage_scope"]
                .as_str()
                .unwrap()
                .contains("Does not establish alternate-pool, all-market, or lifetime ATH")
        );
    }

    #[test]
    fn solana_tracker_4ev_observed_curve_post_pool_fdv_replays_exactly() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_4ev_observed_curve_post_pool_fdv_window_case.json"
        ))
        .unwrap();
        let hash = |raw: &str| format!("{:x}", Sha256::digest(raw.as_bytes()));
        macro_rules! check {
            ($actual:expr, $expected:expr $(,)?) => {
                assert_eq!($actual, $expected)
            };
        }
        let (cert_raw, replay_raw, canonical_raw) = (
            fixture["root_certificate_json_raw"].as_str().unwrap(),
            fixture["observed_replay_json_raw"].as_str().unwrap(),
            fixture["canonical_manifest_json_raw"].as_str().unwrap(),
        );
        check!(
            serde_json::json!([hash(cert_raw), hash(replay_raw), hash(canonical_raw)]),
            serde_json::json!([
                "cb9f46719c24fa0190159b38da678485ffd32edd4d6422abc670ffe3161cd254",
                "ec357d365bf7123aedb4bebfda79a4fb7ae03e0c563c8e07ed12233a965a1929",
                "cbd88a9b8d431a9131597c1b2dce10581d2e65b68a273d86627e2c847dd43c0b"
            ]),
        );
        let (cert, replay, canonical): (serde_json::Value, serde_json::Value, serde_json::Value) = (
            serde_json::from_str(cert_raw).unwrap(),
            serde_json::from_str(replay_raw).unwrap(),
            serde_json::from_str(canonical_raw).unwrap(),
        );
        check!(
            serde_json::json!([
                cert["source"],
                cert["observed_mark_count"],
                cert["network_calls"],
                cert["database_writes"],
                cert["conditional_curve_pre_diagnostics_count"],
                cert["conditional_pre_marks_not_observed_or_promoted"],
                fixture["source_hashes"],
                cert["source_sha256"],
                fixture["supply_source_hashes"],
                cert["supply_binding"]["source_sha256"],
                replay["counts"]["observed_marks"],
                replay["counts"]["conditional_curve_pre_intervals_retained_diagnostic_only"],
                replay["counts"]["conditional_pre_intervals_in_observed_maxima"]
            ]),
            serde_json::json!([
                fixture["source_name"],
                119,
                0,
                0,
                96,
                true,
                cert["source_sha256"],
                cert["source_sha256"],
                cert["supply_binding"]["source_sha256"],
                cert["supply_binding"]["source_sha256"],
                119,
                96,
                0
            ]),
        );
        check!(
            serde_json::json!([
                canonical["status"],
                canonical["calls_used"],
                canonical["calls"].as_array().unwrap().len()
            ]),
            serde_json::json!(["complete", 2, 2]),
        );
        assert!(
            canonical["calls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["http_status"] == 200 && c["matches_frozen_tfa_object"] == true)
        );
        let sigs = canonical["target_signatures"].as_array().unwrap();
        assert!(sigs.iter().any(|r| r["label"] == "birth" && r["signature"] == "JdZDSakaWn65KXkioATKrKSNABbPWKMaxhATY7rLPRkDTXaYDQECVZiHYVtLMNxtWPmk7K2cyhKUiDG9g8ujqwS"));
        assert!(sigs.iter().any(|r| r["label"] == "observed_window_winner"
            && r["signature"] == cert["winner"]["signature"]));

        let supply = &fixture["supply_binding"];
        let birth = &supply["birth"];
        let init = &birth["initialize_mint2"][0];
        let bytes = bytes_from_hex(init["data_hex"].as_str().unwrap());
        check!(
            serde_json::json!([
                birth["strict_allowlist_pass"],
                birth["mint_sum_raw"],
                bytes.len(),
                bytes[0],
                bytes[1],
                bytes[34]
            ]),
            serde_json::json!([true, "2000000000000000", 35, 20, 6, 0]),
        );
        assert_eq!(
            bs58::encode(&bytes[2..34]).into_string(),
            init["mint_authority"]
        );
        check!(
            serde_json::json!([
                birth["authority_revocation"]["kind"],
                fixture["mint_capture_index"]
            ]),
            serde_json::json!(["set_mint_authority_none", 12]),
        );
        let current = &supply["current_mint"];
        check!(
            serde_json::json!([
                current["snapshot_is_finalized"],
                current["snapshot_value_count"],
                current["raw_account_length"],
                current["mint_authority_none"],
                current["extension_set_exact"],
                current["extensions"][0]["type"],
                current["extensions"][0]["length"],
                current["extensions"][1]["type"],
                current["extensions"][1]["length"]
            ]),
            serde_json::json!([true, 20, 411, true, true, 18, 64, 19, 173]),
        );
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut raw = Vec::new();
        let (mut acc, mut bits) = (0_u32, 0_u32);
        for byte in current["raw_account_base64"]
            .as_str()
            .unwrap()
            .bytes()
            .filter(|b| *b != b'=')
        {
            acc = (acc << 6)
                | u32::try_from(alphabet.iter().position(|c| *c == byte).unwrap()).unwrap();
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                raw.push(u8::try_from(acc >> bits).unwrap());
                acc &= (1 << bits) - 1;
            }
        }
        check!(
            serde_json::json!([
                raw.len(),
                raw[..4].iter().all(|b| *b == 0),
                raw[44],
                raw[45],
                format!("{:x}", Sha256::digest(&raw))
            ]),
            serde_json::json!([411, true, 6, 1, current["raw_account_sha256"]]),
        );
        check!(
            serde_json::json!([
                current["snapshot_request_sha256"],
                current["snapshot_response_sha256"]
            ]),
            serde_json::json!([
                "fc78555c299aa488995eccb8f66730599cbe4a30acff4badc0f2a45bcc51cf15",
                "ad472a7c58c59bb56860484192668c252abfcacb6ff8b601af5a7b3989e64727"
            ]),
        );
        assert_ne!(
            supply["supply_at_price_window_end_raw"],
            current["supply_raw"]
        );
        assert_eq!(supply["supply_at_price_window_end_raw"], "1121771374045406");

        let flow = &fixture["native_flow_and_schema_gaps"];
        check!(
            serde_json::json!([
                flow["curve_buy_event_inputs_exact_scoped_system_transfer"],
                flow["curve_parented_token_transfers_exact"],
                flow["curve_single_trade_transaction_pda_lamport_delta_mismatches"],
                flow["curve_multi_trade_transaction_group_delta_exact"],
                flow["pool_unparsed_trailing_bytes"]
            ]),
            serde_json::json!([60, 96, 2, 1, 40]),
        );
        let bundled = &flow["bundled_trade_reconciliation"];
        check!(
            serde_json::json!([
                bundled["target_curve_trade_event_count"],
                bundled["grouped_balance_matches"],
                bundled["signed_event_sol_lamports_sum"],
                bundled["whole_transaction_curve_pda_lamport_delta"]
            ]),
            serde_json::json!([2, true, "2", "2"]),
        );
        let shapes = flow["curve_instruction_shapes"].as_array().unwrap();
        assert_eq!(
            shapes
                .iter()
                .filter(|r| r["data_bytes"] == 24
                    && matches!(r["ix_name"].as_str(), Some("buy" | "buy_exact_sol_in")))
                .map(|r| r["count"].as_u64().unwrap())
                .sum::<u64>(),
            31
        );
        assert!(shapes.iter().all(|r| r["data_bytes"] != 25
            || !matches!(r["ix_name"].as_str(), Some("buy" | "buy_exact_sol_in"))
            || matches!(r["track_volume_byte"].as_u64(), Some(0 | 1))));
        assert_eq!(
            flow["unresolved_trailing_account_shapes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["extra_account_count"].as_u64().unwrap() > 0)
                .map(|r| r["count"].as_u64().unwrap())
                .sum::<u64>(),
            55
        );
        let limits = flow["limits"].as_array().unwrap();
        assert!(
            limits
                .iter()
                .any(|r| r.as_str().unwrap().contains("no false default applied"))
                && limits.iter().any(|r| r
                    .as_str()
                    .unwrap()
                    .contains("No per-instruction lamport delta is exposed"))
        );

        let ratio = |s: &str| {
            let (n, d) = s.split_once('/').unwrap();
            (
                U512::from_str_radix(n, 10).unwrap(),
                U512::from_str_radix(d, 10).unwrap(),
            )
        };
        let cmp = |a: (U512, U512), b: (U512, U512)| (a.0 * b.1).cmp(&(b.0 * a.1));
        let equal =
            |a: &str, b: &str| assert_eq!(cmp(ratio(a), ratio(b)), std::cmp::Ordering::Equal);
        let ceil = |n: U512, d: U512| (n + d - U512::from(1)) / d;
        let decimal = |s: &str| {
            let d = ExactDecimal::parse(s).unwrap();
            (
                U512::from(d.coefficient),
                (0..u32::try_from(d.scale).unwrap())
                    .try_fold(U512::from(1), |v, _| v.checked_mul(U512::from(10)))
                    .unwrap(),
            )
        };
        let pass_2pct = |a: (U512, U512), b: (U512, U512)| {
            let x = a.0 * b.1;
            let y = b.0 * a.1;
            x.abs_diff(y) * U512::from(100) <= y * U512::from(2)
        };
        let marks = fixture["observed_marks"].as_array().unwrap();
        let diagnostics = &fixture["excluded_pre_interval_diagnostics"];
        check!(
            serde_json::json!([
                marks.len(),
                diagnostics["count"],
                diagnostics["diagnostic_rows"].as_array().unwrap().len()
            ]),
            serde_json::json!([119, 96, 96]),
        );
        check!(
            serde_json::json!([
                marks
                    .iter()
                    .filter(|m| m["mark_kind"] == "create_post")
                    .count(),
                marks
                    .iter()
                    .filter(|m| m["mark_kind"] == "trade_post")
                    .count(),
                marks
                    .iter()
                    .filter(|m| m["venue"] == "pumpswap_selected_pool")
                    .count()
            ]),
            serde_json::json!([1, 96, 22]),
        );
        assert!(marks.iter().all(|m| m["mark_kind"] != "trade_pre_interval"));
        let events = supply["supply_events"].as_array().unwrap();
        let source_rows = replay["observed_mark_rows"].as_array().unwrap();
        let mut maxima = [None, None, None, None];
        for mark in marks {
            let native = (
                U512::from(
                    mark["reserve_quote_raw"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                ),
                U512::from_str_radix(mark["reserve_token_raw"].as_str().unwrap(), 10).unwrap()
                    * U512::from(1_000),
            );
            equal(
                mark["native_price_sol_per_token_fraction"]
                    .as_str()
                    .unwrap(),
                &format!("{}/{}", native.0, native.1),
            );
            let pos = (
                mark["slot"].as_u64().unwrap(),
                mark["transaction_index"].as_u64().unwrap(),
            );
            let raw_supply = events
                .iter()
                .filter(|e| {
                    (
                        e["slot"].as_u64().unwrap(),
                        e["transaction_index"].as_u64().unwrap(),
                    ) <= pos
                })
                .fold(U512::from(0), |sum, e| {
                    let amount = U512::from(e["amount_raw"].as_u64().unwrap());
                    if e["kind"] == "mint" {
                        sum + amount
                    } else {
                        assert_eq!(e["kind"], "burn");
                        sum - amount
                    }
                });
            assert_eq!(
                raw_supply,
                U512::from_str_radix(mark["supply_raw_6_decimals"].as_str().unwrap(), 10).unwrap()
            );
            equal(
                mark["inventory_native_price_times_raw_supply_unscaled_fraction"]
                    .as_str()
                    .unwrap(),
                &format!("{}/{}", native.0 * raw_supply, native.1),
            );
            let fx_low = ratio(mark["coinbase_sol_usd_low_fraction"].as_str().unwrap());
            let fx_high = ratio(mark["coinbase_sol_usd_high_fraction"].as_str().unwrap());
            let low_price = (native.0 * fx_low.0, native.1 * fx_low.1);
            let high_price = (native.0 * fx_high.0, native.1 * fx_high.1);
            let low = (
                low_price.0 * raw_supply,
                low_price.1 * U512::from(1_000_000),
            );
            let high = (
                high_price.0 * raw_supply,
                high_price.1 * U512::from(1_000_000),
            );
            for (field, value) in [
                ("price_usd_low_fraction", low_price),
                ("price_usd_high_fraction", high_price),
                ("fdv_usd_low_fraction", low),
                ("fdv_usd_high_fraction", high),
            ] {
                equal(
                    mark[field].as_str().unwrap(),
                    &format!("{}/{}", value.0, value.1),
                );
            }
            assert_eq!(
                (
                    low.0 * U512::from(100) / low.1,
                    ceil(high.0 * U512::from(100), high.1)
                ),
                (
                    U512::from(mark["fdv_usd_low_floor_cents"].as_u64().unwrap()),
                    U512::from(mark["fdv_usd_high_ceil_cents"].as_u64().unwrap())
                )
            );
            for (i, value) in [low, high, low_price, high_price].into_iter().enumerate() {
                if maxima[i].is_none_or(|old| cmp(value, old).is_gt()) {
                    maxima[i] = Some(value);
                }
            }
            let source = source_rows
                .iter()
                .find(|r| {
                    r["signature"] == mark["signature"]
                        && r["venue"] == mark["venue"]
                        && r["mark_kind"] == mark["mark_kind"]
                        && r["log_index"] == mark["log_index"]
                        && r["event_sha256"] == mark["event_sha256"]
                })
                .unwrap();
            for field in [
                "native_price_sol_per_token_fraction",
                "supply_raw_6_decimals",
                "price_usd_low_fraction",
                "price_usd_high_fraction",
                "fdv_usd_low_fraction",
                "fdv_usd_high_fraction",
                "fdv_usd_low_floor_cents",
                "fdv_usd_high_ceil_cents",
            ] {
                assert_eq!(mark[field], source[field]);
            }
        }
        let proof = &fixture["global_observed_fdv_maxima"];
        for (value, expected) in [
            (
                maxima[0].unwrap(),
                proof["low"]["exact_usd"].as_str().unwrap(),
            ),
            (
                maxima[1].unwrap(),
                proof["high"]["exact_usd"].as_str().unwrap(),
            ),
            (
                maxima[2].unwrap(),
                fixture["global_price_maxima"]["low"]["exact_usd_per_token"]
                    .as_str()
                    .unwrap(),
            ),
            (
                maxima[3].unwrap(),
                fixture["global_price_maxima"]["high"]["exact_usd_per_token"]
                    .as_str()
                    .unwrap(),
            ),
        ] {
            equal(&format!("{}/{}", value.0, value.1), expected);
        }
        check!(
            serde_json::json!([
                proof["low"]["floor_cents"],
                proof["high"]["ceil_cents"],
                proof["observed_source_2pct_gate_pass"],
                proof["low"]["winner"],
                proof["high"]["winner"]
            ]),
            serde_json::json!([
                719780,
                720530,
                true,
                proof["low"]["winner"],
                proof["low"]["winner"]
            ]),
        );
        let winner = &proof["low"]["winner"];
        assert_eq!(
            winner["signature"],
            "2ku1GWYraWgiRKtdtVpeLvqPUdBHx3xEx4Godpkty85kgYGzYkgvbh9SQV9EyHFDw8osJhVAN4GWhCSAJtnMSEjm"
        );
        assert_eq!(
            i64::try_from(winner["block_time_unix_s"].as_u64().unwrap()).unwrap()
                - i64::try_from(
                    fixture["provider_reference"]["peak_timestamp_ms"]
                        .as_u64()
                        .unwrap()
                        / 1_000
                )
                .unwrap(),
            -1
        );
        assert!(marks.iter().any(|m| m["signature"] == winner["signature"]
            && m["event_sha256"] == winner["event_sha256"]
            && m["mark_kind"] == winner["mark_kind"]));
        let provider_price = decimal(
            fixture["provider_reference"]["saved_st_price_usd"]
                .as_str()
                .unwrap(),
        );
        assert!(
            pass_2pct(maxima[2].unwrap(), provider_price)
                && pass_2pct(maxima[3].unwrap(), provider_price)
        );
        let provider_fdv = ratio(
            fixture["provider_reference"]["peak_supply_fdv_exact_usd"]
                .as_str()
                .unwrap(),
        );
        assert!(
            pass_2pct(maxima[0].unwrap(), provider_fdv)
                && pass_2pct(maxima[1].unwrap(), provider_fdv)
        );
        let width = ratio(
            proof["combined_interval_width_over_upper"]
                .as_str()
                .unwrap(),
        );
        assert!(
            width.0 * U512::from(100) <= width.1 * U512::from(2)
                && proof["combined_interval_width_within_2pct"] == true
        );
        assert_ne!(
            fixture["historical_supply"]["supply_at_peak_plus3_window_end_raw"],
            fixture["historical_supply"]["current_account_supply_raw_not_used"]
        );
        assert!(
            fixture["scope"]["limits"]
                .as_str()
                .unwrap()
                .contains("not all venues")
        );
        assert!(
            cert["scope"]
                .as_str()
                .unwrap()
                .contains("Interval width does not bound unknown-market/state error")
        );
    }

    #[test]
    fn solana_tracker_jc_two_pool_cross_quote_fdv_vq_models_replay_exactly() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_jc_cross_quote_fdv_window_case.json"
        ))
        .unwrap();
        let proof_raw = fixture["proof_json_raw"].as_str().unwrap();
        let math_raw = fixture["root_math_audit_json_raw"].as_str().unwrap();
        let gate_raw = fixture["independent_raw_gate_json_raw"].as_str().unwrap();
        let cert_raw = fixture["root_certificate_json_raw"].as_str().unwrap();
        let hash = |raw: &str| format!("{:x}", Sha256::digest(raw.as_bytes()));
        assert_eq!(hash(proof_raw), fixture["observed_proof_sha256"]);
        assert_eq!(
            hash(proof_raw),
            "c3ff61d31681134b71d76e38636934aba14e374d8f59a3afcdd24d976fd52fdb"
        );
        assert_eq!(hash(math_raw), fixture["root_math_audit_sha256"]);
        assert_eq!(
            hash(math_raw),
            "651bdd167428abfc3e3935a98a0f3c76742fd0c2ae89b509314f37d1938203af"
        );
        assert_eq!(
            hash(gate_raw),
            "e2da976cea2b78e9a061da004308c5695855f4f72cf0a8c53b7a8bd047ebfb2d"
        );
        assert_eq!(
            hash(cert_raw),
            "9d968b747ca3d5522eb56a5020968a605e6a6b3e510daa627c259f017562d2e7"
        );
        assert_eq!(hash(cert_raw), fixture["source_sha256"]["root_certificate"]);
        let (proof, math, gate, cert): (
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
            serde_json::Value,
        ) = (
            serde_json::from_str(proof_raw).unwrap(),
            serde_json::from_str(math_raw).unwrap(),
            serde_json::from_str(gate_raw).unwrap(),
            serde_json::from_str(cert_raw).unwrap(),
        );
        assert_eq!(
            fixture["schema"],
            "solana-tracker-jc-two-pool-cross-quote-fdv-window-case-v1"
        );
        assert_eq!(fixture["root_certificate_pending"], false);
        assert_eq!(
            fixture["source"],
            "onchain-selected-pool-cross-quote-observed-fdv-window-vq-model-cex-v1"
        );
        for (fixture_key, gate_key) in [
            ("current_mint_request", "batch11_accounts_request"),
            ("current_mint_response", "batch11_accounts_response"),
            ("birth_transaction", "birth_transaction"),
            ("supply_ledger", "supply_ledger"),
        ] {
            let body_key = format!("{fixture_key}_json_raw");
            let raw = fixture[body_key].as_str().unwrap();
            let digest = hash(raw);
            assert_eq!(digest, fixture["source_sha256"][fixture_key]);
            assert_eq!(digest, gate["input_sha256"][gate_key]["sha256"]);
        }
        assert_eq!(cert["raw_replay_byte_identical"], true);
        assert_eq!(
            (cert["observed_marks"].as_u64(), cert["vq_models"].as_u64()),
            (Some(5), Some(2))
        );
        assert_eq!(
            (
                cert["network_calls"].as_u64(),
                cert["database_writes"].as_u64()
            ),
            (Some(0), Some(0))
        );
        assert_eq!(
            proof["schema"],
            "solana-jc-two-pool-observed-fdv-feasibility-v3"
        );
        assert_eq!(
            proof["scope"]["target_mint"],
            "JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump"
        );
        assert_eq!(
            proof["scope"]["quote_mint"],
            "87eaXHno6VtRXUyuYpSVm8oYpSpHjLMQE9mvbcnKpump"
        );
        assert_eq!(
            proof["source_hashes"]["idl"],
            "b7d8c57a4d9c4dd0109a9ab893052352333252d4eedced10ac091d4d66cab89b"
        );
        assert_eq!(
            proof["idl_temporal_witness"]["captured_at_commit"],
            "2293f9a66c654e9fe82dc5e8f4618538f24bb35f"
        );
        assert_eq!(
            (proof["fx"]["low"].as_str(), proof["fx"]["high"].as_str()),
            (Some("110.31"), Some("110.38"))
        );
        assert_eq!(
            cert["provider_price_usd_decimal"],
            gate["price_model"]["global_price_check"]["tracker_peak_usd"]
        );
        assert_eq!(
            proof["event_decode"]["cva_buy"]["buy_event_cpi_payload_matches_log"],
            true
        );
        assert_eq!(
            proof["supply_chronology"]["cva_same_transaction_burn"]["burn_precedes_buy_event_cpi"],
            true
        );
        let request: serde_json::Value =
            serde_json::from_str(fixture["current_mint_request_json_raw"].as_str().unwrap())
                .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(fixture["current_mint_response_json_raw"].as_str().unwrap())
                .unwrap();
        assert_eq!(request["method"], "getMultipleAccounts");
        assert_eq!(request["params"][1]["encoding"], "base64");
        assert_eq!(response["result"]["context"]["slot"], 456655008);
        let keys = request["params"][0].as_array().unwrap();
        let mint = "JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump";
        let mint_index = keys.iter().position(|key| key == mint).unwrap();
        assert_eq!(mint_index, 18);
        let account = &response["result"]["value"][mint_index];
        assert_eq!(
            account["owner"],
            "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
        );
        assert_eq!(account["space"], 408);
        assert_eq!(
            (
                cert["current_mint"]["decimals"].as_u64(),
                cert["current_mint"]["mint_authority"].is_null(),
                cert["current_mint"]["freeze_authority"].is_null(),
                cert["current_mint"]["initialized"].as_bool(),
                cert["current_mint"]["extensions"][0]["type"].as_u64(),
                cert["current_mint"]["extensions"][0]["length"].as_u64(),
                cert["current_mint"]["extensions"][1]["type"].as_u64(),
                cert["current_mint"]["extensions"][1]["length"].as_u64(),
            ),
            (
                Some(6),
                true,
                true,
                Some(true),
                Some(18),
                Some(64),
                Some(19),
                Some(170),
            )
        );
        let decode_base64 = |encoded: &str| {
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let (mut acc, mut bits, mut bytes) = (0_u32, 0_u32, Vec::new());
            for byte in encoded.bytes().filter(|byte| *byte != b'=') {
                acc = (acc << 6)
                    | u32::try_from(alphabet.iter().position(|c| *c == byte).unwrap()).unwrap();
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    bytes.push(u8::try_from(acc >> bits).unwrap());
                    acc &= (1 << bits) - 1;
                }
            }
            bytes
        };
        let mint_bytes = decode_base64(account["data"][0].as_str().unwrap());
        assert_eq!(mint_bytes.len(), 408);
        assert_eq!(
            format!("{:x}", Sha256::digest(&mint_bytes)),
            cert["current_mint"]["raw_sha256"]
        );
        let quote_req_raw = fixture["quote_pool_snapshot_request_json_raw"]
            .as_str()
            .unwrap();
        let quote_resp_raw = fixture["quote_pool_snapshot_response_json_raw"]
            .as_str()
            .unwrap();
        assert_eq!(
            hash(quote_req_raw),
            fixture["source_sha256"]["quote_pool_snapshot_request"]
        );
        assert_eq!(
            hash(quote_req_raw),
            "2077c827f6e89fc18c05b75e43b6a8b150bbddc4625806275c523ed2b6d3c32a"
        );
        assert_eq!(
            hash(quote_resp_raw),
            fixture["source_sha256"]["quote_pool_snapshot_response"]
        );
        assert_eq!(
            hash(quote_resp_raw),
            gate["input_sha256"]["account_snapshot"]["sha256"]
        );
        assert_eq!(
            hash(quote_resp_raw),
            cert["raw_source_sha256"][".scout-data/diagnostics/solana-jc-quote-vault-window-2026-10-14/raw/01-accounts-finalized.response.json"]
        );
        assert_eq!(
            u64::from_le_bytes(mint_bytes[36..44].try_into().unwrap()),
            965_761_089_271_256
        );
        assert_eq!((mint_bytes[44], mint_bytes[45], mint_bytes[165]), (6, 1, 1));
        assert_eq!(&mint_bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(&mint_bytes[46..50], &[0, 0, 0, 0]);
        let mut offset = 166;
        let mut extensions = Vec::new();
        while offset < mint_bytes.len() {
            let kind = u16::from_le_bytes(mint_bytes[offset..offset + 2].try_into().unwrap());
            let len = u16::from_le_bytes(mint_bytes[offset + 2..offset + 4].try_into().unwrap());
            extensions.push((kind, len));
            offset += 4 + usize::from(len);
        }
        assert_eq!(extensions, [(18, 64), (19, 170)]);
        assert_eq!(offset, mint_bytes.len());
        let pool_request: serde_json::Value = serde_json::from_str(quote_req_raw).unwrap();
        let pool_response: serde_json::Value = serde_json::from_str(quote_resp_raw).unwrap();
        let pool_keys = pool_request["params"][0].as_array().unwrap();
        let cf3_index = pool_keys
            .iter()
            .position(|key| key == "CF3ML9g28j2bdcKZ4jDqtffomELUhcxPCSTtbcmpQ7JX")
            .unwrap();
        let cf3_bytes = decode_base64(
            pool_response["result"]["value"][cf3_index]["data"][0]
                .as_str()
                .unwrap(),
        );
        let unparsed = usize::try_from(
            cert["unparsed_quote_pool_zero_tail_bytes"]
                .as_u64()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            (cf3_bytes.len(), cf3_bytes.len() - unparsed, unparsed),
            (301, 287, 14)
        );
        assert!(cf3_bytes[287..].iter().all(|byte| *byte == 0));

        let birth_raw = fixture["birth_transaction_json_raw"].as_str().unwrap();
        let birth: serde_json::Value = serde_json::from_str(birth_raw).unwrap();
        let birth_result = &birth["result"];
        let birth_sig = "Rn3xkFNW9jn1dBG2viwAwffVqxGWxLbFWPBWX6BJe1HD6nPep6AYfhLUPCYNY1wrxWFDAQEQQtr7PvRAVLDHQ6F";
        assert_eq!(birth_result["transaction"]["signatures"][0], birth_sig);
        assert_eq!(
            (
                birth_result["slot"].as_u64(),
                birth_result["transactionIndex"].as_u64(),
                birth_result["blockTime"].as_u64()
            ),
            (Some(455_422_883), Some(707), Some(1_791_671_908))
        );
        assert!(birth_result["meta"]["err"].is_null());
        let ix = birth_result["meta"]["innerInstructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| group["index"] == 3)
            .unwrap()["instructions"]
            .as_array()
            .unwrap()
            .clone();
        let typed: Vec<_> = ix
            .iter()
            .filter_map(|instruction| {
                instruction
                    .get("parsed")
                    .and_then(|p| p.get("type"))
                    .and_then(serde_json::Value::as_str)
                    .map(|kind| (kind, &instruction["parsed"]["info"]))
            })
            .collect();
        let initialized = typed
            .iter()
            .find(|(kind, _)| *kind == "initializeMint2")
            .unwrap()
            .1;
        let minted = typed.iter().find(|(kind, _)| *kind == "mintTo").unwrap().1;
        let revoked = typed
            .iter()
            .find(|(kind, _)| *kind == "setAuthority")
            .unwrap()
            .1;
        assert_eq!(
            (
                initialized["mint"].as_str(),
                initialized["decimals"].as_u64()
            ),
            (Some(mint), Some(6))
        );
        assert_eq!(minted["amount"], "1000000000000000");
        assert_eq!(minted["mint"], mint);
        assert_eq!(revoked["authorityType"], "mintTokens");
        assert!(revoked["newAuthority"].is_null());

        let ledger: serde_json::Value =
            serde_json::from_str(fixture["supply_ledger_json_raw"].as_str().unwrap()).unwrap();
        let burns = ledger["burns"].as_array().unwrap();
        assert_eq!(burns.len(), 30);
        let mut burned = U512::from(0);
        let mut previous = (0_u64, 0_u64);
        let mut signatures = std::collections::BTreeSet::new();
        for burn in burns {
            let position = (
                burn["slot"].as_u64().unwrap(),
                burn["transaction_index"].as_u64().unwrap(),
            );
            assert!(position >= previous);
            previous = position;
            assert_eq!(
                (
                    burn["kind"].as_str(),
                    burn["program"].as_str(),
                    burn["location"].as_str(),
                    burn["instruction_index"].as_u64()
                ),
                (Some("Burn"), Some("token-2022"), Some("inner:2"), Some(1))
            );
            assert!(signatures.insert(burn["signature"].as_str().unwrap()));
            burned += U512::from_str_radix(burn["amount_raw"].as_str().unwrap(), 10).unwrap();
        }
        assert_eq!(
            burned.to_string(),
            ledger["burn_total_raw"].as_str().unwrap()
        );
        assert_eq!(
            U512::from(1_000_000_000_000_000_u64) - burned,
            U512::from(965_761_089_271_256_u64)
        );
        let cva_burn = burns
            .iter()
            .find(|burn| burn["signature"] == proof["event_decode"]["cva_buy"]["signature"])
            .unwrap();
        assert_eq!(
            (
                cva_burn["slot"].as_u64(),
                cva_burn["transaction_index"].as_u64(),
                cva_burn["instruction_index"].as_u64()
            ),
            (Some(455_424_461), Some(321), Some(1))
        );
        assert_eq!(cva_burn["amount_raw"], "1188170011339");
        assert_eq!(ledger["post_birth_mint_to_count"], 0);
        assert_eq!(
            ledger["current_finalized_anchor_raw_supply"],
            "965761089271256"
        );
        assert_eq!(
            gate["pool_and_mint_identities"]["CF3_pool"]["_unparsed_zero_tail_bytes"],
            14
        );
        assert_eq!(cert["unparsed_quote_pool_zero_tail_bytes"], 14);
        assert_eq!(cert["conditional_vq_model"], true);
        assert_eq!(cert["deployment_revision_verified"], false);
        assert_eq!(
            cert["unknown_model_market_state_error_not_bounded_by_reference_width"],
            true
        );
        assert_eq!(cert["annual_allmarket_lifetime_complete"], false);

        let fraction = |text: &str| {
            let (n, d) = text.split_once('/').unwrap();
            (
                U512::from_str_radix(n, 10).unwrap(),
                U512::from_str_radix(d, 10).unwrap(),
            )
        };
        let compare = |a: (U512, U512), b: (U512, U512)| (a.0 * b.1).cmp(&(b.0 * a.1));
        let same = |actual: (U512, U512), expected: (U512, U512)| {
            assert_eq!(compare(actual, expected), std::cmp::Ordering::Equal)
        };
        let ceil = |n: U512, d: U512| (n + d - U512::from(1)) / d;
        let decimal_ratio = |text: &str| {
            let value = ExactDecimal::parse(text).unwrap();
            let denominator = (0..u32::try_from(value.scale).unwrap())
                .try_fold(U512::from(1), |n, _| n.checked_mul(U512::from(10)))
                .unwrap();
            (U512::from(value.coefficient), denominator)
        };
        let marks = proof["observed_marks"].as_array().unwrap();
        assert_eq!(marks.len(), 5);
        let fx = [
            (U512::from(11031), U512::from(100)),
            (U512::from(11038), U512::from(100)),
        ];
        let models = ["pre_event_field", "post_event_field_sensitivity"];
        type Fraction = (U512, U512);
        type Maxima<'a> = std::collections::BTreeMap<(&'a str, &'a str), (&'a str, Fraction)>;
        let mut maxima = Maxima::new();
        let mut price_maxima = maxima.clone();

        for mark in marks {
            let supply = U512::from_str_radix(mark["supply_raw"].as_str().unwrap(), 10).unwrap();
            for model in models {
                let alternate = model == "post_event_field_sensitivity";
                let target_quote = if alternate {
                    &mark["post_event_field_sensitivity"]["target_effective_quote_raw"]
                } else {
                    &mark["target_effective_quote_raw"]
                };
                let cross_quote = if alternate {
                    &mark["post_event_field_sensitivity"]["quote_effective_quote_raw"]
                } else {
                    &mark["cf3_effective_quote_raw"]
                };
                let base =
                    U512::from_str_radix(mark["target_base_raw"].as_str().unwrap(), 10).unwrap();
                let cross_base =
                    U512::from_str_radix(mark["cf3_base_raw"].as_str().unwrap(), 10).unwrap();
                let model_fractions = if alternate {
                    &mark["post_event_field_sensitivity"]["exact_fraction_numerators"]
                } else {
                    &mark["exact_fraction_numerators"]
                };
                for (end, (fx_n, fx_d)) in ["low", "high"].into_iter().zip(fx) {
                    let price = (
                        U512::from_str_radix(target_quote.as_str().unwrap(), 10).unwrap()
                            * U512::from_str_radix(cross_quote.as_str().unwrap(), 10).unwrap()
                            * fx_n,
                        base * cross_base * U512::from(1000) * fx_d,
                    );
                    let price_expected = fraction(&format!(
                        "{}/{}",
                        model_fractions[format!("jc_price_{end}")].as_str().unwrap(),
                        model_fractions[format!("jc_price_{end}_denominator")]
                            .as_str()
                            .unwrap()
                    ));
                    same(price, price_expected);
                    let price_key = (
                        model,
                        if end == "low" {
                            "price_low"
                        } else {
                            "price_high"
                        },
                    );
                    if price_maxima
                        .get(&price_key)
                        .is_none_or(|(_, old)| compare(price, *old).is_gt())
                    {
                        price_maxima.insert(price_key, (mark["name"].as_str().unwrap(), price));
                    }
                    let fdv = (price.0 * supply, price.1 * U512::from(1_000_000));
                    let fdv_expected = fraction(&format!(
                        "{}/{}",
                        model_fractions[format!("fdv_{end}")].as_str().unwrap(),
                        model_fractions[format!("fdv_{end}_denominator")]
                            .as_str()
                            .unwrap()
                    ));
                    same(fdv, fdv_expected);
                    let key = (model, if end == "low" { "fdv_low" } else { "fdv_high" });
                    if maxima
                        .get(&key)
                        .is_none_or(|(_, old)| compare(fdv, *old).is_gt())
                    {
                        maxima.insert(key, (mark["name"].as_str().unwrap(), fdv));
                    }
                    let rounded = if end == "low" {
                        fdv.0 * U512::from(100) / fdv.1
                    } else {
                        ceil(fdv.0 * U512::from(100), fdv.1)
                    };
                    if !alternate {
                        let field = if end == "low" {
                            "low_floor"
                        } else {
                            "high_ceil"
                        };
                        assert_eq!(
                            rounded,
                            U512::from_str_radix(mark["fdv_cents"][field].as_str().unwrap(), 10)
                                .unwrap()
                        );
                    }
                }
            }
        }

        for model in models {
            for (set, endpoints) in [
                (&maxima, ["fdv_low", "fdv_high"]),
                (&price_maxima, ["price_low", "price_high"]),
            ] {
                for endpoint in endpoints {
                    let (name, actual) = set[&(model, endpoint)];
                    let expected = fraction(
                        math["models_exact_global_maxima"][model][endpoint]["fraction"]
                            .as_str()
                            .unwrap(),
                    );
                    same(actual, expected);
                    assert_eq!(
                        name,
                        math["models_exact_global_maxima"][model][endpoint]["mark"]
                    );
                }
            }
        }
        let tracker_price = decimal_ratio(cert["provider_price_usd_decimal"].as_str().unwrap());
        for model in models {
            for endpoint in ["price_low", "price_high"] {
                let price = price_maxima[&(model, endpoint)].1;
                assert!(
                    price.0 * tracker_price.1 * U512::from(100)
                        <= tracker_price.0 * price.1 * U512::from(102)
                );
            }
        }
        let low_models = [
            maxima[&("pre_event_field", "fdv_low")].1,
            maxima[&("post_event_field_sensitivity", "fdv_low")].1,
        ];
        let high_models = [
            maxima[&("pre_event_field", "fdv_high")].1,
            maxima[&("post_event_field_sensitivity", "fdv_high")].1,
        ];
        let lower = *low_models.iter().min_by(|a, b| compare(**a, **b)).unwrap();
        let upper = *high_models.iter().max_by(|a, b| compare(**a, **b)).unwrap();
        same(
            lower,
            fraction(math["lower_usd_fraction"].as_str().unwrap()),
        );
        same(
            upper,
            fraction(math["upper_usd_fraction"].as_str().unwrap()),
        );
        assert_eq!(lower.0 * U512::from(100) / lower.1, U512::from(22_707_992));
        assert_eq!(
            ceil(upper.0 * U512::from(100), upper.1),
            U512::from(22_722_406)
        );
        assert_ne!(lower.0 * U512::from(100) / lower.1, U512::from(22_707_995));
        let width = fraction(
            math["combined_reference_width_including_floor"]
                .as_str()
                .unwrap(),
        );
        assert!(width.0 * U512::from(100) < width.1 * U512::from(2));
        assert_eq!(math["conditional_vq_model"], true);
        assert_eq!(
            math["unknown_model_market_state_error_not_bounded_by_reference_width"],
            true
        );
        assert_eq!(math["annual_allmarket_lifetime_complete"], false);
        assert_eq!(
            cert["scope"]["states"],
            "five observed chronological target/quote pool marginal states only; not lifetime ATH"
        );
        assert_eq!(cert["unparsed_quote_pool_zero_tail_bytes"], 14);
        assert_eq!(cert["conditional_vq_model"], true);
        assert_eq!(cert["deployment_revision_verified"], false);
        assert_eq!(
            cert["unknown_model_market_state_error_not_bounded_by_reference_width"],
            true
        );
        assert_eq!(cert["annual_allmarket_lifetime_complete"], false);
        assert_eq!(cert["provider_peak_timestamp_ms"], 1_791_672_254_212_u64);
        assert_eq!(
            proof["global_results"]["both_global_price_endpoints_within_2pct_of_tracker_peak"],
            true
        );
        assert!(proof["limitations"].as_array().unwrap().iter().any(|item| {
            item.as_str()
                .unwrap()
                .contains("not all pools, all venues, lifetime maximum")
        }));
        assert!(
            proof["reserve_and_fee_reconciliation"]["target_vq_field_convention"]
                .as_str()
                .unwrap()
                .contains("conditional on that documented deployed rule")
        );
    }

    #[test]
    fn solana_tracker_doy_full_neighborhood_marks_and_supply_match() {
        use alloy_primitives::U512;
        use sha2::{Digest, Sha256};
        use std::collections::BTreeSet;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_doy_legacy_neighborhood_case.json"
        ))
        .unwrap();
        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap()
        };
        let ratio = |value: &str| {
            let (numerator, denominator) = value.split_once('/').unwrap();
            (
                U512::from_str_radix(numerator, 10).unwrap(),
                U512::from_str_radix(denominator, 10).unwrap(),
            )
        };
        let cmp_ratio =
            |left: (U512, U512), right: (U512, U512)| (left.0 * right.1).cmp(&(right.0 * left.1));

        let verify_window = |row: &serde_json::Value,
                             expected_state_count: usize,
                             expected_status: &str,
                             expected_match: bool| {
            let mint = row["mint"].as_str().unwrap();
            let pool = row["pool"].as_str().unwrap();
            let provider =
                ExactDecimal::parse(row["provider_price_usd"].as_str().unwrap()).unwrap();
            let provider_num = U512::from(provider.coefficient);
            let provider_den = pow10(u32::try_from(provider.scale).unwrap());
            let states = row["verified_states"].as_array().unwrap();
            assert_eq!(states.len(), expected_state_count);
            let mut signatures = BTreeSet::new();
            let mut marks = Vec::with_capacity(states.len());

            for (index, state) in states.iter().enumerate() {
                assert!(state["matched_event_cpi"].as_bool().unwrap());
                assert_eq!(state["mint"].as_str(), Some(mint));
                assert_eq!(state["pool"].as_str(), Some(pool));
                assert_eq!(
                    state["quote_mint"].as_str(),
                    Some("So11111111111111111111111111111111111111112")
                );
                assert_eq!(
                    state["event_layout"].as_str(),
                    Some("historical_legacy_no_virtual_quote_field")
                );
                assert!(state["event_virtual_quote_raw"].is_null());
                signatures.insert(state["signature"].as_str().unwrap());

                let event_fields = &state["event_fields"];
                let (event_name, event_timestamp, pool_base, pool_quote) = if let Some(data) =
                    state["event_cpi_data_hex"].as_str()
                {
                    let instruction = raw_instruction_from_hex(data);
                    let digest = format!("{:x}", Sha256::digest(&instruction.data[8..]));
                    assert_eq!(state["event_data_sha256"].as_str(), Some(digest.as_str()));
                    match scout_dex_solana::classify_pump_amm_event(&instruction) {
                        scout_dex_solana::PumpAmmEventOutcome::Trade(
                            scout_dex_solana::PumpAmmEvent::Buy(event),
                        ) => {
                            assert_eq!(state["event"].as_str(), Some("BuyEvent"));
                            assert_eq!(bs58::encode(event.pool).into_string(), pool);
                            assert_eq!(
                                bs58::encode(event.user).into_string(),
                                event_fields["user"]
                            );
                            assert_eq!(
                                bs58::encode(event.user_base_token_account).into_string(),
                                event_fields["user_base_token_account"]
                            );
                            assert_eq!(
                                bs58::encode(event.user_quote_token_account).into_string(),
                                event_fields["user_quote_token_account"]
                            );
                            assert_eq!(event.cashback_fee_basis_points, Some(0));
                            assert_eq!(event.cashback, Some(0));
                            assert_eq!(event.virtual_quote_reserves, None);
                            assert_eq!(event.ix_name.as_deref(), event_fields["ix_name"].as_str());
                            assert_eq!(
                                event.base_amount_out,
                                event_fields["base_amount_out"].as_u64().unwrap()
                            );
                            assert_eq!(
                                event.quote_amount_in,
                                event_fields["quote_amount_in"].as_u64().unwrap()
                            );
                            assert_eq!(
                                event.user_quote_amount_in,
                                event_fields["user_quote_amount_in"].as_u64().unwrap()
                            );
                            assert_eq!(event.lp_fee, event_fields["lp_fee"].as_u64().unwrap());
                            assert_eq!(
                                event.protocol_fee,
                                event_fields["protocol_fee"].as_u64().unwrap()
                            );
                            assert_eq!(
                                event.coin_creator_fee,
                                event_fields["coin_creator_fee"].as_u64().unwrap()
                            );
                            assert_eq!(
                                event.timestamp,
                                i64::try_from(state["block_time_unix_s"].as_u64().unwrap())
                                    .unwrap()
                            );
                            assert_eq!(
                                event.pool_base_token_reserves.to_string(),
                                event_fields["pool_base_token_reserves"]
                                    .as_u64()
                                    .unwrap()
                                    .to_string()
                            );
                            assert_eq!(
                                event.pool_quote_token_reserves.to_string(),
                                event_fields["pool_quote_token_reserves"]
                                    .as_u64()
                                    .unwrap()
                                    .to_string()
                            );
                            (
                                "BuyEvent",
                                event.timestamp,
                                U512::from(event.pool_base_token_reserves),
                                U512::from(event.pool_quote_token_reserves),
                            )
                        }
                        scout_dex_solana::PumpAmmEventOutcome::Trade(
                            scout_dex_solana::PumpAmmEvent::Sell(event),
                        ) => {
                            assert_eq!(state["event"].as_str(), Some("SellEvent"));
                            assert_eq!(bs58::encode(event.pool).into_string(), pool);
                            assert_eq!(
                                bs58::encode(event.user).into_string(),
                                event_fields["user"]
                            );
                            assert_eq!(
                                bs58::encode(event.user_base_token_account).into_string(),
                                event_fields["user_base_token_account"]
                            );
                            assert_eq!(
                                bs58::encode(event.user_quote_token_account).into_string(),
                                event_fields["user_quote_token_account"]
                            );
                            assert_eq!(event.cashback_fee_basis_points, Some(0));
                            assert_eq!(event.cashback, Some(0));
                            assert_eq!(event.virtual_quote_reserves, None);
                            assert_eq!(
                                event.timestamp,
                                i64::try_from(state["block_time_unix_s"].as_u64().unwrap())
                                    .unwrap()
                            );
                            assert_eq!(
                                event.pool_base_token_reserves.to_string(),
                                event_fields["pool_base_token_reserves"]
                                    .as_u64()
                                    .unwrap()
                                    .to_string()
                            );
                            assert_eq!(
                                event.pool_quote_token_reserves.to_string(),
                                event_fields["pool_quote_token_reserves"]
                                    .as_u64()
                                    .unwrap()
                                    .to_string()
                            );
                            (
                                "SellEvent",
                                event.timestamp,
                                U512::from(event.pool_base_token_reserves),
                                U512::from(event.pool_quote_token_reserves),
                            )
                        }
                        other => panic!(
                            "captured Doy event CPI was not classified as a trade: {other:?}"
                        ),
                    }
                } else {
                    assert!(
                        !expected_match,
                        "the positive Doy candidate requires production CPI parsing"
                    );
                    assert_eq!(state["matched_event_cpi"].as_bool(), Some(true));
                    assert!(event_fields.get("cashback_fee_basis_points").is_none());
                    assert!(event_fields.get("cashback").is_none());
                    assert_eq!(
                        event_fields["timestamp"].as_u64(),
                        state["block_time_unix_s"].as_u64()
                    );
                    assert_eq!(
                        event_fields["pool_base_token_reserves"]
                            .as_u64()
                            .unwrap()
                            .to_string(),
                        state["event_reserves_raw"]["base"].as_str().unwrap()
                    );
                    assert_eq!(
                        event_fields["pool_quote_token_reserves"]
                            .as_u64()
                            .unwrap()
                            .to_string(),
                        state["event_reserves_raw"]["quote"].as_str().unwrap()
                    );
                    (
                        state["event"].as_str().unwrap(),
                        i64::try_from(event_fields["timestamp"].as_u64().unwrap()).unwrap(),
                        U512::from(event_fields["pool_base_token_reserves"].as_u64().unwrap()),
                        U512::from(event_fields["pool_quote_token_reserves"].as_u64().unwrap()),
                    )
                };
                assert_eq!(state["event"].as_str(), Some(event_name));
                assert_eq!(
                    u64::try_from(event_timestamp).unwrap(),
                    state["block_time_unix_s"].as_u64().unwrap()
                );
                assert_eq!(
                    event_fields["timestamp"].as_u64(),
                    state["block_time_unix_s"].as_u64()
                );
                let pre_base =
                    U512::from_str_radix(state["pre_vault_raw"]["base"].as_str().unwrap(), 10)
                        .unwrap();
                let pre_quote =
                    U512::from_str_radix(state["pre_vault_raw"]["quote"].as_str().unwrap(), 10)
                        .unwrap();
                let post_base =
                    U512::from_str_radix(state["post_vault_raw"]["base"].as_str().unwrap(), 10)
                        .unwrap();
                let post_quote =
                    U512::from_str_radix(state["post_vault_raw"]["quote"].as_str().unwrap(), 10)
                        .unwrap();
                assert_eq!((pool_base, pool_quote), (pre_base, pre_quote));
                assert_eq!(
                    state["event_reserves_raw"]["base"].as_str(),
                    Some(pre_base.to_string().as_str())
                );
                assert_eq!(
                    state["event_reserves_raw"]["quote"].as_str(),
                    Some(pre_quote.to_string().as_str())
                );
                let (base, quote) = match state["state_side"].as_str().unwrap() {
                    "pre" => (pre_base, pre_quote),
                    "post" => (post_base, post_quote),
                    other => panic!("unknown state side {other}"),
                };
                assert_eq!(
                    state["state_vault_raw"]["base"].as_str(),
                    Some(base.to_string().as_str())
                );
                assert_eq!(
                    state["state_vault_raw"]["quote"].as_str(),
                    Some(quote.to_string().as_str())
                );
                assert!(base > U512::ZERO && quote > U512::ZERO);

                let spot = (
                    quote * U512::from(1_000_000_u64),
                    base * U512::from(1_000_000_000_u64),
                );
                assert_eq!(
                    cmp_ratio(spot, ratio(state["native_spot_fraction"].as_str().unwrap())),
                    std::cmp::Ordering::Equal
                );
                let candle = state["coinbase_candle"].as_array().unwrap();
                assert_eq!(
                    candle[0].as_str(),
                    Some(
                        state["coinbase_minute_unix_s"]
                            .as_u64()
                            .unwrap()
                            .to_string()
                            .as_str()
                    )
                );
                assert_eq!(
                    event_timestamp.div_euclid(60) * 60,
                    i64::try_from(state["coinbase_minute_unix_s"].as_u64().unwrap()).unwrap()
                );
                let fx_low = ExactDecimal::parse(candle[1].as_str().unwrap()).unwrap();
                let fx_high = ExactDecimal::parse(candle[2].as_str().unwrap()).unwrap();
                let low = (
                    spot.0 * U512::from(fx_low.coefficient),
                    spot.1 * pow10(u32::try_from(fx_low.scale).unwrap()),
                );
                let high = (
                    spot.0 * U512::from(fx_high.coefficient),
                    spot.1 * pow10(u32::try_from(fx_high.scale).unwrap()),
                );
                assert_eq!(
                    cmp_ratio(low, ratio(state["usd_mark_low_fraction"].as_str().unwrap())),
                    std::cmp::Ordering::Equal
                );
                assert_eq!(
                    cmp_ratio(
                        high,
                        ratio(state["usd_mark_high_fraction"].as_str().unwrap())
                    ),
                    std::cmp::Ordering::Equal
                );
                marks.push((index, low, high));
            }

            assert_eq!(signatures.len(), expected_state_count / 2);
            let global = &row["global_minus3_to_plus3_pre_post_maxima"];
            let low_index = marks
                .iter()
                .enumerate()
                .max_by(|left, right| cmp_ratio(left.1.1, right.1.1))
                .unwrap()
                .0;
            let high_index = marks
                .iter()
                .enumerate()
                .max_by(|left, right| cmp_ratio(left.1.2, right.1.2))
                .unwrap()
                .0;
            let maximum_low = marks[low_index].1;
            let maximum_high = marks[high_index].2;
            assert_eq!(
                cmp_ratio(
                    maximum_low,
                    ratio(global["low"]["usd_mark_low_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                cmp_ratio(
                    maximum_high,
                    ratio(global["high"]["usd_mark_high_fraction"].as_str().unwrap())
                ),
                std::cmp::Ordering::Equal
            );
            let low_state = states
                .iter()
                .find(|state| {
                    state["signature"] == global["low"]["signature"]
                        && state["state_side"] == global["low"]["state_side"]
                })
                .unwrap();
            let high_state = states
                .iter()
                .find(|state| {
                    state["signature"] == global["high"]["signature"]
                        && state["state_side"] == global["high"]["state_side"]
                })
                .unwrap();
            assert_eq!(
                cmp_ratio(
                    ratio(low_state["usd_mark_low_fraction"].as_str().unwrap()),
                    maximum_low
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                cmp_ratio(
                    ratio(high_state["usd_mark_high_fraction"].as_str().unwrap()),
                    maximum_high
                ),
                std::cmp::Ordering::Equal
            );
            assert_eq!(
                low_state["offset_from_provider_peak_s"],
                global["low"]["offset_from_provider_peak_s"]
            );
            assert_eq!(
                high_state["offset_from_provider_peak_s"],
                global["high"]["offset_from_provider_peak_s"]
            );

            let within_two_percent = |mark: (U512, U512)| {
                let difference = (mark.0 * provider_den).abs_diff(provider_num * mark.1);
                difference * U512::from(50) <= provider_num * mark.1
            };
            let low_matches = within_two_percent(marks[low_index].1);
            let high_matches = within_two_percent(marks[high_index].2);
            assert_eq!(low_matches, global["low_within_2pct"].as_bool().unwrap());
            assert_eq!(high_matches, global["high_within_2pct"].as_bool().unwrap());
            assert_eq!(low_matches, expected_match);
            assert_eq!(high_matches, expected_match);
            assert_eq!(row["candidate_status"].as_str(), Some(expected_status));

            if !expected_match {
                for side in ["pre", "post"] {
                    let indices: Vec<usize> = marks
                        .iter()
                        .enumerate()
                        .filter_map(|(mark_index, (state_index, _, _))| {
                            (states[*state_index]["state_side"] == side).then_some(mark_index)
                        })
                        .collect();
                    let side_low = *indices
                        .iter()
                        .max_by(|left, right| cmp_ratio(marks[**left].1, marks[**right].1))
                        .unwrap();
                    let side_high = *indices
                        .iter()
                        .max_by(|left, right| cmp_ratio(marks[**left].2, marks[**right].2))
                        .unwrap();
                    assert!(!within_two_percent(marks[side_low].1));
                    assert!(!within_two_percent(marks[side_high].2));
                }
            }
        };

        let row = &fixture["row"];
        assert_eq!(
            fixture["source"].as_str(),
            Some("solana-tracker-window-peak-supply-legacy-cashback-pool-fx-validated-v1")
        );
        assert!(fixture["ath_at"].is_null());
        assert_eq!(
            row["mint"].as_str(),
            Some("DoyZoNkB5oiu9epD3xXSKSKkHqC6c8v6xVuEgwSJpump")
        );
        assert_eq!(
            row["pool"].as_str(),
            Some("HBw7L7ppu9aHMZDqd2f4JXKUofs41caL4XE6NbZAu5EQ")
        );
        assert_eq!(
            row["candidate_status"].as_str(),
            Some("both_endpoints_within_2pct")
        );
        assert_eq!(row["diagnostic_gaps"].as_array().unwrap().len(), 0);
        assert_eq!(
            row["maxima_temporal_scope"]["low_offset_from_peak_s"].as_i64(),
            Some(0)
        );
        assert_eq!(
            row["maxima_temporal_scope"]["high_offset_from_peak_s"].as_i64(),
            Some(0)
        );
        assert_eq!(
            row["global_minus3_to_plus3_pre_post_maxima"]["low"]["state_side"].as_str(),
            Some("post")
        );
        assert_eq!(
            row["global_minus3_to_plus3_pre_post_maxima"]["high"]["state_side"].as_str(),
            Some("post")
        );
        verify_window(row, 172, "both_endpoints_within_2pct", true);

        let negative = &fixture["negative_price_control"];
        assert_eq!(
            negative["mint"].as_str(),
            Some("27ES77p62B4edFBnGatr7NfnpudwwAkFC9eM2wQBpump")
        );
        verify_window(negative, 68, "one_or_both_endpoints_outside_2pct", false);

        let selected_signature = row["global_minus3_to_plus3_pre_post_maxima"]["high"]["signature"]
            .as_str()
            .unwrap();
        let selected_buy = row["verified_states"]
            .as_array()
            .unwrap()
            .iter()
            .find(|state| {
                state["signature"].as_str() == Some(selected_signature)
                    && state["state_side"].as_str() == Some("pre")
                    && state["event_fields"]["ix_name"].as_str() == Some("buy_exact_quote_in")
            })
            .unwrap();
        assert_eq!(
            selected_buy["offset_from_provider_peak_s"].as_i64(),
            Some(0)
        );
        let instruction =
            raw_instruction_from_hex(selected_buy["event_cpi_data_hex"].as_str().unwrap());
        let scout_dex_solana::PumpAmmEventOutcome::Trade(scout_dex_solana::PumpAmmEvent::Buy(
            event,
        )) = scout_dex_solana::classify_pump_amm_event(&instruction)
        else {
            panic!("selected Doy maximum event must be a BuyEvent");
        };
        assert_eq!(event.ix_name.as_deref(), Some("buy_exact_quote_in"));
        assert_eq!(event.lp_fee_basis_points, 20);
        assert_eq!(event.protocol_fee_basis_points, 5);
        assert_eq!(event.coin_creator_fee_basis_points, 95);
        let net = U512::from(event.user_quote_amount_in);
        let gross = U512::from(event.quote_amount_in);
        let fee_ceil = |basis_points: u64| {
            (net * U512::from(basis_points) + U512::from(9_999)) / U512::from(10_000)
        };
        let lp_fee = fee_ceil(event.lp_fee_basis_points);
        let protocol_fee = fee_ceil(event.protocol_fee_basis_points);
        let creator_fee = fee_ceil(event.coin_creator_fee_basis_points);
        assert_eq!(lp_fee, U512::from(event.lp_fee));
        assert_eq!(protocol_fee, U512::from(event.protocol_fee));
        assert_eq!(creator_fee, U512::from(event.coin_creator_fee));
        assert_eq!(net + lp_fee, U512::from(event.quote_amount_in_with_lp_fee));
        assert_eq!(net + lp_fee + protocol_fee + creator_fee, gross);
        let base = U512::from(event.pool_base_token_reserves);
        let quote = U512::from(event.pool_quote_token_reserves);
        let net_minus_one = net - U512::from(1);
        let expected_out = net_minus_one * base / (quote + net_minus_one);
        assert_eq!(expected_out, U512::from(event.base_amount_out));
        let pre_base =
            U512::from_str_radix(selected_buy["pre_vault_raw"]["base"].as_str().unwrap(), 10)
                .unwrap();
        let pre_quote =
            U512::from_str_radix(selected_buy["pre_vault_raw"]["quote"].as_str().unwrap(), 10)
                .unwrap();
        let post_base =
            U512::from_str_radix(selected_buy["post_vault_raw"]["base"].as_str().unwrap(), 10)
                .unwrap();
        let post_quote = U512::from_str_radix(
            selected_buy["post_vault_raw"]["quote"].as_str().unwrap(),
            10,
        )
        .unwrap();
        assert_eq!(pre_base - post_base, expected_out);
        assert_eq!(
            post_quote - pre_quote,
            U512::from(event.quote_amount_in_with_lp_fee)
        );

        let supply = &fixture["supply"];
        assert_eq!(supply["mint"].as_str(), Some(row["mint"].as_str().unwrap()));
        assert_eq!(supply["history_rows"].as_u64(), Some(921));
        assert_eq!(supply["successful_signature_rows"].as_u64(), Some(920));
        assert!(
            supply["signature_sets_equal_with_explicit_birth"]
                .as_bool()
                .unwrap()
        );
        assert!(
            supply["all_before_finalized_snapshot_slot"]
                .as_u64()
                .is_some()
        );
        assert!(supply["authority_revoked_at_birth"].as_bool().unwrap());
        assert!(
            supply["no_close_or_supply_scaling_extension"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            supply["birth_mint_instructions"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            supply["birth_mint_instructions"][0]["kind"].as_str(),
            Some("mint")
        );
        assert_eq!(
            supply["total_birth_mints_raw"].as_str(),
            Some("1000000000000000")
        );
        assert_eq!(supply["burns"].as_array().unwrap().len(), 1);
        assert_eq!(supply["burns"][0]["amount_raw"].as_u64(), Some(1_073_000));
        assert!(
            supply["burns"][0]["block_time_unix_s"].as_u64().unwrap()
                < supply["peak_ms"].as_u64().unwrap() / 1_000
        );
        let peak_supply_low = supply["peak_supply_bounds_raw"][0]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        let peak_supply_high = supply["peak_supply_bounds_raw"][1]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        assert_eq!(peak_supply_low, 999_999_998_927_000);
        assert_eq!(peak_supply_high, 999_999_998_927_000);
        assert_eq!(1_000_000_000_000_000_u128 - 1_073_000, peak_supply_low);
        let provider = ExactDecimal::parse(supply["price_decimal"].as_str().unwrap()).unwrap();
        let fdv_low = provider
            .fdv_cents_for_raw_supply(peak_supply_low, 6)
            .unwrap();
        let fdv_high = provider
            .fdv_cents_for_raw_supply(peak_supply_high, 6)
            .unwrap();
        assert_eq!((fdv_low, fdv_high), (4_286_370, 4_286_370));
        assert_eq!(
            supply["maximum_fdv_bound_cent_floors"][0].as_i64(),
            Some(fdv_low)
        );
        assert_eq!(
            supply["maximum_fdv_bound_cent_floors"][1].as_i64(),
            Some(fdv_high)
        );
        assert_eq!(fixture["stored_fdv_cents"].as_i64(), Some(4_286_370));
    }

    #[test]
    fn solana_tracker_mayhem_curve_marks_match_window_price_and_peak_supply() {
        use alloy_primitives::U512;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_mayhem_curve_mark_cases.json"
        ))
        .unwrap();
        let fraction = |numerator: U512, denominator: U512| (numerator, denominator);
        let decimal_fraction = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            let denominator = (0..u32::try_from(decimal.scale).unwrap())
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap();
            fraction(U512::from(decimal.coefficient), denominator)
        };
        let compare =
            |left: (U512, U512), right: (U512, U512)| (left.0 * right.1).cmp(&(right.0 * left.1));
        let floor_fdv_cents = |price: (U512, U512), supply_raw: U512| {
            price.0 * supply_raw * U512::from(100) / (price.1 * U512::from(1_000_000_u64))
        };
        let expected_fdv_cents = [1_430_309_u64, 1_567_048, 5_643_677];
        let expected_half_supply_cents = [715_154_u64, 783_524, 2_821_838];
        let mut total_marks = 0;

        for (row_index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                row["source"].as_str(),
                Some("solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1")
            );
            assert!(row["ath_at"].is_null());
            let mint = row["mint"].as_str().unwrap();
            let provider = decimal_fraction(row["provider_price_usd"].as_str().unwrap());
            let provider_time = row["timestamp_ms"].as_u64().unwrap() / 1_000;
            let native = &row["native_execution"];
            assert_eq!(native["mint"].as_str(), Some(mint));
            assert_eq!(
                native["nested_pump_program"].as_str(),
                Some("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
            );
            assert!(native["finalized"].as_bool().unwrap());
            assert!(native["transaction_success"].as_bool().unwrap());

            let mut marks = Vec::new();
            for mark in row["neighborhood"]["events"].as_array().unwrap() {
                let raw_program = bs58::decode(mark["program_id"].as_str().unwrap())
                    .into_vec()
                    .unwrap()
                    .try_into()
                    .unwrap();
                assert_eq!(raw_program, scout_dex_solana::PUMP_PROGRAM_ID_BYTES);
                assert_eq!(
                    mark["program_id"].as_str(),
                    Some("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
                );
                let instruction = scout_core::RawSolanaInstruction {
                    program_id: raw_program,
                    accounts: Vec::new(),
                    data: bytes_from_hex(mark["event_cpi_data_hex"].as_str().unwrap()),
                    instruction_index: 0,
                };
                let expected = &mark["event"];
                match scout_dex_solana::classify_pump_event(&instruction) {
                    scout_dex_solana::PumpEventOutcome::Trade(event) => {
                        assert_eq!(bs58::encode(event.mint).into_string(), mint);
                        assert_eq!(event.sol_amount, expected["sol_amount"].as_u64().unwrap());
                        assert_eq!(
                            event.token_amount,
                            expected["token_amount"].as_u64().unwrap()
                        );
                        assert_eq!(event.is_buy, expected["is_buy"].as_bool().unwrap());
                        assert_eq!(event.timestamp, expected["timestamp"].as_i64().unwrap());
                        assert_eq!(
                            event.virtual_sol_reserves,
                            expected["virtual_sol_reserves"].as_u64().unwrap()
                        );
                        assert_eq!(
                            event.virtual_token_reserves,
                            expected["virtual_token_reserves"].as_u64().unwrap()
                        );
                        assert_eq!(
                            event.real_sol_reserves,
                            expected["real_sol_reserves"].as_u64().unwrap()
                        );
                        assert_eq!(
                            event.real_token_reserves,
                            expected["real_token_reserves"].as_u64().unwrap()
                        );
                        assert_eq!(event.mayhem_mode, Some(true));
                        assert_eq!(expected["mayhem_mode"].as_bool(), Some(true));
                        assert_eq!(event.quote_mint, None);
                        assert_eq!(event.quote_amount, None);
                        assert_eq!(event.virtual_quote_reserves, None);
                        assert_eq!(event.real_quote_reserves, None);
                        assert_eq!(event.holder_rewards_bps, None);
                        assert_eq!(event.holder_rewards, None);
                        assert_eq!(event.trailing_bytes, 0);
                        assert_eq!(event.data_len, instruction.data.len());
                        assert_eq!(event.last_field_present, "cashback");

                        let event_fraction = fraction(
                            U512::from(event.virtual_sol_reserves),
                            U512::from(event.virtual_token_reserves) * U512::from(1_000_u64),
                        );
                        let low_fx = decimal_fraction(mark["fx_low_usd"].as_str().unwrap());
                        let high_fx = decimal_fraction(mark["fx_high_usd"].as_str().unwrap());
                        let low_usd =
                            fraction(event_fraction.0 * low_fx.0, event_fraction.1 * low_fx.1);
                        let high_usd =
                            fraction(event_fraction.0 * high_fx.0, event_fraction.1 * high_fx.1);
                        assert_eq!(
                            compare(
                                low_usd,
                                fraction_from_text(mark["usd_low_fraction"].as_str().unwrap())
                            ),
                            std::cmp::Ordering::Equal
                        );
                        assert_eq!(
                            compare(
                                high_usd,
                                fraction_from_text(mark["usd_high_fraction"].as_str().unwrap())
                            ),
                            std::cmp::Ordering::Equal
                        );
                        marks.push((mark, event_fraction, low_usd, high_usd));
                    }
                    other => panic!("expected a Pump TradeEvent, got {other:?}"),
                }
            }
            total_marks += marks.len();
            assert_eq!(
                marks.len(),
                usize::try_from(row["neighborhood"]["event_count"].as_u64().unwrap()).unwrap()
            );

            let low_max = marks
                .iter()
                .max_by(|left, right| compare(left.2, right.2))
                .unwrap();
            let high_max = marks
                .iter()
                .max_by(|left, right| compare(left.3, right.3))
                .unwrap();
            let selected = marks
                .iter()
                .max_by(|left, right| compare(left.1, right.1))
                .unwrap();
            assert_eq!(low_max.0["signature"], high_max.0["signature"]);
            assert_eq!(
                selected.0["signature"],
                row["neighborhood"]["maximum_event_signature"]
            );
            assert_eq!(selected.0["signature"], native["signature"]);
            assert!(
                selected.0["event"]["timestamp"]
                    .as_i64()
                    .unwrap()
                    .abs_diff(i64::try_from(provider_time).unwrap())
                    <= 1
            );
            assert_eq!(
                selected.0["event"]["sol_amount"].as_u64(),
                native["event_sol_amount_lamports"].as_u64()
            );
            assert_eq!(
                selected.0["event"]["token_amount"].as_u64(),
                native["event_token_amount_raw"].as_u64()
            );
            for price in [low_max.2, high_max.3] {
                let difference = (price.0 * provider.1).abs_diff(provider.0 * price.1);
                assert!(difference * U512::from(100) <= provider.0 * price.1 * U512::from(2));
            }

            let supply_raw = row["supply"]["total_birth_mints_raw"]
                .as_str()
                .unwrap()
                .parse::<U512>()
                .unwrap();
            assert_eq!(supply_raw, U512::from(2_000_000_000_000_000_u64));
            assert_eq!(
                row["supply"]["birth_mint_instructions"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            let birth_mint_sum = row["supply"]["birth_mint_instructions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|mint| U512::from(mint["amount_raw"].as_u64().unwrap()))
                .fold(U512::ZERO, |sum, amount| sum + amount);
            assert_eq!(birth_mint_sum, supply_raw);
            assert_eq!(row["supply"]["burns"].as_array().unwrap().len(), 0);
            assert_eq!(
                row["supply"]["history_rows"].as_u64(),
                Some([17, 25, 181][row_index])
            );
            assert_eq!(
                row["supply"]["successful_signature_rows"].as_u64(),
                Some([16, 24, 180][row_index])
            );
            assert!(
                row["supply"]["signature_sets_equal_with_explicit_birth"]
                    .as_bool()
                    .unwrap()
            );

            let fdv = floor_fdv_cents(provider, supply_raw);
            assert_eq!(fdv, U512::from(expected_fdv_cents[row_index]));
            assert_eq!(fdv, U512::from(row["ath_fdv_cents"].as_u64().unwrap()));
            assert_eq!(
                row["supply"]["maximum_fdv_bound_cent_floors"][0].as_u64(),
                Some(expected_fdv_cents[row_index])
            );
            let wrong_supply_fdv = floor_fdv_cents(provider, supply_raw / U512::from(2));
            assert_eq!(
                wrong_supply_fdv,
                U512::from(expected_half_supply_cents[row_index])
            );
            assert_ne!(wrong_supply_fdv, fdv);

            let execution_price = fraction(
                U512::from(native["event_sol_amount_lamports"].as_u64().unwrap()),
                U512::from(native["event_token_amount_raw"].as_u64().unwrap())
                    * U512::from(1_000_u64),
            );
            assert_ne!(
                compare(execution_price, selected.1),
                std::cmp::Ordering::Equal
            );
        }
        assert_eq!(total_marks, 27);

        fn fraction_from_text(text: &str) -> (U512, U512) {
            let (numerator, denominator) = text.split_once('/').unwrap();
            (numerator.parse().unwrap(), denominator.parse().unwrap())
        }
    }

    #[test]
    fn solana_tracker_mayhem_fee_curve_marks_reconcile_scoped_buys() {
        use alloy_primitives::U512;

        let fraction = |numerator: U512, denominator: U512| (numerator, denominator);
        let decimal_fraction = |value: &str| {
            let decimal = ExactDecimal::parse(value).unwrap();
            let denominator = (0..u32::try_from(decimal.scale).unwrap())
                .try_fold(U512::from(1), |value, _| value.checked_mul(U512::from(10)))
                .unwrap();
            fraction(U512::from(decimal.coefficient), denominator)
        };
        let compare =
            |left: (U512, U512), right: (U512, U512)| (left.0 * right.1).cmp(&(right.0 * left.1));
        let within_two_percent = |left: (U512, U512), right: (U512, U512)| {
            let difference = (left.0 * right.1).abs_diff(right.0 * left.1);
            difference * U512::from(100) <= right.0 * left.1 * U512::from(2)
        };
        let ceil_fee = |amount: u64, bps: u64| {
            (U512::from(amount) * U512::from(bps) + U512::from(9_999_u64)) / U512::from(10_000_u64)
        };
        let datasets = [
            (
                include_str!("../tests/fixtures/solana_tracker_mayhem_curve_mark_fee_cases.json"),
                &[
                    500_586_u64,
                    487_004,
                    522_541,
                    703_446,
                    499_152,
                    546_571,
                    642_711,
                ][..],
                &[2_u64, 2, 1, 2, 4, 2, 5][..],
                &[1_u64, 1, 0, 1, 3, 1, 4][..],
                &[-1_i64, -1, -1, -1, -2, -2, -2][..],
                36_usize,
                5_usize,
                Some("5N9Zfj9tNob9ShYFrCXzapKCLFR7uR8Zsf4CUjMGpump"),
                &[true, true][..],
            ),
            (
                include_str!(
                    "../tests/fixtures/solana_tracker_mayhem_curve_mark_batch7_cases.json"
                ),
                &[
                    535_428_u64,
                    1_116_635,
                    2_474_933,
                    602_043,
                    683_696,
                    651_093,
                    2_236_670,
                ][..],
                &[6_u64, 10, 47, 14, 70, 14, 23][..],
                &[5_u64, 9, 46, 13, 69, 13, 22][..],
                &[-1_i64, 0, -1, -1, -1, -1, -1][..],
                101_usize,
                3_usize,
                None,
                &[true][..],
            ),
            (
                include_str!(
                    "../tests/fixtures/solana_tracker_mayhem_curve_mark_batch8_cases.json"
                ),
                &[
                    565_021_u64,
                    2_435_430,
                    851_171,
                    609_137,
                    1_100_700,
                    1_208_986,
                    1_150_218,
                    1_052_206,
                ][..],
                &[14_u64, 202, 65, 22, 62, 84, 25, 229][..],
                &[13_u64, 201, 64, 21, 61, 83, 24, 228][..],
                &[0_i64, -1, -1, -1, 0, -1, -1, -1][..],
                140_usize,
                1_usize,
                None,
                &[false][..],
            ),
            (
                include_str!(
                    "../tests/fixtures/solana_tracker_mayhem_curve_mark_batch9_cases.json"
                ),
                &[
                    2_922_033_u64,
                    12_277_898,
                    3_189_985,
                    8_491_258,
                    1_272_774,
                    1_738_769,
                    6_871_894,
                ][..],
                &[85_u64, 157, 106, 58, 76, 294, 221][..],
                &[84_u64, 156, 105, 57, 75, 293, 220][..],
                &[-1_i64, -1, -2, -1, 0, -1, -1][..],
                53_usize,
                0_usize,
                None,
                &[][..],
            ),
        ];

        for (
            fixture_text,
            expected_fdv_cents,
            expected_history_rows,
            expected_successful_rows,
            expected_offsets,
            expected_total_events,
            expected_nonzero_fees,
            extra_fee_mint,
            expected_prior_control_matches,
        ) in datasets
        {
            let fixture: serde_json::Value = serde_json::from_str(fixture_text).unwrap();
            let mut total_events = 0;
            let mut nonzero_fees = 0;
            for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
                assert_eq!(
                    row["source"].as_str(),
                    Some("solana-tracker-window-peak-supply-mayhem-curve-mark-fx-validated-v1")
                );
                assert!(row["ath_at"].is_null());
                let mint = row["mint"].as_str().unwrap();
                let provider = decimal_fraction(row["provider_price_usd"].as_str().unwrap());
                let provider_second = row["timestamp_ms"].as_u64().unwrap() / 1_000;
                let mut marks = Vec::new();
                for mark in row["neighborhood"]["events"].as_array().unwrap() {
                    let program_id = bs58::decode(mark["program_id"].as_str().unwrap())
                        .into_vec()
                        .unwrap()
                        .try_into()
                        .unwrap();
                    assert_eq!(program_id, scout_dex_solana::PUMP_PROGRAM_ID_BYTES);
                    assert_eq!(
                        mark["program_id"].as_str(),
                        Some("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
                    );
                    let instruction = scout_core::RawSolanaInstruction {
                        program_id,
                        accounts: Vec::new(),
                        data: bytes_from_hex(mark["event_cpi_data_hex"].as_str().unwrap()),
                        instruction_index: 0,
                    };
                    let expected = &mark["event"];
                    let event = match scout_dex_solana::classify_pump_event(&instruction) {
                        scout_dex_solana::PumpEventOutcome::Trade(event) => event,
                        other => panic!("expected Pump TradeEvent, got {other:?}"),
                    };
                    assert_eq!(bs58::encode(event.mint).into_string(), mint);
                    assert_eq!(event.sol_amount, expected["sol_amount"].as_u64().unwrap());
                    assert_eq!(
                        event.token_amount,
                        expected["token_amount"].as_u64().unwrap()
                    );
                    assert_eq!(event.timestamp, expected["timestamp"].as_i64().unwrap());
                    assert_eq!(
                        event.virtual_sol_reserves,
                        expected["virtual_sol_reserves"].as_u64().unwrap()
                    );
                    assert_eq!(
                        event.virtual_token_reserves,
                        expected["virtual_token_reserves"].as_u64().unwrap()
                    );
                    assert_eq!(
                        event.real_sol_reserves,
                        expected["real_sol_reserves"].as_u64().unwrap()
                    );
                    assert_eq!(
                        event.real_token_reserves,
                        expected["real_token_reserves"].as_u64().unwrap()
                    );
                    assert_eq!(event.mayhem_mode, Some(true));
                    assert_eq!(event.trailing_bytes, 0);
                    assert_eq!(event.data_len, instruction.data.len());
                    let event_offset = event.timestamp - i64::try_from(provider_second).unwrap();
                    assert!((-3..=3).contains(&event_offset));

                    let price_sol = fraction(
                        U512::from(event.virtual_sol_reserves),
                        U512::from(event.virtual_token_reserves) * U512::from(1_000_u64),
                    );
                    let low_fx = decimal_fraction(mark["fx_low_usd"].as_str().unwrap());
                    let high_fx = decimal_fraction(mark["fx_high_usd"].as_str().unwrap());
                    let low_price = fraction(price_sol.0 * low_fx.0, price_sol.1 * low_fx.1);
                    let high_price = fraction(price_sol.0 * high_fx.0, price_sol.1 * high_fx.1);
                    assert_eq!(
                        compare(
                            low_price,
                            fraction_from_text(mark["usd_low_fraction"].as_str().unwrap())
                        ),
                        std::cmp::Ordering::Equal
                    );
                    assert_eq!(
                        compare(
                            high_price,
                            fraction_from_text(mark["usd_high_fraction"].as_str().unwrap())
                        ),
                        std::cmp::Ordering::Equal
                    );
                    assert!(within_two_percent(
                        low_price,
                        fraction_from_text(mark["usd_low_fraction"].as_str().unwrap())
                    ));
                    assert!(within_two_percent(
                        high_price,
                        fraction_from_text(mark["usd_high_fraction"].as_str().unwrap())
                    ));
                    marks.push((mark, price_sol, low_price, high_price));
                }
                total_events += marks.len();
                assert_eq!(
                    marks.len(),
                    usize::try_from(row["neighborhood"]["event_count"].as_u64().unwrap()).unwrap()
                );
                let low_max = marks
                    .iter()
                    .max_by(|left, right| compare(left.2, right.2))
                    .unwrap();
                let high_max = marks
                    .iter()
                    .max_by(|left, right| compare(left.3, right.3))
                    .unwrap();
                assert_eq!(low_max.0["signature"], high_max.0["signature"]);
                assert_eq!(
                    low_max.0["signature"],
                    row["neighborhood"]["maximum_event_signature"]
                );
                let selected_offset = low_max.0["event"]["timestamp"].as_i64().unwrap()
                    - i64::try_from(provider_second).unwrap();
                assert!((-3..=0).contains(&selected_offset));
                assert_eq!(selected_offset, expected_offsets[index]);
                assert!(within_two_percent(low_max.2, provider));
                assert!(within_two_percent(high_max.3, provider));
                assert!(
                    row["neighborhood"]["both_fx_endpoints_within_2pct"]
                        .as_bool()
                        .unwrap()
                );
                assert_eq!(
                    row["neighborhood"]["selected_max_offset_seconds"].as_i64(),
                    Some(selected_offset)
                );

                let event = &low_max.0["event"];
                let native = &row["native_execution"];
                assert_eq!(native["signature"], low_max.0["signature"]);
                assert_eq!(native["mint"].as_str(), Some(mint));
                assert_eq!(
                    native["roles"]["program"].as_str(),
                    Some("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")
                );
                assert_eq!(native["roles"]["mint"].as_str(), Some(mint));
                assert_eq!(
                    native["roles"]["system_program"].as_str(),
                    Some("11111111111111111111111111111111")
                );
                assert!(
                    native["scoped_system_transfers_only_within_buy_invocation"]
                        .as_bool()
                        .unwrap()
                );
                assert!(
                    native["no_fee_payer_equals_trade_amount_assumption"]
                        .as_bool()
                        .unwrap()
                );
                let buy_data = bytes_from_hex(native["buy_instruction_data_hex"].as_str().unwrap());
                assert_eq!(
                    &buy_data[..8],
                    &scout_dex_solana::BUY_INSTRUCTION_DISCRIMINATOR
                );
                assert!(native["buy_outer_index"].as_u64().is_some());
                assert!(native["buy_inner_index"].as_i64().is_some());

                let roles = &native["roles"];
                let mut sums = std::collections::BTreeMap::<String, U512>::new();
                let mut creator_transfers = Vec::new();
                for transfer in native["system_transfers"].as_array().unwrap() {
                    assert_eq!(transfer["from"], roles["user"]);
                    let role = transfer["role"].as_str().unwrap();
                    let recipient_role = match role {
                        "bonding_curve" => "bonding_curve",
                        "fee_recipient" => "fee_recipient",
                        "creator_vault" => "creator_vault",
                        other => panic!("unexpected transfer role {other}"),
                    };
                    assert_eq!(transfer["to"], roles[recipient_role]);
                    let data = bytes_from_hex(transfer["data_hex"].as_str().unwrap());
                    assert_eq!(data.len(), 12);
                    assert_eq!(u32::from_le_bytes(data[..4].try_into().unwrap()), 2);
                    let amount = u64::from_le_bytes(data[4..12].try_into().unwrap());
                    assert_eq!(transfer["lamports"].as_u64(), Some(amount));
                    *sums.entry(role.to_string()).or_default() += U512::from(amount);
                    if role == "creator_vault" {
                        creator_transfers.push(amount);
                    }
                }
                for role in ["bonding_curve", "fee_recipient", "creator_vault"] {
                    assert_eq!(
                        sums.get(role).copied().unwrap_or(U512::ZERO),
                        U512::from(native["role_sums_lamports"][role].as_u64().unwrap())
                    );
                }
                assert_eq!(
                    sums.get("bonding_curve").copied().unwrap_or(U512::ZERO),
                    U512::from(event["sol_amount"].as_u64().unwrap())
                );
                assert_eq!(
                    sums.get("fee_recipient").copied().unwrap_or(U512::ZERO),
                    U512::from(event["fee"].as_u64().unwrap())
                );
                let creator_fee = event["creator_fee"].as_u64().unwrap();
                if creator_fee > 0 {
                    assert!(creator_transfers.contains(&creator_fee));
                }
                let creator_sum = sums.get("creator_vault").copied().unwrap_or(U512::ZERO);
                let extra = creator_sum - U512::from(creator_fee);
                assert_eq!(
                    extra,
                    U512::from(native["creator_vault_extra_lamports"].as_u64().unwrap())
                );
                let expected_extra = if extra_fee_mint == Some(mint) {
                    890_880
                } else {
                    0
                };
                assert_eq!(extra, U512::from(expected_extra));
                let fee = event["fee"].as_u64().unwrap();
                let fee_bps = event["fee_basis_points"].as_u64().unwrap();
                let creator_bps = event["creator_fee_basis_points"].as_u64().unwrap();
                assert_eq!(
                    U512::from(fee),
                    ceil_fee(event["sol_amount"].as_u64().unwrap(), fee_bps)
                );
                assert_eq!(
                    U512::from(creator_fee),
                    ceil_fee(event["sol_amount"].as_u64().unwrap(), creator_bps)
                );
                nonzero_fees += usize::from(fee > 0);
                if extra_fee_mint.is_some()
                    && mint == "5WrHHbJwcs6DN1BtZpikeVRxFnam4K1hGZCKM4BBpump"
                {
                    assert_eq!(fee, 0);
                    assert_eq!(creator_fee, 0);
                }
                if fee == 0 {
                    assert_eq!(creator_fee, 0);
                }

                let supply = &row["supply"];
                let supply_raw = supply["total_birth_mints_raw"]
                    .as_str()
                    .unwrap()
                    .parse::<U512>()
                    .unwrap();
                let birth_mints = supply["birth_mint_instructions"].as_array().unwrap();
                assert_eq!(birth_mints.len(), 2);
                let mint_sum = birth_mints
                    .iter()
                    .map(|entry| U512::from(entry["amount_raw"].as_u64().unwrap()))
                    .fold(U512::ZERO, |sum, amount| sum + amount);
                assert_eq!(mint_sum, U512::from(2_000_000_000_000_000_u64));
                assert_eq!(supply_raw, mint_sum);
                assert_eq!(supply["burns"].as_array().unwrap().len(), 0);
                assert_eq!(
                    supply["history_rows"].as_u64(),
                    Some(expected_history_rows[index])
                );
                assert_eq!(
                    supply["successful_signature_rows"].as_u64(),
                    Some(expected_successful_rows[index])
                );
                assert!(
                    supply["signature_sets_equal_with_explicit_birth"]
                        .as_bool()
                        .unwrap()
                );
                assert_eq!(
                    supply["peak_supply_bounds_raw"][0],
                    supply["peak_supply_bounds_raw"][1]
                );
                let fdv = provider.0 * supply_raw * U512::from(100)
                    / (provider.1 * U512::from(1_000_000_u64));
                assert_eq!(fdv, U512::from(expected_fdv_cents[index]));
                assert_eq!(fdv, U512::from(row["ath_fdv_cents"].as_u64().unwrap()));

                if extra_fee_mint.is_none() {
                    assert_eq!(
                        supply["history_rows"].as_u64().unwrap(),
                        supply["successful_signature_rows"].as_u64().unwrap() + 1
                    );
                    for failed in supply["failed_signature_query_rows_retained"]
                        .as_array()
                        .unwrap()
                    {
                        assert!(!failed["err"].is_null());
                    }
                }
                if mint == "FgFgrJLZ8Q7GBUzBsnHoeE4vyN2zjfMp6czb8nPVpump" {
                    let outside = supply["signature_query_rows_outside_peak_time_bound"]
                        .as_array()
                        .unwrap();
                    let failed = supply["failed_signature_query_rows_retained"]
                        .as_array()
                        .unwrap();
                    assert_eq!(outside.len(), 1);
                    assert_eq!(failed.len(), 2);
                    assert!(outside[0]["err"].is_null());
                    assert_eq!(
                        outside[0]["blockTime"].as_u64().unwrap(),
                        supply["peak_ms"].as_u64().unwrap() / 1_000 + 2
                    );
                    assert_eq!(
                        supply["history_rows"].as_u64().unwrap(),
                        supply["successful_signature_rows"].as_u64().unwrap() + 1
                    );
                }
                if mint == "EzqFMWBuU7YJmBZBPg6hWuaNh3kofJU55vK2pt91pump" {
                    let pages = supply["signature_pages"].as_array().unwrap();
                    let failed = supply["failed_signature_query_rows_retained"]
                        .as_array()
                        .unwrap();
                    assert_eq!(pages.len(), 3);
                    assert_eq!(pages[0]["rows"].as_u64(), Some(1_000));
                    assert_eq!(pages[1]["rows"].as_u64(), Some(564));
                    assert_eq!(pages[2]["rows"].as_u64(), Some(0));
                    assert_eq!(failed.len(), 1_336);
                    assert!(failed.iter().all(|entry| !entry["err"].is_null()));
                    assert_eq!(
                        1_564_u64,
                        supply["successful_signature_rows"].as_u64().unwrap()
                            + u64::try_from(failed.len()).unwrap()
                    );
                    assert_eq!(supply["history_rows"].as_u64(), Some(229));
                }
            }
            assert_eq!(total_events, expected_total_events);
            assert_eq!(nonzero_fees, expected_nonzero_fees);

            for (control_index, control) in fixture["negative_price_controls"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_eq!(
                    control["both_fx_endpoints_within_2pct"].as_bool(),
                    Some(false)
                );
                let expected_offset = control["selected_max_offset_seconds"].as_i64().unwrap();
                assert_eq!(
                    control["maximum_event_second"].as_i64().unwrap()
                        - i64::try_from(control["peak_ms"].as_u64().unwrap() / 1_000).unwrap(),
                    expected_offset
                );
                assert_eq!(
                    control["events"].as_array().unwrap().len(),
                    usize::try_from(control["event_count"].as_u64().unwrap()).unwrap()
                );
                let provider =
                    decimal_fraction(control["provider_price_usd_decimal"].as_str().unwrap());
                let mut prices = Vec::new();
                for event in control["events"].as_array().unwrap() {
                    let base = fraction(
                        U512::from(
                            event["virtual_sol_raw"]
                                .as_str()
                                .unwrap()
                                .parse::<u64>()
                                .unwrap(),
                        ),
                        U512::from(
                            event["virtual_token_raw"]
                                .as_str()
                                .unwrap()
                                .parse::<u64>()
                                .unwrap(),
                        ) * U512::from(1_000_u64),
                    );
                    let low_fx = decimal_fraction(event["fx_low_usd"].as_str().unwrap());
                    let high_fx = decimal_fraction(event["fx_high_usd"].as_str().unwrap());
                    let low = fraction(base.0 * low_fx.0, base.1 * low_fx.1);
                    let high = fraction(base.0 * high_fx.0, base.1 * high_fx.1);
                    prices.push((event, low, high));
                }
                let earlier_price_match = prices.iter().any(|(event, low, high)| {
                    event["event"]["timestamp"].as_i64().unwrap()
                        < i64::try_from(control["maximum_event_second"].as_u64().unwrap()).unwrap()
                        && within_two_percent(*low, provider)
                        && within_two_percent(*high, provider)
                });
                assert_eq!(
                    earlier_price_match,
                    expected_prior_control_matches[control_index]
                );
                let max_low = prices.iter().max_by(|a, b| compare(a.1, b.1)).unwrap();
                let max_high = prices.iter().max_by(|a, b| compare(a.2, b.2)).unwrap();
                assert_eq!(max_low.0["signature"], max_high.0["signature"]);
                assert_eq!(max_low.0["signature"], control["maximum_event_signature"]);
                assert_eq!(
                    max_low.0["event"]["timestamp"].as_i64().unwrap()
                        - i64::try_from(control["peak_ms"].as_u64().unwrap() / 1_000).unwrap(),
                    expected_offset
                );
                assert!(!within_two_percent(max_low.1, provider));
                assert!(!within_two_percent(max_high.2, provider));
            }
        }

        fn fraction_from_text(text: &str) -> (U512, U512) {
            let (numerator, denominator) = text.split_once('/').unwrap();
            (numerator.parse().unwrap(), denominator.parse().unwrap())
        }
    }
}
