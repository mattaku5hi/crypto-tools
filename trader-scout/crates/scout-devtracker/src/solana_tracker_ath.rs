//! Bounded Solana Tracker ATH observations. Provider extrema are stored as
//! source observations and are never promoted to a lifetime ATH or FDV.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_BODY_BYTES_U64: u64 = 2 * 1024 * 1024;
const MAX_DECIMAL_LENGTH: usize = 80;

/// Provider response retained without inferring price, cap, or time from nulls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerAthObservation {
    pub token: String,
    pub highest_price_usd: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub pool_id: Option<String>,
    pub highest_market_cap_usd: Option<String>,
    pub response_sha256: String,
}

/// Successful HTTP status and bounded response bytes, before schema parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerAthBody {
    pub status: u16,
    pub bytes: Vec<u8>,
}

/// Failures from one Solana Tracker Data API request.
#[derive(Debug, thiserror::Error)]
pub enum TrackerAthError {
    #[error("invalid Solana Tracker request")]
    InvalidRequest,
    #[error("Solana Tracker transport failed")]
    Transport,
    #[error("Solana Tracker returned HTTP {0}")]
    HttpStatus(u16),
    #[error("Solana Tracker access denied (HTTP {0})")]
    AccessDenied(u16),
    #[error("Solana Tracker rate limited (HTTP {0})")]
    RateLimited(u16),
    #[error("Solana Tracker response exceeded 2 MiB")]
    ResponseTooLarge,
    #[error("Solana Tracker response was malformed")]
    InvalidResponse,
    #[error("Solana Tracker returned an invalid decimal")]
    InvalidDecimal,
    #[error("Solana Tracker returned an invalid timestamp")]
    InvalidTimestamp,
    #[error("Solana Tracker returned an invalid pool address")]
    InvalidPool,
}

/// Hash the key for request provenance without retaining or displaying it.
#[must_use]
pub fn key_fingerprint(api_key: &str) -> String {
    format!("{:x}", Sha256::digest(api_key.as_bytes()))
}

/// Construct a client that never follows redirects or retries requests.
pub fn http_client() -> Result<reqwest::Client, TrackerAthError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| TrackerAthError::Transport)
}

/// Request one provider-recorded ATH observation. Callers supplying a client
/// directly must disable reqwest retries with `retry(reqwest::retry::never())`.
pub async fn fetch_ath(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    token: &str,
) -> Result<TrackerAthObservation, TrackerAthError> {
    let body = fetch_ath_body(http, endpoint, api_key, token).await?;
    parse_ath_observation(token, &body.bytes)
}

/// Fetch one bounded successful response body without interpreting its schema.
/// This lets callers archive malformed provider data before parsing it. A
/// supplied client must disable reqwest retries with `retry(reqwest::retry::never())`.
pub async fn fetch_ath_body(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    token: &str,
) -> Result<TrackerAthBody, TrackerAthError> {
    if api_key.is_empty()
        || token.len() > 44
        || !bs58::decode(token)
            .into_vec()
            .is_ok_and(|bytes| bytes.len() == 32)
    {
        return Err(TrackerAthError::InvalidRequest);
    }

    let url = format!("{}/{token}/ath", endpoint.trim_end_matches('/'));
    let response = http
        .get(url)
        .header("x-api-key", api_key)
        .timeout(Duration::from_secs(25))
        .send()
        .await
        .map_err(|_| TrackerAthError::Transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401..=403 => TrackerAthError::AccessDenied(status.as_u16()),
            429 => TrackerAthError::RateLimited(429),
            code => TrackerAthError::HttpStatus(code),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES_U64)
    {
        return Err(TrackerAthError::ResponseTooLarge);
    }

    let mut stream = response.bytes_stream();
    let mut body = Vec::with_capacity(8 * 1024);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| TrackerAthError::Transport)?;
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(TrackerAthError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    Ok(TrackerAthBody {
        status: status.as_u16(),
        bytes: body,
    })
}

/// Validate and parse an archived successful response body.
pub fn parse_ath_observation(
    token: &str,
    body: &[u8],
) -> Result<TrackerAthObservation, TrackerAthError> {
    let fields: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(body).map_err(|_| TrackerAthError::InvalidResponse)?;
    if let Some(success) = fields.get("success") {
        let success: bool =
            serde_json::from_str(success.get()).map_err(|_| TrackerAthError::InvalidResponse)?;
        if !success {
            return Err(TrackerAthError::InvalidResponse);
        }
    }
    if ![
        "highest_price",
        "highest_market_cap",
        "timestamp",
        "pool_id",
    ]
    .iter()
    .any(|name| fields.contains_key(*name))
    {
        return Err(TrackerAthError::InvalidResponse);
    }
    let highest_price_usd = exact_optional_field(&fields, "highest_price")?;
    let highest_market_cap_usd = exact_optional_field(&fields, "highest_market_cap")?;
    let timestamp_ms = integer_optional_field(&fields, "timestamp")?;
    let pool_id = optional_string_field(&fields, "pool_id")?;
    let digest = format!("{:x}", Sha256::digest(body));

    Ok(TrackerAthObservation {
        token: token.to_owned(),
        highest_price_usd,
        timestamp_ms,
        pool_id,
        highest_market_cap_usd,
        response_sha256: digest,
    })
}

fn exact_optional_field(
    fields: &BTreeMap<String, Box<RawValue>>,
    name: &str,
) -> Result<Option<String>, TrackerAthError> {
    let Some(raw) = fields.get(name) else {
        return Ok(None);
    };
    if raw.get() == "null" {
        return Ok(None);
    }
    let decimal = scout_devtracker_decimal(raw.get())?;
    if decimal.len() > MAX_DECIMAL_LENGTH {
        return Err(TrackerAthError::InvalidDecimal);
    }
    Ok(Some(decimal))
}

fn scout_devtracker_decimal(raw: &str) -> Result<String, TrackerAthError> {
    let value =
        crate::birdeye::ExactDecimal::parse(raw).map_err(|_| TrackerAthError::InvalidDecimal)?;
    Ok(value.to_plain_string())
}

fn integer_optional_field(
    fields: &BTreeMap<String, Box<RawValue>>,
    name: &str,
) -> Result<Option<i64>, TrackerAthError> {
    let Some(raw) = fields.get(name) else {
        return Ok(None);
    };
    if raw.get() == "null" {
        return Ok(None);
    }
    let value = raw
        .get()
        .parse::<i64>()
        .map_err(|_| TrackerAthError::InvalidTimestamp)?;
    if value < 0 {
        return Err(TrackerAthError::InvalidTimestamp);
    }
    Ok(Some(value))
}

fn optional_string_field(
    fields: &BTreeMap<String, Box<RawValue>>,
    name: &str,
) -> Result<Option<String>, TrackerAthError> {
    let Some(raw) = fields.get(name) else {
        return Ok(None);
    };
    if raw.get() == "null" {
        return Ok(None);
    }
    let value: String =
        serde_json::from_str(raw.get()).map_err(|_| TrackerAthError::InvalidPool)?;
    if value.len() > 44
        || !bs58::decode(&value)
            .into_vec()
            .is_ok_and(|bytes| bytes.len() == 32)
    {
        return Err(TrackerAthError::InvalidPool);
    }
    Ok(Some(value))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const JC: &str = "JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump";

    #[tokio::test]
    async fn archived_provider_extrema_keep_exact_fields_and_identity() {
        let server = MockServer::start().await;
        let fixture = include_str!("../tests/fixtures/solana_tracker_ath_jc.json");
        Mock::given(matchers::method("GET"))
            .and(matchers::path(format!("/{JC}/ath")))
            .and(matchers::header("x-api-key", "fixture-key"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(fixture, "application/json"))
            .mount(&server)
            .await;
        let row = fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
            .await
            .unwrap();
        assert_eq!(row.token, JC);
        assert_eq!(
            row.highest_price_usd.as_deref(),
            Some("0.00023536979981208752")
        );
        assert_eq!(row.highest_market_cap_usd.as_deref(), Some("227310.99"));
        assert_eq!(row.timestamp_ms, Some(1_791_672_254_212));
        assert_eq!(
            row.pool_id.as_deref(),
            Some("4QnxqajHAqiKuTZjfsfkBA8qSxb7AY4p8axVZeNmVNuq")
        );
        assert_eq!(row.response_sha256.len(), 64);
    }

    #[test]
    fn all_three_archived_extrema_keep_exact_provider_values() {
        let cases = [
            (
                "48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump",
                include_bytes!("../tests/fixtures/solana_tracker_ath_48.json").as_slice(),
                "0.00013242878993078837",
                1_791_669_895_327,
            ),
            (
                "81kiaJSjaU9w6FVwRVT2K5TP1EhDghAXCm6Y4zmRpump",
                include_bytes!("../tests/fixtures/solana_tracker_ath_81.json").as_slice(),
                "0.00009431116993278875",
                1_791_672_347_033,
            ),
            (
                JC,
                include_bytes!("../tests/fixtures/solana_tracker_ath_jc.json").as_slice(),
                "0.00023536979981208752",
                1_791_672_254_212,
            ),
        ];
        for (token, body, expected_price, expected_at) in cases {
            let observation = parse_ath_observation(token, body).unwrap();
            assert_eq!(observation.token, token);
            assert_eq!(
                observation.highest_price_usd.as_deref(),
                Some(expected_price)
            );
            assert_eq!(observation.timestamp_ms, Some(expected_at));
            assert!(observation.highest_market_cap_usd.is_some());
            assert!(observation.pool_id.is_some());
        }
    }

    #[test]
    fn scoped_sol_window_peak_supply_bounds_require_equal_maxima_and_price_gate() {
        use alloy_primitives::U256;

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/solana_tracker_window_peak_supply_gecko_cases.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["source"].as_str(),
            Some("solana-tracker-window-peak-supply-geckoterminal-validated-v1")
        );
        assert_eq!(fixture["ath_at"], serde_json::Value::Null);
        assert_eq!(fixture["provider_window_scope"].as_str(), Some("unknown"));
        assert_eq!(
            fixture["supply_analysis_sha256"].as_str(),
            Some("4e129fb0a33bca1adb62f430cf6ea483d0758d5aee93e3faf828f6297ef8abb7")
        );
        assert!(
            fixture["price_kind"]
                .as_str()
                .unwrap()
                .contains("same-UTC-day GeckoTerminal daily high")
        );
        assert!(
            fixture["price_kind"]
                .as_str()
                .unwrap()
                .contains("not an execution ratio")
        );

        let pow10 = |scale: u32| {
            (0..scale)
                .try_fold(U256::from(1), |value, _| value.checked_mul(U256::from(10)))
                .unwrap()
        };
        let decimal_parts = |raw: &str| {
            let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
            let coefficient = format!("{whole}{fraction}").parse::<u128>().unwrap();
            (
                U256::from(coefficient),
                u32::try_from(fraction.len()).unwrap(),
            )
        };
        let same_day_price_gate = |provider: &str, gecko_high: &str| {
            let (provider_coefficient, provider_scale) = decimal_parts(provider);
            let (gecko_coefficient, gecko_scale) = decimal_parts(gecko_high);
            let scale = provider_scale.max(gecko_scale);
            let provider_aligned = provider_coefficient
                .checked_mul(pow10(scale - provider_scale))
                .unwrap();
            let gecko_aligned = gecko_coefficient
                .checked_mul(pow10(scale - gecko_scale))
                .unwrap();
            let difference = if provider_aligned >= gecko_aligned {
                provider_aligned.checked_sub(gecko_aligned).unwrap()
            } else {
                gecko_aligned.checked_sub(provider_aligned).unwrap()
            };
            difference.checked_mul(U256::from(100)).unwrap()
                <= provider_aligned.checked_mul(U256::from(2)).unwrap()
        };

        for (case, expected_first_burn_delay) in fixture["accepted_cases"]
            .as_array()
            .unwrap()
            .iter()
            .zip([19, 54])
        {
            assert_eq!(case["price_gate"].as_str(), Some("pass_within_2pct"));
            assert_eq!(case["peak_day"].as_str(), Some("2026-04-29"));
            assert!(same_day_price_gate(
                case["price_usd"].as_str().unwrap(),
                case["gecko_daily_high_usd"].as_str().unwrap()
            ));
            assert_eq!(
                case["peak_supply_raw"].as_u64(),
                case["birth_supply_raw"].as_u64()
            );
            assert_eq!(
                case["first_burn_after_peak_seconds"].as_u64(),
                Some(expected_first_burn_delay)
            );
            assert_eq!(case["burns_before_peak_second_raw"].as_u64(), Some(0));
            assert!(
                case["full_signature_history_reaches_birth"]
                    .as_bool()
                    .unwrap()
            );
            assert!(
                case["successful_transaction_set_matches_signatures"]
                    .as_bool()
                    .unwrap()
            );
            assert!(
                case["successful_transaction_history_terminal"]
                    .as_bool()
                    .unwrap()
            );
            assert!(case["no_post_birth_mint"].as_bool().unwrap());
            assert!(case["mint_authority_absent_at_snapshot"].as_bool().unwrap());
            assert!(case["burns_reconcile_current_supply"].as_bool().unwrap());
            assert!(
                !case["unknown_or_malformed_supply_instructions"]
                    .as_bool()
                    .unwrap()
            );

            let price =
                crate::birdeye::ExactDecimal::parse(case["price_usd"].as_str().unwrap()).unwrap();
            let decimals = u8::try_from(case["decimals"].as_u64().unwrap()).unwrap();
            let birth_raw = u128::from(case["birth_supply_raw"].as_u64().unwrap());
            let peak_raw = u128::from(case["peak_supply_raw"].as_u64().unwrap());
            let peak_cents = price.fdv_cents_for_raw_supply(peak_raw, decimals).unwrap();
            let birth_cents = price.fdv_cents_for_raw_supply(birth_raw, decimals).unwrap();
            assert_eq!(
                peak_cents,
                i64::try_from(case["fdv_floor_cents_at_peak_supply"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(
                birth_cents,
                i64::try_from(case["fdv_floor_cents_at_birth_supply"].as_u64().unwrap()).unwrap()
            );
            assert_eq!(peak_cents, birth_cents);
            assert_eq!(
                peak_cents,
                i64::try_from(case["provider_window_max_fdv_cents"].as_u64().unwrap()).unwrap()
            );
        }

        let negative = &fixture["negative_control"];
        assert_eq!(negative["price_gate"].as_str(), Some("pass_within_2pct"));
        assert!(same_day_price_gate(
            negative["price_usd"].as_str().unwrap(),
            negative["gecko_daily_high_usd"].as_str().unwrap()
        ));
        assert!(negative["burns_before_peak_second_raw"].as_u64().unwrap() > 0);
        assert_eq!(negative["burns_same_peak_second_raw"].as_u64(), Some(0));
        assert!(
            !negative["provider_window_max_fdv_claim_allowed"]
                .as_bool()
                .unwrap()
        );
        let negative_price =
            crate::birdeye::ExactDecimal::parse(negative["price_usd"].as_str().unwrap()).unwrap();
        let decimals = u8::try_from(negative["decimals"].as_u64().unwrap()).unwrap();
        let peak_cents = negative_price
            .fdv_cents_for_raw_supply(
                u128::from(negative["peak_supply_raw"].as_u64().unwrap()),
                decimals,
            )
            .unwrap();
        let birth_cents = negative_price
            .fdv_cents_for_raw_supply(
                u128::from(negative["birth_supply_raw"].as_u64().unwrap()),
                decimals,
            )
            .unwrap();
        assert_eq!(
            peak_cents,
            i64::try_from(negative["fdv_floor_cents_at_peak_supply"].as_u64().unwrap()).unwrap()
        );
        assert_eq!(
            birth_cents,
            i64::try_from(
                negative["fdv_floor_cents_at_birth_supply"]
                    .as_u64()
                    .unwrap()
            )
            .unwrap()
        );
        assert_ne!(peak_cents, birth_cents);
    }

    #[tokio::test]
    async fn malformed_success_body_remains_available_for_archival() {
        let server = MockServer::start().await;
        let body = br#"{"message":"new provider schema"}"#;
        Mock::given(matchers::method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.as_slice(), "application/json"),
            )
            .mount(&server)
            .await;
        let raw = fetch_ath_body(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
            .await
            .unwrap();
        assert_eq!(raw.status, 200);
        assert_eq!(raw.bytes, body);
        assert!(matches!(
            parse_ath_observation(JC, &raw.bytes),
            Err(TrackerAthError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn null_extrema_are_unknown_and_invalid_values_fail_closed() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"highest_price":null,"highest_market_cap":null,"timestamp":null,"pool_id":null}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let row = fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
            .await
            .unwrap();
        assert_eq!(row.highest_price_usd, None);
        assert_eq!(row.highest_market_cap_usd, None);
        assert_eq!(row.timestamp_ms, None);
        assert_eq!(row.pool_id, None);

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"highest_price":0,"highest_market_cap":null,"timestamp":null}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        assert_eq!(
            fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
                .await
                .unwrap()
                .highest_price_usd
                .as_deref(),
            Some("0")
        );

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"highest_price":1.23e-5,"highest_market_cap":1.23e4,"timestamp":1,"pool_id":null}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let exact = fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
            .await
            .unwrap();
        assert_eq!(exact.highest_price_usd.as_deref(), Some("0.0000123"));
        assert_eq!(exact.highest_market_cap_usd.as_deref(), Some("12300"));

        for body in [
            r#"{"highest_price":-1,"highest_market_cap":null,"timestamp":1,"pool_id":null}"#,
            r#"{"highest_price":1e99,"highest_market_cap":null,"timestamp":1,"pool_id":null}"#,
            r#"{"highest_price":1,"highest_market_cap":null,"timestamp":-1,"pool_id":null}"#,
            r#"{"highest_price":1,"highest_market_cap":null,"timestamp":1.5,"pool_id":null}"#,
            r#"{"highest_price":1,"highest_market_cap":null,"timestamp":1,"pool_id":"bad"}"#,
            r#"{"success":false,"highest_price":null}"#,
            r#"{"message":"unavailable"}"#,
        ] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
                .mount(&server)
                .await;
            assert!(
                fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn rate_limit_and_oversized_body_are_typed_without_retry() {
        for status in [401, 402, 403] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("GET"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
            assert!(matches!(
                fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC).await,
                Err(TrackerAthError::AccessDenied(actual)) if actual == status
            ));
        }

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC).await,
            Err(TrackerAthError::RateLimited(429))
        ));

        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_BODY_BYTES + 1]))
            .mount(&server)
            .await;
        assert!(matches!(
            fetch_ath(&http_client().unwrap(), &server.uri(), "fixture-key", JC).await,
            Err(TrackerAthError::ResponseTooLarge)
        ));
    }
}
