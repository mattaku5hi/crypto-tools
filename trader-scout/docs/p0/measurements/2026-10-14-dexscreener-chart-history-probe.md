# DexScreener chart-history path check

**Result (2026-10-14):** no usable public historical-chart route was established. The official REST reference documents current pair/token snapshots and does not list OHLCV, candles, or time-range history. One unauthenticated request to the ordinary first-party page for a pinned Solana pool returned HTTP 403 before any chart data or page assets could be inspected. The probe stopped there; it does not establish why the page was refused or whether the chart is available in an ordinary browser session.

## Evidence

- The official [API reference](https://docs.dexscreener.com/api/reference) documents current pair lookup (`/latest/dex/pairs/{chainId}/{pairId}`), token-pair lookup (`/token-pairs/v1/{chainId}/{tokenAddress}`), and token lookup (`/tokens/v1/{chainId}/{tokenAddresses}`). Returned pair data includes current `priceUsd`, current transaction/volume/change fields, liquidity, FDV/market cap, and pair creation time. The reference does not document a candle/OHLCV endpoint or historical query parameters.
- The [official FAQ](https://docs.dexscreener.com/) describes charts as being built from DexScreener's blockchain indexer; it does not document a public historical-chart API contract.
- The [official token-listing page](https://docs.dexscreener.com/token-listing) describes when tokens appear and how FDV is calculated, not retention or historical candles.
- The [API terms](https://docs.dexscreener.com/api/api-terms-and-conditions) govern use of the documented API; they do not add a history endpoint or retention guarantee.

## Bounded page probe

- Target came from the existing saved proof for mint `27ES77p62B4edFBnGatr7NfnpudwwAkFC9eM2wQBpump`, selected pool `J4iEduCrhpsKmE9rioq9XSZtBxRrZ1sihRpHoVLy3pPq`.
- Request: ordinary unauthenticated `GET https://dexscreener.com/solana/J4iEduCrhpsKmE9rioq9XSZtBxRrZ1sihRpHoVLy3pPq`, with no cookies or authorization headers, redirects disabled, 25-second timeout, and a 2 MiB read cap.
- Outcome: HTTP 403, 5,655 response bytes, `text/html; charset=UTF-8`; captured response SHA-256 `a9745051e15bd34e7384e91ab1b764ef0601627be2e2dad24f6b162d95b59e69`. A bounded private response copy and request metadata are in `.scout-data/diagnostics/dexscreener-chart-probe-2026-10-14/`.
- No asset, chart query, or historical data request followed. The response does not identify the refusal cause, so this is not evidence that DexScreener lacks historical chart data; it means this bounded public probe could not establish an accessible route. No retry, alternate host, browser session, or access-control workaround was used.

## Scope conclusion

Treat DexScreener as documented current-price/pair metadata corroboration for this project. Do not use its undocumented chart UI as an ATH or OHLCV source based on this probe. A future ordinary-access test would need to begin only after the first-party page itself is reachable and an official or visibly public chart request can be identified; any resulting candle series would still need birth, pool/migration, quote-unit, and retention validation before it could support an observed-window high.
