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
//! Which launches get a signer lookup (ADR-021 amendment 3, owner decision
//! 2026-10-07): every migrated launch through an intermediary, and every
//! launch younger than [`RECENT_SIGNER_WINDOW`] (so new launches are always
//! attributed); on chains with few such launches ([`resolves_every_signer`])
//! all of them. An older non-migrated launch is attributed only through its
//! dev's own history: once a dev has a migrated launch through an intermediary
//! (it can qualify), its transactions to every intermediary are listed once
//! (`alchemy_getAssetTransfers`, [`history_supported`]) and matched by hash.
//! A dev without any migration stays unattributed; it cannot qualify.
//!
//! Only creators with at least two launches are classified (a single launch
//! is its creator's own). Cost per pass: one `eth_getCode` per new repeat
//! creator, up to [`OWNER_SAMPLES`] lookups per new contract creator, one
//! lookup per queued launch, ≈ one `alchemy_getAssetTransfers` per
//! intermediary and new candidate dev.

use std::collections::BTreeSet;

use alloy_primitives::B256;
use futures::stream::{self, StreamExt};
use scout_devdb::{AddressKind, DevDb, DevDbError};
use scout_providers::{EvmRpcClient, EvmSourceError};
use serde_json::json;

/// Launches sampled to decide whether a contract creator has one owner.
pub const OWNER_SAMPLES: i64 = 4;

/// Launches younger than this always get a signer lookup.
pub const RECENT_SIGNER_WINDOW: i64 = 7 * 86_400;

/// Chains whose launches through intermediaries are few enough to resolve
/// every signer (Robinhood: ≈ 270/day).
#[must_use]
pub fn resolves_every_signer(chain: &str) -> bool {
    chain == "robinhood"
}

/// Chains where `alchemy_getAssetTransfers` lists a dev's transactions.
#[must_use]
pub fn history_supported(chain: &str) -> bool {
    chain == "bsc"
}

/// Failure of an enrichment pass.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("rpc: {0}")]
    Rpc(#[from] EvmSourceError),
    #[error("{0}")]
    Db(#[from] DevDbError),
}

/// Per-pass limits of [`enrich_identities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityBudget {
    /// New repeat creators classified.
    pub max_creators: i64,
    /// Queued launch signers looked up.
    pub max_signers: i64,
    /// Candidate devs whose history is fetched (0 = none).
    pub max_histories: i64,
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityReport {
    pub creators_checked: usize,
    pub contracts: usize,
    pub single_owner: usize,
    pub shared: usize,
    pub signers_resolved: usize,
    pub histories_fetched: usize,
    pub launches_from_histories: u64,
}

async fn is_contract(rpc: &EvmRpcClient, address: &str) -> Result<bool, EvmSourceError> {
    let v = rpc
        .call_raw("eth_getCode", json!([address, "latest"]))
        .await?;
    Ok(v.as_str().is_some_and(|c| c.len() > 2))
}

/// The signer of a launch transaction. A provider answering `null` (dRPC does
/// for some known transactions) is asked again through `retry` (Alchemy);
/// still unknown → `None` (the launch stays unresolved and is retried later).
async fn signer_of(
    rpc: &EvmRpcClient,
    retry: Option<&EvmRpcClient>,
    tx_hash: &str,
) -> Result<Option<String>, EvmSourceError> {
    let Ok(h) = tx_hash.parse::<B256>() else {
        return Ok(None);
    };
    let first = match rpc.transaction_by_hash(h).await {
        Ok(tx) => return Ok(Some(format!("{:#x}", tx.from))),
        Err(EvmSourceError::NotFound { .. }) => None,
        Err(e) => Some(e),
    };
    if let Some(e) = first {
        return Err(e);
    }
    match retry {
        Some(r) => match r.transaction_by_hash(h).await {
            Ok(tx) => Ok(Some(format!("{:#x}", tx.from))),
            Err(EvmSourceError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        },
        None => Ok(None),
    }
}

/// Classify up to `max_creators` new creators of `chain`, then resolve up to
/// `max_signers` signers of launches through shared intermediaries.
/// `retry` serves transaction lookups `rpc` answers with `null` (the backfill
/// runs `rpc` on dRPC and retries on Alchemy).
///
/// # Errors
/// RPC (budget, transport) or database failure; facts written so far stay.
pub async fn enrich_identities(
    db: &DevDb,
    rpc: &EvmRpcClient,
    retry: Option<&EvmRpcClient>,
    chain: &str,
    budget: IdentityBudget,
    now: i64,
) -> Result<IdentityReport, IdentityError> {
    let IdentityBudget {
        max_creators,
        max_signers,
        max_histories,
    } = budget;
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
                .map(|(_, h)| async move { signer_of(rpc, retry, &h).await })
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
    let recent_since = if resolves_every_signer(chain) {
        i64::MIN
    } else {
        now.saturating_sub(RECENT_SIGNER_WINDOW)
    };
    let pending = db
        .shared_launches_without_signer(chain, max_signers, recent_since)
        .await?;
    let resolved: Vec<Result<(String, Option<String>), EvmSourceError>> = stream::iter(pending)
        .map(|(token, h)| async move { signer_of(rpc, retry, &h).await.map(|s| (token, s)) })
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
    if history_supported(chain) && max_histories > 0 {
        let devs = db.history_candidates(chain, max_histories).await?;
        if !devs.is_empty() {
            let intermediaries = db.shared_intermediaries(chain).await?;
            for dev in devs {
                let mut attributed = 0u64;
                for inter in &intermediaries {
                    let hashes = transactions_to(rpc, &dev, inter).await?;
                    if !hashes.is_empty() {
                        attributed += db.set_signer_by_txs(chain, inter, &hashes, &dev).await?;
                    }
                }
                db.mark_history_fetched(
                    chain,
                    &dev,
                    i32::try_from(attributed).unwrap_or(i32::MAX),
                    now,
                )
                .await?;
                report.histories_fetched += 1;
                report.launches_from_histories += attributed;
            }
        }
    }
    Ok(report)
}

/// Pages of `alchemy_getAssetTransfers` per (dev, intermediary) listing.
const MAX_TRANSFER_PAGES: usize = 50;

/// Hashes of every transaction `from` sent to `to` (top-level calls,
/// zero-value ones included), via `alchemy_getAssetTransfers`.
async fn transactions_to(
    rpc: &EvmRpcClient,
    from: &str,
    to: &str,
) -> Result<Vec<String>, EvmSourceError> {
    let mut out = Vec::new();
    let mut page_key: Option<String> = None;
    for _ in 0..MAX_TRANSFER_PAGES {
        let mut params = json!({
            "fromBlock": "0x0",
            "toBlock": "latest",
            "fromAddress": from,
            "toAddress": to,
            "category": ["external"],
            "excludeZeroValue": false,
            "withMetadata": false,
            "maxCount": "0x3e8",
        });
        if let (Some(k), Some(obj)) = (&page_key, params.as_object_mut()) {
            obj.insert("pageKey".into(), json!(k));
        }
        let v = rpc
            .call_raw("alchemy_getAssetTransfers", json!([params]))
            .await?;
        if let Some(ts) = v.get("transfers").and_then(serde_json::Value::as_array) {
            out.extend(
                ts.iter()
                    .filter_map(|t| t.get("hash").and_then(serde_json::Value::as_str))
                    .map(str::to_ascii_lowercase),
            );
        }
        page_key = v
            .get("pageKey")
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        if page_key.is_none() {
            break;
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}
