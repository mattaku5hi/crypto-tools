//! Approximate per-method cost weights for client-side rate limiting
//! (`--rpc-cu-per-sec`), in Alchemy compute units.
//!
//! APPROXIMATE: the numbers follow Alchemy's published compute-unit table as
//! remembered by the authors; they are not measured in this repository and a
//! provider may change them. They only matter when CU weighting is switched
//! on explicitly (`--rpc-cu-per-sec N`); the default limiter counts one unit
//! per request. Other providers price methods differently - use the plain
//! `--rpc-rps` limiter for them. Unknown methods cost
//! [`DEFAULT_METHOD_CU`].

/// Weight of a method that is not in the table.
pub const DEFAULT_METHOD_CU: u64 = 50;

/// Approximate compute units of a JSON-RPC method (never 0).
#[must_use]
pub fn approx_method_cu(method: &str) -> u64 {
    match method {
        "eth_chainId" => 1,
        "eth_blockNumber" => 10,
        "eth_getBlockByNumber" => 16,
        "eth_getBalance" => 19,
        "eth_getTransactionByHash" | "eth_getTransactionReceipt" => 15,
        "eth_call" => 26,
        "eth_getLogs" => 75,
        "eth_getBlockReceipts" => 500,
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
    }
}
