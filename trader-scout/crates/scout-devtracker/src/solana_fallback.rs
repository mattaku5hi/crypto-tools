//! Solana ingestion over standard RPC methods (A8): the fallback when Helius
//! fails. `getSignaturesForAddress` (newest first, 1,000 per page) down to the
//! window start, then `getTransaction` per successful signature in the window;
//! the event-CPI instructions are decoded exactly as on the Helius path
//! (`solana_ingest::facts_from`). Measured 2026-10-07: the keyless public RPC
//! (`https://api.mainnet-beta.solana.com`) serves both methods; the owner's
//! Ankr key has no Solana (HTTP 403) and dRPC has no Solana on the free plan.
//! Steady state needs ≈ 0.6 `getTransaction`/s (≈ 55k transactions/day), well
//! within the public RPC's per-IP limits; a long outage is caught up in
//! windows of at most `max_window_secs`.

use futures::stream::{self, StreamExt};
use scout_core::RawSolanaInstruction;
use scout_devdb::DevDb;
use scout_rpc::RpcClient;
use serde_json::{Value, json};

use crate::solana_ingest::{SolanaIngestError, SolanaIngestReport, SolanaSource, facts_from};

/// The keyless public mainnet RPC.
pub const PUBLIC_SOLANA_RPC: &str = "https://api.mainnet-beta.solana.com";
/// Signature pages read per window before giving up (1,000 each).
const MAX_SIGNATURE_PAGES: usize = 200;

static NULL: Value = Value::Null;

/// `v` at a JSON pointer, `null` when absent (no panicking index).
fn at<'a>(v: &'a Value, pointer: &str) -> &'a Value {
    v.pointer(pointer).unwrap_or(&NULL)
}

fn bad(what: &str) -> SolanaIngestError {
    SolanaIngestError::Fallback(what.to_string())
}

fn pubkey(s: &str) -> Option<[u8; 32]> {
    bs58::decode(s).into_vec().ok()?.try_into().ok()
}

/// Flattened instructions (top level, each followed by its inner ones) of a
/// `getTransaction` result with `encoding: json`.
fn instructions_of(result: &Value) -> Result<Vec<RawSolanaInstruction>, SolanaIngestError> {
    let msg = &at(result, "/transaction/message");
    let meta = &at(result, "/meta");
    let mut keys: Vec<[u8; 32]> = Vec::new();
    let mut add = |list: &Value| -> Result<(), SolanaIngestError> {
        for k in list.as_array().into_iter().flatten() {
            keys.push(
                k.as_str()
                    .and_then(pubkey)
                    .ok_or_else(|| bad("account key is not a pubkey"))?,
            );
        }
        Ok(())
    };
    add(at(msg, "/accountKeys"))?;
    add(at(meta, "/loadedAddresses/writable"))?;
    add(at(meta, "/loadedAddresses/readonly"))?;
    let raw = |ix: &Value, index: u32| -> Result<RawSolanaInstruction, SolanaIngestError> {
        let key = |v: &Value| -> Result<[u8; 32], SolanaIngestError> {
            v.as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| keys.get(i).copied())
                .ok_or_else(|| bad("instruction account index out of range"))
        };
        Ok(RawSolanaInstruction {
            program_id: key(at(ix, "/programIdIndex"))?,
            accounts: at(ix, "/accounts")
                .as_array()
                .into_iter()
                .flatten()
                .map(key)
                .collect::<Result<_, _>>()?,
            data: at(ix, "/data")
                .as_str()
                .and_then(|d| bs58::decode(d).into_vec().ok())
                .ok_or_else(|| bad("instruction data is not base58"))?,
            instruction_index: index,
        })
    };
    let mut out = Vec::new();
    let mut next = 0u32;
    let inner = at(meta, "/innerInstructions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (top, ix) in at(msg, "/instructions")
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push(raw(ix, next)?);
        next = next.saturating_add(1);
        for group in inner
            .iter()
            .filter(|g| at(g, "/index").as_u64() == u64::try_from(top).ok())
        {
            for ix in at(group, "/instructions").as_array().into_iter().flatten() {
                out.push(raw(ix, next)?);
                next = next.saturating_add(1);
            }
        }
    }
    Ok(out)
}

/// Successful signatures of `address` with `from <= blockTime < to`.
async fn signatures_in(
    rpc: &RpcClient,
    address: &str,
    from: i64,
    to: i64,
) -> Result<Vec<String>, SolanaIngestError> {
    let mut out = Vec::new();
    let mut before: Option<String> = None;
    for _ in 0..MAX_SIGNATURE_PAGES {
        let mut opts = json!({"limit": 1000});
        if let (Some(b), Some(o)) = (&before, opts.as_object_mut()) {
            o.insert("before".into(), json!(b));
        }
        let page: Vec<Value> = rpc
            .call("getSignaturesForAddress", json!([address, opts]))
            .await?;
        let Some(last) = page.last() else {
            return Ok(out);
        };
        before = at(last, "/signature").as_str().map(String::from);
        let mut reached_start = false;
        for s in &page {
            let t = at(s, "/blockTime").as_i64();
            if t.is_some_and(|t| t < from) {
                reached_start = true;
                continue;
            }
            if let Some(sig) = at(s, "/signature").as_str()
                && t.is_some_and(|t| t < to)
                && at(s, "/err").is_null()
            {
                out.push(sig.to_string());
            }
        }
        if reached_start || before.is_none() {
            return Ok(out);
        }
    }
    Err(bad("signature listing exceeded its page budget"))
}

/// Read and store `[from, to)` of one source over standard RPC.
///
/// # Errors
/// RPC, decoding or database failure (nothing is marked done).
pub async fn fallback_scan_window(
    db: &DevDb,
    rpc: &RpcClient,
    src: &SolanaSource,
    from: i64,
    to: i64,
    concurrency: usize,
) -> Result<SolanaIngestReport, SolanaIngestError> {
    let mut report = SolanaIngestReport {
        from,
        to,
        ..SolanaIngestReport::default()
    };
    let sigs = signatures_in(rpc, src.address, from, to).await?;
    let txs: Vec<Result<(String, Value), SolanaIngestError>> = stream::iter(sigs)
        .map(|sig| async move {
            let v: Value = rpc
                .call(
                    "getTransaction",
                    json!([sig, {"encoding": "json", "maxSupportedTransactionVersion": 1}]),
                )
                .await?;
            Ok((sig, v))
        })
        .buffered(concurrency.max(1))
        .collect()
        .await;
    let (mut launches, mut migrations) = (Vec::new(), Vec::new());
    for t in txs {
        let (sig, v) = t?;
        if v.is_null() || !at(&v, "/meta/err").is_null() {
            continue;
        }
        report.transactions += 1;
        let slot = at(&v, "/slot")
            .as_u64()
            .ok_or_else(|| bad("transaction without slot"))?;
        let (l, m, undecodable) = facts_from(
            &instructions_of(&v)?,
            slot,
            at(&v, "/blockTime").as_i64(),
            &sig,
            src.kind,
            src.key,
        );
        report.decoded += l.len() + m.len();
        report.undecodable += undecodable;
        launches.extend(l);
        migrations.extend(m);
    }
    report.inserted += db.insert_launches(&launches).await?;
    report.inserted += db.insert_migrations(&migrations).await?;
    Ok(report)
}

/// The fallback counterpart of `ingest_solana_source`: from the source's
/// cursor, at most `max_window_secs` per pass, then the cursor moves.
///
/// # Errors
/// As [`fallback_scan_window`].
pub async fn fallback_ingest_source(
    db: &DevDb,
    rpc: &RpcClient,
    src: &SolanaSource,
    settle_secs: i64,
    max_window_secs: i64,
    now: i64,
) -> Result<SolanaIngestReport, SolanaIngestError> {
    let Some(from) = db
        .cursor(src.key)
        .await?
        .map(|c| c.parse::<i64>())
        .transpose()
        .map_err(|_| SolanaIngestError::BadCursor(src.key.to_string()))?
    else {
        return Err(bad(
            "no cursor yet: the first pass needs the primary provider",
        ));
    };
    let to = now
        .saturating_sub(settle_secs)
        .min(from.saturating_add(max_window_secs.max(60)));
    if from >= to {
        return Ok(SolanaIngestReport {
            from,
            to,
            ..SolanaIngestReport::default()
        });
    }
    let report = fallback_scan_window(db, rpc, src, from, to, 4).await?;
    db.set_cursor(src.key, &to.to_string(), now).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn instructions_are_flattened_like_the_helius_path() {
        let k = |b: u8| bs58::encode([b; 32]).into_string();
        let v = json!({
            "transaction": {"message": {
                "accountKeys": [k(1), k(2)],
                "instructions": [
                    {"programIdIndex": 1, "accounts": [0], "data": bs58::encode([7u8]).into_string()},
                    {"programIdIndex": 0, "accounts": [], "data": ""}
                ]
            }},
            "meta": {
                "loadedAddresses": {"writable": [k(3)], "readonly": []},
                "innerInstructions": [{"index": 0, "instructions": [
                    {"programIdIndex": 2, "accounts": [1, 2], "data": bs58::encode([9u8, 9]).into_string()}
                ]}]
            }
        });
        let ixs = instructions_of(&v).unwrap();
        assert_eq!(ixs.len(), 3);
        assert_eq!(ixs[0].program_id, [2; 32]);
        assert_eq!(ixs[0].data, [7]);
        assert_eq!(
            ixs[1].program_id, [3; 32],
            "inner instruction right after its parent"
        );
        assert_eq!(ixs[1].accounts, vec![[2; 32], [3; 32]]);
        assert_eq!(ixs[2].instruction_index, 2);
        let mut broken = v.clone();
        broken["meta"]["innerInstructions"][0]["instructions"][0]["programIdIndex"] = json!(9);
        assert!(instructions_of(&broken).is_err());
    }
}
