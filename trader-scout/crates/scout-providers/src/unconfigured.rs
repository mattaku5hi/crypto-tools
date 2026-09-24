//! `UnconfiguredProvider`: the default for any port whose backend env var
//! is unset. Returns `ProviderError::ConfigurationRequired` from every
//! call — never a fake-empty successful result (ADR-006,
//! `config/scout.example.toml`'s `evm_history` placeholder comment).
//! Implements `scout_api::HistoryProvider` (ADR-008 S3).

use futures::stream::{self, BoxStream};
use scout_api::{
    CapabilityStatus, HistoryProvider, ProviderError, ScanEnvelope, ScanPlan, ScanRequest,
    ScanTask, SourceCapabilities,
};
use tokio_util::sync::CancellationToken;

/// A provider port with no backend configured.
#[derive(Debug, Clone)]
pub struct UnconfiguredProvider {
    /// Human-readable port name, e.g. "evm_history".
    pub port: String,
    /// The env var a user would set to configure this port, e.g.
    /// "SCOUT_EVM_HISTORY_API_KEY".
    pub env_var: String,
}

impl UnconfiguredProvider {
    #[must_use]
    pub fn new(port: impl Into<String>, env_var: impl Into<String>) -> Self {
        Self {
            port: port.into(),
            env_var: env_var.into(),
        }
    }

    fn configuration_error(&self) -> ProviderError {
        ProviderError::ConfigurationRequired {
            port: self.port.clone(),
            detail: format!("set {}", self.env_var),
        }
    }
}

#[async_trait::async_trait]
impl HistoryProvider for UnconfiguredProvider {
    fn capabilities(&self) -> SourceCapabilities {
        // Every capability is Unknown, never Unsupported — we have not
        // investigated anything, we have simply not been configured.
        // Reporting Unsupported here would be a false negative that
        // could later be mistaken for "checked and confirmed absent."
        let mut caps = SourceCapabilities::empty();
        for capability in [
            "token_market_activity",
            "wallet_activity",
            "raw_transactions",
            "historical_state",
        ] {
            caps.by_capability
                .insert(capability.to_string(), CapabilityStatus::Unknown);
        }
        caps
    }

    async fn plan(&self, _request: &ScanRequest) -> Result<ScanPlan, ProviderError> {
        Err(self.configuration_error())
    }

    fn scan(
        &self,
        _task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ProviderError>> {
        // scan() cannot return a Result directly (it's a stream), so the
        // error surfaces as the stream's single yielded item. A caller
        // that only inspects plan() already fails fast; this exists so a
        // caller that skips straight to scan() still gets the same typed
        // error, never an empty-but-successful stream.
        let error = self.configuration_error();
        Box::pin(stream::once(async move { Err(error) }))
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;

    #[tokio::test]
    async fn plan_returns_configuration_required_not_empty_success() {
        let provider = UnconfiguredProvider::new("evm_history", "SCOUT_EVM_HISTORY_API_KEY");
        let request = ScanRequest::WalletActivity {
            wallet: test_wallet(),
        };
        let result = provider.plan(&request).await;
        match result {
            Err(ProviderError::ConfigurationRequired { port, detail }) => {
                assert_eq!(port, "evm_history");
                assert!(detail.contains("SCOUT_EVM_HISTORY_API_KEY"));
            }
            other => panic!("expected ConfigurationRequired, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn scan_never_yields_an_empty_successful_stream() {
        // This is the exact failure mode scout.example.toml warns against:
        // "Этот placeholder ДОЛЖЕН давать CONFIGURATION_REQUIRED, а не
        // fake empty history."
        let provider = UnconfiguredProvider::new("evm_history", "SCOUT_EVM_HISTORY_API_KEY");
        let mut stream = provider.scan(
            ScanTask {
                description: "test".to_string(),
            },
            CancellationToken::new(),
        );
        let first = stream.next().await;
        assert!(
            matches!(
                first,
                Some(Err(ProviderError::ConfigurationRequired { .. }))
            ),
            "expected the stream's first item to be a ConfigurationRequired error, not an empty stream"
        );
        // The stream must not silently claim "no history found" by being
        // empty-but-Ok; it must yield exactly the typed error once.
        let second = stream.next().await;
        assert!(second.is_none());
    }

    #[test]
    fn capabilities_are_all_unknown_never_unsupported() {
        let provider = UnconfiguredProvider::new("evm_history", "SCOUT_EVM_HISTORY_API_KEY");
        let caps = provider.capabilities();
        for status in caps.by_capability.values() {
            assert_eq!(*status, CapabilityStatus::Unknown);
        }
    }

    #[test]
    fn accepts_runtime_constructed_strings_not_just_static() {
        // The actual fix vs the old &'static str constraint: a provider
        // built from config data at runtime must be constructible.
        let port_name = format!("{}-history", "custom");
        let env_var_name = format!("SCOUT_{}_API_KEY", "CUSTOM");
        let provider = UnconfiguredProvider::new(port_name.clone(), env_var_name.clone());
        assert_eq!(provider.port, port_name);
        assert_eq!(provider.env_var, env_var_name);
    }

    fn test_wallet() -> scout_core::WalletKey {
        scout_core::WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: scout_core::AddressBytes::Evm([0x11; 20]),
        }
    }
}
