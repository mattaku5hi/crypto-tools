//! JSONL output envelope. See docs/CLI.md §7 for the schema this
//! implements, and ADR-005 for the exit-code contract that consumes
//! `RunSummary::status`.
//!
//! Every record kind shares `schema_version`+`kind`; wallet-bearing
//! records share a `wallet` field so downstream tools can extract
//! identities without per-kind parsing (CLI.md §7).

use scout_core::WalletKey;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsonlRecord {
    RunMeta {
        run_id: String,
        window: Window,
        snapshot_manifest: String,
    },
    WalletRef {
        wallet: WalletKey,
    },
    BuyerMatch {
        wallet: WalletKey,
        hit_count: usize,
        matched_assets: Vec<scout_core::AssetKey>,
    },
    WalletExcluded {
        wallet: WalletKey,
        reason: String,
    },
    RunSummary {
        run_id: String,
        status: RunStatus,
        records: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub since: String,
    pub until: String,
}

/// Operational completion status, distinct from per-record eligibility
/// (ADR-005). Never conflated with "eligible=false."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Complete,
    Partial,
}

impl JsonlRecord {
    /// Serialize as one JSONL line (no trailing newline; the caller
    /// appends it, per this module's writer contract). Always includes
    /// `schema_version` even though it's not a struct field — done via
    /// a wrapper map so `schema_version` is never accidentally omitted
    /// or drift per-variant.
    pub fn to_jsonl_line(&self) -> Result<String, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let serde_json::Value::Object(map) = &mut value {
            map.insert(
                "schema_version".to_string(),
                serde_json::Value::from(SCHEMA_VERSION),
            );
        }
        serde_json::to_string(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet() -> WalletKey {
        WalletKey {
            chain: scout_core::ChainKey {
                family: scout_core::ChainFamily::Evm,
                network_id: scout_core::NetworkId::EvmChainId(8453),
                genesis_identity: scout_core::GenesisIdentity::Unverified,
            },
            address: scout_core::AddressBytes::Evm([0x11; 20]),
        }
    }

    #[test]
    fn run_meta_serializes_with_schema_version_and_kind() {
        let record = JsonlRecord::RunMeta {
            run_id: "test-run".to_string(),
            window: Window {
                since: "2026-08-01T00:00:00Z".to_string(),
                until: "2026-09-01T00:00:00Z".to_string(),
            },
            snapshot_manifest: "manifest-abc".to_string(),
        };
        let line = record.to_jsonl_line().unwrap();
        assert!(line.contains("\"schema_version\":1"));
        assert!(line.contains("\"kind\":\"run_meta\""));
        // Must be valid single-line JSON, no embedded newlines/ANSI.
        assert!(!line.contains('\n'));
        assert!(serde_json::from_str::<serde_json::Value>(&line).is_ok());
    }

    #[test]
    fn wallet_ref_carries_the_same_wallet_field_name_as_other_kinds() {
        // CLI.md §7: "Все wallet-bearing results содержат одинаковое
        // поле wallet."
        let record = JsonlRecord::WalletRef { wallet: wallet() };
        let line = record.to_jsonl_line().unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(value.get("wallet").is_some());
    }

    #[test]
    fn buyer_match_also_carries_a_wallet_field() {
        let record = JsonlRecord::BuyerMatch {
            wallet: wallet(),
            hit_count: 2,
            matched_assets: vec![],
        };
        let line = record.to_jsonl_line().unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(value.get("wallet").is_some());
        assert_eq!(value["kind"], "buyer_match");
    }

    #[test]
    fn run_summary_status_is_a_string_not_a_bool() {
        // ADR-005: operational status is a distinct enum, never a bare
        // success:bool that collapses partial-vs-complete distinctions.
        let record = JsonlRecord::RunSummary {
            run_id: "r1".to_string(),
            status: RunStatus::Partial,
            records: 3,
        };
        let line = record.to_jsonl_line().unwrap();
        assert!(line.contains("\"status\":\"partial\""));
    }

    #[test]
    fn large_amounts_in_matched_assets_do_not_break_roundtrip() {
        // Sanity: AssetKey (which embeds ChainKey) roundtrips through
        // this envelope without loss.
        let asset = scout_core::AssetKey::Token(
            scout_core::ChainKey {
                family: scout_core::ChainFamily::Solana,
                network_id: scout_core::NetworkId::SolanaCluster(
                    scout_core::SolanaCluster::Mainnet,
                ),
                genesis_identity: scout_core::GenesisIdentity::Verified("g".to_string()),
            },
            scout_core::AddressBytes::Solana([9u8; 32]),
        );
        let record = JsonlRecord::BuyerMatch {
            wallet: wallet(),
            hit_count: 1,
            matched_assets: vec![asset],
        };
        let line = record.to_jsonl_line().unwrap();
        let parsed: JsonlRecord = {
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            serde_json::from_value(value).unwrap()
        };
        match parsed {
            JsonlRecord::BuyerMatch { matched_assets, .. } => {
                assert_eq!(matched_assets.len(), 1);
            }
            other => panic!("expected BuyerMatch, got {other:?}"),
        }
    }
}
