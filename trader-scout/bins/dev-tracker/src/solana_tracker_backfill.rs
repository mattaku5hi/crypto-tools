//! Explicit, resumable Solana Tracker provider-ATH observations.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Args;
use scout_devdb::{AthPriceObservation, DevDb, DevDbError, ProviderRequestBudget};
use scout_devtracker::solana_tracker_ath::{
    TrackerAthBody, TrackerAthError, fetch_ath_body, http_client, key_fingerprint,
    parse_ath_observation,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SOURCE: &str = "solana-tracker-ath-v1";
const PROVIDER: &str = "solana-tracker-data-api";
const QUOTA_GROUP: &str = "owner-soltracker";
const ENDPOINT: &str = "https://data.solanatracker.io/tokens";
const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOKENS: usize = 2_000;
const MAX_APPLICATION_LIMIT: i64 = 250;

#[derive(Debug, Clone, Args)]
pub struct SolanaTrackerAthArgs {
    /// JSON array of Solana mint strings (at most 2,000 tracked mints).
    #[arg(long)]
    tokens: String,
    /// Directory for bounded raw 2xx provider response bodies (content-addressed files).
    #[arg(long)]
    raw_dir: PathBuf,
    /// Maximum HTTP attempts for this invocation; required and never inferred.
    #[arg(long)]
    max_requests: usize,
    /// Owner-selected month for this application's allocation, YYYY-MM.
    #[arg(long)]
    billing_month: String,
    /// Monthly request allocation for this application (maximum 250).
    #[arg(long)]
    application_monthly_limit: i64,
    /// Owner-confirmed requests already charged to this application this month.
    #[arg(long)]
    prior_application_usage: i64,
}

pub async fn run(db: &DevDb, args: SolanaTrackerAthArgs) -> Result<(), String> {
    let current_month = utc_month(unix_now()?);
    if args.max_requests == 0
        || args.max_requests > MAX_TOKENS
        || args.application_monthly_limit <= 0
        || args.application_monthly_limit > MAX_APPLICATION_LIMIT
        || args.prior_application_usage < 0
        || args.prior_application_usage > args.application_monthly_limit
        || i64::try_from(args.max_requests).unwrap_or(i64::MAX)
            > args.application_monthly_limit - args.prior_application_usage
        || !valid_billing_month(&args.billing_month)
        || args.billing_month != current_month
    {
        return Err(
            "invalid Solana Tracker attempt budget; billing month must be the current UTC month"
                .into(),
        );
    }

    let file = tokio::fs::File::open(&args.tokens)
        .await
        .map_err(|_| "cannot open token file")?;
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "cannot read token file")?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_INPUT_BYTES {
        return Err("token file exceeds 2 MiB".into());
    }
    let tokens: Vec<String> = serde_json::from_slice(&bytes).map_err(|_| "invalid token JSON")?;
    if tokens.is_empty() || tokens.len() > MAX_TOKENS {
        return Err("expected between 1 and 2,000 token strings".into());
    }
    let mut normalized = BTreeSet::new();
    for token in tokens {
        if token.len() > 44
            || !bs58::decode(&token)
                .into_vec()
                .is_ok_and(|decoded| decoded.len() == 32)
        {
            return Err("token input contains an invalid Solana mint".into());
        }
        normalized.insert(token);
    }

    let mut tracked = BTreeSet::new();
    let input: Vec<String> = normalized.iter().cloned().collect();
    for batch in input.chunks(200) {
        tracked.extend(
            db.known_launch_tokens("solana", batch)
                .await
                .map_err(|_| "database token lookup failed")?,
        );
    }
    if tracked != normalized {
        return Err("token input contains a mint outside tracked Solana launches".into());
    }

    let api_key = std::env::var("SCOUT_SOLANA_TRACKER_API_KEY")
        .map_err(|_| "SCOUT_SOLANA_TRACKER_API_KEY is not set")?;
    if api_key.is_empty() {
        return Err("SCOUT_SOLANA_TRACKER_API_KEY is empty".into());
    }
    let fingerprint = key_fingerprint(&api_key);
    let http = http_client().map_err(|_| "cannot build Solana Tracker HTTP client")?;
    let mut attempts = 0usize;
    let mut saved = 0usize;
    let mut cached = 0usize;
    let mut failed = 0usize;
    let mut limited = false;
    let mut last_request: Option<Instant> = None;

    for token in normalized {
        if db
            .has_ath_price_observation("solana", &token, SOURCE)
            .await
            .map_err(|_| "database observation lookup failed")?
        {
            cached += 1;
            continue;
        }
        if attempts >= args.max_requests {
            limited = true;
            break;
        }
        if let Some(last) = last_request {
            let wait = Duration::from_secs(1).saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let now = unix_now()?;
        if let Err(error) = db
            .reserve_provider_request_budget(&ProviderRequestBudget {
                provider: PROVIDER.into(),
                quota_group: QUOTA_GROUP.into(),
                billing_period: args.billing_month.clone(),
                application_limit: args.application_monthly_limit,
                prior_usage: args.prior_application_usage,
                key_fingerprint: fingerprint.clone(),
                now,
            })
            .await
        {
            if matches!(error, DevDbError::ProviderRequestBudgetExhausted) {
                limited = true;
                break;
            }
            return Err(match error {
                DevDbError::ProviderRequestBudgetConfigMismatch => {
                    "application quota baseline differs from the persisted month; use the saved values".into()
                }
                DevDbError::InvalidProviderRequestBudget => {
                    "invalid application quota baseline".into()
                }
                _ => "cannot reserve Solana Tracker application request".into(),
            });
        }
        attempts += 1;
        last_request = Some(Instant::now());
        let request_started_at = unix_now()?;
        let result = fetch_ath_body(&http, ENDPOINT, &api_key, &token).await;
        let observed_at = unix_now()?;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                failed += 1;
                eprintln!("dev-tracker: Solana Tracker {token}: {error}");
                if matches!(
                    error,
                    TrackerAthError::AccessDenied(401..=403) | TrackerAthError::RateLimited(429)
                ) {
                    return Err(format!(
                        "Solana Tracker stopped after {attempts} attempts; saved {saved} observations"
                    ));
                }
                continue;
            }
        };
        if response
            .bytes
            .windows(api_key.len())
            .any(|window| window == api_key.as_bytes())
        {
            failed += 1;
            eprintln!(
                "dev-tracker: Solana Tracker response for {token} matched secret scan; raw body not saved"
            );
            continue;
        }
        let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        let response_hash = format!("{:x}", Sha256::digest(&response.bytes));
        let raw_path = args
            .raw_dir
            .join(format!("{token_hash}-{response_hash}.json"));
        archive_response(&args.raw_dir, &raw_path, &response)
            .await
            .map_err(|_| "cannot safely archive raw provider response")?;
        let observation = match parse_ath_observation(&token, &response.bytes) {
            Ok(row) => row,
            Err(error) => {
                failed += 1;
                eprintln!(
                    "dev-tracker: Solana Tracker HTTP {} response for {token} at {request_started_at} archived at {} (SHA-256 {response_hash}) but failed validation: {error}",
                    response.status,
                    raw_path.display()
                );
                continue;
            }
        };
        let row = AthPriceObservation {
            chain: "solana".into(),
            token: observation.token,
            source: SOURCE.into(),
            highest_price_usd: observation.highest_price_usd,
            timestamp_ms: observation.timestamp_ms,
            pool_id: observation.pool_id,
            highest_market_cap_usd: observation.highest_market_cap_usd,
            observed_at,
            response_sha256: observation.response_sha256,
            scope: "provider-recording-window-unknown".into(),
        };
        db.upsert_ath_price_observation(&row)
            .await
            .map_err(|_| "cannot save Solana Tracker observation")?;
        saved += 1;
        println!(
            "{}",
            serde_json::json!({
                "chain": row.chain,
                "token": row.token,
                "source": row.source,
                "highest_price_usd": row.highest_price_usd,
                "timestamp_ms": row.timestamp_ms,
                "pool_id": row.pool_id,
                "highest_market_cap_usd": row.highest_market_cap_usd,
                "observed_at": row.observed_at,
                "response_sha256": row.response_sha256,
                "raw_response_path": raw_path.display().to_string(),
                "http_status": response.status,
                "request_started_at": request_started_at,
                "scope": row.scope,
                "quality": "provider-recording-window-unknown/not-ATH-FDV"
            })
        );
    }

    eprintln!(
        "dev-tracker: Solana Tracker {attempts} attempts, {saved} saved, {cached} cached, {failed} failed; application allocation only, ATH FDV unchanged"
    );
    if failed > 0 || limited {
        return Err(
            "Solana Tracker pass is partial; saved observations resume without repeated calls"
                .into(),
        );
    }
    Ok(())
}

async fn archive_response(
    raw_dir: &Path,
    raw_path: &Path,
    response: &TrackerAthBody,
) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(raw_dir).await?;
    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(raw_path)
        .await
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let existing_file = tokio::fs::File::open(raw_path).await?;
            if existing_file.metadata().await?.len() > 2 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "existing raw response exceeds cap",
                ));
            }
            let mut existing = Vec::new();
            existing_file
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut existing)
                .await?;
            if existing != response.bytes {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "raw response hash path collision",
                ));
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    file.write_all(&response.bytes).await?;
    file.flush().await
}

fn valid_billing_month(month: &str) -> bool {
    let Some((year, month)) = month.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|b| b.is_ascii_digit())
        && month.parse::<u8>().is_ok_and(|m| (1..=12).contains(&m))
        && month.len() == 2
}

fn unix_now() -> Result<i64, String> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock before Unix epoch")?
            .as_secs(),
    )
    .map_err(|_| "clock overflow".into())
}

#[allow(clippy::integer_division)] // Gregorian conversion requires integer floors.
fn utc_month(unix_seconds: i64) -> String {
    let z = unix_seconds.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}")
}

#[cfg(test)]
mod tests {
    use super::{utc_month, valid_billing_month};

    #[test]
    fn billing_month_is_utc_and_well_formed() {
        assert_eq!(utc_month(1_791_672_254), "2026-10");
        assert_eq!(utc_month(1_704_067_200), "2024-01");
        assert!(valid_billing_month("2026-10"));
        assert!(!valid_billing_month("2026-13"));
        assert!(!valid_billing_month("2026-1"));
    }
}
