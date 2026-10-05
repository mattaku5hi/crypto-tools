//! Multi-chain runs of the three CLIs (docs/CLI.md §2, ARCHITECTURE §10).
//!
//! A run whose input spans several chains is PARTITIONED by chain (the first
//! appearance order of chains and of wallets/tokens inside a chain is kept),
//! every chain runs with its own sources, settings and request budget
//! (`--max-requests` is per chain run: budgets belong to provider families),
//! and the outputs are merged by the binaries. Identity stays chain-scoped
//! (AGENTS.md invariant 3): the same address on two chains is two wallets and
//! no hit, rank or card is ever merged across chains. Cross-chain comparison
//! exists only in USD (`--quote usd`, ADR-016/018).

use scout_core::{AddressBytes, ChainFamily, ChainKey, NetworkId, WalletKey};
use scout_sdk::engine::{
    ChainDisplay, SOLANA_DISPLAY, SolanaWalletStats, WalletScanStatus, evm_key,
};

use crate::chain_profile_name;

/// `--chain-concurrency` default: chains run at once.
pub const DEFAULT_CHAIN_CONCURRENCY: u32 = 2;
/// `--chain-concurrency` upper bound (there are four chain profiles).
pub const MAX_CHAIN_CONCURRENCY: u32 = 4;

/// Inputs of one chain: indices into the original input list, in input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainGroup {
    pub chain: ChainKey,
    /// Input-syntax chain name (`solana`, `base`, ...).
    pub name: &'static str,
    pub indices: Vec<usize>,
}

/// Input-syntax name of a chain (`unknown` for a chain without a profile).
#[must_use]
pub fn chain_name(chain: &ChainKey) -> &'static str {
    chain_profile_name(chain).unwrap_or("unknown")
}

/// Partition `items` by chain: groups in the first-appearance order of their
/// chain, indices in input order.
pub fn partition_by_chain<'a, T: 'a>(
    items: &'a [T],
    chain_of: impl Fn(&'a T) -> &'a ChainKey,
) -> Vec<ChainGroup> {
    let mut groups: Vec<ChainGroup> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let c = chain_of(item);
        match groups
            .iter_mut()
            .find(|g| g.chain.family == c.family && g.chain.network_id == c.network_id)
        {
            Some(g) => g.indices.push(i),
            None => groups.push(ChainGroup {
                chain: c.clone(),
                name: chain_name(c),
                indices: vec![i],
            }),
        }
    }
    groups
}

/// Outcome of ONE chain's run, produced by a binary's per-chain function
/// instead of writing to stdout (the caller writes, single- or multi-chain).
#[derive(Debug, Clone, Default)]
pub struct ChainRun {
    /// Rendered stdout lines of the selected `--format` (empty on failure).
    pub lines: Vec<String>,
    /// Exit code the run earns after a successful write: 0, 2, 3 or 4.
    pub status: u8,
    /// The run was cancelled (130, wins over every other code).
    pub cancelled: bool,
    /// HTTP attempts of this chain's run (retries included).
    pub requests_made: u64,
    /// The run produced no output at all (setup/config/provider failure);
    /// the text was already printed on stderr.
    pub failure: Option<String>,
    /// Scrubbed incomplete/failure reasons, listed per chain.
    pub reasons: Vec<String>,
}

impl ChainRun {
    /// A run that produced nothing: `status` (2/3/4) and the stderr text.
    #[must_use]
    pub fn failed(status: u8, message: String) -> Self {
        // The text is stored for cards/meta: drop the CLI's own stderr prefix.
        let message = ["wallet-stats: ", "wallet-rank: ", "buyer-intersect: "]
            .iter()
            .find_map(|p| message.strip_prefix(p).map(str::to_string))
            .unwrap_or(message);
        Self {
            status,
            reasons: vec![message.clone()],
            failure: Some(message),
            ..Self::default()
        }
    }
}

/// Run `f` over `items` with at most `concurrency` in flight, results in
/// item order. A panicking worker becomes `Err` (never aborts the others).
pub fn run_chains_bounded<T: Send, R: Send>(
    items: Vec<T>,
    concurrency: usize,
    f: impl Fn(T) -> R + Sync,
) -> Vec<Result<R, String>> {
    let width = concurrency.max(1);
    let mut out: Vec<Result<R, String>> = Vec::with_capacity(items.len());
    let mut iter = items.into_iter();
    loop {
        let batch: Vec<T> = iter.by_ref().take(width).collect();
        if batch.is_empty() {
            break;
        }
        let f = &f;
        std::thread::scope(|s| {
            let handles: Vec<_> = batch
                .into_iter()
                .map(|item| s.spawn(move || f(item)))
                .collect();
            for h in handles {
                out.push(h.join().map_err(|_| "chain worker panicked".to_string()));
            }
        });
    }
    out
}

/// Overall exit code of a multi-chain run from the per-chain codes (each
/// 0, 2, 3 or 4; usage 2 is normally rejected before any chain runs).
///
/// * every chain failed (2 or 4): 4 (2 only if all are 2);
/// * otherwise any non-zero chain: 3 (the universe is partial);
/// * otherwise 0.
///
/// A chain that errored while another produced data is never exit 4: exit 4
/// means nothing usable was observed anywhere (ADR-005).
#[must_use]
pub fn aggregate_exit(statuses: &[u8]) -> u8 {
    if statuses.is_empty() {
        return 0;
    }
    if statuses.iter().all(|s| matches!(s, 2 | 4)) {
        return if statuses.contains(&4) { 4 } else { 2 };
    }
    if statuses.iter().any(|s| *s != 0) {
        return 3;
    }
    0
}

/// Display identity of a wallet key (units, naming) as the stats engine uses it.
#[must_use]
pub fn display_of_chain(chain: &ChainKey) -> ChainDisplay {
    match (&chain.family, &chain.network_id) {
        (ChainFamily::Evm, NetworkId::EvmChainId(id)) => ChainDisplay::evm_by_chain_id(*id)
            .unwrap_or(ChainDisplay {
                family: ChainFamily::Evm,
                name: "evm",
                chain_id: Some(*id),
                native_unit: scout_sdk::engine::QuoteUnit::Wei,
                native_label: "eth",
                native_symbol: "ETH",
            }),
        _ => SOLANA_DISPLAY,
    }
}

/// Cards for wallets of a chain that was not scanned: `Error` with the
/// reason as the scan error (a failed chain), or `NotScanned` with the
/// reason as an incomplete reason (a chain skipped by design). Unknown, never
/// zero: no ledger, no counts.
#[must_use]
pub fn unscanned_cards(
    keys: &[WalletKey],
    status: WalletScanStatus,
    reason: &str,
) -> Vec<SolanaWalletStats> {
    keys.iter()
        .map(|k| {
            let bytes = match &k.address {
                AddressBytes::Solana(a) => *a,
                AddressBytes::Evm(a) => evm_key(alloy_primitives::Address::from(*a)),
            };
            let is_error = status == WalletScanStatus::Error;
            SolanaWalletStats {
                wallet: bytes,
                chain: display_of_chain(&k.chain),
                status,
                transactions_scanned: None,
                transactions_in_window: None,
                truncated: false,
                unexpected_payloads: 0,
                error: is_error.then(|| reason.to_string()),
                ledger: None,
                incomplete_reasons: if is_error {
                    Vec::new()
                } else {
                    vec![reason.to_string()]
                },
                failure: None,
                not_scanned: None,
            }
        })
        .collect()
}

/// Parse rendered JSONL lines back into records (merge step).
///
/// # Errors
/// A line that is not JSON (internal error: our own renderer produced it).
pub fn parse_records(lines: &[String]) -> Result<Vec<serde_json::Value>, String> {
    lines
        .iter()
        .map(|l| serde_json::from_str(l).map_err(|e| format!("internal: bad JSONL line: {e}")))
        .collect()
}

/// `kind` of a JSONL record (`""` when absent).
#[must_use]
pub fn record_kind(v: &serde_json::Value) -> &str {
    v.get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

/// Serialize records back to lines.
///
/// # Errors
/// Serialization failure (internal error).
pub fn render_records(records: &[serde_json::Value]) -> Result<Vec<String>, String> {
    records
        .iter()
        .map(|r| serde_json::to_string(r).map_err(|e| e.to_string()))
        .collect()
}

/// `chain=ok(0) base=failed(4)` line of a multi-chain run (table footer and
/// stderr).
#[must_use]
pub fn chain_status_text(chains: &[(&str, &ChainRun)]) -> String {
    chains
        .iter()
        .map(|(name, r)| {
            format!(
                "{name}={}({})",
                if r.failure.is_some() { "failed" } else { "ran" },
                r.status
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use scout_core::{GenesisIdentity, SolanaCluster};

    fn key(chain: &str, last: u8) -> WalletKey {
        let (family, network_id) = match chain {
            "solana" => (
                ChainFamily::Solana,
                NetworkId::SolanaCluster(SolanaCluster::Mainnet),
            ),
            "base" => (ChainFamily::Evm, NetworkId::EvmChainId(8453)),
            _ => (ChainFamily::Evm, NetworkId::EvmChainId(4663)),
        };
        let address = if chain == "solana" {
            AddressBytes::Solana([last; 32])
        } else {
            AddressBytes::Evm([last; 20])
        };
        WalletKey {
            chain: ChainKey {
                family,
                network_id,
                genesis_identity: GenesisIdentity::Unverified,
            },
            address,
        }
    }

    #[test]
    fn partition_keeps_first_appearance_order_of_chains_and_items() {
        let items = [
            key("base", 1),
            key("solana", 2),
            key("base", 3),
            key("robinhood", 4),
            key("solana", 5),
        ];
        let g = partition_by_chain(&items, |w| &w.chain);
        let shape: Vec<(&str, Vec<usize>)> =
            g.iter().map(|g| (g.name, g.indices.clone())).collect();
        assert_eq!(
            shape,
            vec![
                ("base", vec![0, 2]),
                ("solana", vec![1, 4]),
                ("robinhood", vec![3])
            ]
        );
    }

    #[test]
    fn aggregate_exit_rules() {
        assert_eq!(aggregate_exit(&[]), 0);
        assert_eq!(aggregate_exit(&[0, 0]), 0);
        assert_eq!(aggregate_exit(&[0, 3]), 3);
        assert_eq!(aggregate_exit(&[0, 4]), 3, "one chain down, the other fine");
        assert_eq!(aggregate_exit(&[4, 3]), 3);
        assert_eq!(aggregate_exit(&[3, 3]), 3);
        assert_eq!(aggregate_exit(&[4, 4]), 4);
        assert_eq!(aggregate_exit(&[4]), 4);
        assert_eq!(aggregate_exit(&[2, 2]), 2);
        assert_eq!(aggregate_exit(&[2, 4]), 4);
    }

    #[test]
    fn bounded_runner_keeps_order_and_reports_panics() {
        let r = run_chains_bounded(vec![1u32, 2, 3, 4, 5], 2, |n| n * 10);
        assert_eq!(
            r.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![10, 20, 30, 40, 50]
        );
        let r = run_chains_bounded(vec![1u32, 2], 2, |n| {
            assert_ne!(n, 2, "boom");
            n
        });
        assert!(r[0].is_ok() && r[1].is_err());
    }

    #[test]
    fn unscanned_cards_are_unknown_never_zero() {
        let keys = [key("base", 7), key("solana", 8)];
        let cards = unscanned_cards(&keys, WalletScanStatus::Error, "no key");
        assert_eq!(cards.len(), 2);
        assert!(
            cards
                .iter()
                .all(|c| c.ledger.is_none() && c.transactions_scanned.is_none())
        );
        assert_eq!(cards[0].chain.name, "base");
        assert_eq!(cards[0].error.as_deref(), Some("no key"));
        let ns = unscanned_cards(&keys, WalletScanStatus::NotScanned, "unit");
        assert!(ns[0].error.is_none() && ns[0].incomplete_reasons == vec!["unit".to_string()]);
    }
}
