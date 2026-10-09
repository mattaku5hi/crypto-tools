//! `dev-tracker` — the dev tracker's entry points (docs/DEV-TRACKER.md):
//! `migrate` applies the schema, `ingest` runs one pass over the sources of the
//! given chains, `derive` prints the categories as JSON, `export` writes the
//! wallet-tracker files (and sends changed lists), `run` is the 24/7 daemon
//! (B5) that loops ingestion and derivation on the config's cadence, re-reading
//! the config every cycle. Secrets (database URL, RPC URLs, API keys, bot token)
//! come from the environment and are never printed.

mod backfill;

use std::collections::BTreeSet;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use scout_devdb::{Delivery, DevDb};
use scout_devtracker::evm_ingest::{EVM_SOURCES, ingest_source};
use scout_devtracker::export::{ExportFile, Format, change_report, render};
use scout_devtracker::solana_ingest::{SOLANA_SOURCES, ingest_solana_source};
use scout_devtracker::telegram::Telegram;
use scout_devtracker::{Category, DevTrackerConfig, DevVerdict, derive_from_db};

const DB_ENV: &str = "SCOUT_DEVTRACKER_DATABASE_URL";
const CODEX_ENV: &str = "SCOUT_CODEX_API_KEY";
const HELIUS_ENV: &str = "SCOUT_HELIUS_API_KEY";
const TELEGRAM_TOKEN_ENV: &str = "SCOUT_TELEGRAM_BOT_TOKEN";
const TELEGRAM_CHAT_ENV: &str = "SCOUT_TELEGRAM_CHAT_ID";
const DEFAULT_CONFIG: &str = "config/dev-tracker.example.toml";

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
    /// One ingestion pass over the sources of the given chains.
    Ingest {
        /// Comma-separated: solana, bsc, base, robinhood.
        #[arg(long, default_value = "bsc,base,robinhood")]
        chains: String,
        /// Where a source without a cursor starts: this many hours back.
        #[arg(long, default_value_t = 24)]
        start_hours_back: u64,
        /// EVM blocks behind the head treated as final.
        #[arg(long, default_value_t = 20)]
        confirmations: u64,
        /// HTTP request budget per chain and pass.
        #[arg(long)]
        max_requests: Option<u64>,
        /// New creators classified per chain and pass (contract or wallet).
        #[arg(long, default_value_t = 2_000)]
        max_new_creators: i64,
        /// Signers of launches through shared intermediaries resolved per chain and pass.
        #[arg(long, default_value_t = 5_000)]
        max_signer_lookups: i64,
        /// Candidate devs whose history through intermediaries is fetched per chain and pass.
        #[arg(long, default_value_t = 500)]
        max_histories: i64,
        /// Codex requests (200 tokens each) per chain and pass for ATH.
        #[arg(long, default_value_t = 20)]
        ath_requests: usize,
        /// Solana: `getTransactionsForAddress` pages (≤ 1000 txs, 10 Helius
        /// credits per 100 txs) per source and pass.
        #[arg(long, default_value_t = 200)]
        solana_max_pages: u32,
        /// Solana: seconds behind now treated as final.
        #[arg(long, default_value_t = 60)]
        solana_settle_secs: i64,
        /// Solana: read through the standard-RPC fallback instead of Helius
        /// (normally used only when Helius fails).
        #[arg(long)]
        solana_via_fallback: bool,
    },
    /// Derive the categories and print them as JSON lines.
    Derive {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: String,
        /// Launches older than this many days are not loaded.
        #[arg(long, default_value_t = 365)]
        since_days: i64,
        /// Print every creator, not only category members.
        #[arg(long)]
        all: bool,
    },
    /// Derive, write the wallet-tracker files to `delivery.out_dir`, and with
    /// `--send` deliver the lists whose wallets changed to Telegram.
    Export {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: String,
        #[arg(long)]
        send: bool,
        /// Send the largest list once, marked as a test; nothing is recorded,
        /// so the real first delivery still happens.
        #[arg(long)]
        test: bool,
    },
    /// The daemon: ingestion every `schedule.ingest_every_minutes`, derivation
    /// and delivery every `schedule.derive_every_minutes`; the config is
    /// re-read every cycle. Stops on SIGTERM / Ctrl-C.
    Run {
        #[arg(long, default_value = DEFAULT_CONFIG)]
        config: String,
    },
    /// One-year history (B8): EVM logs on the keyed provider, EVM identity on
    /// the free fallback endpoint, Solana by whole days, then ATH. Resumable.
    Backfill(backfill::BackfillArgs),
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

/// Budgets of one ingestion pass over one chain.
#[derive(Debug, Clone)]
struct IngestOpts {
    start_hours_back: u64,
    confirmations: u64,
    max_requests: Option<u64>,
    max_new_creators: i64,
    max_signer_lookups: i64,
    max_histories: i64,
    ath_requests: usize,
    solana_max_pages: u32,
    solana_settle_secs: i64,
    solana_via_fallback: bool,
}

impl IngestOpts {
    fn from_config(cfg: &DevTrackerConfig) -> Self {
        let s = &cfg.schedule;
        Self {
            start_hours_back: s.start_hours_back,
            confirmations: 20,
            max_requests: None,
            max_new_creators: s.max_new_creators,
            max_signer_lookups: s.max_signer_lookups,
            max_histories: s.max_dev_histories,
            ath_requests: s.ath_requests_per_chain,
            solana_max_pages: s.solana_max_pages,
            solana_settle_secs: 60,
            solana_via_fallback: false,
        }
    }
}

fn read_config(path: &str) -> Result<DevTrackerConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    DevTrackerConfig::from_toml(&text).map_err(|e| format!("{path}: {e}"))
}

/// One ingestion pass over one chain: sources, (EVM) dev identities, ATH.
/// Degradations of a successful pass: `(incident key, error)` — e.g. a
/// provider that failed while its fallback served the pass.
type Degraded = Vec<(String, String)>;

async fn ingest_chain(db: &DevDb, chain: &str, o: &IngestOpts) -> Result<Degraded, String> {
    if chain == "solana" {
        let degraded = ingest_solana(db, o).await?;
        observe_ath(db, chain, o.ath_requests).await?;
        return Ok(degraded);
    }
    let profile = match chain {
        "bsc" => scout_sdk::evm::BSC,
        "base" => scout_sdk::evm::BASE,
        "robinhood" => scout_sdk::evm::ROBINHOOD,
        other => return Err(format!("unknown chain {other}")),
    };
    let notice: scout_app::LimiterNotice = Arc::new(|m| eprintln!("dev-tracker: {m}"));
    let setup = scout_app::setup_evm(
        &profile.verified_chain_key(),
        false,
        o.max_requests,
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
    let head = setup
        .rpc
        .block_number()
        .await
        .map_err(|e| setup_scrub(&setup, &e.to_string()))?;
    let start = head.saturating_sub(o.start_hours_back.saturating_mul(blocks_per_hour(chain)));
    for src in EVM_SOURCES.iter().filter(|s| s.chain == chain) {
        let r = ingest_source(db, &setup.rpc, src, head, o.confirmations, start, now())
            .await
            .map_err(|e| format!("{}: {}", src.key, setup_scrub(&setup, &e.to_string())))?;
        eprintln!(
            "dev-tracker: {:28} blocks {}..={} logs {} decoded {} new {} undecodable {}",
            src.key, r.from_block, r.to_block, r.logs, r.decoded, r.inserted, r.undecodable
        );
    }
    let id = scout_devtracker::identity::enrich_identities(
        db,
        &setup.rpc,
        None,
        chain,
        scout_devtracker::identity::IdentityBudget {
            max_creators: o.max_new_creators,
            max_signers: o.max_signer_lookups,
            max_histories: o.max_histories,
        },
        now(),
    )
    .await
    .map_err(|e| format!("{chain}: identity: {}", setup_scrub(&setup, &e.to_string())))?;
    eprintln!(
        "dev-tracker: {chain}: identities: {} creator(s) checked, {} contract(s) ({} single-owner, {} shared), {} signer(s) resolved, {} dev histories → {} launches",
        id.creators_checked,
        id.contracts,
        id.single_owner,
        id.shared,
        id.signers_resolved,
        id.histories_fetched,
        id.launches_from_histories
    );
    observe_ath(db, chain, o.ath_requests).await?;
    for line in setup.rate_limit_report() {
        eprintln!("dev-tracker: {chain}: {line}");
    }
    eprintln!(
        "dev-tracker: {chain}: requests_made={}",
        setup.rpc.total_requests_made()
    );
    Ok(Vec::new())
}

async fn observe_ath(db: &DevDb, chain: &str, ath_requests: usize) -> Result<(), String> {
    if ath_requests == 0 {
        return Ok(());
    }
    match std::env::var(CODEX_ENV) {
        Ok(key) if !key.trim().is_empty() => {
            let a = scout_devtracker::ath::observe_ath(db, key.trim(), chain, ath_requests, now())
                .await
                .map_err(|e| format!("{chain}: ath: {e}"))?;
            eprintln!(
                "dev-tracker: {chain}: ath: {} codex request(s), {} token(s) asked, {} observed, {} missing",
                a.requests, a.tokens_asked, a.observed, a.missing
            );
        }
        _ => eprintln!("dev-tracker: {chain}: ath skipped ({CODEX_ENV} not set)"),
    }
    Ok(())
}

/// One pass over the pump.fun sources on Helius (`SCOUT_HELIUS_API_KEY`).
async fn ingest_solana(db: &DevDb, o: &IngestOpts) -> Result<Degraded, String> {
    let key = std::env::var(HELIUS_ENV)
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| format!("solana: {HELIUS_ENV} is not set"))?;
    let key = key.trim().to_string();
    let scrub = |t: String| t.replace(&key, "<redacted>");
    let provider = scout_providers::HeliusProvider::new(&key, 60_000, 3)
        .map_err(|e| scrub(e.to_string()))?
        .with_status_filter(scout_providers::StatusFilter::Succeeded)
        .with_skip_undecodable()
        .with_page_limit(1_000)
        .with_max_pages(
            std::num::NonZeroU32::new(o.solana_max_pages.max(1))
                .unwrap_or(std::num::NonZeroU32::MIN),
        )
        .with_max_total_requests(o.max_requests);
    let start = i64::try_from(o.start_hours_back.saturating_mul(3_600)).unwrap_or(i64::MAX);
    let fallback = solana_fallback_rpc()?;
    let mut degraded: Degraded = Vec::new();
    for src in &SOLANA_SOURCES {
        let primary = if o.solana_via_fallback {
            Err("forced to the fallback".to_string())
        } else {
            ingest_solana_source(db, &provider, src, start, o.solana_settle_secs, now())
                .await
                .map_err(|e| scrub(e.to_string()))
        };
        let (r, via) = match primary {
            Ok(r) => (r, "helius"),
            Err(e) => {
                eprintln!(
                    "dev-tracker: {}: helius: {e}; trying the standard-RPC fallback",
                    src.key
                );
                if !o.solana_via_fallback {
                    degraded.push(("provider:helius".to_string(), format!("{}: {e}", src.key)));
                }
                let (rpc, fb_scrub) = &fallback;
                let r = scout_devtracker::solana_fallback::fallback_ingest_source(
                    db,
                    rpc,
                    src,
                    o.solana_settle_secs,
                    SOLANA_FALLBACK_MAX_WINDOW,
                    now(),
                )
                .await
                .map_err(|e| format!("{}: fallback: {}", src.key, fb_scrub(&e.to_string())))?;
                (r, "fallback")
            }
        };
        eprintln!(
            "dev-tracker: {:28} time {}..{} txs {} decoded {} new {} undecodable {} via {via}{}",
            src.key,
            r.from,
            r.to,
            r.transactions,
            r.decoded,
            r.inserted,
            r.undecodable,
            if r.truncated {
                " (page budget hit; continues next pass)"
            } else {
                ""
            }
        );
    }
    eprintln!(
        "dev-tracker: solana: requests_made={}",
        provider.total_requests_made()
    );
    if let Some(skipped) = provider.skipped_undecodable().filter(|s| s.count() > 0) {
        // coverage gap, reported (never silent): these transactions were left out
        eprintln!(
            "dev-tracker: solana: {} undecodable transaction(s) skipped",
            skipped.count()
        );
        for (sig, err) in skipped.kept().iter().take(5) {
            eprintln!("dev-tracker: solana:   skipped {sig}: {err}");
        }
    }
    Ok(degraded)
}

/// Longest window one fallback pass reads (a long Helius outage is caught up
/// over several passes).
const SOLANA_FALLBACK_MAX_WINDOW: i64 = 6 * 3_600;
/// Requests per second on the Solana fallback endpoint (public RPC limits).
const SOLANA_FALLBACK_RPS: u64 = 4;
const SOLANA_FALLBACK_ENV: &str = "SCOUT_SOLANA_FALLBACK_RPC_URL";

/// The standard-RPC Solana fallback (`SCOUT_SOLANA_FALLBACK_RPC_URL`, else the
/// keyless public RPC) and a scrubber for its URL.
#[allow(clippy::type_complexity)]
fn solana_fallback_rpc() -> Result<(scout_rpc::RpcClient, Box<dyn Fn(&str) -> String>), String> {
    let url = std::env::var(SOLANA_FALLBACK_ENV)
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| scout_devtracker::solana_fallback::PUBLIC_SOLANA_RPC.to_string());
    let secret = url.clone();
    let scrub = Box::new(move |t: &str| t.replace(&secret, "<redacted>"));
    let rpc = scout_rpc::RpcClient::new(scout_rpc::RpcEndpoint::new(url), 30_000, 3)
        .map_err(|e| scrub(&e.to_string()))?
        .with_rate_limiter(
            Arc::new(scout_rpc::RateLimiter::new(
                SOLANA_FALLBACK_RPS,
                SOLANA_FALLBACK_RPS,
            )),
            None,
        );
    Ok((rpc, scrub))
}

/// Members of the last delivered lists as `(chain, creator)` (their verdicts
/// are kept so a dropped wallet's reason can be reported).
fn watch_set(deliveries: &[Delivery]) -> BTreeSet<(String, String)> {
    deliveries
        .iter()
        .filter_map(|d| {
            let chain = d.category.split_once(':')?.1.to_string();
            Some(d.members.iter().map(move |m| (chain.clone(), m.clone())))
        })
        .flatten()
        .collect()
}

async fn derive(
    db: &DevDb,
    cfg: &DevTrackerConfig,
    since_days: i64,
    keep_all: bool,
    watch: &BTreeSet<(String, String)>,
) -> Result<Vec<DevVerdict>, String> {
    let t = now();
    let since = t.saturating_sub(since_days.saturating_mul(86_400));
    let (verdicts, totals) = derive_from_db(db, since, t, cfg, keep_all, watch)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!(
        "dev-tracker: {} launch rows, {} creators, {} in a category",
        totals.launch_rows, totals.creators, totals.members
    );
    Ok(verdicts)
}

fn telegram_from_env() -> Option<Telegram> {
    let token = std::env::var(TELEGRAM_TOKEN_ENV)
        .ok()
        .filter(|t| !t.trim().is_empty())?;
    let chat = std::env::var(TELEGRAM_CHAT_ENV)
        .ok()
        .filter(|t| !t.trim().is_empty())?;
    Telegram::new(&token, &chat).ok()
}

/// Render every file and write it to `out_dir`; with a bot, send each list
/// whose wallets changed since its last delivery as one album (the changed
/// formats, timestamped names) captioned with the change report. A list never
/// delivered is not sent while empty. `test` sends only the largest list,
/// marked as a test, and records nothing. Returns the number of lists sent.
async fn export(
    db: &DevDb,
    cfg: &DevTrackerConfig,
    verdicts: &[DevVerdict],
    deliveries: &[Delivery],
    telegram: Option<&Telegram>,
    test: bool,
) -> Result<usize, String> {
    let d = &cfg.delivery;
    let t = now();
    if !d.out_dir.is_empty() {
        std::fs::create_dir_all(&d.out_dir).map_err(|e| format!("{}: {e}", d.out_dir))?;
    }
    let last = |key: &str, format: &str| {
        deliveries
            .iter()
            .find(|x| x.category == key && x.format == format)
    };
    // (files of one list, formats to send)
    let mut lists: Vec<(Vec<ExportFile>, Vec<usize>)> = Vec::new();
    for chain in &d.chains {
        for category in Category::ALL {
            let files: Vec<ExportFile> = d
                .formats
                .iter()
                .filter_map(|f| Format::parse(f))
                .map(|f| render(verdicts, category, chain, f, d.max_wallets_per_file, t))
                .collect();
            if !d.out_dir.is_empty() {
                for file in &files {
                    let path = std::path::Path::new(&d.out_dir).join(&file.file_name);
                    let tmp = path.with_extension("tmp");
                    std::fs::write(&tmp, &file.content)
                        .and_then(|()| std::fs::rename(&tmp, &path))
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                }
            }
            let changed: Vec<usize> = files
                .iter()
                .enumerate()
                .filter(|(_, f)| match last(&f.delivery_key(), f.format.name()) {
                    Some(prev) => prev.content_hash != f.members_hash,
                    None => !f.members.is_empty(),
                })
                .map(|(i, _)| i)
                .collect();
            lists.push((files, changed));
        }
    }
    let Some(tg) = telegram else {
        return Ok(0);
    };
    if test {
        // the largest list, every format
        lists.sort_by_key(|(files, _)| {
            std::cmp::Reverse(files.first().map_or(0, |f| f.members.len()))
        });
        lists.truncate(1);
        for (files, changed) in &mut lists {
            *changed = (0..files.len()).collect();
        }
    }
    let mut sent = 0usize;
    for (files, changed) in &lists {
        let Some(&first) = changed.first() else {
            continue;
        };
        let Some(head) = files.get(first) else {
            continue;
        };
        let prev = changed
            .iter()
            .filter_map(|&i| files.get(i))
            .find_map(|f| last(&f.delivery_key(), f.format.name()))
            .map(|x| x.members.as_slice());
        let parts = change_report(head, prev, verdicts, cfg, t, d.max_wallets_per_file, test);
        let album: Vec<(String, Vec<u8>)> = changed
            .iter()
            .filter_map(|&i| files.get(i))
            .map(|f| (f.timestamped_name(t), f.content.clone()))
            .collect();
        let caption = parts.first().cloned().unwrap_or_default();
        if let Err(e) = tg.send_album(album, &caption).await {
            // retried next cycle (nothing recorded)
            eprintln!("dev-tracker: {}: {e}", head.delivery_key());
            continue;
        }
        for p in parts.iter().skip(1) {
            tokio::time::sleep(Duration::from_millis(1_100)).await;
            if let Err(e) = tg.send_message(p).await {
                eprintln!("dev-tracker: {}: report: {e}", head.delivery_key());
            }
        }
        if !test {
            for f in changed.iter().filter_map(|&i| files.get(i)) {
                db.record_delivery(
                    &f.delivery_key(),
                    f.format.name(),
                    &f.members_hash,
                    &f.members,
                    t,
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        sent += 1;
        // Bot API: about one message per second per chat
        tokio::time::sleep(Duration::from_millis(1_100)).await;
    }
    Ok(sent)
}

/// Telegram alerts of the daemon (`[alerts]`): start / stop, an unclean
/// previous stop, and incidents — a key failing for `after_minutes` is alerted
/// once and its recovery once, never every failing pass.
struct Alerts<'a> {
    db: &'a DevDb,
    tg: Option<Telegram>,
    enabled: bool,
    after_secs: i64,
}

/// Untrusted error text for a chat message: no control characters, bounded.
fn alert_text(e: &str) -> String {
    e.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect()
}

fn duration_text(secs: i64) -> String {
    let m = secs.max(0).div_euclid(60);
    if m < 120 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m.div_euclid(60), m.rem_euclid(60))
    }
}

impl Alerts<'_> {
    async fn send(&self, text: &str) {
        eprintln!("dev-tracker: alert: {text}");
        if !self.enabled {
            return;
        }
        if let Some(tg) = &self.tg
            && let Err(e) = tg.send_message(text).await
        {
            eprintln!("dev-tracker: alert not delivered: {e}");
        }
    }

    async fn failed(&self, key: &str, error: &str) {
        let t = now();
        match self.db.record_failure(key, &alert_text(error), t).await {
            Ok(inc) if inc.alerted_at.is_none() && t - inc.first_failed_at >= self.after_secs => {
                self.send(&format!(
                    "🔴 dev-tracker: {key} failing for {} ({} failed pass(es)); needs a look.\nlast error: {}",
                    duration_text(t - inc.first_failed_at),
                    inc.failures,
                    inc.last_error
                ))
                .await;
                if let Err(e) = self.db.mark_alerted(key, t).await {
                    eprintln!("dev-tracker: incident {key}: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("dev-tracker: incident {key}: {e}"),
        }
    }

    async fn ok(&self, key: &str) {
        match self.db.resolve_incident(key).await {
            Ok(Some(inc)) if inc.alerted_at.is_some() => {
                self.send(&format!(
                    "🟢 dev-tracker: {key} recovered after {}.",
                    duration_text(now() - inc.first_failed_at)
                ))
                .await;
            }
            Ok(_) => {}
            Err(e) => eprintln!("dev-tracker: incident {key}: {e}"),
        }
    }
}

async fn daemon(db: &DevDb, config: &str) -> Result<(), String> {
    let mut cfg = read_config(config)?;
    let mut next_ingest = 0i64;
    let mut next_derive = 0i64;
    let mut shutdown = Box::pin(shutdown_signal());
    eprintln!("dev-tracker: daemon started ({config})");
    let alerts = |cfg: &DevTrackerConfig| Alerts {
        db,
        tg: telegram_from_env(),
        enabled: cfg.alerts.enabled,
        after_secs: minutes(cfg.alerts.after_minutes),
    };
    {
        let a = alerts(&cfg);
        let started = now();
        match db.daemon_started(started).await {
            Ok(Some(prev)) if !prev.clean_stop => {
                a.send(&format!(
                    "🟠 dev-tracker restarted after an unclean stop (crash, kill or power loss): last sign of life {} ago.",
                    duration_text(started - prev.last_beat)
                ))
                .await;
            }
            Ok(_) => a.send("🟢 dev-tracker started.").await,
            Err(e) => eprintln!("dev-tracker: daemon state: {e}"),
        }
    }
    // liveness beat every minute (a crash leaves the last beat behind)
    let beat_db = db.clone();
    let beat = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = beat_db.daemon_beat(now()).await {
                eprintln!("dev-tracker: liveness beat: {e}");
            }
        }
    });
    let reason = loop {
        match read_config(config) {
            Ok(c) => cfg = c,
            Err(e) => eprintln!("dev-tracker: config not reloaded, keeping the last good one: {e}"),
        }
        let s = cfg.schedule.clone();
        let a = alerts(&cfg);
        // one cycle; a stop request interrupts it (writes are idempotent and
        // cursors only move after facts are stored, so nothing is lost)
        let cycle = async {
            if now() >= next_ingest {
                let opts = IngestOpts::from_config(&cfg);
                for chain in &s.chains {
                    let key = format!("ingest:{chain}");
                    match ingest_chain(db, chain, &opts).await {
                        Ok(degraded) => {
                            a.ok(&key).await;
                            if chain == "solana" {
                                match degraded.iter().find(|(k, _)| k == "provider:helius") {
                                    Some((k, e)) => a.failed(k, e).await,
                                    None => a.ok("provider:helius").await,
                                }
                            }
                        }
                        Err(e) => {
                            // one chain's failure never stops the others
                            eprintln!("dev-tracker: {chain}: pass failed: {e}");
                            a.failed(&key, &e).await;
                        }
                    }
                }
                next_ingest = now().saturating_add(minutes(s.ingest_every_minutes));
            }
            if now() >= next_derive {
                let deliveries = match db.deliveries().await {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("dev-tracker: deliveries not readable: {e}");
                        Vec::new()
                    }
                };
                match derive(db, &cfg, s.since_days, false, &watch_set(&deliveries)).await {
                    Ok(v) => {
                        a.ok("derive").await;
                        let tg = if cfg.delivery.telegram {
                            telegram_from_env()
                        } else {
                            None
                        };
                        if cfg.delivery.telegram && tg.is_none() {
                            eprintln!(
                                "dev-tracker: telegram skipped ({TELEGRAM_TOKEN_ENV} / {TELEGRAM_CHAT_ENV} not set)"
                            );
                        }
                        match export(db, &cfg, &v, &deliveries, tg.as_ref(), false).await {
                            Ok(n) => {
                                a.ok("export").await;
                                eprintln!("dev-tracker: export: {n} changed list(s) sent");
                            }
                            Err(e) => {
                                eprintln!("dev-tracker: export failed: {e}");
                                a.failed("export", &e).await;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("dev-tracker: derive failed: {e}");
                        a.failed("derive", &e).await;
                    }
                }
                next_derive = now().saturating_add(minutes(s.derive_every_minutes));
            }
        };
        tokio::select! {
            () = cycle => {}
            () = &mut shutdown => break "stop requested during a pass",
        }
        let wait = next_ingest.min(next_derive).saturating_sub(now()).max(1);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(u64::try_from(wait).unwrap_or(60))) => {}
            () = &mut shutdown => break "stop requested",
        }
    };
    beat.abort();
    eprintln!("dev-tracker: shutdown ({reason}), exiting");
    if let Err(e) = db.daemon_stopped(now()).await {
        eprintln!("dev-tracker: daemon state: {e}");
    }
    alerts(&cfg)
        .send(&format!("⏹ dev-tracker stopped ({reason})."))
        .await;
    Ok(())
}

fn minutes(m: u64) -> i64 {
    i64::try_from(m.saturating_mul(60)).unwrap_or(i64::MAX)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match term {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
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
            max_histories,
            ath_requests,
            solana_max_pages,
            solana_settle_secs,
            solana_via_fallback,
        } => {
            db.migrate().await.map_err(|e| e.to_string())?;
            let opts = IngestOpts {
                start_hours_back,
                confirmations,
                max_requests,
                max_new_creators,
                max_signer_lookups,
                max_histories,
                ath_requests,
                solana_max_pages,
                solana_settle_secs,
                solana_via_fallback,
            };
            for chain in chains.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                for (key, e) in ingest_chain(&db, chain, &opts).await? {
                    eprintln!("dev-tracker: degraded ({key}): {e}");
                }
            }
        }
        Cmd::Derive {
            config,
            since_days,
            all,
        } => {
            let cfg = read_config(&config)?;
            let verdicts = derive(&db, &cfg, since_days, all, &BTreeSet::new()).await?;
            for v in &verdicts {
                if !all && v.categories.is_empty() {
                    continue;
                }
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
        }
        Cmd::Export { config, send, test } => {
            db.migrate().await.map_err(|e| e.to_string())?;
            let cfg = read_config(&config)?;
            let deliveries = db.deliveries().await.map_err(|e| e.to_string())?;
            let verdicts = derive(
                &db,
                &cfg,
                cfg.schedule.since_days,
                false,
                &watch_set(&deliveries),
            )
            .await?;
            let tg = if send || test {
                Some(telegram_from_env().ok_or_else(|| {
                    format!("--send/--test need {TELEGRAM_TOKEN_ENV} and {TELEGRAM_CHAT_ENV}")
                })?)
            } else {
                None
            };
            let n = export(&db, &cfg, &verdicts, &deliveries, tg.as_ref(), test).await?;
            eprintln!(
                "dev-tracker: files in {:?}; {n} changed list(s) sent",
                cfg.delivery.out_dir
            );
        }
        Cmd::Backfill(a) => {
            db.migrate().await.map_err(|e| e.to_string())?;
            backfill::run_backfill(&db, &a).await?;
        }
        Cmd::Run { config } => {
            db.migrate().await.map_err(|e| e.to_string())?;
            daemon(&db, &config).await?;
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
