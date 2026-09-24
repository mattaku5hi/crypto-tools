//! Minimal end-to-end wiring for `buyer-intersect`: input identities →
//! provider scan → net-delta classification → hit aggregation → report.
//!
//! This is the first vertical slice proving every layer connects
//! (ARCHITECTURE.md §1). It deliberately works against any
//! `HistoryProvider` (in practice `FixtureProvider` until live
//! credentials exist, per ADR-006) and is synchronous apart from the
//! provider's own async `plan`/`scan` calls — no bounded channels, no
//! wallet sharding. Those are real concurrency concerns for scanning
//! many wallets against live RPC in parallel; building them now, against
//! a handful of synthetic fixtures, would be exactly the speculative
//! complexity AGENTS.md invariant #14 warns against for lock-free
//! structures generalized to concurrency machinery at large: add it
//! when a measured need exists.

use std::collections::BTreeMap;

use futures::StreamExt as _;
use scout_api::{HistoryProvider, ProviderError, ScanRequest, ScanTask};
use scout_core::{AssetKey, WalletKey};
use scout_normalize::{NetDeltaInput, classify_buy};
use tokio_util::sync::CancellationToken;

/// One wallet's qualifying hit count against the input token set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyerMatch {
    pub wallet: WalletKey,
    pub hit_count: usize,
    pub matched_assets: Vec<AssetKey>,
}

/// Full result of a `buyer-intersect` run: matches meeting the
/// threshold, plus the declared input scope so callers can report N
/// honestly even when some tokens failed to scan (ACCEPTANCE B08: a
/// scan failure on one token must never shrink N).
#[derive(Debug, Clone)]
pub struct BuyerIntersectReport {
    pub matches: Vec<BuyerMatch>,
    pub input_token_count: usize,
    pub min_token_hits: usize,
}

/// Run the full offline `buyer-intersect` pipeline against a single
/// history provider for one wallet's net-delta input per token.
///
/// This signature takes pre-built `NetDeltaInput` per `(wallet, token)`
/// rather than raw provider envelopes, because decoding a provider's raw
/// payload into normalized asset flows is protocol-specific (owned by
/// `scout-dex-evm`/`scout-dex-solana`, ADR-006) and this engine function
/// must not assume one chain family. The `provider` parameter is still
/// exercised (via `plan`) so the `ConfigurationRequired` contract from
/// ADR-006 surfaces exactly as it would in a live run — an unconfigured
/// provider fails this call loudly, not silently.
pub async fn run_buyer_intersect(
    provider: &dyn HistoryProvider,
    wallet_token_flows: &BTreeMap<WalletKey, BTreeMap<AssetKey, NetDeltaInput>>,
    input_tokens: &[AssetKey],
    min_token_hits: usize,
) -> Result<BuyerIntersectReport, ProviderError> {
    // Exercise the provider's plan() so an unconfigured provider (per
    // ADR-006's UnconfiguredProvider) surfaces ConfigurationRequired
    // here, exactly as a live run would — this function is not
    // reachable purely through in-memory data without ever consulting
    // the declared provider.
    if let Some(first_token) = input_tokens.first() {
        provider
            .plan(&ScanRequest::TokenMarketActivity {
                asset: first_token.clone(),
            })
            .await?;
    }

    // A provider that reports zero capability for every input is not
    // itself an error here (see ADR-005: operational status vs. per-
    // record eligibility are distinct axes) — the caller of this
    // function is expected to inspect capabilities via a separate
    // pre-flight and decide the run's IncompleteCoverage/exit-3 status.
    // This function focuses on the classification math once inputs are
    // available.
    let mut scan_stream = provider.scan(
        ScanTask {
            description: "buyer-intersect: exercised for capability/config surfacing".to_string(),
        },
        CancellationToken::new(),
    );
    while let Some(item) = scan_stream.next().await {
        // A ConfigurationRequired error from scan() must surface exactly
        // as it would from plan() — this loop lets it propagate via `?`
        // rather than being silently dropped as an unused stream.
        item?;
    }

    let mut matches = Vec::new();
    for (wallet, token_flows) in wallet_token_flows {
        let mut combined = NetDeltaInput::default();
        for flows in token_flows.values() {
            for (asset, flow) in &flows.flows {
                combined.flows.insert(asset.clone(), flow.clone());
            }
        }
        let qualifying = classify_buy(&combined);
        // Only count qualifying assets that are actually in the declared
        // input token set — a wallet's incidental buy of some unrelated
        // token must never inflate hit_count (ACCEPTANCE §7: hit_count
        // is per distinct input AssetKey).
        let matched_assets: Vec<AssetKey> = qualifying
            .into_iter()
            .filter(|asset| input_tokens.contains(asset))
            .collect();

        if matched_assets.len() >= min_token_hits {
            matches.push(BuyerMatch {
                wallet: wallet.clone(),
                hit_count: matched_assets.len(),
                matched_assets,
            });
        }
    }

    // Deterministic ordering: hit_count desc, then canonical WalletKey
    // asc as an explicit tiebreaker (CLI.md §3: "Сортировка: hit_count
    // desc, canonical wallet/group key asc") — never relying on
    // insertion order or an unstable sort's incidental behavior for
    // equal keys (ACCEPTANCE F01: concurrency-independent output).
    matches.sort_by(|a, b| {
        b.hit_count
            .cmp(&a.hit_count)
            .then_with(|| a.wallet.cmp(&b.wallet))
    });

    Ok(BuyerIntersectReport {
        matches,
        input_token_count: input_tokens.len(),
        min_token_hits,
    })
}

#[cfg(test)]
mod tests {
    use scout_core::{
        AddressBytes, ChainFamily, ChainKey, GenesisIdentity, NetworkId, SignedAmount,
    };
    use scout_normalize::{ActionKind, AssetFlow};
    use scout_providers::FixtureProvider;

    use super::*;

    fn chain() -> ChainKey {
        ChainKey {
            family: ChainFamily::Evm,
            network_id: NetworkId::EvmChainId(8453),
            genesis_identity: GenesisIdentity::Unverified,
        }
    }

    fn wallet(byte: u8) -> WalletKey {
        WalletKey {
            chain: chain(),
            address: AddressBytes::Evm([byte; 20]),
        }
    }

    fn asset(byte: u8) -> AssetKey {
        AssetKey::Token(chain(), AddressBytes::Evm([byte; 20]))
    }

    fn buy_flow(asset_key: AssetKey, units: i64) -> NetDeltaInput {
        let mut input = NetDeltaInput::default();
        input.flows.insert(
            asset_key.clone(),
            AssetFlow {
                asset: asset_key,
                net_delta: SignedAmount::from_i256(
                    alloy_primitives::I256::try_from(units).unwrap(),
                ),
                kinds_observed: vec![ActionKind::Swap],
            },
        );
        input
    }

    #[tokio::test]
    async fn wallet_meeting_min_hits_is_included_and_others_are_not() {
        // ACCEPTANCE B01: wallet A bought T1/T2 (K=2 -> included), B
        // bought only T1 (excluded).
        let t1 = asset(1);
        let t2 = asset(2);
        let wallet_a = wallet(0xA1);
        let wallet_b = wallet(0xB1);

        let mut flows = BTreeMap::new();
        let mut a_flows = BTreeMap::new();
        a_flows.insert(t1.clone(), buy_flow(t1.clone(), 100));
        a_flows.insert(t2.clone(), buy_flow(t2.clone(), 50));
        flows.insert(wallet_a.clone(), a_flows);

        let mut b_flows = BTreeMap::new();
        b_flows.insert(t1.clone(), buy_flow(t1.clone(), 200));
        flows.insert(wallet_b, b_flows);

        let provider = FixtureProvider::new();
        let report = run_buyer_intersect(&provider, &flows, &[t1, t2], 2)
            .await
            .unwrap();

        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].wallet, wallet_a);
        assert_eq!(report.matches[0].hit_count, 2);
        assert_eq!(report.input_token_count, 2);
    }

    #[tokio::test]
    async fn incidental_buy_outside_input_token_set_does_not_inflate_hit_count() {
        let t1 = asset(1);
        let unrelated = asset(99);
        let w = wallet(0xA1);

        let mut flows = BTreeMap::new();
        let mut w_flows = BTreeMap::new();
        w_flows.insert(t1.clone(), buy_flow(t1.clone(), 10));
        w_flows.insert(unrelated.clone(), buy_flow(unrelated, 10));
        flows.insert(w.clone(), w_flows);

        let provider = FixtureProvider::new();
        // min_token_hits=1 so wallet still needs at least the declared
        // input token to qualify, not the unrelated one.
        let report = run_buyer_intersect(&provider, &flows, &[t1], 1)
            .await
            .unwrap();

        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].hit_count, 1);
    }

    #[tokio::test]
    async fn results_are_sorted_by_hit_count_desc_then_wallet_key_asc() {
        // ACCEPTANCE F01 / CLI.md §3: deterministic order, explicit
        // tiebreaker on equal hit_count.
        let t1 = asset(1);
        let t2 = asset(2);
        let wallet_high = wallet(0xFF); // higher byte, but higher hit_count
        let wallet_low_a = wallet(0x01);
        let wallet_low_b = wallet(0x02);

        let mut flows = BTreeMap::new();

        let mut high_flows = BTreeMap::new();
        high_flows.insert(t1.clone(), buy_flow(t1.clone(), 1));
        high_flows.insert(t2.clone(), buy_flow(t2.clone(), 1));
        flows.insert(wallet_high.clone(), high_flows);

        let mut low_a_flows = BTreeMap::new();
        low_a_flows.insert(t1.clone(), buy_flow(t1.clone(), 1));
        flows.insert(wallet_low_a.clone(), low_a_flows);

        let mut low_b_flows = BTreeMap::new();
        low_b_flows.insert(t1.clone(), buy_flow(t1.clone(), 1));
        flows.insert(wallet_low_b.clone(), low_b_flows);

        let provider = FixtureProvider::new();
        let report = run_buyer_intersect(&provider, &flows, &[t1, t2], 1)
            .await
            .unwrap();

        assert_eq!(report.matches.len(), 3);
        // wallet_high has hit_count=2, sorts first.
        assert_eq!(report.matches[0].wallet, wallet_high);
        assert_eq!(report.matches[0].hit_count, 2);
        // wallet_low_a and wallet_low_b both have hit_count=1; tiebreak
        // by canonical WalletKey ascending.
        assert_eq!(report.matches[1].wallet, wallet_low_a);
        assert_eq!(report.matches[2].wallet, wallet_low_b);
    }

    #[tokio::test]
    async fn scan_failure_on_one_token_never_shrinks_declared_n() {
        // ACCEPTANCE B08: N stays the full declared input set even if a
        // token has zero matches (simulated here as simply absent from
        // every wallet's flows, which is the fixture-level stand-in for
        // "this token's scan produced no usable data").
        let t1 = asset(1);
        let t2_never_scanned_successfully = asset(2);
        let w = wallet(0xA1);

        let mut flows = BTreeMap::new();
        let mut w_flows = BTreeMap::new();
        w_flows.insert(t1.clone(), buy_flow(t1.clone(), 1));
        flows.insert(w, w_flows);

        let provider = FixtureProvider::new();
        let report =
            run_buyer_intersect(&provider, &flows, &[t1, t2_never_scanned_successfully], 1)
                .await
                .unwrap();

        assert_eq!(report.input_token_count, 2);
    }
}
