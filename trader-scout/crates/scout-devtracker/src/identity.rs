//! Dev identity enrichment (ADR-021 amendment 1).
//!
//! A launchpad's creator field can be a contract. Measured 2026-10-07: shared
//! intermediaries (Flap VaultPortal, launcher services — a different signer
//! per launch) and single-operator bot contracts (one signer for every
//! launch, the same operator on BSC and Robinhood). The dev is the creator
//! field for an EOA and for a single-owner contract (its signer may be a
//! relayer/bundler shared by many users), the signer (`tx.from`) of each
//! launch through a shared intermediary — only on launchpads whose creator
//! field is the caller (`scout_devdb::SIGNER_RESOLVED_LAUNCHPADS`).
//!
//! Cost per pass: one `eth_getCode` per new creator, up to [`OWNER_SAMPLES`]
//! transaction lookups per new contract creator, one lookup per launch through
//! a shared intermediary.

use std::collections::BTreeSet;

use alloy_primitives::B256;
use futures::stream::{self, StreamExt};
use scout_devdb::{AddressKind, DevDb, DevDbError};
use scout_providers::{EvmRpcClient, EvmSourceError};
use serde_json::json;

/// Launches sampled to decide whether a contract creator has one owner.
pub const OWNER_SAMPLES: i64 = 8;

/// Failure of an enrichment pass.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("rpc: {0}")]
    Rpc(#[from] EvmSourceError),
    #[error("{0}")]
    Db(#[from] DevDbError),
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityReport {
    pub creators_checked: usize,
    pub contracts: usize,
    pub single_owner: usize,
    pub shared: usize,
    pub signers_resolved: usize,
}

async fn is_contract(rpc: &EvmRpcClient, address: &str) -> Result<bool, EvmSourceError> {
    let v = rpc
        .call_raw("eth_getCode", json!([address, "latest"]))
        .await?;
    Ok(v.as_str().is_some_and(|c| c.len() > 2))
}

async fn signer_of(rpc: &EvmRpcClient, tx_hash: &str) -> Result<Option<String>, EvmSourceError> {
    let Ok(h) = tx_hash.parse::<B256>() else {
        return Ok(None);
    };
    Ok(Some(format!(
        "{:#x}",
        rpc.transaction_by_hash(h).await?.from
    )))
}

/// Classify up to `max_creators` new creators of `chain`, then resolve up to
/// `max_signers` signers of launches through shared intermediaries.
///
/// # Errors
/// RPC (budget, transport) or database failure; facts written so far stay.
pub async fn enrich_identities(
    db: &DevDb,
    rpc: &EvmRpcClient,
    chain: &str,
    max_creators: i64,
    max_signers: i64,
    now: i64,
) -> Result<IdentityReport, IdentityError> {
    let mut report = IdentityReport::default();
    let concurrency = rpc.config().concurrency.max(1);
    let creators = db.creators_without_kind(chain, max_creators).await?;
    report.creators_checked = creators.len();
    let kinds: Vec<Result<(String, bool), EvmSourceError>> = stream::iter(creators)
        .map(|c| async move { is_contract(rpc, &c).await.map(|k| (c, k)) })
        .buffered(concurrency)
        .collect()
        .await;
    for k in kinds {
        let (address, contract) = k?;
        let mut kind = AddressKind {
            chain: chain.to_string(),
            address: address.clone(),
            is_contract: contract,
            owner: None,
            sampled: 0,
            checked_at: now,
        };
        if contract {
            report.contracts += 1;
            let sample = db.launches_of(chain, &address, OWNER_SAMPLES).await?;
            let signers: Vec<Result<Option<String>, EvmSourceError>> = stream::iter(sample)
                .map(|(_, h)| async move { signer_of(rpc, &h).await })
                .buffered(concurrency)
                .collect()
                .await;
            let mut distinct = BTreeSet::new();
            for s in signers {
                if let Some(s) = s? {
                    distinct.insert(s);
                    kind.sampled = kind.sampled.saturating_add(1);
                }
            }
            if distinct.len() == 1 {
                kind.owner = distinct.into_iter().next();
                report.single_owner += 1;
            } else {
                report.shared += 1;
            }
        }
        db.set_address_kind(&kind).await?;
    }
    let pending = db
        .shared_launches_without_signer(chain, max_signers)
        .await?;
    let resolved: Vec<Result<(String, Option<String>), EvmSourceError>> = stream::iter(pending)
        .map(|(token, h)| async move { signer_of(rpc, &h).await.map(|s| (token, s)) })
        .buffered(concurrency)
        .collect()
        .await;
    for r in resolved {
        let (token, signer) = r?;
        if let Some(s) = signer {
            db.set_launch_signer(chain, &token, &s).await?;
            report.signers_resolved += 1;
        }
    }
    Ok(report)
}
