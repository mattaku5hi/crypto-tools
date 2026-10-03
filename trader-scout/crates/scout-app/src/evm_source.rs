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

/// Env var with the optional Blockscout API key (wallet-centric history).
pub const BLOCKSCOUT_KEY_ENV: &str = "SCOUT_BLOCKSCOUT_API_KEY";

/// Public Robinhood Chain RPC (rate limited; the only keyless endpoint
/// measured, research doc section 3).
pub const ROBINHOOD_PUBLIC_RPC: &str = "https://rpc.mainnet.chain.robinhood.com";

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
        assert!(!format!("{u:?} {u}").contains("SECRETKEY"));
        let r = u.redact("error sending request https://rh.g.alchemy.com/v2/SECRETKEY123456: boom, key SECRETKEY123456");
        assert!(!r.contains("SECRETKEY") && r.contains("boom"));
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
