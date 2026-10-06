//! Approximate per-method cost weights for client-side rate limiting
//! (`--rpc-cu-per-sec`), in Alchemy compute units.
//!
//! The numbers are Alchemy's THROUGHPUT weights (the published compute-unit
//! table, fetched 2026-10-06): what counts against the CU/s limit. Billing can
//! differ — `eth_getBlockReceipts` is billed 20 CU but weighs 500 against
//! throughput. A provider may change them; they are not measured here. They only matter when CU weighting is switched
//! on explicitly (`--rpc-cu-per-sec N`); the default limiter counts one unit
//! per request. Other providers price methods differently - use the plain
//! `--rpc-rps` limiter for them. Unknown methods cost
//! [`DEFAULT_METHOD_CU`].

/// Alchemy Enhanced API method listing transfers by address.
pub const ALCHEMY_TRANSFERS_METHOD: &str = "alchemy_getAssetTransfers";
/// Approximate weight of one `alchemy_getAssetTransfers` page.
pub const ALCHEMY_TRANSFERS_CU: u64 = 120;

/// Weight of a method that is not in the table.
pub const DEFAULT_METHOD_CU: u64 = 50;

/// Approximate compute units of a JSON-RPC method (never 0).
#[must_use]
pub fn approx_method_cu(method: &str) -> u64 {
    match method {
        "eth_chainId" => 1,
        "eth_blockNumber" => 10,
        "eth_getBlockByNumber" => 16,
        "eth_getBalance" | "eth_getTransactionReceipt" => 20,
        "eth_getTransactionByHash" => 15,
        "eth_call" => 26,
        "eth_getLogs" => 60,
        // throughput weight; billed 20 CU
        "eth_getBlockReceipts" => 500,
        // Enhanced API, one page of up to 1,000 transfers (approximate).
        ALCHEMY_TRANSFERS_METHOD => ALCHEMY_TRANSFERS_CU,
        "trace_transaction" => 90,
        "debug_traceTransaction" => 309,
        _ => DEFAULT_METHOD_CU,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_positive_and_block_receipts_are_the_heavy_call() {
        for m in ["eth_chainId", "eth_getLogs", "something_else"] {
            assert!(approx_method_cu(m) >= 1);
        }
        assert!(approx_method_cu("eth_getBlockReceipts") > approx_method_cu("eth_getLogs"));
        assert_eq!(approx_method_cu("nope"), DEFAULT_METHOD_CU);
        assert_eq!(approx_method_cu(ALCHEMY_TRANSFERS_METHOD), 120);
    }
}
