# Robinhood 35-token alternative-factory scan — 2026-10-14

This bounded read-only scan checked three documented mainnet pool factories against the exact 35-address frozen cohort. It complements the configured-v4 audit; it does not establish all-market ATH completeness.

## Capture and result

The input is [`root-immutable-current35-input.json`](../../../.scout-data/diagnostics/rh-remaining35-poolmanager-discovery-2026-10-14/root-immutable-current35-input.json), SHA-256 `aac0cfb7bd2c0ba4827fc16218054a48e12d206d63b30b6c5d4c0dfa54b4e9a5`. The new private capture is [manifest.json](../../../.scout-data/diagnostics/rh35-alternative-factories-2026-10-14/manifest.json), SHA-256 `919846167b088efd1da83add51e9d5d79072ca8ccf7034d3e2bfe6d4a7a6e904`; raw request/response files are under the adjacent `raw/` directory.

The configured Robinhood RPC returned chain ID `0x1237` (4663) and pinned head `0x5487fea` (88,637,418). Eleven distinct calls were attempted, with no retry, redirect, database write, or price-provider call. Six `eth_getLogs` requests returned successful empty arrays after filtering both indexed token positions for all 35 exact addresses:

| Factory | Start block | Indexed positions queried | Matching logs |
|---|---:|---|---:|
| Uniswap v2 `0x8bceaa40b9acdfaedf85adf4ff01f5ad6517937f` | 0 | `token0`, `token1` | 0 |
| Uniswap v3 `0x1f7d7550b1b028f7571e69a784071f0205fd2efa` | 0 | `token0`, `token1` | 0 |
| Goo V3 `0x221A6239E40709792b0d4bdc140fa36158CD41C7` | 73,266,708 | `token0`, `token1` | 0 |

The v2 factory and Goo factory returned nonempty runtime code (13,859 and 1,954 bytes respectively). The v3 `eth_getCode` request timed out; it was not retried. Its two log queries did return HTTP-200 JSON-RPC success with empty arrays. This result is therefore “no matching `PairCreated`/`PoolCreated` logs returned by this configured endpoint for the queried scopes,” not proof that the v3 deployment is absent or that no alternative venue exists.

The topic0 filters are the canonical `PairCreated(address,address,address,uint256)` and `PoolCreated(address,address,uint24,int24,address)` signatures, matching the constants and signature tests in [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs). Target address topics were padded to 32 bytes; each filter searched one indexed currency position at a time. All six raw responses were checked offline for valid JSON arrays, zero rows, matching factory address, and matching topic0.

## Interpretation

No additional pool addresses were discovered for these 35 tokens at the three queried factories. Goo’s first-party docs state that its launchpad creates an ordinary pool at the documented Goo V3 factory, so the factory scan also covers the pool side of that launch path. Its separate `Launched` event was not queried. The scan did not query 4.fun launch events or establish its actual pool deployment ABI; the 4.fun docs do not publish a launch block or event signature. It also did not inspect Pleiades or any other unlisted factory, later migrations outside these factories, or non-pool price sources. Empty results remain missing price evidence, never zero ATH/FDV.

The request cap was 12; 11 were used. The one code timeout remains a recorded gap, with no retry or endpoint fallback. The scan used the source-defined RH chain identity (4663), and did not change registry, database, or production code.

## Sources

- [Uniswap official deployments for chain 4663](https://github.com/Uniswap/contracts/blob/main/deployments/4663.md) — canonical RH v2/v3 factories and v4 PoolManager.
- [Goo Exchange documentation](https://goo.exchange/docs/) — Goo V3 factory, deployment block, pool discovery event, and launchpad pool path.
- [4.fun documentation](https://api.4.fun/docs) — documented RH factories and launch mechanics; launch event and deployment block are not exposed in the page material reviewed.
- [Robinhood Chain network configuration](https://docs.robinhood.com/chain/connecting/) — mainnet ID 4663 and testnet ID 46630.
- [Existing configured-v4 scope report](2026-10-14-rh-remaining35-v4-scope.md) — exact frozen cohort and limitation of the initial v4-only search.
