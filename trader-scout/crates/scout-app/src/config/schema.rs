//! Typed representation of `config/scout.example.toml`'s schema.
//!
//! This is a loader for the *proposed* config contract documented in
//! that file — it does not invent new fields, and every field here maps
//! directly to one in the example TOML. Per that file's own header
//! comment ("Numeric limits — стартовые настройки для измерений, не
//! обещание провайдерских квот"), the numeric defaults are starting
//! points, not verified guarantees.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse config: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Top-level config structure, mirroring `scout.example.toml`'s
/// top-level tables exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoutConfig {
    pub analysis: AnalysisConfig,
    pub quality: QualityConfig,
    pub scan: ScanConfig,
    pub storage: StorageConfig,
    /// Keyed by chain name (`"solana"`, `"bsc"`, `"base"`, `"robinhood"`),
    /// matching `[chains.<name>]` tables. `BTreeMap` for deterministic
    /// iteration if this config is ever echoed back in a manifest.
    pub chains: BTreeMap<String, ChainConfig>,
    /// Keyed by provider slot name (`"solana_history"`, `"evm_history"`),
    /// matching `[providers.<name>]` tables.
    pub providers: BTreeMap<String, ProviderConfig>,
    pub output: OutputConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnalysisConfig {
    pub period: String,
    pub finality: String,
    pub cost_basis: String,
    pub rank_by: String,
    pub profile: String,
    pub top: u32,
    pub quote_currency: String,
    pub max_backfill_days: u32,
    pub identity: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QualityConfig {
    pub min_closed_episodes: u32,
    pub min_active_days: u32,
    pub require_declared_scope_complete: bool,
    pub require_known_ranking_critical_basis: bool,
    pub require_verified_ranking_critical_ownership: bool,
    pub require_ranking_critical_prices: bool,
    pub require_resolved_open_exposure: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanConfig {
    pub backend: String,
    pub global_max_inflight: u32,
    pub default_endpoint_max_inflight: u32,
    pub request_timeout_ms: u64,
    pub max_attempts: u32,
    pub queue_max_batches: u32,
    pub queue_max_bytes: u64,
    pub max_response_bytes: u64,
    pub memory_budget_mib: u64,
    /// `"auto"` or a specific worker count as a string in the example
    /// file — kept as `String` here rather than parsed to a number,
    /// since resolving `"auto"` to an actual thread count is a runtime
    /// decision for scout-engine, not this loader's job.
    pub decode_workers: String,
    pub reorder_spill: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub backend: String,
    pub path: String,
    pub metadata: String,
    pub raw: String,
    pub writer_batch_rows: u32,
    pub writer_flush_ms: u64,
    pub busy_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    pub enabled: bool,
    pub family: String,
    /// Present for EVM chains (`chain_id`), absent for Solana (which
    /// uses `network` instead). Both are `Option` because the schema
    /// genuinely differs per family — forcing one shape here would mean
    /// either a fake default chain_id for Solana or a fake network
    /// string for EVM, both of which ADR-002 explicitly rejects
    /// (network identity must never be guessed or defaulted).
    pub chain_id: Option<u64>,
    pub network: Option<String>,
    pub verify_genesis_identity: bool,
    pub rpc_url_env: String,
    pub fee_policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub adapter: String,
    pub api_key_env: String,
    pub quota_group: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputConfig {
    pub format: String,
    pub progress: String,
    pub color: String,
    pub redact_secrets: bool,
}

impl ScoutConfig {
    /// Load and parse a config file from `path`. Does not validate that
    /// referenced env vars are actually set — that check belongs to
    /// whatever resolves a `ChainConfig`/`ProviderConfig` into a live
    /// connection (scout-engine), per ADR-006's `ConfigurationRequired`
    /// contract, not to this loader.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path_ref = path.as_ref();
        let text = std::fs::read_to_string(path_ref).map_err(|source| ConfigError::Io {
            path: path_ref.display().to_string(),
            source,
        })?;
        let config: ScoutConfig = toml::from_str(&text)?;
        Ok(config)
    }

    /// Parse from an in-memory TOML string (used by tests, and by any
    /// caller that already has the config text — e.g. read via a
    /// different I/O path than `std::fs`).
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: ScoutConfig = toml::from_str(text)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_TOML: &str = include_str!("../../../../config/scout.example.toml");

    #[test]
    fn parses_the_actual_example_config_file() {
        // This is the real regression test: if scout.example.toml's
        // schema drifts from this loader (someone adds/renames a field
        // in one but not the other), this test fails immediately rather
        // than silently diverging.
        let config = ScoutConfig::parse(EXAMPLE_TOML).unwrap();
        assert_eq!(config.analysis.period, "30d");
        assert_eq!(config.analysis.top, 20);
        assert_eq!(config.quality.min_closed_episodes, 20);
        assert_eq!(config.chains.len(), 4);
        assert!(config.chains.contains_key("solana"));
        assert!(config.chains.contains_key("bsc"));
        assert!(config.chains.contains_key("base"));
        assert!(config.chains.contains_key("robinhood"));
        assert_eq!(config.providers.len(), 2);
    }

    #[test]
    fn solana_chain_has_network_not_chain_id() {
        let config = ScoutConfig::parse(EXAMPLE_TOML).unwrap();
        let solana = &config.chains["solana"];
        assert_eq!(solana.family, "solana");
        assert_eq!(solana.network.as_deref(), Some("mainnet"));
        assert_eq!(solana.chain_id, None);
    }

    #[test]
    fn evm_chains_have_chain_id_not_network() {
        let config = ScoutConfig::parse(EXAMPLE_TOML).unwrap();
        let bsc = &config.chains["bsc"];
        assert_eq!(bsc.family, "evm");
        assert_eq!(bsc.chain_id, Some(56));
        assert_eq!(bsc.network, None);

        let base = &config.chains["base"];
        assert_eq!(base.chain_id, Some(8453));

        let robinhood = &config.chains["robinhood"];
        assert_eq!(robinhood.chain_id, Some(4663));
    }

    #[test]
    fn evm_history_provider_placeholder_parses_without_a_real_adapter() {
        // scout.example.toml's own comment: this placeholder MUST give
        // CONFIGURATION_REQUIRED at runtime, not fake empty history —
        // but at the config-parsing layer it just needs to parse as a
        // valid (if unresolvable) provider entry.
        let config = ScoutConfig::parse(EXAMPLE_TOML).unwrap();
        let evm_history = &config.providers["evm_history"];
        assert_eq!(evm_history.adapter, "configured-after-p0");
        assert_eq!(evm_history.api_key_env, "SCOUT_EVM_HISTORY_API_KEY");
    }

    #[test]
    fn missing_required_field_is_a_typed_parse_error_not_a_panic() {
        let broken = r#"
            [analysis]
            period = "30d"
        "#;
        let result = ScoutConfig::parse(broken);
        assert!(matches!(result, Err(ConfigError::Parse(_))));
    }

    #[test]
    fn load_from_nonexistent_path_is_a_typed_io_error() {
        let result = ScoutConfig::load("/nonexistent/path/scout.toml");
        assert!(matches!(result, Err(ConfigError::Io { .. })));
    }
}
