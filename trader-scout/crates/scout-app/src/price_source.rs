//! Composition-root helper shared by the CLIs: the ADR-018 Coinbase price
//! source with the optional test/mirror endpoint override. No secrets are
//! involved: the source sends no credentials of any kind.

use scout_pricing::{CoinbaseConfig, CoinbasePriceSource};

/// Test/mirror override of the Coinbase base URL (never needed in production).
pub const COINBASE_ENDPOINT_ENV: &str = "SCOUT_COINBASE_ENDPOINT";

/// Build the price source. Returns the source and whether the endpoint was
/// overridden through [`COINBASE_ENDPOINT_ENV`]. `max_requests` bounds the
/// total HTTP attempts for prices (`None` = unlimited, still counted).
pub fn build_coinbase_source(
    max_requests: Option<u64>,
) -> Result<(CoinbasePriceSource, bool), String> {
    let mut cfg = CoinbaseConfig {
        max_requests,
        ..CoinbaseConfig::default()
    };
    let mut overridden = false;
    if let Ok(url) = std::env::var(COINBASE_ENDPOINT_ENV)
        && !url.trim().is_empty()
    {
        cfg.endpoint = url;
        overridden = true;
    }
    CoinbasePriceSource::new(cfg)
        .map(|s| (s, overridden))
        .map_err(|c| format!("could not build the price source: {}", c.label()))
}
