//! Strict JSON -> typed parsing of EVM JSON-RPC objects (logs, receipts,
//! transactions). Provider data is external input (invariant #17): every
//! field is validated, nothing is trusted or defaulted silently.

use alloy_primitives::{Address, B256, Bytes, U256};
use scout_core::{EvmTxStatus, RawEvmLog};
use serde_json::Value;

use crate::evm_rpc::EvmSourceError;

pub(crate) fn malformed(what: &'static str, detail: impl Into<String>) -> EvmSourceError {
    EvmSourceError::Malformed {
        what,
        detail: detail.into(),
    }
}

fn field<'a>(v: &'a Value, key: &str, what: &'static str) -> Result<&'a Value, EvmSourceError> {
    v.get(key)
        .filter(|x| !x.is_null())
        .ok_or_else(|| malformed(what, format!("missing field `{key}`")))
}

fn opt_field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|x| !x.is_null())
}

fn hex_str<'a>(v: &'a Value, what: &'static str) -> Result<&'a str, EvmSourceError> {
    v.as_str()
        .and_then(|s| s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")))
        .ok_or_else(|| malformed(what, "expected 0x-prefixed hex string"))
}

pub(crate) fn quantity_u256(v: &Value, what: &'static str) -> Result<U256, EvmSourceError> {
    let h = hex_str(v, what)?;
    if h.is_empty() || h.len() > 64 {
        return Err(malformed(what, "quantity has invalid length"));
    }
    U256::from_str_radix(h, 16).map_err(|e| malformed(what, e.to_string()))
}

pub(crate) fn quantity_u64(v: &Value, what: &'static str) -> Result<u64, EvmSourceError> {
    u64::try_from(quantity_u256(v, what)?).map_err(|_| malformed(what, "quantity exceeds u64"))
}

pub(crate) fn address(v: &Value, what: &'static str) -> Result<Address, EvmSourceError> {
    let h = hex_str(v, what)?;
    let bytes = decode_hex(h, what)?;
    if bytes.len() != 20 {
        return Err(malformed(what, "address is not 20 bytes"));
    }
    Ok(Address::from_slice(&bytes))
}

pub(crate) fn b256(v: &Value, what: &'static str) -> Result<B256, EvmSourceError> {
    let h = hex_str(v, what)?;
    let bytes = decode_hex(h, what)?;
    if bytes.len() != 32 {
        return Err(malformed(what, "hash is not 32 bytes"));
    }
    Ok(B256::from_slice(&bytes))
}

fn decode_hex(h: &str, what: &'static str) -> Result<Vec<u8>, EvmSourceError> {
    if !h.len().is_multiple_of(2) {
        return Err(malformed(what, "odd-length hex"));
    }
    (0..h.len())
        .step_by(2)
        .map(|i| {
            h.get(i..i + 2)
                .and_then(|p| u8::from_str_radix(p, 16).ok())
                .ok_or_else(|| malformed(what, "invalid hex digit"))
        })
        .collect()
}

pub(crate) fn bytes(v: &Value, what: &'static str) -> Result<Bytes, EvmSourceError> {
    Ok(Bytes::from(decode_hex(hex_str(v, what)?, what)?))
}

/// Max topics/data a single log may carry (EVM: <= 4 topics; data bounded by
/// the response body cap, this is a sanity limit).
const MAX_TOPICS: usize = 4;

pub(crate) fn parse_log(v: &Value) -> Result<RawEvmLog, EvmSourceError> {
    const W: &str = "log";
    let topics = field(v, "topics", W)?
        .as_array()
        .ok_or_else(|| malformed(W, "`topics` is not an array"))?;
    if topics.len() > MAX_TOPICS {
        return Err(malformed(W, "more than 4 topics"));
    }
    Ok(RawEvmLog {
        address: address(field(v, "address", W)?, W)?,
        topics: topics
            .iter()
            .map(|t| b256(t, W))
            .collect::<Result<Vec<_>, _>>()?,
        data: bytes(field(v, "data", W)?, W)?,
        block_number: quantity_u64(field(v, "blockNumber", W)?, W)?,
        transaction_index: quantity_u64(field(v, "transactionIndex", W)?, W)?,
        log_index: quantity_u64(field(v, "logIndex", W)?, W)?,
    })
}

/// `(block, transaction index) -> transactionHash` of the logs that carry a
/// parseable hash (lenient: a log without one is simply absent, and the
/// caller falls back to block receipts for it).
pub(crate) fn log_tx_hashes(v: &Value) -> Vec<((u64, u64), B256)> {
    const W: &str = "log";
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter(|i| i.get("removed").and_then(Value::as_bool) != Some(true))
                .filter_map(|i| {
                    let h = b256(i.get("transactionHash")?, W).ok()?;
                    let b = quantity_u64(i.get("blockNumber")?, W).ok()?;
                    let t = quantity_u64(i.get("transactionIndex")?, W).ok()?;
                    Some(((b, t), h))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_logs(v: &Value) -> Result<Vec<RawEvmLog>, EvmSourceError> {
    let arr = v
        .as_array()
        .ok_or_else(|| malformed("logs", "result is not an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        // Reorged-out logs are never evidence.
        if item.get("removed").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        out.push(parse_log(item)?);
    }
    Ok(out)
}

/// Transaction fields from `eth_getTransactionByHash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmTxInfo {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub block_number: u64,
    pub transaction_index: u64,
    /// Legacy `gasPrice` (fallback when a receipt has no `effectiveGasPrice`).
    pub gas_price: Option<U256>,
}

pub(crate) fn parse_tx(v: &Value) -> Result<EvmTxInfo, EvmSourceError> {
    const W: &str = "transaction";
    Ok(EvmTxInfo {
        hash: b256(field(v, "hash", W)?, W)?,
        from: address(field(v, "from", W)?, W)?,
        to: opt_field(v, "to").map(|t| address(t, W)).transpose()?,
        value: quantity_u256(field(v, "value", W)?, W)?,
        block_number: quantity_u64(field(v, "blockNumber", W)?, W)?,
        transaction_index: quantity_u64(field(v, "transactionIndex", W)?, W)?,
        gas_price: opt_field(v, "gasPrice")
            .map(|g| quantity_u256(g, W))
            .transpose()?,
    })
}

/// Receipt fields needed for extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmReceiptInfo {
    pub tx_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub status: EvmTxStatus,
    pub gas_used: u64,
    pub effective_gas_price: Option<U256>,
    /// OP-stack `l1Fee`. `gasUsedForL1` (Arbitrum) is already inside
    /// `gas_used` and is intentionally not separated.
    pub l1_fee: Option<U256>,
    pub logs: Vec<RawEvmLog>,
    /// Receipt `from` / `to` when present (needed to decide whether an
    /// account is touched by other transactions of the same block).
    pub from: Option<Address>,
    pub to: Option<Address>,
}

pub(crate) fn parse_receipt(v: &Value) -> Result<EvmReceiptInfo, EvmSourceError> {
    const W: &str = "receipt";
    let status = match quantity_u64(field(v, "status", W)?, W)? {
        1 => EvmTxStatus::Success,
        0 => EvmTxStatus::Failed,
        other => return Err(malformed(W, format!("unknown status {other}"))),
    };
    let logs = parse_logs(field(v, "logs", W)?)?;
    Ok(EvmReceiptInfo {
        tx_hash: b256(field(v, "transactionHash", W)?, W)?,
        block_number: quantity_u64(field(v, "blockNumber", W)?, W)?,
        transaction_index: quantity_u64(field(v, "transactionIndex", W)?, W)?,
        status,
        gas_used: quantity_u64(field(v, "gasUsed", W)?, W)?,
        effective_gas_price: opt_field(v, "effectiveGasPrice")
            .map(|g| quantity_u256(g, W))
            .transpose()?,
        l1_fee: opt_field(v, "l1Fee")
            .map(|g| quantity_u256(g, W))
            .transpose()?,
        logs,
        from: opt_field(v, "from").map(|a| address(a, W)).transpose()?,
        to: opt_field(v, "to").map(|a| address(a, W)).transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_quantities_and_rejects_garbage() {
        assert_eq!(quantity_u64(&json!("0x10"), "t").unwrap(), 16);
        assert!(quantity_u64(&json!("16"), "t").is_err());
        assert!(quantity_u64(&json!("0x"), "t").is_err());
        assert!(quantity_u64(&json!("0x1ffffffffffffffff"), "t").is_err());
        assert!(quantity_u256(&json!(5), "t").is_err());
    }

    #[test]
    fn removed_logs_are_skipped_and_bad_logs_fail() {
        let good = json!({"address":"0x0000000000000000000000000000000000000001","topics":[],
            "data":"0x","blockNumber":"0x1","transactionIndex":"0x0","logIndex":"0x2"});
        let mut removed = good.clone();
        removed["removed"] = json!(true);
        let logs = parse_logs(&json!([good, removed])).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].log_index, 2);
        let mut bad = good;
        bad["address"] = json!("0x12");
        assert!(parse_logs(&json!([bad])).is_err());
    }
}
