# Frozen empty-window cohort: configured v4 pool-scope audit

As of 2026-10-11, the frozen baseline contains 78 tokens whose indexed price window had no candles: 48 Base (41 Zora, 4 Clanker, 3 Flaunch) and 30 Robinhood Chain Doppler tokens. This audit checks whether configured Uniswap-v4 PoolManager pools for those exact token addresses had any observed swaps in the baseline window. It does not claim full-chain market or lifetime-price coverage.

## Result

The exact annual cutoff is Base block `0x3203a4e` at `2026-10-10T23:59:59Z`; the next block, `0x3203a4f`, is at `2026-10-11T00:00:01Z`. Through that cutoff, the configured managers yielded 36 unique active pools and 1,194 deduplicated Swap logs. Those pools connect 36 distinct cohort tokens through 47 token-to-pool associations. All 36 active tokens are Zora/Base tokens. The other 42 cohort tokens have no observed swap in the configured-v4 scope: 5 Zora, 4 Clanker, 3 Flaunch, and all 30 Doppler/RH tokens.

The search found 80 unique initialized pools for the 78 tokens (93 token/pool associations, including shared pools). Every initialized pool was scanned through its recorded chain anchor; the annual cutoff filters Base events to the requested window. Duplicate scans from the initial/resume boundary are deduplicated by `(chain, pool_id)`. One pool’s only observed swap is after the cutoff, so it is excluded from the annual active-pool total.

No annual-active pool directly pairs a cohort token with canonical Base USDC (`0x833589fcd6edb6e08f4c7c32d4f71b54bda02913`) or WETH (`0x4200000000000000000000000000000000000006`). The active pool currencies instead include cohort-token pairs and ten quote addresses outside the frozen cohort. Their identity and historical USD conversion have not been established here. Consequently, the archived exact post-swap `sqrtPriceX96` values are raw pool-spot evidence, not USD ATHs. No price or FDV is inferred from this audit.

## Method and evidence

The audit used the exact frozen 78-address input, both indexed-currency positions, and the configured Base/RH v4 PoolManager deployments. The event shapes match the project’s `Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)` and v4 `Swap` decoders in [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs#L26). The configured manager addresses are in [`gate.rs`](../../../crates/scout-dex-evm/src/gate.rs#L330). This is a configured-manager/address-list search only; it does not cover other DEX families, other managers, or pools created outside the exact token set.

The frozen input and archived RPC responses, request parameters, timestamps, raw-file paths and SHA-256 digests are retained in `.scout-data/diagnostics/empty-cohort-v4-scope-2026-10-11/manifest.json`. The exact cutoff block search is in `window-cutoff-manifest.json`. The deduplicated annual pool/token summary is `annual-v4-pool-classification.json`; for every active pool it records currencies, Initialize fee/tick spacing/hooks, first and last observed swap block timestamps, maximum observed post-swap `sqrtPriceX96` event, token orientation and exact atomic quote/token ratio, plus raw query provenance. It keeps values in integer/rational form and does not assign decimals or USD conversion.

RPC accounting for this audit: 6 preflight calls, 16 grouped Initialize searches, 89 Swap queries (including 9 duplicate-pool scans during the resume boundary), 40 event-block header checks, and 16 cutoff-search headers: 167 calls total against the explicit 180-call bound. No Codex, GeckoTerminal, Birdeye, or Solana Tracker requests were made; no database writes were made. Responses were capped at 2 MiB, calls spaced by at least 1.5 seconds, and automatic retries were disabled.

## Next evidence needed

The 36 active Zora tokens have swap observations, but the configured-v4 prices are mostly denominated in other tokens. To turn any into defensible USD prices requires independently validating quote-token identity, quote-token decimal scaling, a dated USD route, and pool/migration completeness. The 42 zero-swap results mean only “no observed swap in these configured v4 pools and this time range”; they do not establish that a token never traded or had zero price.
