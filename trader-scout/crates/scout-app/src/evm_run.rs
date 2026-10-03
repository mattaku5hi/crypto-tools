//! Shared EVM run setup of the three CLIs (ADR-020 step 2): which chain a
//! run is about, endpoint construction (secret-safe), the chain identity
//! preflight and the live `decimals()` check of pinned quote tokens.
//!
//! Exit-code mapping used by the binaries: [`EvmSetupError::Usage`] = 2,
//! every other variant = 4.

use alloy_primitives::Address;
use scout_core::{ChainFamily, ChainKey, NetworkId};
use scout_providers::{
    BlockscoutApiKey, BlockscoutEvmConfig, BlockscoutEvmSource, EvmHistoryScanner, EvmRpcClient,
    EvmSourceError, NativeLegPolicy, NativeLegResolver, ScanLimits,
};
use scout_rpc::{RpcClient, RpcEndpoint};
use scout_sdk::engine::{
    AnalysisWindow, EvmExtractionConfig, EvmRunInfo, EvmStatsSources, SolanaWalletStatsReport,
    run_evm_wallet_stats,
};
use scout_sdk::evm::EvmChainProfile;
use tokio_util::sync::CancellationToken;

use crate::evm_source::{EvmRpcUrl, evm_rpc_url_from_env};

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
}

/// Build the endpoint, preflight the chain and check the quote decimals.
///
/// `allow_unverified`: Base/BSC have no verified venue/quote set yet; without
/// the explicit flag the run is refused (exit 4, "not verified yet").
///
/// # Errors
/// [`EvmSetupError`]; every message is URL-free.
pub async fn setup_evm(
    chain: &ChainKey,
    allow_unverified: bool,
    max_requests: Option<u64>,
    concurrency: usize,
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
    let rpc = RpcClient::new(RpcEndpoint::new(url.expose_for_transport()), 30_000, 3)
        .map_err(|e| EvmSetupError::Config(url.redact(&e.to_string())))?
        .with_max_total_requests(max_requests);
    let cfg_rpc = scout_providers::EvmRpcConfig {
        concurrency,
        ..scout_providers::EvmRpcConfig::default()
    };
    let rpc = EvmRpcClient::with_config(rpc, profile, cfg_rpc);
    let verified = rpc.preflight().await.map_err(|e| {
        EvmSetupError::Config(format!(
            "chain identity preflight failed: {}",
            url.redact(&e.to_string())
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
                format!("not checked: {}", url.redact(&e.to_string()))
            }
            Err(e) => format!("not checked: {}", url.redact(&e.to_string())),
        };
    }
    if !url.from_env {
        warnings.push("public endpoint: eth_getLogs result cap 10,000".to_string());
    }
    Ok(EvmSetup {
        profile,
        rpc,
        chain: verified,
        url,
        cfg,
        info,
        warnings,
    })
}

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
    env: impl Fn(&str) -> Option<String>,
) -> Result<EvmStatsRun, EvmStatsError> {
    let Some(bs_key) = env(crate::evm_source::BLOCKSCOUT_KEY_ENV).filter(|k| !k.trim().is_empty())
    else {
        return Err(EvmStatsError::Config(format!(
            "{tool}: {} EVM wallet(s) parsed; no wallet history source configured: \
             configuration required, set {} (a wallet's transactions cannot be listed over \
             plain RPC)",
            wallets.len(),
            crate::evm_source::BLOCKSCOUT_KEY_ENV
        )));
    };
    let setup = setup_evm(
        chain,
        allow_unverified,
        max_requests,
        concurrency.max(1).saturating_mul(2),
        &env,
    )
    .await
    .map_err(|e| match e {
        EvmSetupError::Usage(m) => EvmStatsError::Usage(format!("{tool}: {m}")),
        EvmSetupError::Config(m) => {
            EvmStatsError::Config(format!("{tool}: {}", m.replace(&bs_key, "<redacted>")))
        }
    })?;
    let mut secrets = setup.url.secrets();
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
    if let Some(info) = report.evm.as_mut() {
        info.quote_assets = setup.info.quote_assets.clone();
    }
    Ok(EvmStatsRun {
        report,
        requests_made,
        secrets,
        warnings: setup.warnings,
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
}
