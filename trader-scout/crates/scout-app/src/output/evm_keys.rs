//! EVM spelling of the shared ledger JSON (ADR-020 step 2).
//!
//! The wallet-stats / wallet-rank record DTOs are the Solana-era shapes whose
//! native-currency fields are named after lamports and SOL. On an EVM chain
//! the SAME fields carry wei and the chain's native currency (ETH), already
//! formatted with the chain's own decimals; this pass renames the keys (and
//! the unit labels) of the serialized record so no EVM output ever calls wei
//! "lamports". Pure and deterministic: a segment-wise rename of object keys
//! (`realized_net_pnl_sol` -> `realized_net_pnl_eth`, `lamports` -> `wei`)
//! plus the `route_swaps_by_quote` object, which on EVM lists the native and
//! USDG/USDC counts only (zero USDC dropped). Values are never touched except unit labels.

use serde_json::Value;

/// Keys whose string value is a unit label.
const UNIT_KEYS: [&str; 5] = ["quote_unit", "unit", "pnl_unit", "headline_unit", "quote"];

fn map_segment<'a>(seg: &'a str, native_label: &'a str) -> &'a str {
    match seg {
        "lamports" => "wei",
        "sol" => native_label,
        other => other,
    }
}

fn rename_key(key: &str, native_label: &str) -> String {
    key.split('_')
        .map(|s| map_segment(s, native_label))
        .collect::<Vec<_>>()
        .join("_")
}

/// Rewrite `v` in place for an EVM chain whose native label is
/// `native_label` (`eth`, `bnb`).
pub fn evm_spelling(v: &mut Value, native_label: &str) {
    match v {
        Value::Object(map) => {
            let old = std::mem::take(map);
            for (k, mut child) in old {
                if k == "route_swaps_by_quote"
                    && let Value::Object(counts) = &mut child
                {
                    // USDC is a real quote unit on Base: dropped only when
                    // unused (Robinhood), like the Solana-only USDT.
                    if counts.get("usdc").and_then(Value::as_u64) == Some(0) {
                        counts.remove("usdc");
                    }
                    counts.remove("usdt");
                }
                if k == "route_swaps_usdc" || k == "route_swaps_usdt" {
                    continue; // Solana-only quote units
                }
                if UNIT_KEYS.contains(&k.as_str())
                    && let Value::String(s) = &mut child
                {
                    *s = map_segment(s, native_label).to_string();
                }
                evm_spelling(&mut child, native_label);
                map.insert(rename_key(&k, native_label), child);
            }
        }
        Value::Array(items) => {
            for i in items {
                evm_spelling(i, native_label);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn renames_native_keys_and_unit_labels_only() {
        let mut v = json!({
            "quote_unit": "lamports",
            "realized_net_pnl": {"status": "observed", "lamports": "5", "sol": "0.5", "sol_exact": "1"},
            "realized_trade_pnl_sol_exact": "1",
            "failed_trade_fees": {"lamports": "0", "sol": "0.0"},
            "route": {"route_swaps_by_quote": {"sol": 2, "usdc": 0, "usdt": 0, "usdg": 1}},
            "quote_units": [{"unit": "sol", "decimals": 18}, {"unit": "usdg"}],
            "wallet": {"chain": "robinhood", "address": "0xsol"},
            "mint": "solana_is_untouched"
        });
        evm_spelling(&mut v, "eth");
        assert_eq!(v["quote_unit"], "wei");
        assert_eq!(v["realized_net_pnl"]["wei"], "5");
        assert_eq!(v["realized_net_pnl"]["eth"], "0.5");
        assert_eq!(v["realized_net_pnl"]["eth_exact"], "1");
        assert_eq!(v["realized_trade_pnl_eth_exact"], "1");
        assert_eq!(v["failed_trade_fees"]["wei"], "0");
        assert_eq!(
            v["route"]["route_swaps_by_quote"],
            json!({"eth": 2, "usdg": 1})
        );
        assert_eq!(v["quote_units"][0]["unit"], "eth");
        assert_eq!(v["quote_units"][1]["unit"], "usdg");
        // A used USDC count (Base) is kept.
        let mut b = json!({"route_swaps_by_quote": {"sol": 1, "usdc": 3, "usdt": 0}});
        evm_spelling(&mut b, "eth");
        assert_eq!(b["route_swaps_by_quote"], json!({"eth": 1, "usdc": 3}));
        // Values other than unit labels are never rewritten.
        assert_eq!(v["wallet"]["address"], "0xsol");
        assert_eq!(v["mint"], "solana_is_untouched");
        assert!(v.get("realized_net_pnl").unwrap().get("lamports").is_none());
    }
}
