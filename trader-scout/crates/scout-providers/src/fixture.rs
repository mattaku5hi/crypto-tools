//! `FixtureProvider`: reads from `tests/fixtures/` for offline acceptance
//! tests. See ADR-006 for the mandatory `provenance` block that keeps
//! synthetic fixtures from being mistaken for real mainnet data
//! (AGENTS.md's explicit ban on that).

use std::collections::BTreeMap;

use futures::stream::{self, BoxStream};
use scout_core::ScoutError;
use tokio_util::sync::CancellationToken;

use crate::capability::{CapabilityStatus, SourceCapabilities};
use crate::port::{HistoryProvider, ScanEnvelope, ScanPlan, ScanRequest, ScanTask};

/// Provenance of a fixture. `kind: Mainnet` requires all of
/// `chain`/`block_or_slot`/`tx`/`captured_at`/`source` to be populated —
/// enforced by [`Fixture::validate`], not left as a convention.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureProvenance {
    Synthetic {
        source: String,
    },
    Mainnet {
        chain: String,
        block_or_slot: String,
        tx: String,
        captured_at: String,
        source: String,
    },
}

/// One stored fixture: an identifier, its provenance, and an opaque
/// payload description (the real payload shape lands with the decoder
/// crates that consume it; this crate only owns provenance validation).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Fixture {
    pub id: String,
    pub provenance: FixtureProvenance,
}

/// Errors specific to fixture provenance validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FixtureError {
    #[error("fixture `{id}` claims mainnet provenance with an empty required field: {field}")]
    IncompleteMainnetProvenance { id: String, field: &'static str },
}

impl Fixture {
    /// Reject any fixture claiming `Mainnet` provenance with a blank
    /// required field. This is the concrete enforcement of AGENTS.md's
    /// "Нельзя объявлять synthetic fixture реальной mainnet транзакцией":
    /// a `Mainnet`-tagged fixture must carry real, non-empty identifying
    /// data, not just the label.
    pub fn validate(&self) -> Result<(), FixtureError> {
        if let FixtureProvenance::Mainnet {
            chain,
            block_or_slot,
            tx,
            captured_at,
            source,
        } = &self.provenance
        {
            let fields: [(&'static str, &str); 5] = [
                ("chain", chain),
                ("block_or_slot", block_or_slot),
                ("tx", tx),
                ("captured_at", captured_at),
                ("source", source),
            ];
            for (name, value) in fields {
                if value.trim().is_empty() {
                    return Err(FixtureError::IncompleteMainnetProvenance {
                        id: self.id.clone(),
                        field: name,
                    });
                }
            }
        }
        Ok(())
    }
}

/// A provider backed by an in-memory (or, later, on-disk) fixture set.
/// Reports `FixtureVerified` for exactly the capabilities its loaded
/// fixtures cover — never a blanket claim.
#[derive(Debug, Clone, Default)]
pub struct FixtureProvider {
    fixtures: BTreeMap<String, Fixture>,
}

impl FixtureProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a fixture, validating its provenance first. Returns the
    /// validation error instead of silently accepting a malformed
    /// mainnet claim.
    pub fn with_fixture(mut self, fixture: Fixture) -> Result<Self, FixtureError> {
        fixture.validate()?;
        self.fixtures.insert(fixture.id.clone(), fixture);
        Ok(self)
    }

    #[must_use]
    pub fn fixture_count(&self) -> usize {
        self.fixtures.len()
    }
}

#[async_trait::async_trait]
impl HistoryProvider for FixtureProvider {
    fn capabilities(&self) -> SourceCapabilities {
        let mut caps = SourceCapabilities::empty();
        let status = if self.fixtures.is_empty() {
            CapabilityStatus::Unknown
        } else {
            CapabilityStatus::FixtureVerified
        };
        caps.by_capability
            .insert("token_market_activity".to_string(), status);
        caps.by_capability
            .insert("wallet_activity".to_string(), status);
        caps
    }

    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ScoutError> {
        Ok(ScanPlan {
            request_echo: format!("{request:?}"),
            capabilities: self.capabilities(),
        })
    }

    fn scan(
        &self,
        _task: ScanTask,
        _cancel: CancellationToken,
    ) -> BoxStream<'_, Result<ScanEnvelope, ScoutError>> {
        let envelopes: Vec<_> = self
            .fixtures
            .values()
            .map(|f| {
                Ok(ScanEnvelope {
                    raw_payload_description: f.id.clone(),
                })
            })
            .collect();
        Box::pin(stream::iter(envelopes))
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;

    #[test]
    fn mainnet_provenance_with_blank_tx_is_rejected() {
        // AGENTS.md: synthetic fixtures must never be presented as real
        // mainnet transactions. A Mainnet-tagged fixture with an empty
        // tx field is exactly that failure mode and must be rejected.
        let fixture = Fixture {
            id: "bad-fixture".to_string(),
            provenance: FixtureProvenance::Mainnet {
                chain: "base".to_string(),
                block_or_slot: "12345".to_string(),
                tx: String::new(),
                captured_at: "2026-09-01T00:00:00Z".to_string(),
                source: "manual capture".to_string(),
            },
        };
        let result = fixture.validate();
        assert!(matches!(
            result,
            Err(FixtureError::IncompleteMainnetProvenance { field: "tx", .. })
        ));
    }

    #[test]
    fn synthetic_provenance_never_requires_mainnet_fields() {
        let fixture = Fixture {
            id: "synthetic-c02".to_string(),
            provenance: FixtureProvenance::Synthetic {
                source: "hand-constructed for ACCEPTANCE C02".to_string(),
            },
        };
        assert!(fixture.validate().is_ok());
    }

    #[test]
    fn provider_with_no_fixtures_reports_unknown_not_fixture_verified() {
        let provider = FixtureProvider::new();
        let caps = provider.capabilities();
        assert_eq!(
            caps.status_for("wallet_activity"),
            CapabilityStatus::Unknown
        );
    }

    #[tokio::test]
    async fn provider_with_a_fixture_reports_fixture_verified_and_scans_it() {
        let fixture = Fixture {
            id: "synthetic-c01".to_string(),
            provenance: FixtureProvenance::Synthetic {
                source: "hand-constructed for ACCEPTANCE C01".to_string(),
            },
        };
        let provider = FixtureProvider::new().with_fixture(fixture).unwrap();
        assert_eq!(
            provider.capabilities().status_for("wallet_activity"),
            CapabilityStatus::FixtureVerified
        );
        let mut stream = provider.scan(
            ScanTask {
                description: "test".to_string(),
            },
            CancellationToken::new(),
        );
        let first = stream.next().await;
        assert!(matches!(first, Some(Ok(_))));
    }

    #[test]
    fn rejecting_a_bad_fixture_does_not_insert_it() {
        let bad_fixture = Fixture {
            id: "bad".to_string(),
            provenance: FixtureProvenance::Mainnet {
                chain: String::new(),
                block_or_slot: "1".to_string(),
                tx: "0xabc".to_string(),
                captured_at: "2026-09-01T00:00:00Z".to_string(),
                source: "x".to_string(),
            },
        };
        let result = FixtureProvider::new().with_fixture(bad_fixture);
        assert!(result.is_err());
    }
}
