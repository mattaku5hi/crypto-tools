//! EVM endpoint configuration shared by the CLIs (ADR-020 step 2).
//!
//! RPC URLs of keyed providers (Alchemy: the API key is a path segment)
//! are SECRETS: [`EvmRpcUrl`] never prints its value (`Debug`/`Display`
//! are redacted), exposes the secret substrings so every user-visible text
//! can be scrubbed with [`EvmRpcUrl::redact`], and nothing here logs it.

use std::fmt;

/// Env var with the RPC URL of a chain profile name, `None` when the chain
/// has no configured variable.
#[must_use]
pub fn rpc_env_name(chain: &str) -> Option<&'static str> {
    match chain {
        "robinhood" => Some("SCOUT_ROBINHOOD_RPC_URL"),
        "base" => Some("SCOUT_BASE_RPC_URL"),
        "bsc" => Some("SCOUT_BSC_RPC_URL"),
        _ => None,
    }
}

/// Env var with the OPTIONAL separate `eth_getLogs` endpoint of a chain
/// profile (per-method routing: logs there, receipts/state/balance on the
/// main RPC). Secret like the main URL.
#[must_use]
pub fn logs_rpc_env_name(chain: &str) -> Option<&'static str> {
    match chain {
        "robinhood" => Some("SCOUT_ROBINHOOD_LOGS_RPC_URL"),
        "base" => Some("SCOUT_BASE_LOGS_RPC_URL"),
        "bsc" => Some("SCOUT_BSC_LOGS_RPC_URL"),
        _ => None,
    }
}

/// Default client-side request rate of a KEYED endpoint (requests/s).
pub const KEYED_RPC_RPS: u32 = 10;
/// Default (and ceiling) request rate of a PUBLIC keyless endpoint.
pub const PUBLIC_RPC_RPS: u32 = 5;

/// Default compute-unit rate of an Alchemy endpoint when neither
/// `--rpc-rps` nor `--rpc-cu-per-sec` is given (the free tier allows about
/// 300 CU/s; 250 leaves headroom).
pub const ALCHEMY_DEFAULT_CU_PER_SEC: u32 = 250;

/// Default compute-unit rate once an Alchemy endpoint is detected on Pay As
/// You Go (its `eth_getLogs` is not range capped). PAYG throughput starts at
/// 10,000 CU/s (pricing page, 2026-10-06); half leaves headroom.
pub const ALCHEMY_PAYG_CU_PER_SEC: u32 = 5_000;

/// `true` when the URL's host is an Alchemy endpoint (`*.g.alchemy.com`).
/// Looks at the host only, never at the key.
#[must_use]
pub fn is_alchemy_url(url: &str) -> bool {
    let Some(rest) = url.split_once("://").map(|x| x.1) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    host.ends_with(".g.alchemy.com")
}

/// Network politeness options shared by the three CLIs (`#[command(flatten)]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::Args)]
pub struct EvmNetOptions {
    /// EVM only: client-side request rate of keyed RPC endpoints, requests
    /// per second (default 10, except Alchemy hosts which default to
    /// --rpc-cu-per-sec 250; the keyless public endpoint is capped at 5
    /// whatever this says; the explorer defaults to 5). Applies to every HTTP
    /// attempt, retries included, and is shared by all concurrent tasks. A
    /// 429 without Retry-After halves the rate for the rest of the run.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=10_000))]
    pub rpc_rps: Option<u32>,
    /// EVM only: weight requests by an APPROXIMATE per-method compute-unit
    /// table (Alchemy-shaped: eth_getLogs 75, eth_getBlockReceipts 500, ...)
    /// and pace them to this many units per second (Alchemy free tier is
    /// about 300). Replaces the flat --rpc-rps for RPC endpoints.
    #[arg(
        long,
        conflicts_with = "rpc_rps",
        value_parser = clap::value_parser!(u32).range(1..=1_000_000)
    )]
    pub rpc_cu_per_sec: Option<u32>,
}

/// Env var with the optional Blockscout API key (wallet-centric history).
pub const BLOCKSCOUT_KEY_ENV: &str = "SCOUT_BLOCKSCOUT_API_KEY";

/// Public Robinhood Chain RPC (rate limited; the only keyless endpoint
/// measured, research doc section 3).
pub const ROBINHOOD_PUBLIC_RPC: &str = "https://rpc.mainnet.chain.robinhood.com";

/// Env var with a known maximum `eth_getLogs` block span of a chain's logs
/// endpoint (`SCOUT_<CHAIN>_LOGS_MAX_SPAN`).
#[must_use]
pub fn logs_max_span_env_name(chain: &str) -> Option<&'static str> {
    match chain {
        "robinhood" => Some("SCOUT_ROBINHOOD_LOGS_MAX_SPAN"),
        "base" => Some("SCOUT_BASE_LOGS_MAX_SPAN"),
        "bsc" => Some("SCOUT_BSC_LOGS_MAX_SPAN"),
        _ => None,
    }
}

/// An RPC URL that never prints.
#[derive(Clone)]
pub struct EvmRpcUrl {
    url: String,
    /// `true` when it came from the environment (a keyed provider is
    /// assumed), `false` for the built-in public fallback.
    pub from_env: bool,
}

impl fmt::Debug for EvmRpcUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EvmRpcUrl(<redacted>, from_env={})", self.from_env)
    }
}

impl fmt::Display for EvmRpcUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted rpc url>")
    }
}

impl EvmRpcUrl {
    #[must_use]
    pub fn new(url: impl Into<String>, from_env: bool) -> Self {
        Self {
            url: url.into(),
            from_env,
        }
    }

    /// The host is an Alchemy endpoint (`*.g.alchemy.com`).
    #[must_use]
    pub fn is_alchemy(&self) -> bool {
        is_alchemy_url(&self.url)
    }

    /// The URL for the transport ONLY (never for output).
    #[must_use]
    pub fn expose_for_transport(&self) -> &str {
        &self.url
    }

    /// Every substring that must never reach output or fixtures: the whole
    /// URL, everything after the host, and each path/query part >= 8 chars.
    #[must_use]
    pub fn secrets(&self) -> Vec<String> {
        let mut v = vec![self.url.clone()];
        if let Some(rest) = self.url.split_once("://").map(|x| x.1)
            && let Some(i) = rest.find(['/', '?'])
        {
            let tail = rest.get(i..).unwrap_or("");
            if tail.len() > 1 {
                v.push(tail.to_string());
                v.extend(
                    tail.split(['/', '?', '&', '='])
                        .filter(|p| p.len() >= 8)
                        .map(str::to_string),
                );
            }
        }
        v
    }

    /// `text` with every secret replaced by `<redacted>`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for s in self.secrets() {
            if s.len() >= 4 {
                out = out.replace(s.as_str(), "<redacted>");
            }
        }
        out
    }
}

/// Resolve the RPC URL of `chain`: the env var when set and non-empty, else
/// (Robinhood only) the public RPC with `Ok((url, Some(warning)))`; any other
/// chain without a variable is an error naming the variable (never a URL).
///
/// # Errors
/// Text naming the missing variable.
pub fn evm_rpc_url_from_env(
    chain: &str,
    get: impl Fn(&str) -> Option<String>,
) -> Result<(EvmRpcUrl, Option<String>), String> {
    let var = rpc_env_name(chain).ok_or_else(|| format!("no RPC variable for chain `{chain}`"))?;
    if let Some(v) = get(var).filter(|v| !v.trim().is_empty()) {
        return Ok((EvmRpcUrl::new(v.trim().to_string(), true), None));
    }
    if chain == "robinhood" {
        return Ok((
            EvmRpcUrl::new(ROBINHOOD_PUBLIC_RPC, false),
            Some(format!(
                "{var} is not set: using the public Robinhood RPC, which is rate limited and has \
                 no trace or historical-state support (native sells will stay Unknown)"
            )),
        ));
    }
    Err(format!(
        "configuration required: set {var} (no public fallback for {chain})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_never_prints_and_redaction_scrubs_path_keys() {
        let u = EvmRpcUrl::new("https://rh.g.alchemy.com/v2/SECRETKEY123456", true);
        assert!(u.is_alchemy());
        assert!(!format!("{u:?} {u}").contains("SECRETKEY"));
        let r = u.redact("error sending request https://rh.g.alchemy.com/v2/SECRETKEY123456: boom, key SECRETKEY123456");
        assert!(!r.contains("SECRETKEY") && r.contains("boom"));
    }

    #[test]
    fn alchemy_host_detection_looks_at_the_host_only() {
        assert!(is_alchemy_url(
            "https://robinhood-mainnet.g.alchemy.com/v2/k"
        ));
        assert!(is_alchemy_url(
            "HTTPS://Base-Mainnet.G.Alchemy.com:443/v2/k"
        ));
        assert!(!is_alchemy_url("https://rpc.mainnet.chain.robinhood.com"));
        assert!(!is_alchemy_url("https://evil.example/x.g.alchemy.com/v2/k"));
        assert!(!is_alchemy_url("https://g.alchemy.com.evil.example/v2/k"));
        assert!(!is_alchemy_url("http://127.0.0.1:1234/v2/k"));
        assert!(!is_alchemy_url("nonsense"));
    }

    #[test]
    fn env_resolution_and_public_fallback() {
        let none = |_: &str| None;
        let (u, w) = evm_rpc_url_from_env("robinhood", none).unwrap();
        assert!(!u.from_env && w.unwrap().contains("rate limited"));
        assert!(
            evm_rpc_url_from_env("base", none)
                .unwrap_err()
                .contains("SCOUT_BASE_RPC_URL")
        );
        let set =
            |k: &str| (k == "SCOUT_BSC_RPC_URL").then(|| " https://x/v2/abcdefgh12 ".to_string());
        let (u, w) = evm_rpc_url_from_env("bsc", set).unwrap();
        assert!(u.from_env && w.is_none());
        assert_eq!(u.expose_for_transport(), "https://x/v2/abcdefgh12");
        assert!(evm_rpc_url_from_env("mars", none).is_err());
    }
}
