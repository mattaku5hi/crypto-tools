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
    BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, EvmHistoryScanner, EvmRpcClient,
    EvmSourceError, NativeLegPolicy, NativeLegResolver, ScanLimits,
};
use scout_rpc::{HalvingHook, RateLimiter, RateLimiterStats, RpcClient, RpcEndpoint};
use scout_sdk::engine::{
    AnalysisWindow, EvmExtractionConfig, EvmRunInfo, EvmStatsSources, SolanaWalletStatsReport,
    run_evm_wallet_stats,
};
use scout_sdk::evm::EvmChainProfile;
use tokio_util::sync::CancellationToken;

use crate::evm_source::{
    EvmNetOptions, EvmRpcUrl, KEYED_RPC_RPS, PUBLIC_RPC_RPS, ROBINHOOD_PUBLIC_RPC,
    evm_rpc_url_from_env, logs_rpc_env_name,
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

    /// One stderr line per endpoint: initial/final rate, halvings, waits.
    #[must_use]
    pub fn rate_limit_report(&self) -> Vec<String> {
        self.limiters
            .iter()
            .map(|(label, l)| rate_limit_line(label, &l.stats()))
            .collect()
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

/// Human line of one limiter's counters (never contains a URL).
#[must_use]
pub fn rate_limit_line(label: &str, st: &RateLimiterStats) -> String {
    format!(
        "{label}: rate {}/s -> {}/s ({} halving(s) after 429 without Retry-After), {} request(s) \
         paced, {} ms spent waiting for tokens",
        fmt_rate(st.initial_rate_milli),
        fmt_rate(st.rate_milli),
        st.halvings,
        st.acquired,
        st.waited_ms
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
         that cap, narrow the window, or raise --max-requests knowingly",
        if max_requests.is_some() {
            " (--max-requests)"
        } else {
            " (default; pass --max-requests to override)"
        }
    ))
}

/// Per-method cost table of a limiter.
type CostFn = fn(&str) -> u64;

fn make_limiter(
    net: &EvmNetOptions,
    public: bool,
    label: &str,
    on_halve: &Arc<dyn Fn(String) + Send + Sync>,
) -> (Arc<RateLimiter>, Option<CostFn>) {
    let hook_label = label.to_string();
    let notify = Arc::clone(on_halve);
    let hook: HalvingHook = Arc::new(move |old, new| {
        notify(format!(
            "{hook_label}: provider answered 429 without Retry-After; client rate halved \
             {}/s -> {}/s for the rest of the run",
            fmt_rate(old),
            fmt_rate(new)
        ));
    });
    if let Some(cu) = net.rpc_cu_per_sec {
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
/// `allow_unverified`: Base/BSC have no verified venue/quote set yet; without
/// the explicit flag the run is refused (exit 4, "not verified yet").
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
    if profile.quote_assets.is_empty() && profile.name != "robinhood" && !allow_unverified {
        return Err(EvmSetupError::Config(format!(
            "chain `{}`: venue/source not verified yet (no FixtureVerified venue and no pinned \
             quote assets); pass --allow-unverified-chain to run anyway (every trade is then \
             IdlOnly and the run is partial)",
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
    let (main_limiter, main_cost) = make_limiter(net, main_public, &main_label, notice);
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
    if let Some((lu, public, label)) = logs_target {
        let (l, cost) = make_limiter(net, public, &label, notice);
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
    net: &EvmNetOptions,
    notice: &LimiterNotice,
    env: impl Fn(&str) -> Option<String>,
) -> Result<EvmStatsRun, EvmStatsError> {
    let Some(bs_key) = env(crate::evm_source::BLOCKSCOUT_KEY_ENV).filter(|k| !k.trim().is_empty())
    else {
        return Err(EvmStatsError::Config(format!(
            "{tool}: {} EVM wallet(s) parsed; no wallet history source configured: \
             configuration required, set {} (a wallet's transactions cannot be listed over \
             plain RPC; a window scan via eth_getLogs would need about blocks/span requests - \
             864,000 blocks per day on a 0.1 s chain, 86,400 requests at a 10-block provider \
             cap - so it is never attempted. Nothing was scanned)",
            wallets.len(),
            crate::evm_source::BLOCKSCOUT_KEY_ENV
        )));
    };
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
        EvmSetupError::Config(m) => {
            EvmStatsError::Config(format!("{tool}: {}", m.replace(&bs_key, "<redacted>")))
        }
    })?;
    let mut secrets = setup.secrets();
    secrets.push(bs_key.clone());
    let scrub = |t: &str| {
        let mut o = t.to_string();
        for s in &secrets {
            if s.len() >= 4 {
                o = o.replace(s.as_str(), "<redacted>");
            }
        }
        o
    };
    let mut bs_cfg =
        BlockscoutEvmConfig::new(setup.profile.chain_id, BlockscoutApiKey::new(&bs_key));
    if let Some(u) = env(BLOCKSCOUT_URL_OVERRIDE_ENV).filter(|u| !u.is_empty()) {
        bs_cfg.base_url = u;
    }
    bs_cfg.max_total_requests = max_requests;
    // The explorer is metered separately; a flat request rate, 5/s unless
    // the user lowered/raised --rpc-rps.
    let bs_label = "explorer (blockscout)".to_string();
    let bs_rps = u64::from(net.rpc_rps.unwrap_or(5).max(1));
    let bs_notice = Arc::clone(notice);
    let bs_label2 = bs_label.clone();
    let bs_limiter = Arc::new(RateLimiter::new(bs_rps, bs_rps).with_halving_hook(Arc::new(
        move |old, new| {
            bs_notice(format!(
                "{bs_label2}: 429/rate limit answer; client rate halved {}/s -> {}/s for the \
                 rest of the run",
                fmt_rate(old),
                fmt_rate(new)
            ));
        },
    )));
    bs_cfg.rate_limiter = Some(Arc::clone(&bs_limiter));
    let explorer = BlockscoutEvmSource::new(bs_cfg).map_err(|e| {
        EvmStatsError::Config(format!(
            "{tool}: explorer unavailable: {}",
            scrub(&e.to_string())
        ))
    })?;
    let scanner = EvmHistoryScanner::new(
        setup.rpc.clone(),
        setup.chain.clone(),
        ScanLimits::default(),
    );
    let resolver = NativeLegResolver::new(setup.rpc.clone(), NativeLegPolicy::default());
    let result = run_evm_wallet_stats(
        &setup.cfg,
        &EvmStatsSources {
            scanner: &scanner,
            explorer: &explorer,
            resolver: Some(&resolver),
        },
        wallets,
        window,
        concurrency,
        CancellationToken::new(),
    )
    .await;
    let requests_made = setup.rpc.total_requests_made() + explorer.total_requests_made();
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
    rate_limits.push(rate_limit_line(&bs_label, &bs_limiter.stats()));
    if let Some(info) = report.evm.as_mut() {
        info.quote_assets = setup.info.quote_assets.clone();
        info.state_source = setup.state_source.clone();
        info.rate_limits = rate_limits.clone();
    }
    Ok(EvmStatsRun {
        report,
        requests_made,
        secrets,
        warnings: setup.warnings,
        rate_limits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let s = MockServer::start().await;
        on(&s, "eth_chainId", ok(json!("0x1237"))).await;
        on(
            &s,
            "eth_getBlockByNumber",
            ok(json!({"hash": format!("{:#x}", ROBINHOOD.genesis_hash), "timestamp": "0x0"})),
        )
        .await;
        on(&s, "eth_blockNumber", ok(json!("0x1000"))).await;
        on(&s, "eth_call", ok(json!(format!("0x{:064x}", 6)))).await;
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
            make_limiter(&net, public, "x", &n)
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
        let (l, cost) = make_limiter(&cu, false, "x", &n);
        assert_eq!(l.stats().initial_rate_milli, 300_000);
        assert!(cost.is_some_and(|c| c("eth_getBlockReceipts") > c("eth_getLogs")));
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
