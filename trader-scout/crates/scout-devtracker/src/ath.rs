//! ATH observations from Codex (B3; B0.3 measurement).
//!
//! `filterTokens(tokens: ["address:networkId", … ≤ 200])` returns
//! `token.extrema.athFdv` (USD, decimal string) and its timestamp. Codex's
//! free tier is request-limited, so regular observation selects likely runners:
//! migrated tokens and qualifying launchpads without a curve, refreshed by
//! age (`scout_devdb::DevDb::ath_candidates`). An explicit Base backfill also
//! enumerates positive ATHs through Codex's createdAt filter, intersected with
//! all tracked Base launches (ordinary refresh keeps its migrated-token and
//! three-launch creator eligibility). Index days scope pair creation time;
//! returned ATH values are current provider observations. The mutable index
//! is not a complete or canonical launch registry.
//! The current holder count is stored with each observation (a runner needs
//! `top_runners.min_holders`: thin pools fake huge ATHs).
//! A token Codex does not return is stored as an observation of 0 with source
//! `codex-missing`, so it is retried on the age schedule, not every pass.

use std::time::Duration;

use scout_devdb::{AthObservation, CODEX_REQUEST_BUDGET_KEY, DevDb, DevDbError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Codex GraphQL endpoint.
pub const CODEX_URL: &str = "https://graph.codex.io/graphql";
/// Tokens per `filterTokens` request (Codex maximum).
pub const CODEX_BATCH: usize = 200;
const BASE_INDEX_KEY: &str = "ath-discovery:codex:base:v1";
const BASE_INDEX_FILTER: &str = "base-positive-ath-created-at-v1";
const BASE_INDEX_NETWORK: u64 = 8453;
const BASE_INDEX_MAX_OFFSET: usize = 10_000;
const BASE_INDEX_MAX_BODY: usize = 4 * 1024 * 1024;
const BASE_RUNNERS_INDEX_KEY: &str = "ath-runners:codex:base:v1";
const BASE_RUNNERS_INDEX_PAGE_SIZE: usize = 200;
const BASE_RUNNERS_INDEX_MAX_OFFSET: usize = 10_000;
const UTC_DAY: i64 = 86_400;
const MAX_DATE_MISMATCH_EXAMPLES: usize = 4;

/// Codex network id of a chain name.
#[must_use]
pub fn codex_network(chain: &str) -> Option<u64> {
    match chain {
        "solana" => Some(1_399_811_149),
        "bsc" => Some(56),
        "base" => Some(8453),
        "robinhood" => Some(4663),
        _ => None,
    }
}

/// USD decimal string (`"10990755"`, `"380014.127"`) to whole cents, floored;
/// `None` for anything that is not a plain non-negative decimal.
#[must_use]
pub fn usd_to_cents(s: &str) -> Option<i64> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = int.parse().ok()?;
    let mut cents = whole.checked_mul(100)?;
    let mut digits = frac.bytes().map(|b| i64::from(b - b'0'));
    cents = cents.checked_add(digits.next().unwrap_or(0).checked_mul(10)?)?;
    cents.checked_add(digits.next().unwrap_or(0))
}

/// Failure of an ATH pass.
#[derive(Debug, thiserror::Error)]
pub enum AthError {
    #[error("codex http: {0}")]
    Http(String),
    #[error("codex answered an error: {0}")]
    Codex(String),
    #[error("Codex access denied (HTTP {0})")]
    AccessDenied(u16),
    #[error("Codex request budget is exhausted")]
    QuotaExhausted,
    #[error("Codex page-attempt budget {0} is exhausted")]
    PassBudgetExhausted(usize),
    #[error("Codex response exceeded {0} bytes")]
    ResponseTooLarge(usize),
    #[error("Codex requested retry after {0} seconds; cursor is unchanged")]
    RetryLater(u64),
    #[error("Codex index returned malformed data: {0}")]
    MalformedIndex(String),
    #[error("Codex index returned network {actual}, expected Base network {BASE_INDEX_NETWORK}")]
    WrongIndexNetwork { actual: u64 },
    #[error("Codex runner index returned network {actual}, expected {expected}")]
    WrongRunnerIndexNetwork { actual: u64, expected: u64 },
    #[error("Codex index interval [{start}, {end}) remains capped at {max_results} rows")]
    IndexIntervalSaturated {
        start: i64,
        end: i64,
        max_results: usize,
    },
    #[error("Codex index cursor is incompatible or malformed")]
    InvalidIndexCursor,
    #[error("ATH request budget must be greater than zero")]
    EmptyIndexBudget,
    #[error("Base ATH index only supports chain `base`, got `{0}`")]
    IndexChain(String),
    #[error("Codex runner index cursor is incompatible or malformed")]
    InvalidRunnerIndexCursor,
    #[error("Codex runner index reached {max_results} rows without proving the natural end")]
    RunnerIndexSaturated { max_results: usize },
    #[error("{0}")]
    Db(#[from] DevDbError),
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AthReport {
    pub requests: usize,
    pub tokens_asked: usize,
    pub observed: usize,
    pub missing: usize,
}

/// Result of one bounded provider-index pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AthIndexReport {
    pub requests: usize,
    pub provider_rows: usize,
    pub tracked_rows: usize,
    pub completed_days: usize,
    pub date_mismatch_rows: usize,
    pub complete: bool,
    pub window_start: i64,
    pub window_end: i64,
}

/// Integer-USD filters matching the current `top_runners` candidate gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerAthIndexFilter {
    pub min_ath_usd: i64,
    pub max_ath_usd: i64,
    pub min_holders: i64,
    pub min_token_created_at: Option<i64>,
}

/// All-time provider-filtered runner candidate enumeration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerAthIndexReport {
    pub chain: String,
    pub network_id: u64,
    pub requests: usize,
    pub provider_rows: usize,
    pub tracked_rows: usize,
    pub complete: bool,
    pub next_offset: usize,
    pub min_ath_usd: i64,
    pub max_ath_usd: i64,
    pub min_holders: i64,
    pub min_token_created_at: Option<i64>,
    pub token_created_at_gap_rows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunnerIndexCursor {
    version: u8,
    filter: String,
    offset: usize,
    complete: bool,
    #[serde(default)]
    chain: String,
    #[serde(default)]
    network_id: u64,
    #[serde(default)]
    token_created_at_gap_rows: usize,
}

struct ParsedRunnerIndexPage {
    rows: Vec<AthObservation>,
    token_created_at_gap_rows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexCursor {
    version: u8,
    filter: String,
    day_start: i64,
    day_end: i64,
    intervals: Vec<IndexInterval>,
    complete: bool,
    #[serde(default)]
    date_mismatch_rows: usize,
    #[serde(default)]
    date_mismatch_examples: Vec<DateMismatchExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DateMismatchExample {
    token: String,
    created_at: i64,
    interval_start: i64,
    interval_end: i64,
}

struct ParsedIndexPage {
    rows: Vec<AthObservation>,
    date_mismatch_rows: usize,
    date_mismatch_examples: Vec<DateMismatchExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexInterval {
    start: i64,
    end: i64,
    offset: usize,
}

#[derive(Clone, Copy)]
struct IndexConfig {
    page_size: usize,
    max_offset: usize,
    max_body: usize,
    minimum_interval: Duration,
}

impl IndexConfig {
    const PRODUCTION: Self = Self {
        page_size: CODEX_BATCH,
        max_offset: BASE_INDEX_MAX_OFFSET,
        max_body: BASE_INDEX_MAX_BODY,
        minimum_interval: Duration::from_secs(1),
    };
}

async fn query(
    db: &DevDb,
    http: &reqwest::Client,
    key: &str,
    ids: &[String],
    now: i64,
) -> Result<Value, AthError> {
    let q = format!(
        "{{ filterTokens(tokens: {}, limit: {}) {{ results {{ holders liquidity token {{ address networkId extrema {{ athFdv athFdvTimestamp }} }} }} }} }}",
        serde_json::to_string(ids).map_err(|e| AthError::Http(e.to_string()))?,
        CODEX_BATCH
    );
    let mut attempts = 0;
    let mut last_request = None;
    request_graphql(
        db,
        http,
        key,
        CODEX_URL,
        &q,
        now,
        BASE_INDEX_MAX_BODY,
        0,
        Duration::ZERO,
        None,
        CODEX_REQUEST_BUDGET_KEY,
        &mut attempts,
        &mut last_request,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn request_graphql(
    db: &DevDb,
    http: &reqwest::Client,
    key: &str,
    endpoint: &str,
    query: &str,
    now: i64,
    max_body: usize,
    max_retries: usize,
    minimum_interval: Duration,
    pass_budget: Option<usize>,
    budget_key: &str,
    attempts: &mut usize,
    last_request: &mut Option<std::time::Instant>,
) -> Result<Value, AthError> {
    for retry in 0..=max_retries {
        if pass_budget.is_some_and(|budget| *attempts >= budget) {
            return Err(AthError::PassBudgetExhausted(
                pass_budget.unwrap_or_default(),
            ));
        }
        if let Some(last) = *last_request
            && let Some(wait) = minimum_interval.checked_sub(last.elapsed())
        {
            tokio::time::sleep(wait).await;
        }
        db.reserve_request_budget(budget_key, now)
            .await
            .map_err(|e| match e {
                DevDbError::CodexRequestBudgetExhausted => AthError::QuotaExhausted,
                other => AthError::Db(other),
            })?;
        *attempts += 1;
        *last_request = Some(std::time::Instant::now());
        let response = http
            .post(endpoint)
            .header("Authorization", key)
            .json(&json!({ "query": query }))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() || error.is_connect() || error.is_request() => {
                if retry < max_retries {
                    tokio::time::sleep(Duration::from_secs(1u64 << retry.min(2))).await;
                    continue;
                }
                return Err(AthError::Http(error.without_url().to_string()));
            }
            Err(error) => return Err(AthError::Http(error.without_url().to_string())),
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let bytes = read_bounded(response, max_body).await?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AthError::AccessDenied(status.as_u16()));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            let body = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            if body.contains("quota") || body.contains("monthly") {
                return Err(AthError::QuotaExhausted);
            }
            if retry < max_retries {
                tokio::time::sleep(retry_delay(retry_after, retry)?).await;
                continue;
            }
            return Err(AthError::Http(format!("HTTP {status}")));
        }
        if !status.is_success() {
            return Err(AthError::Http(format!("HTTP {status}")));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AthError::Http(format!("invalid JSON response: {error}")))?;
        if let Some(errors) = value.get("errors") {
            let detail = errors.to_string().chars().take(300).collect::<String>();
            if detail.to_ascii_lowercase().contains("quota")
                || detail.to_ascii_lowercase().contains("monthly")
            {
                return Err(AthError::QuotaExhausted);
            }
            return Err(AthError::Codex(detail));
        }
        return Ok(value);
    }
    Err(AthError::Http(
        "Codex request retry loop ended unexpectedly".into(),
    ))
}

fn retry_delay(retry_after: Option<u64>, retry: usize) -> Result<Duration, AthError> {
    let Some(value) = retry_after else {
        return Ok(Duration::from_secs(1u64 << retry.min(2)));
    };
    if value > 30 {
        return Err(AthError::RetryLater(value));
    }
    Ok(Duration::from_secs(value))
}

async fn read_bounded(
    mut response: reqwest::Response,
    max_body: usize,
) -> Result<Vec<u8>, AthError> {
    let max_body_u64 = u64::try_from(max_body).unwrap_or(u64::MAX);
    if response
        .content_length()
        .is_some_and(|len| len > max_body_u64)
    {
        return Err(AthError::ResponseTooLarge(max_body));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| AthError::Http(error.without_url().to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > max_body {
            return Err(AthError::ResponseTooLarge(max_body));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Enumerate positive ATH tokens through Base's createdAt filter for closed UTC
/// days, intersecting each page with launches already tracked by this store.
///
/// # Errors
/// Provider, database, malformed cursor/response, or an unsplittable capped
/// provider interval. Partial pages are committed before their cursor moves.
pub async fn backfill_base_ath_from_index(
    db: &DevDb,
    codex_key: &str,
    days: i64,
    max_requests: usize,
    now: i64,
) -> Result<AthIndexReport, AthError> {
    backfill_base_ath_from_index_at(
        db,
        codex_key,
        days,
        max_requests,
        now,
        CODEX_URL,
        IndexConfig::PRODUCTION,
        BASE_INDEX_KEY,
        CODEX_REQUEST_BUDGET_KEY,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn backfill_base_ath_from_index_at(
    db: &DevDb,
    codex_key: &str,
    days: i64,
    max_requests: usize,
    now: i64,
    endpoint: &str,
    config: IndexConfig,
    cursor_prefix: &str,
    budget_key: &str,
) -> Result<AthIndexReport, AthError> {
    if days <= 0 || max_requests == 0 {
        return Err(AthError::EmptyIndexBudget);
    }
    if config.page_size == 0 || config.max_offset < config.page_size || config.max_body == 0 {
        return Err(AthError::InvalidIndexCursor);
    }
    let window_end = now.div_euclid(UTC_DAY) * UTC_DAY;
    let window_start = window_end
        .checked_sub(
            days.checked_mul(UTC_DAY)
                .ok_or(AthError::InvalidIndexCursor)?,
        )
        .ok_or(AthError::InvalidIndexCursor)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|error| AthError::Http(error.to_string()))?;
    let mut report = AthIndexReport {
        window_start,
        window_end,
        ..AthIndexReport::default()
    };
    let mut last_request = None;
    let pass_started = std::time::Instant::now();
    let mut day_start = window_end - UTC_DAY;
    while day_start >= window_start {
        let key = format!("{cursor_prefix}:{day_start}");
        let mut cursor = match db.cursor(&key).await? {
            Some(position) => {
                let cursor: IndexCursor =
                    serde_json::from_str(&position).map_err(|_| AthError::InvalidIndexCursor)?;
                validate_index_cursor(&cursor, day_start, config)?;
                report.date_mismatch_rows = report
                    .date_mismatch_rows
                    .checked_add(cursor.date_mismatch_rows)
                    .ok_or(AthError::InvalidIndexCursor)?;
                if cursor.complete {
                    report.completed_days += 1;
                    day_start -= UTC_DAY;
                    continue;
                }
                cursor
            }
            None => IndexCursor {
                version: 1,
                filter: BASE_INDEX_FILTER.to_string(),
                day_start,
                day_end: day_start + UTC_DAY,
                intervals: vec![IndexInterval {
                    start: day_start,
                    end: day_start + UTC_DAY,
                    offset: 0,
                }],
                complete: false,
                date_mismatch_rows: 0,
                date_mismatch_examples: Vec::new(),
            },
        };
        while let Some(interval) = cursor.intervals.last().cloned() {
            let query = index_query(&interval, config.page_size);
            let value = match request_graphql(
                db,
                &client,
                codex_key,
                endpoint,
                &query,
                now,
                config.max_body,
                2,
                config.minimum_interval,
                Some(max_requests),
                budget_key,
                &mut report.requests,
                &mut last_request,
            )
            .await
            {
                Ok(value) => value,
                Err(AthError::PassBudgetExhausted(_)) => return Ok(report),
                Err(error) => return Err(error),
            };
            let page_now = now.saturating_add(
                i64::try_from(pass_started.elapsed().as_secs()).unwrap_or(i64::MAX),
            );
            let page = parse_index_page(&value, &interval, page_now, config.page_size)?;
            report.provider_rows += page.rows.len();
            report.date_mismatch_rows = report
                .date_mismatch_rows
                .checked_add(page.date_mismatch_rows)
                .ok_or(AthError::InvalidIndexCursor)?;
            cursor.date_mismatch_rows = cursor
                .date_mismatch_rows
                .checked_add(page.date_mismatch_rows)
                .ok_or(AthError::InvalidIndexCursor)?;
            let example_slots =
                MAX_DATE_MISMATCH_EXAMPLES.saturating_sub(cursor.date_mismatch_examples.len());
            cursor
                .date_mismatch_examples
                .extend(page.date_mismatch_examples.into_iter().take(example_slots));
            let tokens: Vec<String> = page.rows.iter().map(|row| row.token.clone()).collect();
            let known = db.known_launch_tokens("base", &tokens).await?;
            let known: std::collections::BTreeMap<String, String> = known
                .into_iter()
                .map(|token| (token.to_ascii_lowercase(), token))
                .collect();
            let observations: Vec<AthObservation> = page
                .rows
                .into_iter()
                .filter_map(|row| {
                    known
                        .get(&row.token.to_ascii_lowercase())
                        .map(|token| AthObservation {
                            token: token.clone(),
                            ..row
                        })
                })
                .collect();
            report.tracked_rows += observations.len();
            db.upsert_ath(&observations).await?;

            let result_len = tokens.len();
            if result_len == config.page_size {
                if interval.offset + result_len >= config.max_offset {
                    let width = interval.end - interval.start;
                    if width <= 1 {
                        return Err(AthError::IndexIntervalSaturated {
                            start: interval.start,
                            end: interval.end,
                            max_results: config.max_offset,
                        });
                    }
                    cursor.intervals.pop();
                    #[allow(clippy::integer_division)]
                    let midpoint = interval.start + width / 2;
                    cursor.intervals.push(IndexInterval {
                        start: midpoint,
                        end: interval.end,
                        offset: 0,
                    });
                    cursor.intervals.push(IndexInterval {
                        start: interval.start,
                        end: midpoint,
                        offset: 0,
                    });
                } else if let Some(current) = cursor.intervals.last_mut() {
                    current.offset += result_len;
                }
            } else {
                cursor.intervals.pop();
            }
            if cursor.intervals.is_empty() {
                cursor.complete = true;
                report.completed_days += 1;
            }
            save_index_cursor(db, &key, &cursor, page_now).await?;
            if cursor.complete {
                break;
            }
        }
        day_start -= UTC_DAY;
    }
    report.complete = true;
    Ok(report)
}

fn validate_index_cursor(
    cursor: &IndexCursor,
    day_start: i64,
    config: IndexConfig,
) -> Result<(), AthError> {
    let day_end = day_start + UTC_DAY;
    let max_date_mismatch_rows = config.max_offset.saturating_mul(86_400);
    if cursor.version != 1
        || cursor.filter != BASE_INDEX_FILTER
        || cursor.day_start != day_start
        || cursor.day_end != day_end
        || cursor.complete != cursor.intervals.is_empty()
        || cursor.intervals.len() > 18
        || cursor.date_mismatch_rows > max_date_mismatch_rows
        || cursor.date_mismatch_rows < cursor.date_mismatch_examples.len()
        || cursor.date_mismatch_examples.len() > MAX_DATE_MISMATCH_EXAMPLES
        || cursor.date_mismatch_examples.iter().any(|example| {
            !valid_evm_address(&example.token)
                || example.token != example.token.to_ascii_lowercase()
                || example.created_at < 0
                || example.interval_start < day_start
                || example.interval_end > day_end
                || example.interval_start >= example.interval_end
                || (example.created_at >= example.interval_start
                    && example.created_at < example.interval_end)
        })
        || cursor.intervals.iter().any(|interval| {
            interval.start < day_start
                || interval.end > day_end
                || interval.start >= interval.end
                || interval.offset > config.max_offset - config.page_size
                || interval.offset % config.page_size != 0
        })
    {
        return Err(AthError::InvalidIndexCursor);
    }
    let mut intervals: Vec<_> = cursor.intervals.iter().collect();
    intervals.sort_by_key(|interval| interval.start);
    if intervals.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.end > right.start)
    }) {
        return Err(AthError::InvalidIndexCursor);
    }
    Ok(())
}

async fn save_index_cursor(
    db: &DevDb,
    key: &str,
    cursor: &IndexCursor,
    now: i64,
) -> Result<(), AthError> {
    let position = serde_json::to_string(cursor).map_err(|_| AthError::InvalidIndexCursor)?;
    db.set_cursor(key, &position, now).await?;
    Ok(())
}

/// Enumerate Codex's all-time Base token index after applying the configured
/// runner ATH and holder filters, then store observations for known launches.
/// The provider's first-pair `createdAt` is ordered for pagination only; it
/// does not bound the enumeration or define the annual derivation scope.
///
/// # Errors
/// Provider, database, malformed cursor/response, or provider offset cap.
pub async fn backfill_base_runner_ath_from_index(
    db: &DevDb,
    codex_key: &str,
    filter: RunnerAthIndexFilter,
    max_requests: usize,
    now: i64,
) -> Result<RunnerAthIndexReport, AthError> {
    backfill_runner_ath_from_index(db, codex_key, "base", filter, max_requests, now).await
}

/// Enumerate a chain's all-time Codex index with the configured runner
/// filters and store observations for already tracked launch tokens.
/// Supported chains are Solana, Base, BSC, and Robinhood Chain.
///
/// # Errors
/// Unsupported chain, provider, database, malformed cursor/response, or offset cap.
pub async fn backfill_runner_ath_from_index(
    db: &DevDb,
    codex_key: &str,
    chain: &str,
    filter: RunnerAthIndexFilter,
    max_requests: usize,
    now: i64,
) -> Result<RunnerAthIndexReport, AthError> {
    let Some(network) =
        codex_network(chain).filter(|_| matches!(chain, "solana" | "base" | "bsc" | "robinhood"))
    else {
        return Err(AthError::IndexChain(chain.to_string()));
    };
    let cursor_prefix = if chain == "base" {
        BASE_RUNNERS_INDEX_KEY.to_string()
    } else {
        format!("ath-runners:codex:{chain}:{network}:v1")
    };
    backfill_runner_ath_from_index_at(
        db,
        codex_key,
        chain,
        network,
        filter,
        max_requests,
        now,
        CODEX_URL,
        IndexConfig {
            page_size: BASE_RUNNERS_INDEX_PAGE_SIZE,
            max_offset: BASE_RUNNERS_INDEX_MAX_OFFSET,
            ..IndexConfig::PRODUCTION
        },
        &cursor_prefix,
        CODEX_REQUEST_BUDGET_KEY,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn backfill_base_runner_ath_from_index_at(
    db: &DevDb,
    codex_key: &str,
    filter: RunnerAthIndexFilter,
    max_requests: usize,
    now: i64,
    endpoint: &str,
    config: IndexConfig,
    cursor_prefix: &str,
    budget_key: &str,
) -> Result<RunnerAthIndexReport, AthError> {
    backfill_runner_ath_from_index_at(
        db,
        codex_key,
        "base",
        BASE_INDEX_NETWORK,
        filter,
        max_requests,
        now,
        endpoint,
        config,
        cursor_prefix,
        budget_key,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn backfill_runner_ath_from_index_at(
    db: &DevDb,
    codex_key: &str,
    chain: &str,
    network: u64,
    filter: RunnerAthIndexFilter,
    max_requests: usize,
    now: i64,
    endpoint: &str,
    config: IndexConfig,
    cursor_prefix: &str,
    budget_key: &str,
) -> Result<RunnerAthIndexReport, AthError> {
    let _min_cents = filter
        .min_ath_usd
        .checked_mul(100)
        .ok_or(AthError::InvalidRunnerIndexCursor)?;
    let _max_cents = filter
        .max_ath_usd
        .checked_mul(100)
        .ok_or(AthError::InvalidRunnerIndexCursor)?;
    if filter.min_ath_usd <= 0
        || filter.max_ath_usd < filter.min_ath_usd
        || filter.min_holders < 0
        || filter
            .min_token_created_at
            .is_some_and(|timestamp| timestamp < 0)
        || max_requests == 0
        || config.page_size == 0
        || config.max_offset < config.page_size
        || config.max_body == 0
    {
        return Err(AthError::InvalidRunnerIndexCursor);
    }
    let filter_id = {
        let base = if chain == "base" {
            format!(
                "base-runner-ath-holders-v1:{}:{}:{}",
                filter.min_ath_usd, filter.max_ath_usd, filter.min_holders
            )
        } else {
            format!(
                "{chain}-{network}-runner-ath-holders-v1:{}:{}:{}",
                filter.min_ath_usd, filter.max_ath_usd, filter.min_holders
            )
        };
        match filter.min_token_created_at {
            Some(timestamp) => format!("{base}:token-created-since-{timestamp}"),
            None => base,
        }
    };
    let cursor_key = format!("{cursor_prefix}:{filter_id}");
    let mut cursor = match db.cursor(&cursor_key).await? {
        Some(position) => {
            let cursor: RunnerIndexCursor =
                serde_json::from_str(&position).map_err(|_| AthError::InvalidRunnerIndexCursor)?;
            validate_runner_index_cursor(&cursor, &filter_id, chain, network, config)?;
            cursor
        }
        None => RunnerIndexCursor {
            version: 1,
            filter: filter_id,
            offset: 0,
            complete: false,
            chain: chain.to_string(),
            network_id: network,
            token_created_at_gap_rows: 0,
        },
    };
    let mut report = RunnerAthIndexReport {
        next_offset: cursor.offset,
        chain: chain.to_string(),
        network_id: network,
        min_ath_usd: filter.min_ath_usd,
        max_ath_usd: filter.max_ath_usd,
        min_holders: filter.min_holders,
        min_token_created_at: filter.min_token_created_at,
        token_created_at_gap_rows: cursor.token_created_at_gap_rows,
        complete: cursor.complete,
        ..RunnerAthIndexReport::default()
    };
    if cursor.complete {
        return Ok(report);
    }
    if cursor.offset >= config.max_offset {
        return Err(AthError::RunnerIndexSaturated {
            max_results: config.max_offset,
        });
    }
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|error| AthError::Http(error.to_string()))?;
    let mut last_request = None;
    while report.requests < max_requests {
        let query = runner_index_query(network, filter, cursor.offset, config.page_size);
        let value = match request_graphql(
            db,
            &http,
            codex_key,
            endpoint,
            &query,
            now,
            config.max_body,
            0,
            config.minimum_interval,
            Some(max_requests),
            budget_key,
            &mut report.requests,
            &mut last_request,
        )
        .await
        {
            Ok(value) => value,
            Err(AthError::PassBudgetExhausted(_)) => break,
            Err(error) => return Err(error),
        };
        let parsed =
            parse_runner_index_page(&value, now, config.page_size, filter, network, chain)?;
        let page_len = parsed.rows.len();
        let page_date_gaps = parsed.token_created_at_gap_rows;
        report.provider_rows = report
            .provider_rows
            .checked_add(page_len)
            .ok_or(AthError::InvalidRunnerIndexCursor)?;
        let tokens: Vec<String> = parsed.rows.iter().map(|row| row.token.clone()).collect();
        let known = db.known_launch_tokens(chain, &tokens).await?;
        let known: std::collections::BTreeMap<String, String> = known
            .into_iter()
            .map(|token| (runner_token_key(chain, &token), token))
            .collect();
        let observations: Vec<AthObservation> = parsed
            .rows
            .into_iter()
            .filter_map(|row| {
                known
                    .get(&runner_token_key(chain, &row.token))
                    .map(|token| AthObservation {
                        token: token.clone(),
                        ..row
                    })
            })
            .collect();
        report.tracked_rows = report
            .tracked_rows
            .checked_add(observations.len())
            .ok_or(AthError::InvalidRunnerIndexCursor)?;
        db.upsert_ath(&observations).await?;
        cursor.token_created_at_gap_rows = cursor
            .token_created_at_gap_rows
            .checked_add(page_date_gaps)
            .ok_or(AthError::InvalidRunnerIndexCursor)?;
        report.token_created_at_gap_rows = cursor.token_created_at_gap_rows;

        // `provider_rows` is cumulative, so use the page's own length for
        // offset progression even when no row maps to a tracked launch.
        if page_len == config.page_size {
            let next = cursor
                .offset
                .checked_add(page_len)
                .ok_or(AthError::InvalidRunnerIndexCursor)?;
            if next >= config.max_offset {
                cursor.offset = next;
                cursor.chain = chain.to_string();
                cursor.network_id = network;
                save_runner_index_cursor(db, &cursor_key, &cursor, now).await?;
                return Err(AthError::RunnerIndexSaturated {
                    max_results: config.max_offset,
                });
            }
            cursor.offset = next;
        } else {
            cursor.complete = true;
        }
        cursor.chain = chain.to_string();
        cursor.network_id = network;
        save_runner_index_cursor(db, &cursor_key, &cursor, now).await?;
        report.next_offset = cursor.offset;
        report.complete = cursor.complete;
        if cursor.complete {
            break;
        }
    }
    Ok(report)
}

fn validate_runner_index_cursor(
    cursor: &RunnerIndexCursor,
    filter: &str,
    chain: &str,
    network: u64,
    config: IndexConfig,
) -> Result<(), AthError> {
    if cursor.version != 1
        || cursor.filter != filter
        || (!cursor.chain.is_empty() && (cursor.chain != chain || cursor.network_id != network))
        || (cursor.chain.is_empty() && (chain != "base" || network != BASE_INDEX_NETWORK))
        || cursor.offset > config.max_offset
        || (cursor.complete && cursor.offset >= config.max_offset)
        || !cursor.offset.is_multiple_of(config.page_size)
    {
        return Err(AthError::InvalidRunnerIndexCursor);
    }
    Ok(())
}

async fn save_runner_index_cursor(
    db: &DevDb,
    key: &str,
    cursor: &RunnerIndexCursor,
    now: i64,
) -> Result<(), AthError> {
    let position = serde_json::to_string(cursor).map_err(|_| AthError::InvalidRunnerIndexCursor)?;
    db.set_cursor(key, &position, now).await?;
    Ok(())
}

fn runner_index_query(
    network: u64,
    filter: RunnerAthIndexFilter,
    offset: usize,
    page_size: usize,
) -> String {
    let token_created_filter = filter
        .min_token_created_at
        .map_or_else(String::new, |timestamp| {
            format!(", tokenCreatedAt: {{gte: {timestamp}}}")
        });
    format!(
        "{{ filterTokens(filters: {{network: [{network}], isTestnet: false, includeScams: true, athFdv: {{gte: {}, lte: {}}}, holders: {{gte: {}}}{token_created_filter}}}, rankings: [{{attribute: createdAt, direction: ASC}}], limit: {page_size}, offset: {offset}) {{ results {{ createdAt token {{ address networkId createdAt extrema {{ athFdv athFdvTimestamp }} }} holders liquidity }} }} }}",
        filter.min_ath_usd, filter.max_ath_usd, filter.min_holders
    )
}

fn parse_runner_index_page(
    value: &Value,
    as_of: i64,
    page_size: usize,
    filter: RunnerAthIndexFilter,
    expected_network: u64,
    chain: &str,
) -> Result<ParsedRunnerIndexPage, AthError> {
    let min_cents = filter
        .min_ath_usd
        .checked_mul(100)
        .ok_or(AthError::InvalidRunnerIndexCursor)?;
    let max_cents = filter
        .max_ath_usd
        .checked_mul(100)
        .ok_or(AthError::InvalidRunnerIndexCursor)?;
    let results = value
        .pointer("/data/filterTokens/results")
        .and_then(Value::as_array)
        .ok_or_else(|| AthError::MalformedIndex("missing results array".into()))?;
    if results.len() > page_size {
        return Err(AthError::MalformedIndex(
            "page exceeded requested limit".into(),
        ));
    }
    let mut token_created_at_gap_rows = 0usize;
    let rows = results
        .iter()
        .map(|result| {
            let token = result
                .pointer("/token/address")
                .and_then(Value::as_str)
                .filter(|address| valid_runner_token_address(chain, address))
                .ok_or_else(|| AthError::MalformedIndex("invalid token address".into()))?;
            let network = result
                .pointer("/token/networkId")
                .and_then(Value::as_u64)
                .ok_or_else(|| AthError::MalformedIndex("missing token networkId".into()))?;
            if network != expected_network {
                return Err(AthError::WrongRunnerIndexNetwork {
                    actual: network,
                    expected: expected_network,
                });
            }
            let ath = result
                .pointer("/token/extrema/athFdv")
                .and_then(Value::as_str)
                .ok_or_else(|| AthError::MalformedIndex("missing ATH".into()))?;
            let cents = usd_to_cents(ath)
                .filter(|_| {
                    decimal_within_usd_range(
                        ath,
                        min_cents.div_euclid(100),
                        max_cents.div_euclid(100),
                    )
                })
                .ok_or_else(|| AthError::MalformedIndex("ATH outside runner filter".into()))?;
            let holders = optional_integer(result.get("holders"))?
                .filter(|holders| *holders >= filter.min_holders)
                .ok_or_else(|| {
                    AthError::MalformedIndex("holders missing or below filter".into())
                })?;
            let token_created_at = result.pointer("/token/createdAt").and_then(Value::as_i64);
            if token_created_at.is_none_or(|timestamp| {
                timestamp < 0
                    || timestamp > as_of
                    || filter
                        .min_token_created_at
                        .is_some_and(|floor| timestamp < floor)
            }) {
                token_created_at_gap_rows = token_created_at_gap_rows
                    .checked_add(1)
                    .ok_or(AthError::InvalidRunnerIndexCursor)?;
            }
            Ok(AthObservation {
                chain: chain.into(),
                token: runner_token_key(chain, token),
                ath_fdv_cents: cents,
                ath_at: optional_integer(result.pointer("/token/extrema/athFdvTimestamp"))?,
                source: "codex-runner-index".into(),
                observed_at: as_of,
                holders: Some(holders),
                liquidity_cents: optional_decimal_cents(result.get("liquidity"))?,
            })
        })
        .collect::<Result<Vec<_>, AthError>>()?;
    Ok(ParsedRunnerIndexPage {
        rows,
        token_created_at_gap_rows,
    })
}

fn index_query(interval: &IndexInterval, page_size: usize) -> String {
    format!(
        "{{ filterTokens(filters: {{network: [{BASE_INDEX_NETWORK}], isTestnet: false, includeScams: true, createdAt: {{gte: {}, lt: {}}}, athFdv: {{gt: 0}}}}, rankings: [{{attribute: createdAt, direction: ASC}}], limit: {page_size}, offset: {}) {{ results {{ createdAt token {{ address networkId extrema {{ athFdv athFdvTimestamp }} }} holders liquidity }} }} }}",
        interval.start, interval.end, interval.offset
    )
}

fn parse_index_page(
    value: &Value,
    interval: &IndexInterval,
    as_of: i64,
    page_size: usize,
) -> Result<ParsedIndexPage, AthError> {
    let results = value
        .pointer("/data/filterTokens/results")
        .and_then(Value::as_array)
        .ok_or_else(|| AthError::MalformedIndex("missing results array".into()))?;
    if results.len() > page_size {
        return Err(AthError::MalformedIndex(
            "page exceeded requested limit".into(),
        ));
    }
    let mut date_mismatch_rows = 0usize;
    let mut date_mismatch_examples = Vec::new();
    let rows = results
        .iter()
        .map(|result| {
            let created_at = result
                .get("createdAt")
                .and_then(Value::as_i64)
                .ok_or_else(|| AthError::MalformedIndex("missing createdAt".into()))?;
            if created_at < 0 {
                return Err(AthError::MalformedIndex(
                    "invalid negative createdAt".into(),
                ));
            }
            let date_mismatch = created_at < interval.start || created_at >= interval.end;
            let token = result
                .pointer("/token/address")
                .and_then(Value::as_str)
                .ok_or_else(|| AthError::MalformedIndex("missing token address".into()))?;
            if !valid_evm_address(token) {
                return Err(AthError::MalformedIndex("invalid token address".into()));
            }
            let network = result
                .pointer("/token/networkId")
                .and_then(Value::as_u64)
                .ok_or_else(|| AthError::MalformedIndex("missing token networkId".into()))?;
            if network != BASE_INDEX_NETWORK {
                return Err(AthError::WrongIndexNetwork { actual: network });
            }
            let ath = result
                .pointer("/token/extrema/athFdv")
                .and_then(Value::as_str)
                .ok_or_else(|| AthError::MalformedIndex("missing positive ATH".into()))?;
            if !positive_decimal(ath) {
                return Err(AthError::MalformedIndex("non-positive ATH".into()));
            }
            let cents = usd_to_cents(ath)
                .ok_or_else(|| AthError::MalformedIndex("invalid ATH decimal".into()))?;
            let ath_at = optional_integer(result.pointer("/token/extrema/athFdvTimestamp"))?;
            let holders = optional_integer(result.get("holders"))?;
            let liquidity_cents = optional_decimal_cents(result.get("liquidity"))?;
            if date_mismatch {
                date_mismatch_rows = date_mismatch_rows
                    .checked_add(1)
                    .ok_or(AthError::InvalidIndexCursor)?;
                if date_mismatch_examples.len() < MAX_DATE_MISMATCH_EXAMPLES {
                    date_mismatch_examples.push(DateMismatchExample {
                        token: token.to_ascii_lowercase(),
                        created_at,
                        interval_start: interval.start,
                        interval_end: interval.end,
                    });
                }
            }
            Ok(AthObservation {
                chain: "base".into(),
                token: token.to_ascii_lowercase(),
                ath_fdv_cents: cents,
                ath_at,
                source: if date_mismatch {
                    "codex-index-date-mismatch".into()
                } else {
                    "codex".into()
                },
                observed_at: as_of,
                holders,
                liquidity_cents,
            })
        })
        .collect::<Result<Vec<_>, AthError>>()?;
    Ok(ParsedIndexPage {
        rows,
        date_mismatch_rows,
        date_mismatch_examples,
    })
}

fn optional_integer(value: Option<&Value>) -> Result<Option<i64>, AthError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_i64()
            .filter(|value| *value >= 0)
            .map(Some)
            .ok_or_else(|| AthError::MalformedIndex("invalid negative/non-integer field".into())),
        _ => Err(AthError::MalformedIndex("invalid integer field".into())),
    }
}

fn optional_decimal_cents(value: Option<&Value>) -> Result<Option<i64>, AthError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let cents = match value {
        Value::String(value) => usd_to_cents(value),
        Value::Number(value) => value
            .as_u64()
            .and_then(|whole| whole.checked_mul(100))
            .and_then(|cents| i64::try_from(cents).ok()),
        _ => None,
    };
    cents.map(Some).ok_or_else(|| {
        AthError::MalformedIndex("decimal must be an exact non-negative string or integer".into())
    })
}

fn positive_decimal(value: &str) -> bool {
    value
        .bytes()
        .any(|digit| digit.is_ascii_digit() && digit != b'0')
        && usd_to_cents(value).is_some()
}

fn decimal_within_usd_range(value: &str, min_usd: i64, max_usd: i64) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let Some(whole) = whole.parse::<i64>().ok() else {
        return false;
    };
    if whole < min_usd || whole > max_usd {
        return false;
    }
    whole < max_usd || fraction.bytes().all(|digit| digit == b'0')
}

fn valid_evm_address(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_runner_token_address(chain: &str, value: &str) -> bool {
    if chain == "solana" {
        (32..=44).contains(&value.len())
            && bs58::decode(value)
                .into_vec()
                .is_ok_and(|mint| mint.len() == 32)
    } else {
        valid_evm_address(value)
    }
}

fn runner_token_key(chain: &str, value: &str) -> String {
    if chain == "solana" {
        value.to_string()
    } else {
        value.to_ascii_lowercase()
    }
}

/// Observe up to `max_requests × 200` ATH candidate tokens of `chain`.
///
/// # Errors
/// HTTP / Codex / database failure (observations stored so far stay).
pub async fn observe_ath(
    db: &DevDb,
    codex_key: &str,
    chain: &str,
    max_requests: usize,
    now: i64,
) -> Result<AthReport, AthError> {
    let mut report = AthReport::default();
    let Some(network) = codex_network(chain) else {
        return Ok(report);
    };
    let limit = i64::try_from(max_requests.saturating_mul(CODEX_BATCH)).unwrap_or(i64::MAX);
    let candidates = db.ath_candidates(chain, now, limit).await?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| AthError::Http(e.to_string()))?;
    for batch in candidates.chunks(CODEX_BATCH) {
        let ids: Vec<String> = batch.iter().map(|t| format!("{t}:{network}")).collect();
        let v = query(db, &http, codex_key, &ids, now).await?;
        report.requests += 1;
        report.tokens_asked += batch.len();
        let mut rows: Vec<AthObservation> = Vec::with_capacity(batch.len());
        let results = v
            .pointer("/data/filterTokens/results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for r in &results {
            let Some(addr) = r.pointer("/token/address").and_then(Value::as_str) else {
                continue;
            };
            let Some(cents) = r
                .pointer("/token/extrema/athFdv")
                .and_then(Value::as_str)
                .and_then(usd_to_cents)
            else {
                continue;
            };
            rows.push(AthObservation {
                chain: chain.to_string(),
                token: addr.to_string(),
                ath_fdv_cents: cents,
                ath_at: r
                    .pointer("/token/extrema/athFdvTimestamp")
                    .and_then(Value::as_i64),
                source: "codex".to_string(),
                observed_at: now,
                holders: r.get("holders").and_then(Value::as_i64),
                liquidity_cents: optional_decimal_cents(r.get("liquidity"))?,
            });
        }
        report.observed += rows.len();
        // tokens Codex did not return: an observation of 0 (retried on schedule)
        let seen: std::collections::BTreeSet<String> =
            rows.iter().map(|r| r.token.to_ascii_lowercase()).collect();
        for t in batch {
            if !seen.contains(&t.to_ascii_lowercase()) {
                report.missing += 1;
                rows.push(AthObservation {
                    chain: chain.to_string(),
                    token: t.clone(),
                    ath_fdv_cents: 0,
                    ath_at: None,
                    source: "codex-missing".to_string(),
                    observed_at: now,
                    holders: None,
                    liquidity_cents: None,
                });
            }
        }
        db.upsert_ath(&rows).await?;
        // free tier: 5 requests/s
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use scout_devdb::Launch;
    use sqlx::Row;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    async fn isolated_db() -> Option<DevDb> {
        let url = std::env::var("SCOUT_TEST_DATABASE_URL").ok()?;
        let db = DevDb::connect(&url, 2).await.ok()?;
        db.migrate().await.ok()?;
        Some(db)
    }

    fn token_address(n: u64) -> String {
        format!("0x{n:040x}")
    }

    struct IndexReplay {
        calls: Arc<Mutex<Vec<String>>>,
        first: String,
        second: String,
    }

    impl Respond for IndexReplay {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap().to_string();
            self.calls.lock().unwrap().push(query.clone());
            let start = query
                .split("createdAt: {gte: ")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse::<i64>()
                .unwrap();
            let end = query
                .split(", lt: ")
                .nth(1)
                .unwrap()
                .split('}')
                .next()
                .unwrap()
                .parse::<i64>()
                .unwrap();
            let offset = query
                .rsplit("offset: ")
                .next()
                .unwrap()
                .split(')')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let address = match (end - start == UTC_DAY, offset) {
                (true, 0) => Some(self.first.as_str()),
                (true, 1) => Some(self.second.as_str()),
                _ => None,
            };
            let results = address
                .map(|address| {
                    json!([{
                        "createdAt": start + 1,
                        "token": {
                            "address": address,
                            "networkId": BASE_INDEX_NETWORK,
                            "extrema": {"athFdv": "1.239", "athFdvTimestamp": start + 2}
                        },
                        "holders": 17,
                        "liquidity": "2.5"
                    }])
                })
                .unwrap_or_else(|| json!([]));
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": results}}
            }))
        }
    }

    struct RetryOnce(AtomicUsize);

    impl Respond for RetryOnce {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "0")
                    .set_body_string("temporarily busy")
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"data": {"ok": true}}))
            }
        }
    }

    struct AlwaysCapped(String);

    impl Respond for AlwaysCapped {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            let start = query
                .split("createdAt: {gte: ")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse::<i64>()
                .unwrap();
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": [{
                    "createdAt": start,
                    "token": {"address": self.0, "networkId": BASE_INDEX_NETWORK,
                        "extrema": {"athFdv": "1"}}
                }]}}
            }))
        }
    }

    struct DateMismatchPages {
        rows: Value,
        offsets: Arc<Mutex<Vec<usize>>>,
    }

    struct RunnerIndexPages {
        rows: Value,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Respond for RunnerIndexPages {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap().to_string();
            self.calls.lock().unwrap().push(query.clone());
            let offset = query
                .rsplit("offset: ")
                .next()
                .unwrap()
                .split(')')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let limit = query
                .split("limit: ")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let rows = self.rows.as_array().unwrap();
            let page = rows
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": page}}
            }))
        }
    }

    impl Respond for DateMismatchPages {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            let offset = query
                .rsplit("offset: ")
                .next()
                .unwrap()
                .split(')')
                .next()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            self.offsets.lock().unwrap().push(offset);
            let results = if offset == 0 {
                self.rows.clone()
            } else {
                json!([])
            };
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": results}}
            }))
        }
    }

    #[test]
    fn usd_strings_become_floored_cents() {
        assert_eq!(usd_to_cents("10990755"), Some(1_099_075_500));
        assert_eq!(usd_to_cents("380014.127"), Some(38_001_412));
        assert_eq!(usd_to_cents("0.5"), Some(50));
        assert_eq!(usd_to_cents("12."), Some(1_200));
        assert_eq!(usd_to_cents("-1"), None);
        assert_eq!(usd_to_cents("1e9"), None);
        assert_eq!(usd_to_cents(""), None);
    }

    #[test]
    fn networks_cover_the_four_chains() {
        assert_eq!(codex_network("solana"), Some(1_399_811_149));
        assert_eq!(codex_network("robinhood"), Some(4663));
        assert_eq!(codex_network("polygon"), None);
    }

    #[test]
    fn liquidity_cents_use_exact_strings_or_integer_usd() {
        assert_eq!(
            optional_decimal_cents(Some(&json!("12.349"))).unwrap(),
            Some(1_234)
        );
        assert_eq!(
            optional_decimal_cents(Some(&json!(12))).unwrap(),
            Some(1_200)
        );
        assert_eq!(optional_decimal_cents(Some(&Value::Null)).unwrap(), None);
        assert!(optional_decimal_cents(Some(&json!(12.34))).is_err());
        assert!(optional_decimal_cents(Some(&json!("-1"))).is_err());
    }

    #[test]
    fn live_base_index_subset_preserves_exact_ath_and_unknown_holders() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_base_positive_ath_2026-10-10.json"
        ))
        .unwrap();
        let rows = parse_index_page(
            &fixture,
            &IndexInterval {
                start: 1_791_504_000,
                end: 1_791_590_400,
                offset: 0,
            },
            1_791_662_000,
            200,
        )
        .unwrap();
        assert_eq!(rows.rows.len(), 2);
        assert_eq!(rows.rows[0].ath_fdv_cents, 762_200);
        assert_eq!(rows.rows[1].ath_fdv_cents, 762_700);
        assert_eq!(rows.rows[0].ath_at, Some(1_791_504_003));
        assert_eq!(rows.rows[0].holders, None);
        assert_eq!(rows.rows[0].liquidity_cents, Some(0));
        assert_eq!(rows.rows[0].source, "codex");
    }

    #[test]
    fn indexed_query_uses_positive_base_ath_and_half_open_created_at() {
        let query = index_query(
            &IndexInterval {
                start: 10,
                end: 20,
                offset: 200,
            },
            200,
        );
        assert!(query.contains("network: [8453]"));
        assert!(query.contains("isTestnet: false"));
        assert!(query.contains("includeScams: true"));
        assert!(query.contains("createdAt: {gte: 10, lt: 20}"));
        assert!(query.contains("athFdv: {gt: 0}"));
        assert!(query.contains("offset: 200"));
    }

    #[test]
    fn runner_index_query_is_all_time_and_exact_cents_filter_is_enforced() {
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let query = runner_index_query(8453, filter, 400, 200);
        assert!(query.contains("network: [8453]"));
        assert!(query.contains("isTestnet: false"));
        assert!(query.contains("includeScams: true"));
        assert!(query.contains("athFdv: {gte: 500000, lte: 10000000000}"));
        assert!(query.contains("holders: {gte: 100}"));
        assert!(query.contains("rankings: [{attribute: createdAt, direction: ASC}]"));
        assert!(!query.contains("createdAt: {gte:"));
        assert!(query.contains("token { address networkId createdAt extrema"));

        let row = |address: &str, ath: &str, holders: Value| {
            json!({
                "createdAt": 1_690_997_881,
                "token": {"address": address, "networkId": BASE_INDEX_NETWORK, "createdAt": 1_690_997_881,
                    "extrema": {"athFdv": ath, "athFdvTimestamp": 1_778_068_501}},
                "holders": holders,
                "liquidity": "19626"
            })
        };
        let page = |row| json!({"data":{"filterTokens":{"results":[row]}}});
        let parse = |row| parse_runner_index_page(&page(row), 1_800, 5, filter, 8453, "base");
        for ath in ["500000", "10000000000"] {
            let parsed = parse(row(&token_address(1), ath, json!(100))).unwrap();
            assert_eq!(parsed.rows[0].holders, Some(100));
        }
        assert!(parse(row(&token_address(1), "500000.001", json!(100))).is_ok());
        assert!(parse(row(&token_address(1), "499999.999", json!(100))).is_err());
        assert!(parse(row(&token_address(1), "10000000000.001", json!(100))).is_err());
        assert!(parse(row(&token_address(1), "500000", Value::Null)).is_err());

        let floor = RunnerAthIndexFilter {
            min_token_created_at: Some(1_700_000_000),
            ..filter
        };
        let floor_query = runner_index_query(8453, floor, 0, 200);
        assert!(floor_query.contains("tokenCreatedAt: {gte: 1700000000}"));
        assert!(!floor_query.contains("createdAt: {gte:"));
        let floor_row = |created_at: Value| {
            json!({
                "token": {"address": token_address(2), "networkId": BASE_INDEX_NETWORK,
                    "createdAt": created_at,
                    "extrema": {"athFdv": "500000", "athFdvTimestamp": 1_778_068_501}},
                "holders": 100
            })
        };
        let parsed = parse_runner_index_page(
            &page(floor_row(json!(1_700_000_000))),
            1_800_000_000,
            5,
            floor,
            8453,
            "base",
        )
        .unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.token_created_at_gap_rows, 0);
        for bad_created_at in [
            json!(1_699_999_999),
            json!(-1),
            json!(1_800_000_001),
            json!(1.5),
            Value::Null,
        ] {
            let parsed = parse_runner_index_page(
                &page(floor_row(bad_created_at)),
                1_800_000_000,
                5,
                floor,
                8453,
                "base",
            )
            .unwrap();
            assert_eq!(parsed.rows.len(), 1, "valid ATH is retained");
            assert_eq!(parsed.token_created_at_gap_rows, 1);
        }
    }

    #[test]
    fn runner_index_parser_rejects_wrong_network_addresses_and_invalid_timestamps() {
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let valid = json!({
            "createdAt": 1_690_997_881,
            "token": {"address": token_address(1), "networkId": BASE_INDEX_NETWORK, "createdAt": 1_690_997_881,
                "extrema": {"athFdv": "500000", "athFdvTimestamp": 1_778_068_501}},
            "holders": 100,
            "liquidity": "1"
        });
        let parse = |row: Value| {
            parse_runner_index_page(
                &json!({"data":{"filterTokens":{"results":[row]}}}),
                1_800,
                5,
                filter,
                8453,
                "base",
            )
        };
        let mut row = valid.clone();
        row["token"]["networkId"] = json!(56);
        assert!(matches!(
            parse(row),
            Err(AthError::WrongRunnerIndexNetwork {
                actual: 56,
                expected: 8453
            })
        ));
        let mut row = valid.clone();
        row["token"]["address"] = json!("bad-address");
        assert!(matches!(parse(row), Err(AthError::MalformedIndex(_))));
        let mut row = valid.clone();
        row["token"]["createdAt"] = json!(1.5);
        let parsed = parse(row).unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.token_created_at_gap_rows, 1);
        let mut row = valid.clone();
        row["token"]["extrema"]["athFdvTimestamp"] = json!(-1);
        assert!(matches!(parse(row), Err(AthError::MalformedIndex(_))));
        let mut row = valid.clone();
        row["holders"] = json!(100.5);
        assert!(matches!(parse(row), Err(AthError::MalformedIndex(_))));
    }

    #[test]
    fn solana_runner_index_parser_preserves_and_validates_base58_mints() {
        assert!(!valid_runner_token_address("solana", &"1".repeat(45)));
        assert!(!valid_runner_token_address("solana", &"1".repeat(31)));
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_solana_runner_filter_mock.json"
        ))
        .unwrap();
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let parsed =
            parse_runner_index_page(&fixture, 1_800, 200, filter, 1_399_811_149, "solana").unwrap();
        assert_eq!(parsed.rows[0].chain, "solana");
        assert_eq!(
            parsed.rows[0].token,
            "So11111111111111111111111111111111111111112"
        );

        let mut invalid_mint = fixture.clone();
        invalid_mint["data"]["filterTokens"]["results"][0]["token"]["address"] =
            json!("0x1111111111111111111111111111111111111111");
        assert!(matches!(
            parse_runner_index_page(&invalid_mint, 1_800, 200, filter, 1_399_811_149, "solana"),
            Err(AthError::MalformedIndex(_))
        ));
        let mut wrong_network = fixture;
        wrong_network["data"]["filterTokens"]["results"][0]["token"]["networkId"] = json!(8453);
        assert!(matches!(
            parse_runner_index_page(&wrong_network, 1_800, 200, filter, 1_399_811_149, "solana"),
            Err(AthError::WrongRunnerIndexNetwork {
                actual: 8453,
                expected: 1_399_811_149
            })
        ));
    }

    #[tokio::test]
    async fn runner_index_resumes_by_provider_rows_and_scopes_cursor_to_filter() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_base_runner_filter_2026-10-11.json"
        ))
        .unwrap();
        let rows = fixture
            .pointer("/data/filterTokens/results")
            .unwrap()
            .clone();
        let first_token = rows[0]["token"]["address"].as_str().unwrap().to_string();
        let pid = std::process::id();
        db.insert_launches(&[Launch {
            chain: "base".into(),
            token: first_token.clone(),
            launchpad: "zora".into(),
            creator: format!("runner-index-{pid}"),
            created_block: 1,
            created_at: 1,
            tx_hash: format!("runner-index-{pid}"),
            source: "test".into(),
        }])
        .await
        .unwrap();
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let config = IndexConfig {
            page_size: 2,
            max_offset: 10,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let prefix = format!("runner-index-cursor:{pid}");
        let budget = format!("runner-index-budget:{pid}");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: rows.clone(),
                calls: calls.clone(),
            })
            .mount(&server)
            .await;
        let first = backfill_base_runner_ath_from_index_at(
            &db,
            "fixture-key",
            filter,
            1,
            1_800_000_000,
            &server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(!first.complete);
        assert_eq!(first.provider_rows, 2);
        assert_eq!(first.tracked_rows, 1);
        assert_eq!(first.next_offset, 2);

        let resumed = backfill_base_runner_ath_from_index_at(
            &db,
            "fixture-key",
            filter,
            10,
            1_800_000_001,
            &server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed.provider_rows, 3);
        assert_eq!(resumed.tracked_rows, 0);
        assert_eq!(resumed.next_offset, 4);
        {
            let queries = calls.lock().unwrap();
            assert_eq!(queries.len(), 3);
            assert!(queries[0].contains("offset: 0"));
            assert!(queries[1].contains("offset: 2"));
            assert!(queries[2].contains("offset: 4"));
        }

        let complete_server = MockServer::start().await;
        let complete = backfill_base_runner_ath_from_index_at(
            &db,
            "fixture-key",
            filter,
            10,
            1_800_000_002,
            &complete_server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(complete.complete);
        assert_eq!(complete.requests, 0);
        assert!(
            complete_server
                .received_requests()
                .await
                .unwrap()
                .is_empty()
        );

        let different_filter = RunnerAthIndexFilter {
            min_ath_usd: 1_000_000,
            ..filter
        };
        let isolated_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": []}}
            })))
            .mount(&isolated_server)
            .await;
        let isolated = backfill_base_runner_ath_from_index_at(
            &db,
            "fixture-key",
            different_filter,
            1,
            1_800_000_003,
            &isolated_server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(isolated.complete);
        assert_eq!(isolated.requests, 1);
        let query = isolated_server.received_requests().await.unwrap();
        let request: Value = serde_json::from_slice(&query[0].body).unwrap();
        assert!(request["query"].as_str().unwrap().contains("gte: 1000000"));

        let floor_filter = RunnerAthIndexFilter {
            min_token_created_at: Some(1_760_054_400),
            ..filter
        };
        let floor_server = MockServer::start().await;
        let floor_calls = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: json!([{
                    "token": {"address": token_address(42), "networkId": BASE_INDEX_NETWORK,
                        "createdAt": 1_760_000_000,
                        "extrema": {"athFdv": "500000", "athFdvTimestamp": 1_778_068_501}},
                    "holders": 100
                }]),
                calls: floor_calls.clone(),
            })
            .mount(&floor_server)
            .await;
        let floor_report = backfill_base_runner_ath_from_index_at(
            &db,
            "fixture-key",
            floor_filter,
            1,
            1_800_000_004,
            &floor_server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(floor_report.complete);
        assert_eq!(floor_report.provider_rows, 1);
        assert_eq!(floor_report.token_created_at_gap_rows, 1);
        assert_eq!(floor_calls.lock().unwrap().len(), 1);
        assert!(floor_calls.lock().unwrap()[0].contains("tokenCreatedAt: {gte: 1760054400}"));
        assert!(floor_calls.lock().unwrap()[0].contains("token { address networkId createdAt"));
        assert!(floor_calls.lock().unwrap()[0].contains("offset: 0"));
        let floor_key = format!(
            "{prefix}:base-runner-ath-holders-v1:500000:10000000000:100:token-created-since-1760054400"
        );
        let saved: RunnerIndexCursor =
            serde_json::from_str(&db.cursor(&floor_key).await.unwrap().unwrap()).unwrap();
        assert!(saved.complete);
        assert_eq!(saved.token_created_at_gap_rows, 1);
        assert_eq!(complete.token_created_at_gap_rows, 5);

        let invalid_floor = RunnerAthIndexFilter {
            min_token_created_at: Some(-1),
            ..filter
        };
        assert!(matches!(
            backfill_base_runner_ath_from_index_at(
                &db,
                "fixture-key",
                invalid_floor,
                1,
                1_800_000_005,
                &floor_server.uri(),
                config,
                &prefix,
                &budget,
            )
            .await,
            Err(AthError::InvalidRunnerIndexCursor)
        ));
    }

    #[tokio::test]
    async fn runner_index_keeps_same_address_and_cursor_isolated_across_evm_chains() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let token = token_address(7_654_321);
        let pid = std::process::id();
        for (chain, suffix) in [("bsc", "bsc"), ("robinhood", "rh")] {
            db.insert_launches(&[Launch {
                chain: chain.into(),
                token: token.clone(),
                launchpad: "test".into(),
                creator: format!("runner-network-{pid}-{suffix}"),
                created_block: 1,
                created_at: 1,
                tx_hash: format!("runner-network-{pid}-{suffix}"),
                source: "test".into(),
            }])
            .await
            .unwrap();
        }
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let config = IndexConfig {
            page_size: 2,
            max_offset: 10,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let prefix = format!("runner-network-cursor:{pid}");
        let budget = format!("runner-network-budget:{pid}");
        let row = |network: u64, ath: &str| {
            json!({
                "createdAt": 1_690_997_881,
                "token": {"address": token, "networkId": network,
                    "extrema": {"athFdv": ath, "athFdvTimestamp": 1_778_068_501}},
                "holders": 100,
                "liquidity": "1000"
            })
        };

        let wrong_server = MockServer::start().await;
        let wrong_calls = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: json!([row(8453, "1500000")]),
                calls: wrong_calls,
            })
            .mount(&wrong_server)
            .await;
        assert!(matches!(
            backfill_runner_ath_from_index_at(
                &db,
                "fixture-key",
                "bsc",
                56,
                filter,
                1,
                1_800_000_000,
                &wrong_server.uri(),
                config,
                &prefix,
                &budget,
            )
            .await,
            Err(AthError::WrongRunnerIndexNetwork {
                actual: 8453,
                expected: 56
            })
        ));
        let bsc_cursor = format!("{prefix}:bsc-56-runner-ath-holders-v1:500000:10000000000:100");
        assert!(db.cursor(&bsc_cursor).await.unwrap().is_none());

        let bsc_server = MockServer::start().await;
        let bsc_calls = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: json!([row(56, "1500000")]),
                calls: bsc_calls.clone(),
            })
            .mount(&bsc_server)
            .await;
        let bsc = backfill_runner_ath_from_index_at(
            &db,
            "fixture-key",
            "bsc",
            56,
            filter,
            2,
            1_800_000_001,
            &bsc_server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(bsc.complete);
        assert_eq!((bsc.chain.as_str(), bsc.network_id), ("bsc", 56));
        assert!(bsc_calls.lock().unwrap()[0].contains("network: [56]"));

        let rh_server = MockServer::start().await;
        let rh_calls = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: json!([row(4663, "2500000")]),
                calls: rh_calls.clone(),
            })
            .mount(&rh_server)
            .await;
        let rh = backfill_runner_ath_from_index_at(
            &db,
            "fixture-key",
            "robinhood",
            4663,
            filter,
            2,
            1_800_000_002,
            &rh_server.uri(),
            config,
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(rh.complete);
        assert_eq!((rh.chain.as_str(), rh.network_id), ("robinhood", 4663));
        assert!(rh_calls.lock().unwrap()[0].contains("network: [4663]"));

        let db_pool = sqlx::PgPool::connect(&std::env::var("SCOUT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let ath_rows = sqlx::query(
            "SELECT chain, token, ath_fdv_cents FROM ath WHERE token = $1 AND chain = ANY($2)",
        )
        .bind(&token)
        .bind(vec!["bsc", "robinhood"])
        .fetch_all(&db_pool)
        .await
        .unwrap();
        assert_eq!(ath_rows.len(), 2);
        let values: std::collections::BTreeMap<String, i64> = ath_rows
            .into_iter()
            .map(|row| (row.get("chain"), row.get("ath_fdv_cents")))
            .collect();
        assert_eq!(values.get("bsc"), Some(&150_000_000));
        assert_eq!(values.get("robinhood"), Some(&250_000_000));
        let rh_cursor =
            format!("{prefix}:robinhood-4663-runner-ath-holders-v1:500000:10000000000:100");
        assert!(db.cursor(&rh_cursor).await.unwrap().is_some());
        assert_ne!(bsc_cursor, rh_cursor);
    }

    #[tokio::test]
    async fn solana_runner_index_db_mock_preserves_mint_case_in_lookup_and_storage() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_solana_runner_filter_mock.json"
        ))
        .unwrap();
        let token = fixture["data"]["filterTokens"]["results"][0]["token"]["address"]
            .as_str()
            .unwrap()
            .to_string();
        db.insert_launches(&[Launch {
            chain: "solana".into(),
            token: token.clone(),
            launchpad: "test".into(),
            creator: format!("runner-solana-{}", std::process::id()),
            created_block: 1,
            created_at: 1,
            tx_hash: format!("runner-solana-{}", std::process::id()),
            source: "test".into(),
        }])
        .await
        .unwrap();
        let results = fixture["data"]["filterTokens"]["results"].clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows: results,
                calls: calls.clone(),
            })
            .mount(&server)
            .await;
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let prefix = format!("runner-solana-cursor:{}", std::process::id());
        let budget = format!("runner-solana-budget:{}", std::process::id());
        let report = backfill_runner_ath_from_index_at(
            &db,
            "fixture-key",
            "solana",
            1_399_811_149,
            filter,
            2,
            1_800_000_000,
            &server.uri(),
            IndexConfig {
                page_size: 2,
                max_offset: 10,
                max_body: BASE_INDEX_MAX_BODY,
                minimum_interval: Duration::ZERO,
            },
            &prefix,
            &budget,
        )
        .await
        .unwrap();
        assert!(report.complete);
        assert_eq!(report.chain, "solana");
        assert_eq!(report.network_id, 1_399_811_149);
        assert_eq!(report.tracked_rows, 1);
        assert!(calls.lock().unwrap()[0].contains("network: [1399811149]"));
        let db_pool = sqlx::PgPool::connect(&std::env::var("SCOUT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let stored = sqlx::query(
            "SELECT token, ath_fdv_cents FROM ath WHERE chain = 'solana' AND token = $1",
        )
        .bind(&token)
        .fetch_one(&db_pool)
        .await
        .unwrap();
        assert_eq!(stored.get::<String, _>("token"), token);
        assert_eq!(stored.get::<i64, _>("ath_fdv_cents"), 138_171_100);
        assert_ne!(token.to_ascii_lowercase(), token);
    }

    #[tokio::test]
    async fn runner_index_errors_do_not_advance_and_offset_cap_is_not_complete() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let filter = RunnerAthIndexFilter {
            min_ath_usd: 500_000,
            max_ath_usd: 10_000_000_000,
            min_holders: 100,
            min_token_created_at: None,
        };
        let config = IndexConfig {
            page_size: 1,
            max_offset: 2,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let pid = std::process::id();
        let error_prefix = format!("runner-index-malformed:{pid}");
        let malformed_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"filterTokens": {"results": [{
                    "createdAt": 1,
                    "token": {"address": token_address(90), "networkId": BASE_INDEX_NETWORK,
                        "extrema": {"athFdv": "499999.99"}},
                    "holders": 100
                }]}}
            })))
            .mount(&malformed_server)
            .await;
        assert!(matches!(
            backfill_base_runner_ath_from_index_at(
                &db,
                "fixture-key",
                filter,
                1,
                1_800_000_000,
                &malformed_server.uri(),
                config,
                &error_prefix,
                &format!("runner-index-malformed-budget:{pid}"),
            )
            .await,
            Err(AthError::MalformedIndex(_))
        ));
        let key = format!("{error_prefix}:base-runner-ath-holders-v1:500000:10000000000:100");
        assert!(db.cursor(&key).await.unwrap().is_none());

        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_base_runner_filter_2026-10-11.json"
        ))
        .unwrap();
        let rows = fixture
            .pointer("/data/filterTokens/results")
            .unwrap()
            .clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let capped_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RunnerIndexPages {
                rows,
                calls: calls.clone(),
            })
            .mount(&capped_server)
            .await;
        let cap_prefix = format!("runner-index-cap:{pid}");
        let cap_budget = format!("runner-index-cap-budget:{pid}");
        assert!(matches!(
            backfill_base_runner_ath_from_index_at(
                &db,
                "fixture-key",
                filter,
                10,
                1_800_000_001,
                &capped_server.uri(),
                config,
                &cap_prefix,
                &cap_budget,
            )
            .await,
            Err(AthError::RunnerIndexSaturated { max_results: 2 })
        ));
        assert_eq!(calls.lock().unwrap().len(), 2);
        let cap_key = format!("{cap_prefix}:base-runner-ath-holders-v1:500000:10000000000:100");
        let saved: RunnerIndexCursor =
            serde_json::from_str(&db.cursor(&cap_key).await.unwrap().unwrap()).unwrap();
        assert_eq!(saved.offset, 2);
        assert!(!saved.complete);
        let mut invalid_complete = saved.clone();
        invalid_complete.complete = true;
        assert!(
            validate_runner_index_cursor(
                &invalid_complete,
                &saved.filter,
                "base",
                BASE_INDEX_NETWORK,
                config
            )
            .is_err()
        );
        let empty_server = MockServer::start().await;
        assert!(matches!(
            backfill_base_runner_ath_from_index_at(
                &db,
                "fixture-key",
                filter,
                10,
                1_800_000_002,
                &empty_server.uri(),
                config,
                &cap_prefix,
                &cap_budget,
            )
            .await,
            Err(AthError::RunnerIndexSaturated { max_results: 2 })
        ));
        assert!(empty_server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn indexed_results_require_base_valid_positive_ath() {
        let interval = IndexInterval {
            start: 10,
            end: 20,
            offset: 0,
        };
        let row = |network, address: &str, ath: &str| {
            json!({
                "createdAt": 11,
                "token": {"address": address, "networkId": network,
                    "extrema": {"athFdv": ath}},
                "holders": null,
                "liquidity": "0"
            })
        };
        assert!(matches!(
            parse_index_page(
                &json!({"data":{"filterTokens":{"results":[row(56, &token_address(1), "1")]}}}),
                &interval,
                100,
                200,
            ),
            Err(AthError::WrongIndexNetwork { actual: 56 })
        ));
        assert!(
            parse_index_page(
                &json!({"data":{"filterTokens":{"results":[row(8453, "not-an-address", "1")]}}}),
                &interval,
                100,
                200,
            )
            .is_err()
        );
        assert!(
            parse_index_page(
                &json!({"data":{"filterTokens":{"results":[row(8453, &token_address(1), "0")]}}}),
                &interval,
                100,
                200,
            )
            .is_err()
        );
        let parsed = parse_index_page(
            &json!({
                "data":{"filterTokens":{"results":[row(8453, &token_address(1), "1")]}}
            }),
            &interval,
            100,
            200,
        )
        .unwrap();
        assert_eq!(parsed.rows[0].observed_at, 100);
        assert_eq!(parsed.rows[0].holders, None);
        let malformed = json!({
            "createdAt": 11,
            "token": {"address": token_address(1), "networkId": BASE_INDEX_NETWORK,
                "extrema": {"athFdv": "1", "athFdvTimestamp": -1}},
            "holders": null,
            "liquidity": "0"
        });
        assert!(
            parse_index_page(
                &json!({"data":{"filterTokens":{"results":[malformed]}}}),
                &interval,
                100,
                200,
            )
            .is_err()
        );

        for created_at in [json!(-1), json!(11.5)] {
            let malformed_time = json!({
                "createdAt": created_at,
                "token": {"address": token_address(1), "networkId": BASE_INDEX_NETWORK,
                    "extrema": {"athFdv": "1"}}
            });
            assert!(matches!(
                parse_index_page(
                    &json!({"data":{"filterTokens":{"results":[malformed_time]}}}),
                    &interval,
                    100,
                    200,
                ),
                Err(AthError::MalformedIndex(_))
            ));
        }
    }

    #[test]
    fn date_mismatch_examples_are_bounded_and_old_cursors_default_them() {
        let interval = IndexInterval {
            start: 100,
            end: 200,
            offset: 0,
        };
        let results: Vec<Value> = (0..6)
            .map(|index| {
                json!({
                    "createdAt": index + 10,
                    "token": {"address": token_address(index + 1),
                        "networkId": BASE_INDEX_NETWORK,
                        "extrema": {"athFdv": "1"}}
                })
            })
            .collect();
        let page = parse_index_page(
            &json!({"data":{"filterTokens":{"results":results}}}),
            &interval,
            300,
            10,
        )
        .unwrap();
        assert_eq!(page.date_mismatch_rows, 6);
        assert_eq!(
            page.date_mismatch_examples.len(),
            MAX_DATE_MISMATCH_EXAMPLES
        );
        assert!(
            page.rows
                .iter()
                .all(|row| row.source == "codex-index-date-mismatch")
        );

        let old_cursor: IndexCursor = serde_json::from_value(json!({
            "version": 1,
            "filter": BASE_INDEX_FILTER,
            "day_start": 0,
            "day_end": UTC_DAY,
            "intervals": [{"start": 0, "end": UTC_DAY, "offset": 0}],
            "complete": false
        }))
        .unwrap();
        assert_eq!(old_cursor.date_mismatch_rows, 0);
        assert!(old_cursor.date_mismatch_examples.is_empty());
    }

    #[test]
    fn invalid_date_mismatch_cursor_examples_are_rejected() {
        let config = IndexConfig {
            page_size: 1,
            max_offset: 10,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let mut cursor = IndexCursor {
            version: 1,
            filter: BASE_INDEX_FILTER.into(),
            day_start: 0,
            day_end: UTC_DAY,
            intervals: vec![IndexInterval {
                start: 0,
                end: UTC_DAY,
                offset: 0,
            }],
            complete: false,
            date_mismatch_rows: 1,
            date_mismatch_examples: vec![DateMismatchExample {
                token: token_address(1),
                created_at: 50,
                interval_start: 100,
                interval_end: 200,
            }],
        };
        assert!(validate_index_cursor(&cursor, 0, config).is_ok());

        cursor.date_mismatch_examples[0].token = "not-an-address".into();
        assert!(matches!(
            validate_index_cursor(&cursor, 0, config),
            Err(AthError::InvalidIndexCursor)
        ));
        cursor.date_mismatch_examples[0].token = token_address(1);
        cursor.date_mismatch_examples[0].created_at = 150;
        assert!(matches!(
            validate_index_cursor(&cursor, 0, config),
            Err(AthError::InvalidIndexCursor)
        ));
    }

    #[test]
    fn retry_after_is_bounded_and_falls_back_for_invalid_headers() {
        assert_eq!(retry_delay(Some(0), 0).unwrap(), Duration::ZERO);
        assert_eq!(retry_delay(None, 1).unwrap(), Duration::from_secs(2));
        assert!(matches!(
            retry_delay(Some(31), 0),
            Err(AthError::RetryLater(31))
        ));
    }

    #[tokio::test]
    async fn indexed_backfill_resumes_splits_cap_and_only_upserts_tracked_tokens() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let pid = u64::from(std::process::id());
        let tracked = token_address(pid * 2 + 1);
        let untracked = token_address(pid * 2 + 2);
        db.insert_launches(&[Launch {
            chain: "base".into(),
            token: tracked.clone(),
            launchpad: "zora".into(),
            creator: format!("ath-index-{pid}"),
            created_block: 1,
            created_at: 1,
            tx_hash: "ath-index-test".into(),
            source: "test".into(),
        }])
        .await
        .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(IndexReplay {
                calls: calls.clone(),
                first: tracked.clone(),
                second: untracked.clone(),
            })
            .mount(&server)
            .await;
        let config = IndexConfig {
            page_size: 1,
            max_offset: 2,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let now = 1_800_000_123;
        let cursor = format!("ath-index-test:{pid}");
        let budget = format!("ath-index-budget:{pid}");
        let first = backfill_base_ath_from_index_at(
            &db,
            "test-key",
            1,
            1,
            now,
            &server.uri(),
            config,
            &cursor,
            &budget,
        )
        .await
        .unwrap();
        assert!(!first.complete);
        assert_eq!(first.requests, 1);

        let second = backfill_base_ath_from_index_at(
            &db,
            "test-key",
            1,
            3,
            now,
            &server.uri(),
            config,
            &cursor,
            &budget,
        )
        .await
        .unwrap();
        assert!(second.complete);
        assert_eq!(second.requests, 3);
        {
            let queries = calls.lock().unwrap();
            assert_eq!(queries.len(), 4);
            assert!(queries[0].contains("offset: 0"));
            assert!(queries[1].contains("offset: 1"));
            assert!(queries[2].contains("offset: 0"));
            assert!(queries[3].contains("offset: 0"));
            assert!(queries[2].contains("lt: "));
            assert!(queries[3].contains("gte: "));
        }

        let rows = db.dev_launches(Some("base"), 0).await.unwrap();
        let stored = rows.iter().find(|row| row.token == tracked).unwrap();
        assert_eq!(stored.ath_fdv_cents, Some(123));
        assert_eq!(stored.ath_holders, Some(17));
        assert_eq!(stored.ath_max_liquidity_cents, Some(250));
        assert_eq!(rows.iter().filter(|row| row.token == untracked).count(), 0);
        assert_eq!(second.window_end, now.div_euclid(UTC_DAY) * UTC_DAY);
    }

    #[tokio::test]
    async fn out_of_interval_integer_created_at_is_observed_and_checkpointed() {
        let Ok(url) = std::env::var("SCOUT_TEST_DATABASE_URL") else {
            return;
        };
        assert!(url.split('?').next().unwrap().ends_with("/scout_test"));
        let db = DevDb::connect(&url, 2).await.unwrap();
        db.migrate().await.unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex_base_created_at_mismatch_2026-10-11.json"
        ))
        .unwrap();
        let good = fixture["results"][0].clone();
        let bad = fixture["results"][1].clone();
        let tracked = bad["token"]["address"].as_str().unwrap().to_string();
        let db_pool = sqlx::PgPool::connect(&url).await.unwrap();
        db.insert_launches(&[Launch {
            chain: "base".into(),
            token: tracked.clone(),
            launchpad: "zora".into(),
            creator: format!("ath-date-mismatch-{}", std::process::id()),
            created_block: 1,
            created_at: 1_775_701_583,
            tx_hash: "ath-date-mismatch-test".into(),
            source: "test".into(),
        }])
        .await
        .unwrap();

        let offsets = Arc::new(Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(DateMismatchPages {
                rows: json!([good, bad]),
                offsets: offsets.clone(),
            })
            .mount(&server)
            .await;
        let config = IndexConfig {
            page_size: 2,
            max_offset: 10,
            max_body: BASE_INDEX_MAX_BODY,
            minimum_interval: Duration::ZERO,
        };
        let now = 1_791_158_401;
        let suffix = std::process::id();
        let cursor = format!("ath-date-mismatch:{suffix}");
        let budget = format!("ath-date-mismatch-budget:{suffix}");
        let report = backfill_base_ath_from_index_at(
            &db,
            "test-key",
            1,
            10,
            now,
            &server.uri(),
            config,
            &cursor,
            &budget,
        )
        .await
        .unwrap();

        assert!(report.complete);
        assert_eq!(report.provider_rows, 2);
        assert_eq!(report.tracked_rows, 1);
        assert_eq!(report.date_mismatch_rows, 1);
        assert_eq!(*offsets.lock().unwrap(), [0, 2]);
        let stored = sqlx::query(
            "SELECT ath_fdv_cents, ath_at, source FROM ath WHERE chain = 'base' AND token = $1",
        )
        .bind(&tracked)
        .fetch_one(&db_pool)
        .await
        .unwrap();
        assert_eq!(stored.get::<i64, _>("ath_fdv_cents"), 165_600);
        assert_eq!(stored.get::<Option<i64>, _>("ath_at"), Some(1_791_636_057));
        assert_eq!(
            stored.get::<String, _>("source"),
            "codex-index-date-mismatch"
        );
        let cursor_key = format!("{cursor}:1791072000");
        let saved: IndexCursor =
            serde_json::from_str(&db.cursor(&cursor_key).await.unwrap().unwrap()).unwrap();
        assert!(saved.complete);
        assert_eq!(saved.date_mismatch_rows, 1);
        assert_eq!(
            saved.date_mismatch_examples,
            [DateMismatchExample {
                token: tracked.to_ascii_lowercase(),
                created_at: 1_775_701_583,
                interval_start: 1_791_072_000,
                interval_end: 1_791_158_400,
            }]
        );

        let resume_server = MockServer::start().await;
        let resumed = backfill_base_ath_from_index_at(
            &db,
            "test-key",
            1,
            10,
            now,
            &resume_server.uri(),
            config,
            &cursor,
            &budget,
        )
        .await
        .unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed.requests, 0);
        assert_eq!(resumed.date_mismatch_rows, 1);
        assert!(resume_server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn zero_shared_budget_stops_before_http() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let key = format!("ath-index-zero-budget:{}", std::process::id());
        db.set_cursor(&key, "0", 10).await.unwrap();
        let server = MockServer::start().await;
        let mut attempts = 0;
        let mut last_request = None;
        let result = request_graphql(
            &db,
            &reqwest::Client::new(),
            "test-key",
            &server.uri(),
            "query { test }",
            10,
            128,
            0,
            Duration::ZERO,
            None,
            &key,
            &mut attempts,
            &mut last_request,
        )
        .await;
        assert!(matches!(result, Err(AthError::QuotaExhausted)));
        assert_eq!(attempts, 0);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn retry_attempts_consume_shared_and_per_pass_budgets() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let key = format!("ath-retry-budget:{}", std::process::id());
        db.set_cursor(&key, "2", 10).await.unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(RetryOnce(AtomicUsize::new(0)))
            .mount(&server)
            .await;
        let mut attempts = 0;
        let mut last_request = None;
        assert!(
            request_graphql(
                &db,
                &reqwest::Client::new(),
                "test-key",
                &server.uri(),
                "query { test }",
                10,
                128,
                1,
                Duration::ZERO,
                Some(2),
                &key,
                &mut attempts,
                &mut last_request,
            )
            .await
            .is_ok()
        );
        assert_eq!(attempts, 2);
        assert_eq!(db.cursor(&key).await.unwrap().as_deref(), Some("0"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        let capped_key = format!("ath-pass-budget:{}", std::process::id());
        db.set_cursor(&capped_key, "2", 10).await.unwrap();
        let capped = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
            .mount(&capped)
            .await;
        let mut attempts = 0;
        assert!(matches!(
            request_graphql(
                &db,
                &reqwest::Client::new(),
                "test-key",
                &capped.uri(),
                "query { test }",
                10,
                128,
                2,
                Duration::ZERO,
                Some(1),
                &capped_key,
                &mut attempts,
                &mut last_request,
            )
            .await,
            Err(AthError::PassBudgetExhausted(1))
        ));
        assert_eq!(attempts, 1);
        assert_eq!(db.cursor(&capped_key).await.unwrap().as_deref(), Some("1"));
        assert_eq!(capped.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn oversized_response_does_not_advance_cursor_and_second_saturation_fails() {
        let Some(db) = isolated_db().await else {
            return;
        };
        let now = 1_800_000_123;
        let prefix = format!("ath-response-cap:{}", std::process::id());
        let budget = format!("ath-response-budget:{}", std::process::id());
        let config = IndexConfig {
            page_size: 1,
            max_offset: 1,
            max_body: 1,
            minimum_interval: Duration::ZERO,
        };
        let oversized = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&oversized)
            .await;
        assert!(matches!(
            backfill_base_ath_from_index_at(
                &db,
                "test-key",
                1,
                1,
                now,
                &oversized.uri(),
                config,
                &prefix,
                &budget
            )
            .await,
            Err(AthError::ResponseTooLarge(1))
        ));
        let day = now.div_euclid(UTC_DAY) * UTC_DAY - UTC_DAY;
        assert!(
            db.cursor(&format!("{prefix}:{day}"))
                .await
                .unwrap()
                .is_none()
        );

        let saturated = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(AlwaysCapped(token_address(88)))
            .mount(&saturated)
            .await;
        let mut config = config;
        config.max_body = BASE_INDEX_MAX_BODY;
        assert!(matches!(
            backfill_base_ath_from_index_at(
                &db,
                "test-key",
                1,
                30,
                now,
                &saturated.uri(),
                config,
                &prefix,
                &budget
            )
            .await,
            Err(AthError::IndexIntervalSaturated { .. })
        ));
        let stored = db
            .cursor(&format!("{prefix}:{day}"))
            .await
            .unwrap()
            .unwrap();
        let cursor: IndexCursor = serde_json::from_str(&stored).unwrap();
        assert!(!cursor.complete);
        assert!(!cursor.intervals.is_empty());
        assert!(saturated.received_requests().await.unwrap().len() <= 20);
    }
}
