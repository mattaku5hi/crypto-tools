# Empty-window pool discovery: six-token spot check

## Result

The frozen empty-window baseline has 78 tokens: 48 Base and 30 Robinhood Chain. Its launchpad split is Base: 41 Zora, 4 Clanker, 3 Flaunch; Robinhood: 30 Doppler. The read-only current join matched all 78 distinct frozen tokens and found each saved window row present with `returned_candles=0`; the `ath.holders` value was blank/null for all 78. I sampled three Robinhood Doppler tokens and one token each from Base Flaunch, Clanker, and Zora. Null holder counts meet the task's sample criterion; they do not establish that these tokens have at least 100 holders.

For all six, a successful launch receipt contains a `Uniswap v4 PoolManager.Initialize` event for the exact token and a reproducible 32-byte pool ID. A second exact-token search of the configured chain PoolManager, filtering both indexed currency positions from the earliest sample birth block through the cached anchor, found only these same six pool IDs—no additional v4 pool for any selected token. GeckoTerminal's public pool metadata identifies the same pool and token side. Its unauthenticated daily USD OHLCV endpoint returned an empty candle list for each exact pool over the baseline window, 2025-10-10 through 2026-10-10 UTC. A matching `eth_getLogs` query found zero v4 `Swap` events for each pool from its creation block through the saved chain anchor. The receipt itself contains the initialization event, but no v4 `Swap` event.

This classifies the sampled tokens as having no observed swaps in the configured v4 PoolManager scope through the saved anchors, rather than merely an unindexed v4 pool. [ADR-021](../../adr/ADR-021-dev-tracker-facts-and-storage.md) identifies Zora and Clanker as launchpads without a curve, so the sampled Zora and Clanker tokens have no observed price path in that configured scope; this does not exclude non-v4 pools. Flaunch and Doppler's launch-time/curve scope was not independently source-verified here. More generally, this does not prove that the tokens never traded: other pools, unconfigured launchpad paths, or activity after the anchor remain outside scope. No ATH or zero price is inferred.

## Sampled pools

| Network / chain ID | Launchpad | Token | Pool ID | Currency 0 / GT base side | Currency 1 / GT quote side | GT daily candles | v4 swaps from creation to anchor |
|---|---|---|---|---|---|---:|---:|
| Robinhood / 4663 | Doppler | `0x312646286c7a47edebb522227a6791214a29319e` | `0x071a267ab0e72eb14d76be706c9b1e44bf2f1cf09840191b7a5cdc5da86ea31f` | token | `0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec` | 0 | 0 |
| Robinhood / 4663 | Doppler | `0xc8cb87fea18270f7a2ac43d7039a932a3e801e18` | `0x6cb1038a6677b4ea94e04f7cd0d72464220817589531991454ecd3c1f23f4225` | token | `0x4a0e65a3eccec6dbe60ae065f2e7bb85fae35eea` | 0 | 0 |
| Robinhood / 4663 | Doppler | `0xbb1ced6ce170f71e8f2333c0ab4fb3e778c02e4a` | `0x41997f1b7a39db8bb7a520b44b4bd1fcd26d94975c58b6cb33fa65469feb4aab` | token | `0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec` | 0 | 0 |
| Base / 8453 | Flaunch | `0xd133939166d6634145b923d6df519327e29e0795` | `0x7ae2ed54bcebd2aaf18e50200bd9286e72d51fca2578c91f3a559981b9e0e9fa` | `0x000000000d564d5be76f7f0d28fe52605afc7cf8` | token | 0 | 0 |
| Base / 8453 | Clanker | `0x1dd82743a27b4e11b3d844060c66d82cd4f6db07` | `0xf7ce17b627edc83babb6b454f258a17bdccc83ba9aed2b33d1e1e6cd2c1e1869` | token | `0x833589fcd6edb6e08f4c7c32d4f71b54bda02913` | 0 | 0 |
| Base / 8453 | Zora | `0x7482aa15186075992ee96e19c585fca8d283e60a` | `0xbfa14f9152fdee0346bed1df444f82dd05edebb3433274db789e275d9bcc7cd6` | `0x107fb51dbea200ae8e8d69ecf9afe13bfb6d3479` | token | 0 | 0 |

GeckoTerminal pool metadata reported pool creation dates between 2026-07-24 and 2026-09-19, current USD price `0` or `null`, and zero USD reserves where present. Recent volume and transaction fields were `null`. These are current metadata fields, not historical ATH data. Quote-token symbols and USD conversion were not assumed from address shape or token names.

## Procedure and provenance

The frozen baseline is `.scout-data/diagnostics/empty-window-pool-discovery-2026-10-11/baseline-empty-input.json` (78 rows, source TSV and SHA recorded in the file). Candidate/current DB joins, all 78 zero-candle rows, and holder values are preserved in `current-candidate-join.tsv`; this was read-only and its later 87-row live count was not substituted for the frozen 78-token denominator.

Six `eth_getTransactionReceipt` calls (chain IDs 8453 and 4663) retained full response bodies and SHA-256 hashes in `receipt-probe-manifest.json` and `raw/`. Each receipt succeeded and its event emitter matched the chain's cached PoolManager address. The event layout/topic follows the repo's pinned v4 decoder in [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs) and the upstream [`IPoolManager` interface](https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol). This proves the identified v4 pool's initialization, not that all launches or token trading routes use v4.

The anonymous GeckoTerminal v2 calls used the exact discovered pool IDs, first reading pool metadata and following only when its `base_token`/`quote_token` relationships matched the token. Daily OHLCV used USD, the corresponding token side, `aggregate=1`, `limit=1000`, `before_timestamp=1791676799`, and `include_empty_intervals=false`. The public API and endpoint family are documented at [GeckoTerminal API docs](https://api.geckoterminal.com/docs/index.html). There were 12 GETs total, spaced at least 15 seconds apart; all returned HTTP 200. Exact URLs, timestamps, parsed identity/candle counts, and response hashes are in `gt-exact-pool-manifest.json`; response bodies are in `gt-raw/`.

Six read-only `eth_getLogs` calls filtered the pinned PoolManager address, v4 `Swap` topic, and exact indexed pool ID from each receipt block through the cached anchor. All succeeded and returned zero logs. Four more read-only `eth_getLogs` calls searched all six token addresses at both indexed `currency0` and `currency1` positions at the same configured manager; the only matching Initialize events returned the same six pool IDs. Raw responses, hashes, filters, and anchors are recorded in `swap-log-manifest.json`, `secondary-initialize-manifest.json`, and `rpc-raw/`. Six receipts plus these ten log queries used 16 bounded RPC calls total. The saved anchors are slightly after the OHLCV window end, so the zero log results cover at least the entire requested window for these tokens in the configured v4 scope.

There were no Codex, Birdeye, Solana Tracker, or database write calls in this investigation. Private diagnostics are local and untracked.

## Next bounded check

For a genuinely priceable empty-window candidate, first identify and verify its actual trading path from launch/migration receipts and the protocol's deployment/source registry. Then query only those verified pool/curve events and obtain the quote asset's dated USD rate. This sample did not reach that stage: its exact initialized v4 pools had no swaps. A network-wide or token-wide claim would require broader pool/curve discovery and is not supported by these six samples.
