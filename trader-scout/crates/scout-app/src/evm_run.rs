//! Shared EVM run setup of the three CLIs (ADR-020 step 2): which chain a
//! run is about, endpoint construction (secret-safe), the chain identity
//! preflight and the live `decimals()` check of pinned quote tokens.
//!
//! Exit-code mapping used by the binaries: [`EvmSetupError::Usage`] = 2,
//! every other variant = 4.

use std::sync::Arc;

use alloy_primitives::Address;
use scout_core::{ChainFamily, ChainKey, NetworkId};
use scout_providers::{
    AlchemyConfig, AlchemyTransfersSource, BlockscoutApiKey, BlockscoutEvmConfig,
    BlockscoutEvmSource, EvmHistoryScanner, EvmRpcClient, EvmSourceError, NativeLegPolicy,
    NativeLegResolver, ScanLimits, WalletIndexer,
};
use scout_rpc::{HalvingHook, RateLimiter, RateLimiterStats, RpcClient, RpcEndpoint};
use scout_sdk::engine::{
    AnalysisWindow, EvmExtractionConfig, EvmRunInfo, EvmStatsSources, SolanaWalletStatsReport,
    chain_has_verified_venue, run_evm_wallet_stats,
};
use scout_sdk::evm::EvmChainProfile;
use tokio_util::sync::CancellationToken;

use crate::evm_source::{
    ALCHEMY_DEFAULT_CU_PER_SEC, EvmNetOptions, EvmRpcUrl, KEYED_RPC_RPS, PUBLIC_RPC_RPS,
    ROBINHOOD_PUBLIC_RPC, evm_rpc_url_from_env, logs_rpc_env_name,
};

/// Which chain family an input set belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFamily {
    Solana,
    /// One EVM chain (all inputs on it).
    Evm(ChainKey),
}

/// Decide the family of a run from every input's chain. Mixed families or
/// several EVM chains are unsupported (exit 2): a run never silently drops
/// inputs, it refuses.
///
/// # Errors
/// A usage message naming the problem.
pub fn run_family<'a>(
    chains: impl IntoIterator<Item = &'a ChainKey>,
    tool: &str,
) -> Result<RunFamily, String> {
    let mut solana = 0usize;
    let mut evm: Vec<ChainKey> = Vec::new();
    for c in chains {
        match c.family {
            ChainFamily::Solana => solana += 1,
            ChainFamily::Evm => {
                if !evm.iter().any(|e| e.network_id == c.network_id) {
                    evm.push(c.clone());
                }
            }
        }
    }
    match (solana, evm.len()) {
        (_, 0) => Ok(RunFamily::Solana),
        (0, 1) => Ok(RunFamily::Evm(evm.remove(0))),
        (n, _) if n > 0 => Err(format!(
            "{tool}: mixed Solana and EVM inputs are not supported in one run; run each chain \
             family separately (no input was scanned)"
        )),
        _ => Err(format!(
            "{tool}: inputs span several EVM chains; run one chain per invocation \
             (no input was scanned)"
        )),
    }
}

/// Failure of the EVM run setup.
#[derive(Debug, thiserror::Error)]
pub enum EvmSetupError {
    /// Bad invocation (exit 2).
    #[error("{0}")]
    Usage(String),
    /// Missing configuration / unverified chain or venue / infrastructure (exit 4).
    #[error("{0}")]
    Config(String),
}

/// Everything a run needs after a successful setup.
#[derive(Debug)]
pub struct EvmSetup {
    pub profile: EvmChainProfile,
    pub rpc: EvmRpcClient,
    /// Verified chain key (genesis checked).
    pub chain: ChainKey,
    pub url: EvmRpcUrl,
    pub cfg: EvmExtractionConfig,
    pub info: EvmRunInfo,
    /// Stderr warnings (never contain the URL).
    pub warnings: Vec<String>,
    /// Block span the endpoint that serves `eth_getLogs` is known to cap
    /// (`None` = no cap known). Drives the up-front cost estimate.
    pub logs_span_cap: Option<u64>,
    /// Token scans should list transfers via `alchemy_getAssetTransfers`
    /// (keyed Alchemy with a capped `eth_getLogs`, no logs endpoint).
    pub token_listing_via_transfers: bool,
    /// The probed `eth_getLogs` span cap of the keyed RPC when token scans
    /// were switched to the transfers listing (used if a caller forces
    /// `eth_getLogs` anyway, for the up-front cost estimate).
    pub capped_logs_span: Option<u64>,
    /// Endpoint classes, never URLs (echoed in scope / `run_meta`).
    pub logs_source: String,
    pub state_source: String,
    /// Secret parts of a separate logs URL, when one is used.
    logs_secrets: Vec<String>,
    /// `(label, limiter)` per endpoint, for the end-of-run report.
    limiters: Vec<(String, Arc<RateLimiter>)>,
}

impl EvmSetup {
    /// Every secret substring of every endpoint of the run.
    #[must_use]
    pub fn secrets(&self) -> Vec<String> {
        let mut v = self.url.secrets();
        v.extend(self.logs_secrets.iter().cloned());
        v
    }

    /// Print a [`progress_line`] through `notice` every [`PROGRESS_INTERVAL`]
    /// until the guard is dropped. `extra` adds requests made outside the
    /// main client (explorer). Must be called inside a Tokio runtime.
    #[must_use]
    pub fn spawn_progress(
        &self,
        notice: &LimiterNotice,
        extra: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    ) -> ProgressGuard {
        let rpc = self.rpc.clone();
        let limiters = self.limiters.clone();
        let notice = Arc::clone(notice);
        let started = tokio::time::Instant::now();
        ProgressGuard(tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(started + PROGRESS_INTERVAL, PROGRESS_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let requests = rpc.total_requests_made() + extra.as_ref().map_or(0, |f| f());
                notice(progress_line(started.elapsed(), requests, &limiters));
            }
        }))
    }

    /// One stderr line per endpoint: initial/final rate, halvings, waits.
    #[must_use]
    pub fn rate_limit_report(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .limiters
            .iter()
            .map(|(label, l)| rate_limit_line(label, &l.stats()))
            .collect();
        let (retries, failed) = self.rpc.rate_limit_counts();
        if retries > 0 || failed > 0 {
            lines.push(format!(
                "429 handling: {retries} retr(ies) after 429 (up to {} extra attempts per call, \
                 backoff capped at 30 s, all counted in --max-requests), {failed} call(s) \
                 failed rate limited after that budget",
                scout_rpc::RATE_LIMIT_EXTRA_RETRIES
            ));
        }
        lines
    }
}

/// `3.250` for 3250 milli-units.
fn fmt_rate(milli: u64) -> String {
    format!(
        "{}.{:03}",
        milli.checked_div(1_000).unwrap_or(0),
        milli % 1_000
    )
}

/// Stderr text of a limiter rate change: a halving (`new < old`) or the
/// restore of the initial rate after quiet time (`new > old`).
fn rate_change_text(label: &str, cause: &str, old: u64, new: u64) -> String {
    if new > old {
        format!(
            "{label}: no 429 for a while; client rate restored {}/s -> {}/s",
            fmt_rate(old),
            fmt_rate(new)
        )
    } else {
        format!(
            "{label}: {cause}; client rate halved {}/s -> {}/s (grows back by 1/{} of the \
             initial rate every {} s without 429)",
            fmt_rate(old),
            fmt_rate(new),
            scout_rpc::RECOVER_DIVISOR,
            scout_rpc::RECOVER_STEP.as_secs()
        )
    }
}

/// Human line of one limiter's counters (never contains a URL).
#[must_use]
pub fn rate_limit_line(label: &str, st: &RateLimiterStats) -> String {
    format!(
        "{label}: rate {}/s -> {}/s ({} halving(s) after 429 without Retry-After, {} recovery \
         step(s)), {} request(s) paced, {} ms spent waiting for tokens",
        fmt_rate(st.initial_rate_milli),
        fmt_rate(st.rate_milli),
        st.halvings,
        st.recoveries,
        st.acquired,
        st.waited_ms
    )
}

/// Interval of the stderr progress heartbeat of a long EVM run.
pub const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Aborts the progress heartbeat task when dropped.
#[derive(Debug)]
pub struct ProgressGuard(tokio::task::JoinHandle<()>);

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// URL-free heartbeat line: requests so far and the current rate per
/// endpoint, so a slow run (paced after 429s) never looks hung.
fn progress_line(
    elapsed: std::time::Duration,
    requests: u64,
    limiters: &[(String, Arc<RateLimiter>)],
) -> String {
    let rates: Vec<String> = limiters
        .iter()
        .map(|(label, l)| format!("{label} {}/s", fmt_rate(l.stats().rate_milli)))
        .collect();
    format!(
        "progress: {} s elapsed, {requests} request(s) made; current rate: {}",
        elapsed.as_secs(),
        rates.join(", ")
    )
}

/// Default ceiling of `eth_getLogs` requests an up-front estimate may
/// predict when the user set no `--max-requests`.
pub const DEFAULT_LOG_SCAN_REQUEST_LIMIT: u64 = 2_000;

/// Refuse (exit 4) a token scan that a range-capped logs endpoint would turn
/// into thousands of requests: `tokens x ceil(blocks / span)` against the
/// user's `--max-requests` (or [`DEFAULT_LOG_SCAN_REQUEST_LIMIT`]). The
/// estimate is a lower bound (result-cap splits add more).
///
/// # Errors
/// A URL-free explanation with the ways out.
pub fn check_log_scan_feasible(
    span_cap: Option<u64>,
    blocks: Option<(u64, u64)>,
    tokens: usize,
    max_requests: Option<u64>,
    logs_env_hint: &str,
) -> Result<(), String> {
    let (Some(span), Some((from, to))) = (span_cap.filter(|s| *s > 0), blocks) else {
        return Ok(());
    };
    let n_blocks = to.saturating_sub(from).saturating_add(1);
    let per_token = n_blocks.div_ceil(span);
    let estimate = per_token.saturating_mul(u64::try_from(tokens).unwrap_or(u64::MAX));
    let limit = max_requests.unwrap_or(DEFAULT_LOG_SCAN_REQUEST_LIMIT);
    if estimate <= limit {
        return Ok(());
    }
    Err(format!(
        "the endpoint serving eth_getLogs caps the block range at {span}: scanning {n_blocks} \
         block(s) for {tokens} token(s) needs at least {estimate} requests, over the limit of \
         {limit}{}. Nothing was scanned. Options: set {logs_env_hint} to an endpoint without \
         that cap, narrow the window, or raise --max-requests knowingly{}",
        if max_requests.is_some() {
            " (--max-requests)"
        } else {
            " (default; pass --max-requests to override)"
        },
        if logs_env_hint.contains("BSC") {
            " (BSC blocks come every ~0.45 s: one hour is ~8,000 blocks, so a 10-block cap \
             needs ~800 requests per token; a logs endpoint without the cap is recommended)"
        } else {
            ""
        }
    ))
}

/// Per-method cost table of a limiter.
pub type CostFn = fn(&str) -> u64;

/// The token-bucket limiter (and optional per-method cost table) the CLIs
/// build for one endpoint from `--rpc-rps` / `--rpc-cu-per-sec`; `public`
/// caps a keyless endpoint at [`PUBLIC_RPC_RPS`]. Halvings are reported
/// through `on_halve` (URL-free text). Shared with `evm-capture`.
pub fn make_limiter(
    net: &EvmNetOptions,
    public: bool,
    alchemy: bool,
    label: &str,
    on_halve: &LimiterNotice,
) -> (Arc<RateLimiter>, Option<CostFn>) {
    let hook_label = label.to_string();
    let notify = Arc::clone(on_halve);
    let hook: HalvingHook = Arc::new(move |old, new| {
        notify(rate_change_text(
            &hook_label,
            "provider answered 429 without Retry-After",
            old,
            new,
        ));
    });
    let mut cu_opt = net.rpc_cu_per_sec;
    if cu_opt.is_none() && net.rpc_rps.is_none() && alchemy && !public {
        // Alchemy meters compute units per second, not requests: a flat
        // requests/s limiter is the wrong default there.
        on_halve(format!(
            "limiter: alchemy detected \u{2192} cu-per-sec {ALCHEMY_DEFAULT_CU_PER_SEC} \
             (override with --rpc-rps/--rpc-cu-per-sec)"
        ));
        cu_opt = Some(ALCHEMY_DEFAULT_CU_PER_SEC);
    }
    if let Some(cu) = cu_opt {
        let cu = u64::from(cu.max(1));
        return (
            Arc::new(RateLimiter::new(cu, cu).with_halving_hook(hook)),
            Some(scout_providers::approx_method_cu),
        );
    }
    let rps = match (net.rpc_rps, public) {
        (Some(r), false) => r,
        (Some(r), true) => r.min(PUBLIC_RPC_RPS),
        (None, false) => KEYED_RPC_RPS,
        (None, true) => PUBLIC_RPC_RPS,
    }
    .max(1);
    let rps = u64::from(rps);
    (
        Arc::new(RateLimiter::new(rps, rps).with_halving_hook(hook)),
        None,
    )
}

/// Stderr reporter of limiter halvings (the CLIs pass `eprintln`).
pub type LimiterNotice = Arc<dyn Fn(String) + Send + Sync>;

/// Build the endpoint, preflight the chain and check the quote decimals.
///
/// `allow_unverified`: a chain without any `FixtureVerified` venue is refused
/// (exit 4, "not verified yet") unless the flag is given. Base is enabled since
/// ADR-020 amendment 4 and BSC since amendment 5 (Pancake v2, Uniswap v2/v3
/// verified; Pancake v3 and four.meme stay IdlOnly = lower bound, native BNB
/// is the only quote asset until USDT/USDC are verified).
///
/// # Errors
/// [`EvmSetupError`]; every message is URL-free.
#[allow(clippy::too_many_arguments)]
pub async fn setup_evm(
    chain: &ChainKey,
    allow_unverified: bool,
    max_requests: Option<u64>,
    concurrency: usize,
    net: &EvmNetOptions,
    notice: &LimiterNotice,
    needs_logs: bool,
    env: impl Fn(&str) -> Option<String>,
) -> Result<EvmSetup, EvmSetupError> {
    let NetworkId::EvmChainId(id) = chain.network_id else {
        return Err(EvmSetupError::Usage("not an EVM chain".to_string()));
    };
    let profile = EvmChainProfile::by_chain_id(id)
        .ok_or_else(|| EvmSetupError::Usage(format!("no EVM profile for chain id {id}")))?;
    if !chain_has_verified_venue(profile.chain_id) && !allow_unverified {
        return Err(EvmSetupError::Config(format!(
            "chain `{}`: venue/source not verified yet (no FixtureVerified venue); pass \
             --allow-unverified-chain to run anyway (every trade is then IdlOnly and the run is \
             partial)",
            profile.name
        )));
    }
    let (url, warn) = evm_rpc_url_from_env(profile.name, &env).map_err(EvmSetupError::Config)?;
    let mut warnings: Vec<String> = warn.into_iter().collect();
    let main_public = !url.from_env;
    let main_label = if main_public {
        format!("public {} rpc", profile.name)
    } else {
        "keyed rpc".to_string()
    };
    let (main_limiter, main_cost) =
        make_limiter(net, main_public, url.is_alchemy(), &main_label, notice);
    let rpc = RpcClient::new(RpcEndpoint::new(url.expose_for_transport()), 30_000, 3)
        .map_err(|e| EvmSetupError::Config(url.redact(&e.to_string())))?
        .with_max_total_requests(max_requests)
        .with_rate_limiter(Arc::clone(&main_limiter), main_cost);
    let cfg_rpc = scout_providers::EvmRpcConfig {
        concurrency,
        ..scout_providers::EvmRpcConfig::default()
    };
    let mut rpc = EvmRpcClient::with_config(rpc, profile, cfg_rpc);
    let mut limiters = vec![(main_label.clone(), main_limiter)];
    let mut logs_secrets: Vec<String> = Vec::new();
    let mut logs_span_cap: Option<u64> = None;
    let mut token_listing_via_transfers = false;
    let mut capped_logs_span: Option<u64> = None;
    let state_source = if main_public {
        format!("public {} rpc (keyless)", profile.name)
    } else {
        format!(
            "keyed rpc ({})",
            crate::evm_source::rpc_env_name(profile.name).unwrap_or("SCOUT_<CHAIN>_RPC_URL")
        )
    };
    let mut logs_source = if needs_logs {
        state_source.clone()
    } else {
        "not used".to_string()
    };
    let redact_all = |t: &str, extra: &[String]| {
        let mut o = url.redact(t);
        for sec in extra {
            if sec.len() >= 4 {
                o = o.replace(sec.as_str(), "<redacted>");
            }
        }
        o
    };
    let logs_var = logs_rpc_env_name(profile.name);
    let logs_url = logs_var
        .and_then(&env)
        .filter(|v| !v.trim().is_empty())
        .map(|v| EvmRpcUrl::new(v.trim().to_string(), true));
    // Per-method routing of eth_getLogs.
    let logs_target: Option<(EvmRpcUrl, bool, String)> = if !needs_logs {
        // Wallet scans list through the indexer and never call eth_getLogs:
        // no routing, no capability probe.
        None
    } else if let Some(u) = logs_url {
        let label = format!(
            "separate logs endpoint ({})",
            logs_var.unwrap_or("SCOUT_<CHAIN>_LOGS_RPC_URL")
        );
        Some((u, false, label))
    } else if url.from_env {
        // Detect a range-capped main endpoint (Alchemy free: 10 blocks).
        match rpc.probe_logs_span().await {
            Ok(Some(span)) if span <= 10 && profile.name == "robinhood" => {
                warnings.push(format!(
                    "the keyed RPC caps eth_getLogs at {span} block(s) per request: using the \
                     keyless public Robinhood RPC for eth_getLogs ONLY (receipts, state, \
                     balances stay on the keyed RPC); set {} to choose another logs endpoint",
                    logs_var.unwrap_or("SCOUT_ROBINHOOD_LOGS_RPC_URL")
                ));
                let public = env(PUBLIC_RPC_OVERRIDE_ENV)
                    .filter(|u| !u.trim().is_empty())
                    .unwrap_or_else(|| ROBINHOOD_PUBLIC_RPC.to_string());
                // The test override is not the real public endpoint: no 5/s cap.
                let real_public = env(PUBLIC_RPC_OVERRIDE_ENV).is_none_or(|u| u.trim().is_empty());
                Some((
                    EvmRpcUrl::new(public, false),
                    real_public,
                    format!(
                        "public {} rpc (auto: keyed rpc caps eth_getLogs at {span} block(s))",
                        profile.name
                    ),
                ))
            }
            Ok(Some(span)) if span <= 10 => {
                // Alchemy free caps eth_getLogs at 10 blocks and no keyless
                // archive logs endpoint exists for this chain (the BSC
                // publicnode RPC asks a personal token for old blocks):
                // token scans list the token's transfers through
                // alchemy_getAssetTransfers instead.
                warnings.push(format!(
                    "the keyed RPC caps eth_getLogs at {span} block(s) per request: token scans \
                     list transfers via alchemy_getAssetTransfers (contract filter) instead of \
                     eth_getLogs; set {} to an uncapped logs endpoint to use eth_getLogs",
                    logs_var.unwrap_or("SCOUT_<CHAIN>_LOGS_RPC_URL")
                ));
                token_listing_via_transfers = true;
                capped_logs_span = Some(span);
                logs_source = format!(
                    "alchemy_getAssetTransfers token listing on the keyed rpc (eth_getLogs capped \
                     at {span} block(s))"
                );
                None
            }
            Ok(Some(span)) => {
                logs_span_cap = Some(span);
                warnings.push(format!(
                    "the keyed RPC caps eth_getLogs at {span} block(s) per request and no \
                     alternative logs endpoint is known for {}: token scans over long windows \
                     are refused up front; set {} to an uncapped endpoint",
                    profile.name,
                    logs_var.unwrap_or("SCOUT_<CHAIN>_LOGS_RPC_URL")
                ));
                logs_source = format!("{state_source}, capped at {span} block(s) per request");
                None
            }
            Ok(None) => None,
            Err(e) => {
                warnings.push(format!(
                    "eth_getLogs range probe failed ({}); log range limits are discovered \
                     adaptively",
                    redact_all(&e.to_string(), &[])
                ));
                None
            }
        }
    } else {
        None
    };
    // A known span of the logs endpoint: the BSC public fallback, or the
    // user's SCOUT_<CHAIN>_LOGS_MAX_SPAN for a custom endpoint.
    let span_env = crate::evm_source::logs_max_span_env_name(profile.name)
        .and_then(&env)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0);
    if let Some(span) = span_env.filter(|_| logs_target.is_some()) {
        rpc = rpc.with_logs_max_span(span);
        logs_span_cap = Some(span);
    }
    if let Some((lu, public, label)) = logs_target {
        let (l, cost) = make_limiter(net, public, lu.is_alchemy(), &label, notice);
        let logs_rpc = RpcClient::new(RpcEndpoint::new(lu.expose_for_transport()), 30_000, 3)
            .map_err(|e| EvmSetupError::Config(url.redact(&lu.redact(&e.to_string()))))?
            .with_rate_limiter(Arc::clone(&l), cost);
        rpc = rpc.with_logs_endpoint(logs_rpc);
        if lu.from_env {
            logs_secrets = lu.secrets();
        }
        limiters.push((format!("logs: {label}"), l));
        logs_source = label;
    }
    let redact_all = |t: &str| redact_all(t, &logs_secrets);
    let verified = rpc.preflight().await.map_err(|e| {
        EvmSetupError::Config(format!(
            "chain identity preflight failed: {}",
            redact_all(&e.to_string())
        ))
    })?;
    let cfg = EvmExtractionConfig::for_profile(profile);
    let mut info = EvmRunInfo::from_config(&cfg, "explorer txlist / eth_getLogs + RPC receipts");
    for q in &mut info.quote_assets {
        let Some(spec) = profile.quote_assets.iter().find(|s| s.symbol == q.symbol) else {
            continue;
        };
        q.decimals_check = match rpc.erc20_decimals(spec.address).await {
            Ok(d) if d == spec.decimals => format!("verified live: {d}"),
            Ok(d) => {
                return Err(EvmSetupError::Config(format!(
                    "quote token {} decimals mismatch: pinned {}, chain reports {d}; refusing to \
                     scale amounts",
                    spec.symbol, spec.decimals
                )));
            }
            Err(EvmSourceError::Provider(e)) => {
                format!("not checked: {}", redact_all(&e.to_string()))
            }
            Err(e) => format!("not checked: {}", redact_all(&e.to_string())),
        };
    }
    if main_public || logs_source.starts_with("public") {
        warnings.push("public endpoint: eth_getLogs result cap 10,000".to_string());
    }
    info.logs_source = logs_source.clone();
    info.state_source = state_source.clone();
    info.rate_limits = limiters
        .iter()
        .map(|(l, lim)| rate_limit_line(l, &lim.stats()))
        .collect();
    Ok(EvmSetup {
        profile,
        rpc,
        chain: verified,
        url,
        cfg,
        info,
        warnings,
        logs_span_cap,
        token_listing_via_transfers,
        capped_logs_span,
        logs_source,
        state_source,
        logs_secrets,
        limiters,
    })
}

/// Test-only: replaces the keyless public Robinhood RPC used for the
/// automatic `eth_getLogs` routing (offline wiremock tests).
pub const PUBLIC_RPC_OVERRIDE_ENV: &str = "SCOUT_EVM_PUBLIC_RPC_URL";

/// Test-only: replaces the Blockscout base URL (offline wiremock tests).
pub const BLOCKSCOUT_URL_OVERRIDE_ENV: &str = "SCOUT_EVM_BLOCKSCOUT_URL";

/// Failure of an EVM stats collection, with its exit-code class.
#[derive(Debug, thiserror::Error)]
pub enum EvmStatsError {
    /// Exit 2.
    #[error("{0}")]
    Usage(String),
    /// Exit 4 (configuration, identity, infrastructure).
    #[error("{0}")]
    Config(String),
    /// Exit 3 (user-chosen request budget spent before anything was observed).
    #[error("{0}")]
    Budget(String),
}

/// A finished collection: the card report plus the facts the binaries print.
#[derive(Debug)]
pub struct EvmStatsRun {
    pub report: SolanaWalletStatsReport,
    /// RPC + explorer HTTP attempts (retries included).
    pub requests_made: u64,
    /// Strings to scrub from every output (RPC URL parts, explorer key).
    pub secrets: Vec<String>,
    pub warnings: Vec<String>,
    /// Final per-endpoint rate-limiter report lines (stderr).
    pub rate_limits: Vec<String>,
    /// Logical RPC calls of the run by method (retries not counted).
    pub rpc_calls_by_method: std::collections::BTreeMap<String, u64>,
    /// ADR-019 EVM amendment: the open-position valuation run (`None` with
    /// `--no-valuation`).
    pub valuation: Option<scout_sdk::engine::EvmOpenValuationRun>,
}

impl EvmStatsRun {
    /// `text` with every secret replaced by `<redacted>`.
    #[must_use]
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for s in &self.secrets {
            if s.len() >= 4 {
                out = out.replace(s.as_str(), "<redacted>");
            }
        }
        out
    }
}

/// The whole EVM wallet-stats acquisition shared by `wallet-stats` and
/// `wallet-rank`: setup + preflight + decimals check, explorer, scanner,
/// native-leg resolver (trace -> archive balance diff -> Unknown, detected
/// once) and [`run_evm_wallet_stats`].
///
/// # Errors
/// [`EvmStatsError`] (messages are URL- and key-free).
#[allow(clippy::too_many_arguments)]
pub async fn collect_evm_stats(
    tool: &str,
    chain: &ChainKey,
    wallets: &[Address],
    allow_unverified: bool,
    max_requests: Option<u64>,
    concurrency: usize,
    window: &AnalysisWindow,
    valuation: bool,
    net: &EvmNetOptions,
    notice: &LimiterNotice,
    env: impl Fn(&str) -> Option<String>,
) -> Result<EvmStatsRun, EvmStatsError> {
    let bs_key = env(crate::evm_source::BLOCKSCOUT_KEY_ENV).filter(|k| !k.trim().is_empty());
    let rpc_var =
        crate::evm_source::rpc_env_name(chain_name(chain)).unwrap_or("SCOUT_<CHAIN>_RPC_URL");
    let keyed_rpc = env(rpc_var).is_some_and(|v| !v.trim().is_empty());
    if bs_key.is_none() && !keyed_rpc {
        // No indexer can exist (Alchemy transfers need a keyed RPC): refuse
        // before any network request.
        return Err(no_indexer_error(tool, wallets.len(), rpc_var));
    }
    let setup = setup_evm(
        chain,
        allow_unverified,
        max_requests,
        concurrency.max(1).saturating_mul(2),
        net,
        notice,
        false,
        &env,
    )
    .await
    .map_err(|e| match e {
        EvmSetupError::Usage(m) => EvmStatsError::Usage(format!("{tool}: {m}")),
        EvmSetupError::Config(m) => EvmStatsError::Config(format!(
            "{tool}: {}",
            bs_key
                .as_deref()
                .map_or(m.clone(), |k| m.replace(k, "<redacted>"))
        )),
    })?;
    let mut secrets = setup.secrets();
    if let Some(k) = &bs_key {
        secrets.push(k.clone());
    }
    let scrub = |t: &str| {
        let mut o = t.to_string();
        for s in &secrets {
            if s.len() >= 4 {
                o = o.replace(s.as_str(), "<redacted>");
            }
        }
        o
    };
    // Indexer selection (ADR-020 amendment 4): Blockscout where it serves the
    // chain (Robinhood, with a key), else Alchemy transfers if the keyed RPC
    // answers `alchemy_getAssetTransfers`, else Blockscout when a key is set
    // (the chain may be covered by the plan), else exit 4.
    let mut bs_limiter: Option<Arc<RateLimiter>> = None;
    let bs_label = "explorer (blockscout)".to_string();
    let mut indexer_note: Option<String> = None;
    let mut chosen: Option<WalletIndexer> = None;
    if setup.profile.name == "robinhood"
        && let Some(k) = &bs_key
    {
        let (src, lim) = build_blockscout(
            &setup,
            k,
            max_requests,
            net,
            notice,
            &bs_label,
            env(BLOCKSCOUT_URL_OVERRIDE_ENV),
        )
        .map_err(|e| {
            EvmStatsError::Config(format!("{tool}: explorer unavailable: {}", scrub(&e)))
        })?;
        chosen = Some(WalletIndexer::Blockscout(src));
        bs_limiter = Some(lim);
    }
    if chosen.is_none() && setup.url.from_env {
        let alchemy = AlchemyTransfersSource::new(setup.rpc.clone(), AlchemyConfig::default());
        match alchemy.probe().await {
            Ok(true) => {
                notice(format!(
                    "{tool}: wallet history indexer: alchemy_getAssetTransfers on the keyed {} \
                     RPC (internal transfers are detected per chain on the first listing)",
                    setup.profile.name
                ));
                chosen = Some(alchemy.into());
            }
            Ok(false) => {
                indexer_note = Some(format!(
                    "the keyed RPC does not answer alchemy_getAssetTransfers for {}",
                    setup.profile.name
                ));
            }
            Err(e) if e.is_budget_exhausted() => {
                return Err(EvmStatsError::Budget(format!(
                    "{tool}: request budget exhausted before the wallet indexer probe \
                     completed (IncompleteCoverage)"
                )));
            }
            Err(e) => {
                return Err(EvmStatsError::Config(format!(
                    "{tool}: wallet indexer probe failed: {}",
                    scrub(&e.to_string())
                )));
            }
        }
    }
    if chosen.is_none()
        && let Some(k) = &bs_key
    {
        let (src, lim) = build_blockscout(
            &setup,
            k,
            max_requests,
            net,
            notice,
            &bs_label,
            env(BLOCKSCOUT_URL_OVERRIDE_ENV),
        )
        .map_err(|e| {
            EvmStatsError::Config(format!("{tool}: explorer unavailable: {}", scrub(&e)))
        })?;
        chosen = Some(WalletIndexer::Blockscout(src));
        bs_limiter = Some(lim);
    }
    let Some(explorer) = chosen else {
        let mut e = no_indexer_error(tool, wallets.len(), rpc_var);
        if let (EvmStatsError::Config(m), Some(n)) = (&mut e, indexer_note) {
            m.push_str(&format!(" [{n}]"));
        }
        return Err(e);
    };
    let scanner = EvmHistoryScanner::new(
        setup.rpc.clone(),
        setup.chain.clone(),
        ScanLimits::default(),
    );
    let resolver = NativeLegResolver::new(setup.rpc.clone(), NativeLegPolicy::default());
    let explorer_requests = bs_limiter.clone().map(|l| {
        let f: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(move || l.stats().acquired);
        f
    });
    let _progress = setup.spawn_progress(notice, explorer_requests);
    let result = run_evm_wallet_stats(
        &setup.cfg,
        &EvmStatsSources {
            scanner: &scanner,
            explorer: &explorer,
            resolver: Some(&resolver),
            max_requests,
            notice: Some(&|m: &str| notice(format!("cost: {m}"))),
        },
        wallets,
        window,
        concurrency,
        CancellationToken::new(),
    )
    .await;
    let requests_made = setup.rpc.total_requests_made() + explorer.own_requests_made();
    let mut report = match result {
        Ok(r) => r,
        Err(err) => {
            let text = scrub(&err.to_string());
            if let scout_api::ProviderError::Other(inner) = &err
                && inner
                    .downcast_ref::<scout_rpc::RequestBudgetExhausted>()
                    .is_some()
            {
                return Err(EvmStatsError::Budget(format!(
                    "{tool}: request budget exhausted after {requests_made} requests: {text} \
                     (IncompleteCoverage)"
                )));
            }
            return Err(EvmStatsError::Config(format!(
                "{tool}: provider error: {text}"
            )));
        }
    };
    let mut rate_limits = setup.rate_limit_report();
    if let Some(l) = &bs_limiter {
        rate_limits.push(rate_limit_line(&bs_label, &l.stats()));
    }
    if let Some(info) = report.evm.as_mut() {
        info.quote_assets = setup.info.quote_assets.clone();
        info.state_source = setup.state_source.clone();
        info.rate_limits = rate_limits.clone();
        info.listing_kind = explorer.kind().to_string();
    }
    // ADR-019 EVM amendment: exit quotes of the open positions at the head.
    let valuation = if valuation {
        Some(
            scout_sdk::engine::apply_evm_open_valuation(
                &mut report.wallets,
                &setup.rpc,
                &setup.cfg,
                window,
                &scout_sdk::engine::EvmValuationOptions::default(),
            )
            .await,
        )
    } else {
        None
    };
    let requests_made = setup.rpc.total_requests_made() + explorer.own_requests_made();
    Ok(EvmStatsRun {
        report,
        requests_made,
        valuation,
        secrets,
        warnings: setup.warnings,
        rate_limits,
        rpc_calls_by_method: setup.rpc.calls_by_method(),
    })
}

fn chain_name(chain: &ChainKey) -> &'static str {
    match chain.network_id {
        NetworkId::EvmChainId(id) => EvmChainProfile::by_chain_id(id).map_or("", |p| p.name),
        _ => "",
    }
}

fn no_indexer_error(tool: &str, wallets: usize, rpc_var: &str) -> EvmStatsError {
    EvmStatsError::Config(format!(
        "{tool}: {wallets} EVM wallet(s) parsed; no wallet history source configured: \
         configuration required, set {} (Blockscout, where it serves the chain) or use a keyed \
         Alchemy endpoint in {rpc_var} (alchemy_getAssetTransfers; Base, BSC, Robinhood). A \
         wallet's transactions cannot be listed over plain RPC; a window scan via eth_getLogs \
         would need about blocks/span requests (864,000 blocks per day on a 0.1 s chain), so it \
         is never attempted. Nothing was scanned",
        crate::evm_source::BLOCKSCOUT_KEY_ENV
    ))
}

/// The Blockscout source of a run with its own limiter (metered separately
/// from the RPC; a flat request rate, 5/s unless `--rpc-rps` says otherwise).
fn build_blockscout(
    setup: &EvmSetup,
    key: &str,
    max_requests: Option<u64>,
    net: &EvmNetOptions,
    notice: &LimiterNotice,
    label: &str,
    base_url_override: Option<String>,
) -> Result<(BlockscoutEvmSource, Arc<RateLimiter>), String> {
    let mut bs_cfg = BlockscoutEvmConfig::new(setup.profile.chain_id, BlockscoutApiKey::new(key));
    if let Some(u) = base_url_override.filter(|u| !u.is_empty()) {
        bs_cfg.base_url = u;
    }
    bs_cfg.max_total_requests = max_requests;
    let bs_rps = u64::from(net.rpc_rps.unwrap_or(5).max(1));
    let bs_notice = Arc::clone(notice);
    let bs_label2 = label.to_string();
    let limiter = Arc::new(RateLimiter::new(bs_rps, bs_rps).with_halving_hook(Arc::new(
        move |old, new| {
            bs_notice(rate_change_text(
                &bs_label2,
                "429/rate limit answer",
                old,
                new,
            ));
        },
    )));
    bs_cfg.rate_limiter = Some(Arc::clone(&limiter));
    BlockscoutEvmSource::new(bs_cfg)
        .map(|s| (s, limiter))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_change_and_progress_texts_are_url_free_and_distinguish_restore() {
        let down = rate_change_text("keyed rpc", "429", 250_000, 125_000);
        assert!(down.contains("halved 250.000/s -> 125.000/s"), "{down}");
        assert!(down.contains("grows back by 1/8"), "{down}");
        let up = rate_change_text("keyed rpc", "429", 7_812, 250_000);
        assert!(up.contains("restored 7.812/s -> 250.000/s"), "{up}");
        let l = Arc::new(RateLimiter::new(250, 250));
        let line = progress_line(
            std::time::Duration::from_secs(120),
            42,
            &[("keyed rpc".to_string(), l)],
        );
        assert_eq!(
            line,
            "progress: 120 s elapsed, 42 request(s) made; current rate: keyed rpc 250.000/s"
        );
    }

    fn key(family: ChainFamily, id: Option<u64>) -> ChainKey {
        ChainKey {
            family,
            network_id: match id {
                Some(i) => NetworkId::EvmChainId(i),
                None => NetworkId::SolanaCluster(scout_core::SolanaCluster::Mainnet),
            },
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        }
    }

    #[test]
    fn family_choice_never_drops_inputs() {
        let sol = key(ChainFamily::Solana, None);
        let rh = key(ChainFamily::Evm, Some(4663));
        let base = key(ChainFamily::Evm, Some(8453));
        assert_eq!(run_family([&sol, &sol], "t"), Ok(RunFamily::Solana));
        assert_eq!(run_family([&rh, &rh], "t"), Ok(RunFamily::Evm(rh.clone())));
        assert!(run_family([&sol, &rh], "t").unwrap_err().contains("mixed"));
        assert!(
            run_family([&rh, &base], "t")
                .unwrap_err()
                .contains("several EVM")
        );
    }

    use scout_sdk::evm::ROBINHOOD;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEYED_SECRET: &str = "KEYEDSECRET1234567";
    const LOGS_SECRET: &str = "LOGSSECRET7654321";

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }

    async fn on(server: &MockServer, m: &str, resp: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": m})))
            .respond_with(resp)
            .mount(server)
            .await;
    }

    /// A chain-identity-correct Robinhood endpoint; `logs` answers eth_getLogs.
    async fn endpoint(logs: ResponseTemplate) -> MockServer {
        endpoint_of(&ROBINHOOD, logs).await
    }

    /// A chain-identity-correct endpoint of `profile`.
    async fn endpoint_of(
        profile: &scout_sdk::evm::EvmChainProfile,
        logs: ResponseTemplate,
    ) -> MockServer {
        let s = MockServer::start().await;
        on(
            &s,
            "eth_chainId",
            ok(json!(format!("{:#x}", profile.chain_id))),
        )
        .await;
        on(
            &s,
            "eth_getBlockByNumber",
            ok(json!({"hash": format!("{:#x}", profile.genesis_hash), "timestamp": "0x0"})),
        )
        .await;
        on(&s, "eth_blockNumber", ok(json!("0x1000"))).await;
        // decimals of the pinned quote tokens (USDG/USDC 6, BSC pegs 18)
        let decimals = profile.quote_assets.first().map_or(6, |q| q.decimals);
        on(&s, "eth_call", ok(json!(format!("0x{decimals:064x}")))).await;
        on(&s, "eth_getBalance", ok(json!("0x1"))).await;
        on(&s, "eth_getLogs", logs).await;
        s
    }

    fn capped() -> ResponseTemplate {
        ResponseTemplate::new(400).set_body_json(json!({"jsonrpc":"2.0","id":1,"error":{
            "code":-32600,
            "message":"Under the Free tier plan, you can make eth_getLogs requests with up to a 10 block range. Based on your parameters, this block range should work: [0xff0, 0xff9]"}}))
    }

    async fn methods(s: &MockServer) -> Vec<String> {
        s.received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .filter_map(|b| b["method"].as_str().map(str::to_string))
            .collect()
    }

    fn silent() -> LimiterNotice {
        Arc::new(|_| {})
    }

    fn robinhood() -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(4663),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        }
    }

    #[tokio::test]
    async fn capped_keyed_bsc_rpc_lists_token_transfers_instead_of_logs() {
        let keyed = endpoint_of(&scout_sdk::evm::BSC, capped()).await;
        let k = keyed.uri();
        let env = move |name: &str| match name {
            "SCOUT_BSC_RPC_URL" => Some(format!("{k}/v2/{KEYED_SECRET}")),
            _ => None,
        };
        let bsc = ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(56),
            genesis_identity: scout_core::GenesisIdentity::Unverified,
        };
        let setup = setup_evm(
            &bsc,
            false,
            Some(100),
            4,
            &EvmNetOptions::default(),
            &silent(),
            true,
            env,
        )
        .await
        .unwrap();
        assert!(setup.token_listing_via_transfers);
        assert_eq!(setup.logs_span_cap, None, "no up-front getLogs refusal");
        assert!(
            setup.logs_source.contains("alchemy_getAssetTransfers"),
            "{}",
            setup.logs_source
        );
        assert!(setup.warnings.iter().all(|w| !w.contains(KEYED_SECRET)));
        // one limiter: no second (public) endpoint
        assert_eq!(setup.rate_limit_report().len(), 1);
    }

    #[tokio::test]
    async fn logs_max_span_env_cuts_windows_of_a_custom_logs_endpoint() {
        let keyed = endpoint(ok(json!([]))).await;
        let logs = endpoint(ok(json!([]))).await;
        let (k, l) = (keyed.uri(), logs.uri());
        let env = move |name: &str| match name {
            "SCOUT_ROBINHOOD_RPC_URL" => Some(format!("{k}/v2/{KEYED_SECRET}")),
            "SCOUT_ROBINHOOD_LOGS_RPC_URL" => Some(l.clone()),
            "SCOUT_ROBINHOOD_LOGS_MAX_SPAN" => Some("1000".to_string()),
            _ => None,
        };
        let setup = setup_evm(
            &robinhood(),
            false,
            Some(100),
            4,
            &EvmNetOptions::default(),
            &silent(),
            true,
            env,
        )
        .await
        .unwrap();
        assert_eq!(setup.logs_span_cap, Some(1_000));
        let r = setup
            .rpc
            .get_logs(&scout_providers::LogFilter::default(), 0, 2_499)
            .await
            .unwrap();
        assert_eq!((r.requests, r.splits), (3, 0), "{r:?}");
    }

    #[tokio::test]
    async fn capped_keyed_rpc_routes_logs_to_the_public_endpoint_only() {
        let keyed = endpoint(capped()).await;
        let public = endpoint(ok(json!([]))).await;
        let (k, p) = (keyed.uri(), public.uri());
        let env = move |name: &str| match name {
            "SCOUT_ROBINHOOD_RPC_URL" => Some(format!("{k}/v2/{KEYED_SECRET}")),
            PUBLIC_RPC_OVERRIDE_ENV => Some(p.clone()),
            _ => None,
        };
        let setup = setup_evm(
            &robinhood(),
            false,
            Some(100),
            4,
            &EvmNetOptions::default(),
            &silent(),
            true,
            env,
        )
        .await
        .unwrap();
        assert!(setup.logs_source.contains("auto"), "{}", setup.logs_source);
        assert!(setup.state_source.starts_with("keyed rpc"));
        assert_eq!(
            setup.logs_span_cap, None,
            "the public endpoint is not range capped"
        );
        assert!(
            setup
                .warnings
                .iter()
                .any(|w| w.contains("eth_getLogs ONLY"))
        );
        // the notice never carries the URL
        assert!(setup.warnings.iter().all(|w| !w.contains(KEYED_SECRET)));
        let r = setup
            .rpc
            .get_logs(&scout_providers::LogFilter::default(), 0, 5_000)
            .await
            .unwrap();
        assert!(r.logs.is_empty());
        setup
            .rpc
            .balance_at(alloy_primitives::Address::ZERO, 7)
            .await
            .unwrap();
        let (mk, mp) = (methods(&keyed).await, methods(&public).await);
        // keyed: exactly one eth_getLogs (the capability probe), the balance call
        assert_eq!(
            mk.iter().filter(|m| *m == "eth_getLogs").count(),
            1,
            "{mk:?}"
        );
        assert!(mk.iter().any(|m| m == "eth_getBalance"));
        // public: the routed logs call, never balances/receipts
        assert!(mp.iter().any(|m| m == "eth_getLogs"));
        assert!(!mp.iter().any(|m| m == "eth_getBalance"), "{mp:?}");
        // ONE budget over both endpoints
        let total = u64::try_from(mk.len() + mp.len()).unwrap();
        assert_eq!(setup.rpc.total_requests_made(), total);
        // two endpoints, two limiters (the test override is not capped to 5/s)
        let lines = setup.rate_limit_report();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains("10.000/s") && lines[1].contains("10.000/s"),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn explicit_logs_endpoint_skips_the_probe_and_its_url_is_a_secret() {
        let keyed = endpoint(capped()).await;
        let logs = endpoint(ok(json!([]))).await;
        let (k, l) = (keyed.uri(), logs.uri());
        let env = move |name: &str| match name {
            "SCOUT_ROBINHOOD_RPC_URL" => Some(format!("{k}/v2/{KEYED_SECRET}")),
            "SCOUT_ROBINHOOD_LOGS_RPC_URL" => Some(format!("{l}/v2/{LOGS_SECRET}")),
            _ => None,
        };
        let setup = setup_evm(
            &robinhood(),
            false,
            None,
            4,
            &EvmNetOptions {
                rpc_rps: Some(20),
                rpc_cu_per_sec: None,
            },
            &silent(),
            true,
            env,
        )
        .await
        .unwrap();
        setup
            .rpc
            .get_logs(&scout_providers::LogFilter::default(), 0, 50)
            .await
            .unwrap();
        assert!(!methods(&keyed).await.iter().any(|m| m == "eth_getLogs"));
        assert!(methods(&logs).await.iter().any(|m| m == "eth_getLogs"));
        assert!(setup.logs_source.contains("SCOUT_ROBINHOOD_LOGS_RPC_URL"));
        let secrets = setup.secrets();
        assert!(secrets.iter().any(|s| s.contains(LOGS_SECRET)));
        assert!(secrets.iter().any(|s| s.contains(KEYED_SECRET)));
        assert!(setup.rate_limit_report()[0].contains("20.000/s"));
    }

    #[tokio::test]
    async fn uncapped_keyed_rpc_keeps_logs_on_the_main_endpoint() {
        let keyed = endpoint(ok(json!([]))).await;
        let k = keyed.uri();
        let env = move |name: &str| {
            (name == "SCOUT_ROBINHOOD_RPC_URL").then(|| format!("{k}/v2/{KEYED_SECRET}"))
        };
        let setup = setup_evm(
            &robinhood(),
            false,
            None,
            4,
            &EvmNetOptions {
                rpc_rps: None,
                rpc_cu_per_sec: Some(300),
            },
            &silent(),
            true,
            env,
        )
        .await
        .unwrap();
        assert_eq!(setup.logs_source, setup.state_source);
        assert_eq!(setup.logs_span_cap, None);
        assert!(setup.rate_limit_report()[0].contains("300.000/s"));
    }

    #[test]
    fn default_rates_keyed_10_public_5_and_public_is_capped() {
        let n = silent();
        let rate = |net: EvmNetOptions, public: bool| {
            make_limiter(&net, public, false, "x", &n)
                .0
                .stats()
                .initial_rate_milli
        };
        let none = EvmNetOptions::default();
        assert_eq!((rate(none, false), rate(none, true)), (10_000, 5_000));
        let fast = EvmNetOptions {
            rpc_rps: Some(40),
            rpc_cu_per_sec: None,
        };
        assert_eq!((rate(fast, false), rate(fast, true)), (40_000, 5_000));
        let slow = EvmNetOptions {
            rpc_rps: Some(2),
            rpc_cu_per_sec: None,
        };
        assert_eq!(rate(slow, true), 2_000);
        let cu = EvmNetOptions {
            rpc_rps: None,
            rpc_cu_per_sec: Some(300),
        };
        let (l, cost) = make_limiter(&cu, false, false, "x", &n);
        assert_eq!(l.stats().initial_rate_milli, 300_000);
        assert!(cost.is_some_and(|c| c("eth_getBlockReceipts") > c("eth_getLogs")));
    }

    #[tokio::test(start_paused = true)]
    async fn alchemy_host_defaults_to_cu_mode_and_other_hosts_to_rps() {
        let notes = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let n2 = Arc::clone(&notes);
        let notice: LimiterNotice = Arc::new(move |m| n2.lock().unwrap().push(m));
        let none = EvmNetOptions::default();
        // Alchemy, nothing passed: CU mode at 250 CU/s, announced on stderr.
        let (l, cost) = make_limiter(&none, false, true, "keyed rpc", &notice);
        assert_eq!(l.stats().initial_rate_milli, 250_000);
        let cost = cost.expect("cu table");
        assert_eq!(cost("eth_getBlockReceipts"), 500);
        let seen = notes.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![
                "limiter: alchemy detected \u{2192} cu-per-sec 250 (override with \
                 --rpc-rps/--rpc-cu-per-sec)"
                    .to_string()
            ]
        );
        // A 500-CU call is admitted (waits for the bucket, never stuck).
        let t0 = tokio::time::Instant::now();
        l.acquire(cost("eth_getBlockReceipts")).await.unwrap();
        l.acquire(cost("eth_getBlockReceipts")).await.unwrap();
        let el = tokio::time::Instant::now() - t0;
        assert!(el >= std::time::Duration::from_secs(1), "{el:?}");
        // Non-Alchemy keyed host: flat 10 req/s, no table, no notice.
        let (l, cost) = make_limiter(&none, false, false, "keyed rpc", &notice);
        assert_eq!(l.stats().initial_rate_milli, 10_000);
        assert!(cost.is_none());
        // Explicit flags win on Alchemy.
        let rps = EvmNetOptions {
            rpc_rps: Some(7),
            rpc_cu_per_sec: None,
        };
        let (l, cost) = make_limiter(&rps, false, true, "keyed rpc", &notice);
        assert_eq!(l.stats().initial_rate_milli, 7_000);
        assert!(cost.is_none());
        let cu = EvmNetOptions {
            rpc_rps: None,
            rpc_cu_per_sec: Some(100),
        };
        let (l, _) = make_limiter(&cu, false, true, "keyed rpc", &notice);
        assert_eq!(l.stats().initial_rate_milli, 100_000);
        // Only the first call announced the default.
        assert_eq!(notes.lock().unwrap().len(), 1);
    }

    #[test]
    fn feasibility_estimate_refuses_before_burning_the_budget() {
        // 1 hour of Robinhood (36,000 blocks) x 2 tokens at 10 blocks/request
        let e = check_log_scan_feasible(Some(10), Some((0, 35_999)), 2, Some(3_000), "SCOUT_X")
            .unwrap_err();
        assert!(
            e.contains("7200") && e.contains("3000") && e.contains("SCOUT_X"),
            "{e}"
        );
        assert!(!e.contains("http"));
        // fits the budget / no cap known / no window
        assert!(check_log_scan_feasible(Some(10), Some((0, 999)), 2, Some(300), "X").is_ok());
        assert!(check_log_scan_feasible(None, Some((0, 10_000_000)), 9, None, "X").is_ok());
        assert!(check_log_scan_feasible(Some(10), None, 1, None, "X").is_ok());
        // default ceiling without --max-requests
        assert!(check_log_scan_feasible(Some(10), Some((0, 864_000)), 1, None, "X").is_err());
    }
}
