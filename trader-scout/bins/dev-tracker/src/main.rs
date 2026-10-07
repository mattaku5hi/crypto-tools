//! `dev-tracker` — the dev tracker's batch entry points (docs/DEV-TRACKER.md):
//! `migrate` applies the schema, `ingest` runs one pass over every EVM source,
//! `derive` prints the categories as JSON. The daemon (B5) loops these.
//! Secrets (database URL, RPC URLs) come from the environment and are never
//! printed.

use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use scout_devdb::DevDb;
use scout_devtracker::evm_ingest::{EVM_SOURCES, ingest_source};
use scout_devtracker::{DevTrackerConfig, derive_categories};

const DB_ENV: &str = "SCOUT_DEVTRACKER_DATABASE_URL";
const CODEX_ENV: &str = "SCOUT_CODEX_API_KEY";

#[derive(Debug, Parser)]
#[command(
    name = "dev-tracker",
    about = "Dev tracker: facts in PostgreSQL, categories from config"
)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Apply the schema migrations.
    Migrate,
    /// One ingestion pass over the EVM sources of the given chains.
    Ingest {
        /// Comma-separated: bsc, base, robinhood.
        #[arg(long, default_value = "bsc,base,robinhood")]
        chains: String,
        /// Where a source without a cursor starts: this many hours back.
        #[arg(long, default_value_t = 24)]
        start_hours_back: u64,
        /// Blocks behind the head treated as final.
        #[arg(long, default_value_t = 20)]
        confirmations: u64,
        #[arg(long)]
        max_requests: Option<u64>,
        /// New creators classified per chain and pass (contract or wallet).
        #[arg(long, default_value_t = 2_000)]
        max_new_creators: i64,
        /// Signers of launches through shared intermediaries resolved per chain and pass.
        #[arg(long, default_value_t = 5_000)]
        max_signer_lookups: i64,
        /// Codex requests (200 tokens each) per chain and pass for ATH.
        #[arg(long, default_value_t = 20)]
        ath_requests: usize,
    },
    /// Derive the categories and print them as JSON lines.
    Derive {
        #[arg(long, default_value = "config/dev-tracker.example.toml")]
        config: String,
        /// Launches older than this many days are not loaded.
        #[arg(long, default_value_t = 365)]
        since_days: i64,
        /// Print every creator, not only category members.
        #[arg(long)]
        all: bool,
    },
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Approximate block production per hour (B0 measurements).
fn blocks_per_hour(chain: &str) -> u64 {
    match chain {
        "bsc" => 8_000,
        "base" => 1_800,
        "robinhood" => 35_300,
        _ => 0,
    }
}

async fn run(args: Args) -> Result<(), String> {
    let url = std::env::var(DB_ENV).map_err(|_| format!("{DB_ENV} is not set"))?;
    let db = DevDb::connect(&url, 4).await.map_err(|e| e.to_string())?;
    match args.cmd {
        Cmd::Migrate => {
            db.migrate().await.map_err(|e| e.to_string())?;
            eprintln!("dev-tracker: schema up to date");
        }
        Cmd::Ingest {
            chains,
            start_hours_back,
            confirmations,
            max_requests,
            max_new_creators,
            max_signer_lookups,
            ath_requests,
        } => {
            db.migrate().await.map_err(|e| e.to_string())?;
            let notice: scout_app::LimiterNotice = Arc::new(|m| eprintln!("dev-tracker: {m}"));
            for chain in chains.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                let profile = match chain {
                    "bsc" => scout_sdk::evm::BSC,
                    "base" => scout_sdk::evm::BASE,
                    "robinhood" => scout_sdk::evm::ROBINHOOD,
                    other => return Err(format!("unknown chain {other}")),
                };
                let setup = scout_app::setup_evm(
                    &profile.verified_chain_key(),
                    false,
                    max_requests,
                    8,
                    &scout_app::EvmNetOptions::default(),
                    &notice,
                    true,
                    |k| std::env::var(k).ok(),
                )
                .await
                .map_err(|e| e.to_string())?;
                for w in &setup.warnings {
                    eprintln!("dev-tracker: warning: {w}");
                }
                let head = setup.rpc.block_number().await.map_err(|e| e.to_string())?;
                let start =
                    head.saturating_sub(start_hours_back.saturating_mul(blocks_per_hour(chain)));
                for src in EVM_SOURCES.iter().filter(|s| s.chain == chain) {
                    let r = ingest_source(&db, &setup.rpc, src, head, confirmations, start, now())
                        .await
                        .map_err(|e| {
                            format!("{}: {}", src.key, setup_scrub(&setup, &e.to_string()))
                        })?;
                    eprintln!(
                        "dev-tracker: {:28} blocks {}..={} logs {} decoded {} new {} undecodable {}",
                        src.key,
                        r.from_block,
                        r.to_block,
                        r.logs,
                        r.decoded,
                        r.inserted,
                        r.undecodable
                    );
                }
                let id = scout_devtracker::identity::enrich_identities(
                    &db,
                    &setup.rpc,
                    chain,
                    max_new_creators,
                    max_signer_lookups,
                    now(),
                )
                .await
                .map_err(|e| {
                    format!("{chain}: identity: {}", setup_scrub(&setup, &e.to_string()))
                })?;
                eprintln!(
                    "dev-tracker: {chain}: identities: {} creator(s) checked, {} contract(s) ({} single-owner, {} shared), {} signer(s) resolved",
                    id.creators_checked,
                    id.contracts,
                    id.single_owner,
                    id.shared,
                    id.signers_resolved
                );
                if ath_requests > 0 {
                    match std::env::var(CODEX_ENV) {
                        Ok(key) if !key.trim().is_empty() => {
                            let a = scout_devtracker::ath::observe_ath(
                                &db,
                                key.trim(),
                                chain,
                                ath_requests,
                                now(),
                            )
                            .await
                            .map_err(|e| format!("{chain}: ath: {e}"))?;
                            eprintln!(
                                "dev-tracker: {chain}: ath: {} codex request(s), {} token(s) asked, {} observed, {} missing",
                                a.requests, a.tokens_asked, a.observed, a.missing
                            );
                        }
                        _ => eprintln!("dev-tracker: {chain}: ath skipped ({CODEX_ENV} not set)"),
                    }
                }
                for line in setup.rate_limit_report() {
                    eprintln!("dev-tracker: {chain}: {line}");
                }
                eprintln!(
                    "dev-tracker: {chain}: requests_made={}",
                    setup.rpc.total_requests_made()
                );
            }
        }
        Cmd::Derive {
            config,
            since_days,
            all,
        } => {
            let text = std::fs::read_to_string(&config).map_err(|e| format!("{config}: {e}"))?;
            let cfg = DevTrackerConfig::from_toml(&text).map_err(|e| e.to_string())?;
            let t = now();
            let rows = db
                .dev_launches(None, t.saturating_sub(since_days.saturating_mul(86_400)))
                .await
                .map_err(|e| e.to_string())?;
            let verdicts = derive_categories(&rows, t, &cfg);
            let mut members = 0usize;
            for v in &verdicts {
                if !all && v.categories.is_empty() {
                    continue;
                }
                members += usize::from(!v.categories.is_empty());
                let s = &v.stats;
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": s.chain, "creator": s.creator,
                        "categories": v.categories.iter().map(|c| c.label()).collect::<Vec<_>>(),
                        "launches": s.launches, "curve_launches": s.curve_launches,
                        "migrated": s.migrated, "resolved": s.resolved, "pending": s.pending,
                        "migration_rate_bp": s.migration_rate_bp, "current_streak": s.current_streak,
                        "runners": s.runners, "big_runners": s.big_runners,
                        "last_launch_at": s.last_launch_at,
                    })
                );
            }
            eprintln!(
                "dev-tracker: {} launch rows, {} creators, {members} in a category",
                rows.len(),
                verdicts.len()
            );
        }
    }
    Ok(())
}

fn setup_scrub(setup: &scout_app::EvmSetup, text: &str) -> String {
    let mut o = text.to_string();
    for s in setup.secrets() {
        if s.len() >= 4 {
            o = o.replace(&s, "<redacted>");
        }
    }
    o
}

fn main() -> ExitCode {
    let args = Args::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("dev-tracker: runtime: {e}");
            return ExitCode::from(4);
        }
    };
    match rt.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dev-tracker: {e}");
            ExitCode::from(4)
        }
    }
}
