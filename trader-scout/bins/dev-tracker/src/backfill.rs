//! `dev-tracker backfill` (B8, docs/DEV-TRACKER.md §7): one-year history.
//!
//! - EVM launches / migrations: `eth_getLogs` backwards from where live data
//!   starts, `--evm-step-blocks` per step, on the keyed provider (Alchemy);
//!   each source has its own backfill cursor, so a stopped run resumes.
//! - EVM dev identity: the regular enrichment (ADR-021 amendment 3: migrated
//!   and recent launches, then candidate devs' own histories) in batches until
//!   nothing is left, on the keyed provider (dRPC free answers `null` for older
//!   transactions; `--identity-via-fallback` keeps the option).
//! - Solana: whole UTC days, `--solana-concurrency` at a time, each day marked
//!   done once read completely.
//! - ATH: Codex until the candidate queue is empty or `--ath-requests` is spent.
//!
//! Forward ingestion (`run` / `ingest`) can run at the same time: every write
//! is idempotent.

use futures::stream::{self, StreamExt};
use scout_devtracker::evm_ingest::{EVM_SOURCES, backfill_step};
use scout_devtracker::identity::enrich_identities;
use scout_devtracker::solana_ingest::{SOLANA_SOURCES, backfill_days, backfill_solana_day};

use super::*;

/// What to backfill and with which budgets.
#[derive(Debug, Clone, clap::Args)]
pub struct BackfillArgs {
    /// Comma-separated: bsc, base, robinhood, solana.
    #[arg(long, default_value = "bsc,base,robinhood,solana")]
    chains: String,
    /// Comma-separated phases: logs, identity, solana, ath.
    #[arg(long, default_value = "logs,identity,solana,ath")]
    phases: String,
    /// How far back.
    #[arg(long, default_value_t = 365)]
    days: i64,
    /// EVM blocks per `eth_getLogs` step (the cursor moves after each).
    #[arg(long, default_value_t = 200_000)]
    evm_step_blocks: u64,
    /// Resolve identities on the free fallback endpoint (dRPC) instead of the
    /// keyed one. Measured 2026-10-07: dRPC free answers `null` for launch
    /// transactions older than a few hours, so this is off by default.
    #[arg(long)]
    identity_via_fallback: bool,
    /// Requests per second on the fallback endpoint with `--identity-via-fallback`.
    #[arg(long, default_value_t = 10)]
    fallback_rps: u32,
    /// Creators classified and signers resolved per identity batch.
    #[arg(long, default_value_t = 20_000)]
    identity_batch: i64,
    /// HTTP request budget of the identity client per chain (none = unlimited).
    #[arg(long)]
    identity_max_requests: Option<u64>,
    /// Solana days read at the same time.
    #[arg(long, default_value_t = 6)]
    solana_concurrency: usize,
    /// Helius pages (≤ 1000 txs) per Solana day and source.
    #[arg(long, default_value_t = 300)]
    solana_max_pages: u32,
    /// Codex requests (200 tokens each) per chain.
    #[arg(long, default_value_t = 3_000)]
    ath_requests: usize,
}

impl BackfillArgs {
    fn has(&self, phase: &str) -> bool {
        self.phases.split(',').any(|p| p.trim() == phase)
    }

    fn chains(&self) -> Vec<String> {
        self.chains
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect()
    }
}

/// Run the backfill; EVM chains one after another, Solana alongside, ATH last.
///
/// # Errors
/// Setup failures; per-day / per-chain failures are reported and the rest
/// continues (rerun to retry what is left).
pub async fn run_backfill(db: &DevDb, a: &BackfillArgs) -> Result<(), String> {
    let chains = a.chains();
    // every chain is its own task (and every Solana day, below): the
    // multi-threaded runtime spreads the JSON decoding over the cores; one
    // `join!` would run them all on one thread (measured: Solana's page
    // decoding starved the EVM steps to one per two minutes)
    let mut tasks = Vec::new();
    for chain in chains.iter().filter(|c| c.as_str() != "solana") {
        let (db, a, chain) = (db.clone(), a.clone(), chain.clone());
        tasks.push((
            chain.clone(),
            tokio::spawn(async move { backfill_evm_chain(&db, &chain, &a).await }),
        ));
    }
    if chains.iter().any(|c| c == "solana") && a.has("solana") {
        let (db, a) = (db.clone(), a.clone());
        tasks.push((
            "solana".to_string(),
            tokio::spawn(async move { backfill_solana(&db, &a).await }),
        ));
    }
    let mut failed: Vec<String> = Vec::new();
    for (chain, task) in tasks {
        let outcome = task.await.map_err(|e| format!("task: {e}")).and_then(|r| r);
        if let Err(e) = outcome {
            eprintln!("dev-tracker: backfill {chain}: {e}");
            failed.push(chain);
        }
    }
    if a.has("ath") {
        for chain in &chains {
            backfill_ath(db, chain, a.ath_requests).await?;
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "backfill incomplete (failed: {}); rerun to continue",
            failed.join(",")
        ));
    }
    eprintln!("dev-tracker: backfill done");
    Ok(())
}

/// Smallest EVM step the backfill splits down to.
const MIN_STEP_BLOCKS: u64 = 2_000;

fn profile_of(chain: &str) -> Result<scout_sdk::evm::EvmChainProfile, String> {
    match chain {
        "bsc" => Ok(scout_sdk::evm::BSC),
        "base" => Ok(scout_sdk::evm::BASE),
        "robinhood" => Ok(scout_sdk::evm::ROBINHOOD),
        other => Err(format!("unknown chain {other}")),
    }
}

/// The environment with the keyed and the fallback RPC URL of `chain` swapped,
/// so the free fallback endpoint becomes primary (and the keyed one its
/// automatic fallback).
fn swapped_env(chain: &str) -> impl Fn(&str) -> Option<String> {
    let rpc = scout_app::rpc_env_name(chain);
    let fallback = scout_app::fallback_rpc_env_name(chain);
    move |k: &str| {
        let get = |name: Option<&str>| name.and_then(|n| std::env::var(n).ok());
        if Some(k) == rpc {
            get(fallback)
        } else if Some(k) == fallback {
            get(rpc)
        } else {
            std::env::var(k).ok()
        }
    }
}

async fn backfill_evm_chain(db: &DevDb, chain: &str, a: &BackfillArgs) -> Result<(), String> {
    let profile = profile_of(chain)?;
    let notice: scout_app::LimiterNotice = Arc::new(|m| eprintln!("dev-tracker: {m}"));
    let keyed = scout_app::setup_evm(
        &profile.verified_chain_key(),
        false,
        None,
        8,
        &scout_app::EvmNetOptions::default(),
        &notice,
        true,
        |k| std::env::var(k).ok(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let head = keyed
        .rpc
        .block_number()
        .await
        .map_err(|e| setup_scrub(&keyed, &e.to_string()))?;
    let span = u64::try_from(a.days.max(0))
        .unwrap_or(0)
        .saturating_mul(24)
        .saturating_mul(blocks_per_hour(chain));
    let floor = head.saturating_sub(span);
    if a.has("logs") {
        for src in EVM_SOURCES.iter().filter(|s| s.chain == chain) {
            let (mut logs, mut new) = (0usize, 0u64);
            // a step whose logs exceed the per-call cap is split (live: Zora
            // on Base had > 200k logs in 200k blocks); the cursor did not move
            let mut step_blocks = a.evm_step_blocks.max(1);
            loop {
                let step = match backfill_step(db, &keyed.rpc, src, floor, step_blocks, head, now())
                    .await
                {
                    Ok(step) => step,
                    Err(e)
                        if e.to_string().contains("configured cap")
                            && step_blocks > MIN_STEP_BLOCKS =>
                    {
                        step_blocks = step_blocks.div_euclid(4).max(MIN_STEP_BLOCKS);
                        eprintln!(
                            "dev-tracker: backfill {}: too many logs in one step, retrying with {step_blocks} blocks",
                            src.key
                        );
                        continue;
                    }
                    Err(e) => {
                        return Err(format!(
                            "{}: {}",
                            src.key,
                            setup_scrub(&keyed, &e.to_string())
                        ));
                    }
                };
                let Some(r) = step else {
                    break;
                };
                logs += r.logs;
                new += r.inserted;
                let left = r.from_block.saturating_sub(floor);
                eprintln!(
                    "dev-tracker: backfill {:28} blocks {}..={} logs {} new {} ({} blocks left)",
                    src.key, r.from_block, r.to_block, r.logs, r.inserted, left
                );
            }
            eprintln!(
                "dev-tracker: backfill {:28} done: {logs} logs, {new} new facts",
                src.key
            );
        }
    }
    if a.has("identity") {
        let free = if !a.identity_via_fallback {
            None
        } else {
            let net = scout_app::EvmNetOptions {
                rpc_rps: Some(a.fallback_rps),
                ..scout_app::EvmNetOptions::default()
            };
            Some(
                scout_app::setup_evm(
                    &profile.verified_chain_key(),
                    false,
                    a.identity_max_requests,
                    16,
                    &net,
                    &notice,
                    false,
                    swapped_env(chain),
                )
                .await
                .map_err(|e| format!("identity endpoint: {e}"))?,
            )
        };
        let (rpc, retry, scrub_with) = match &free {
            Some(f) => (&f.rpc, Some(&keyed.rpc), f),
            None => (&keyed.rpc, None, &keyed),
        };
        let started = std::time::Instant::now();
        let mut total = (0usize, 0usize);
        loop {
            let r = enrich_identities(
                db,
                rpc,
                retry,
                chain,
                scout_devtracker::identity::IdentityBudget {
                    max_creators: a.identity_batch,
                    max_signers: a.identity_batch,
                    max_histories: a.identity_batch,
                },
                now(),
            )
            .await
            .map_err(|e| format!("identity: {}", setup_scrub(scrub_with, &e.to_string())))?;
            total.0 += r.creators_checked;
            total.1 += r.signers_resolved;
            eprintln!(
                "dev-tracker: backfill {chain} identity: +{} creator(s), +{} signer(s), +{} dev histories → {} launches (total {} / {}, {} requests, {:.0?})",
                r.creators_checked,
                r.signers_resolved,
                r.histories_fetched,
                r.launches_from_histories,
                total.0,
                total.1,
                rpc.total_requests_made(),
                started.elapsed()
            );
            if r.creators_checked == 0 && r.signers_resolved == 0 && r.histories_fetched == 0 {
                break;
            }
        }
        for line in scrub_with.rate_limit_report() {
            eprintln!("dev-tracker: backfill {chain} identity: {line}");
        }
    }
    eprintln!(
        "dev-tracker: backfill {chain}: keyed requests_made={}",
        keyed.rpc.total_requests_made()
    );
    Ok(())
}

async fn backfill_solana(db: &DevDb, a: &BackfillArgs) -> Result<(), String> {
    let key = std::env::var(HELIUS_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| format!("{HELIUS_ENV} is not set"))?;
    let key = key.trim().to_string();
    let scrub = |t: String| t.replace(&key, "<redacted>");
    let provider = std::sync::Arc::new(
        scout_providers::HeliusProvider::new(&key, 120_000, 4)
            .map_err(|e| scrub(e.to_string()))?
            .with_status_filter(scout_providers::StatusFilter::Succeeded)
            .with_skip_undecodable()
            .with_page_limit(1_000)
            .with_max_pages(
                std::num::NonZeroU32::new(a.solana_max_pages.max(1))
                    .unwrap_or(std::num::NonZeroU32::MIN),
            ),
    );
    let days = backfill_days(now(), a.days);
    let mut failed = 0usize;
    for src in SOLANA_SOURCES {
        let started = std::time::Instant::now();
        let (mut done, mut skipped, mut txs, mut new) = (0usize, 0usize, 0usize, 0u64);
        // each day is a task: page decoding runs on all cores
        let mut results = stream::iter(days.iter().copied())
            .map(|d| {
                let (db, provider) = (db.clone(), provider.clone());
                tokio::spawn(async move {
                    let r = backfill_solana_day(&db, provider.as_ref(), &src, d, now())
                        .await
                        .map_err(|e| e.to_string());
                    (d, r)
                })
            })
            .buffer_unordered(a.solana_concurrency.max(1));
        while let Some(joined) = results.next().await {
            let (day, r) = match joined {
                Ok(v) => v,
                Err(e) => {
                    failed += 1;
                    eprintln!("dev-tracker: backfill {}: task: {e}", src.key);
                    continue;
                }
            };
            match r {
                Ok(None) => skipped += 1,
                Ok(Some(rep)) => {
                    done += 1;
                    txs += rep.transactions;
                    new += rep.inserted;
                    if rep.truncated {
                        failed += 1;
                        eprintln!(
                            "dev-tracker: backfill {} day {day}: page budget hit, not marked done",
                            src.key
                        );
                    }
                    if done % 10 == 0 {
                        eprintln!(
                            "dev-tracker: backfill {}: {done} day(s) read ({skipped} already done), {txs} txs, {new} new facts, {:.0?}",
                            src.key,
                            started.elapsed()
                        );
                    }
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("dev-tracker: backfill {} day {day}: {}", src.key, scrub(e));
                }
            }
        }
        eprintln!(
            "dev-tracker: backfill {} done: {done} day(s) read, {skipped} already done, {txs} txs, {new} new facts, {:.0?}",
            src.key,
            started.elapsed()
        );
    }
    eprintln!(
        "dev-tracker: backfill solana: requests_made={}",
        provider.total_requests_made()
    );
    if let Some(skipped) = provider.skipped_undecodable().filter(|s| s.count() > 0) {
        // coverage gap, reported (never silent): these transactions were left out
        eprintln!(
            "dev-tracker: backfill solana: {} undecodable transaction(s) skipped",
            skipped.count()
        );
        for (sig, err) in skipped.kept().iter().take(5) {
            eprintln!("dev-tracker: backfill solana:   skipped {sig}: {err}");
        }
    }
    if failed > 0 {
        return Err(format!("{failed} day(s) not completed"));
    }
    Ok(())
}

async fn backfill_ath(db: &DevDb, chain: &str, budget: usize) -> Result<(), String> {
    let Ok(key) = std::env::var(CODEX_ENV) else {
        eprintln!("dev-tracker: backfill {chain} ath skipped ({CODEX_ENV} not set)");
        return Ok(());
    };
    let mut spent = 0usize;
    while spent < budget {
        let batch = (budget - spent).min(100);
        let r = scout_devtracker::ath::observe_ath(db, key.trim(), chain, batch, now())
            .await
            .map_err(|e| format!("{chain}: ath: {e}"))?;
        spent += r.requests;
        eprintln!(
            "dev-tracker: backfill {chain} ath: {} request(s) ({spent} total), {} observed, {} missing",
            r.requests, r.observed, r.missing
        );
        if r.requests < batch {
            break;
        }
    }
    Ok(())
}
