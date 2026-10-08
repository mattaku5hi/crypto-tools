//! ATH observations from Codex (B3; B0.3 measurement).
//!
//! `filterTokens(tokens: ["address:networkId", … ≤ 200])` returns
//! `token.extrema.athFdv` (USD, decimal string) and its timestamp. Codex's
//! free tier is 10,000 requests/month at 5 requests/s and the next plan is a
//! subscription, so only tokens that can be runners are observed: migrated
//! tokens of curve launchpads and every token of launchpads without a curve
//! (Zora, Clanker), refreshed by age (`scout_devdb::DevDb::ath_candidates`).
//! The current holder count is stored with each observation (a runner needs
//! `top_runners.min_holders`: thin pools fake huge ATHs).
//! A token Codex does not return is stored as an observation of 0 with source
//! `codex-missing`, so it is retried on the age schedule, not every pass.

use std::time::Duration;

use scout_devdb::{AthObservation, DevDb, DevDbError};
use serde_json::{Value, json};

/// Codex GraphQL endpoint.
pub const CODEX_URL: &str = "https://graph.codex.io/graphql";
/// Tokens per `filterTokens` request (Codex maximum).
pub const CODEX_BATCH: usize = 200;

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

async fn query(http: &reqwest::Client, key: &str, ids: &[String]) -> Result<Value, AthError> {
    let q = format!(
        "{{ filterTokens(tokens: {}, limit: {}) {{ results {{ holders liquidity token {{ address networkId extrema {{ athFdv athFdvTimestamp }} }} }} }} }}",
        serde_json::to_string(ids).map_err(|e| AthError::Http(e.to_string()))?,
        CODEX_BATCH
    );
    let resp = http
        .post(CODEX_URL)
        .header("Authorization", key)
        .json(&json!({ "query": q }))
        .send()
        .await
        .map_err(|e| AthError::Http(e.without_url().to_string()))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .await
        .map_err(|e| AthError::Http(e.without_url().to_string()))?;
    if !status.is_success() {
        return Err(AthError::Http(format!("HTTP {status}")));
    }
    if let Some(err) = v.get("errors") {
        return Err(AthError::Codex(err.to_string().chars().take(300).collect()));
    }
    Ok(v)
}

/// Observe the ATH of up to `max_requests × 200` candidate tokens of `chain`.
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
        let v = query(&http, codex_key, &ids).await?;
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
                liquidity_cents: r.get("liquidity").and_then(|v| match v {
                    Value::String(s) => usd_to_cents(s),
                    Value::Number(n) => usd_to_cents(&n.to_string()),
                    _ => None,
                }),
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
    use super::*;

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
}
