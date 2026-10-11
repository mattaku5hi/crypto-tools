//! Explicit, resumable Birdeye price-window backfill; no FDV substitution.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scout_devdb::{AthPriceWindow, DevDb};
use scout_devtracker::birdeye::{BirdeyeChain, BirdeyeError, OhlcvWindow};
use tokio::io::AsyncReadExt;

const SOURCE: &str = "birdeye-ohlcv-1d-usd-outliers-v1";
const ENDPOINT: &str = "https://public-api.birdeye.so/defi/v3/ohlcv";
const INPUT_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, clap::Args)]
pub struct BirdeyePricesArgs {
    /// JSON array of {"chain":"base","token":"0x..."}; at most 2,000 tracked tokens.
    #[arg(long)]
    tokens: String,
    /// Inclusive Unix seconds; daily candles are checked by UTC bucket.
    #[arg(long)]
    time_from: i64,
    #[arg(long)]
    time_to: i64,
    /// Attempt ceiling for this invocation. Saved exact windows are skipped.
    #[arg(long, default_value_t = 20)]
    max_requests: usize,
}

pub async fn run(db: &DevDb, args: BirdeyePricesArgs) -> Result<(), String> {
    if args.time_from < 0 || args.time_to < args.time_from || args.max_requests == 0 {
        return Err(
            "Birdeye requires a nonnegative ordered window and a positive request budget".into(),
        );
    }
    let file = tokio::fs::File::open(&args.tokens)
        .await
        .map_err(|_| "cannot open token file")?;
    let mut bytes = Vec::new();
    file.take(INPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "cannot read token file")?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > INPUT_LIMIT {
        return Err("token file exceeds 2 MiB".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "invalid token JSON")?;
    let input = value
        .as_array()
        .filter(|rows| rows.len() <= 2_000)
        .ok_or("expected at most 2,000 tokens")?;
    let mut tokens = BTreeSet::new();
    for row in input {
        let chain = row
            .get("chain")
            .and_then(serde_json::Value::as_str)
            .ok_or("missing chain")?;
        let token = row
            .get("token")
            .and_then(serde_json::Value::as_str)
            .ok_or("missing token")?;
        if !matches!(chain, "solana" | "bsc" | "base" | "robinhood") || token.len() > 44 {
            return Err("unsupported chain or invalid token length".into());
        }
        tokens.insert((
            chain.to_owned(),
            if chain == "solana" {
                token.to_owned()
            } else {
                token.to_ascii_lowercase()
            },
        ));
    }
    let key =
        std::env::var("SCOUT_BIRDEYE_API_KEY").map_err(|_| "SCOUT_BIRDEYE_API_KEY is not set")?;
    let http = scout_devtracker::birdeye::http_client().map_err(|e| e.to_string())?;
    let mut attempts = 0;
    let mut saved = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut budget_exhausted = false;
    for (chain, token) in tokens {
        if db
            .has_ath_price_window(&chain, &token, SOURCE, args.time_from, args.time_to)
            .await
            .map_err(|e| e.to_string())?
        {
            skipped += 1;
            continue;
        }
        if attempts >= args.max_requests {
            budget_exhausted = true;
            break;
        }
        if db
            .known_launch_tokens(&chain, std::slice::from_ref(&token))
            .await
            .map_err(|e| e.to_string())?
            .is_empty()
        {
            return Err(
                "Birdeye input contains a token outside the tracked launch registry".into(),
            );
        }
        let network = match chain.as_str() {
            "solana" => BirdeyeChain::Solana,
            "bsc" => BirdeyeChain::Bsc,
            "base" => BirdeyeChain::Base,
            _ => BirdeyeChain::Robinhood,
        };
        if attempts > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        attempts += 1;
        let request = OhlcvWindow {
            chain: network,
            address: token.clone(),
            time_from: args.time_from,
            time_to: args.time_to,
        };
        let result =
            scout_devtracker::birdeye::fetch_window_price_high(&http, ENDPOINT, &key, &request)
                .await;
        let observed_at = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "clock before epoch")?
                .as_secs(),
        )
        .map_err(|_| "clock overflow")?;
        let high = match result {
            Ok(high) => high,
            Err(error) => {
                failed += 1;
                eprintln!("dev-tracker: Birdeye {chain} {token}: {error}");
                if matches!(
                    error,
                    BirdeyeError::AccessDenied(_) | BirdeyeError::RateLimited(_)
                ) {
                    return Err(format!(
                        "Birdeye stopped after {attempts} attempts; saved {saved} windows"
                    ));
                }
                continue;
            }
        };
        let row = AthPriceWindow {
            chain: chain.clone(),
            token: token.clone(),
            source: SOURCE.into(),
            window_from: args.time_from,
            window_to: args.time_to,
            max_price_usd: high.as_ref().map(|h| h.max_price_usd.clone()),
            high_candle_at: high.as_ref().map(|h| h.candle_at),
            first_candle_at: high.as_ref().map(|h| h.time_scope.first_candle_at),
            last_candle_at: high.as_ref().map(|h| h.time_scope.last_candle_at),
            returned_candles: high
                .as_ref()
                .map_or(0, |h| i32::try_from(h.returned_candles).unwrap_or(i32::MAX)),
            observed_at,
        };
        db.upsert_ath_price_window(&row)
            .await
            .map_err(|e| e.to_string())?;
        saved += 1;
        println!(
            "{}",
            serde_json::json!({"chain":chain,"token":token,"saved":true,"max_price_usd":row.max_price_usd,"returned_candles":row.returned_candles,"quality":"observed-price-window/supply-unverified/lifetime-coverage-unproven"})
        );
    }
    eprintln!(
        "dev-tracker: Birdeye {attempts} attempts, {saved} saved windows, {skipped} cached, {failed} failed; ATH FDV remains separate"
    );
    if failed > 0 || budget_exhausted {
        return Err(
            "Birdeye price-window pass is partial; saved windows resume without new requests"
                .into(),
        );
    }
    Ok(())
}
