# Base Flaunch ATH pilot: three frozen unknowns — 2026-10-14

## Result

The three Flaunch launches in the frozen Base unknown-priority snapshot have valid launch receipts and each receipt links a Flaunch `PoolCreated` record to a Base Uniswap v4 `Initialize` event with the same PoolId and token. Bounded `Swap` log queries against those exact pools returned no events from creation through the pinned Base head. This establishes an empty observed history in each initial Flaunch v4 pool for the queried interval; it does not establish that the token had no trades in other pools. There is no price observation from these pool windows, so this pilot cannot produce an ATH/FDV value.

The official Flaunch public price endpoint failed for all three token requests. The capture code did not retain the HTTP status/body for `HTTPError`, so this run cannot distinguish endpoint outage from another HTTP-level rejection. Per the [2026-10-11 launchpad API assessment](2026-10-11-base-launchpad-ath-research.md), even a successful response would need field/supply/market-scope validation before treating Flaunch `allTimeHigh` as project `athFdv`.

## Scope and evidence

Input is exactly the three `chain=base, launchpad=flaunch` entries in `.scout-data/diagnostics/current-2026-10-13-top-runner-unknown-priority.json`, SHA-256 `1368a3473eeeac14c1f367b5695159617bf3250537624159298dbe74d19bae50`. Indexed `launches` rows were read-only queried and matched their frozen token, launch source, creation block, and transaction hash.

Configured Base RPC reported chain ID 8453. The pinned head was block 52,634,520, hash `0x056c3a908a5b9b4627ee49bb9468564da79261d452a5552c9a7f942b5d980905`, timestamp `2026-10-13T21:26:43Z`. Successful receipts (`status=0x1`, 12 logs each) matched the indexed creation block:

| Token | Creation block | PoolId | Exact v4 Swap query | Result |
|---|---:|---|---|---|
| `0x474950e80b5b632d029e94b22762779be9ea6620` | 51,720,544 | `0x2d6f9e6f081f7dbc13941eae5ae8b89229562d8ae3f8d01a46bafa3c33a28442` | 51,720,544–52,634,520 | 0 logs |
| `0xcd113c6130365a2dae005d92ad404e9206cf5763` | 49,436,591 | `0x6f2b97ffa5cf610d440a6b62003cabab786fab5c3e8d66a0cf3930ff0812437b` | 49,436,591–52,634,520 | 0 logs |
| `0xd133939166d6634145b923d6df519327e29e0795` | 49,436,430 | `0x7ae2ed54bcebd2aaf18e50200bd9286e72d51fca2578c91f3a559981b9e0e9fa` | 49,436,430–52,634,520 | 0 logs |

For each receipt, the Flaunch PositionManager `PoolCreated` token and indexed PoolId matched the Uniswap v4 PoolManager `Initialize` token and PoolId. The PoolManager was `0x498581ff718922c3f8e6a244956af099b2652b2b`; Flaunch PositionManager was `0x23321f11a6d44fd1ab790044fdfde5758c902fdc`. Each pool's other currency was `0x000000000d564d5be76f7f0d28fe52605afc7cf8`; this pilot did not establish its identity or USD conversion, and no swap was observed to price.

The official Flaunch repository identifies the legacy PositionManager deployment used by these receipts and documents that the current PositionManager initializes the launched pool; see [Flaunch contract deployments](https://github.com/flayerlabs/flaunchgg-contracts/blob/main/README.md) and [PositionManager source](https://github.com/flayerlabs/flaunchgg-contracts/blob/main/src/contracts/PositionManager.sol). The captured current source is contextual; this pilot did not prove source/bytecode equivalence for the historical deployed contract.

## Reproduction artifacts and limits

Private bounded raw responses, request parameters, response hashes, indexed rows, and the frozen input copy are under `.scout-data/diagnostics/2026-10-14-base-flaunch-ath-3/capture-212629/`. The files are permission-restricted and contain no RPC URL or credential. The three exact PoolId `eth_getLogs` calls were successful, uncapped empty JSON arrays, with no retries. Three keyless Flaunch token-price GETs failed at HTTP level; because the status/body were not captured, no precise HTTP status is claimed. No database or process state was changed and no authenticated/paid provider was used.

This is a source-specific, pool-specific negative result as of the pinned head, not a complete all-pairs token history and not evidence of zero value. Follow-up would need validated Flaunch price semantics or discovery and coverage proof for any migrated/other pools before assigning an ATH.
