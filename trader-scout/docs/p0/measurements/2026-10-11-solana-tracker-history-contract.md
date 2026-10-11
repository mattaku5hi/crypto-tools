# Solana Tracker history contract for the variable-supply JC mint

**Scope:** primary-source documentation review only. No Data API, RPC, or database calls were made for this note. It identifies a one-request price-window strategy for `JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump`; it does not assert the provider returned complete launch-to-anchor coverage.

## Best bounded request

For a single maximum-price observation over the mint's known birth-to-supply-anchor interval, the most bounded documented route is:

```text
GET https://data.solanatracker.io/price/history/range
    ?token=JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump
    &time_from=1791671908
    &time_to=1791706200
```

Send the required `x-api-key` header through the existing authorized key; never put it in the URL or report. The documented parameters are Unix seconds. `time_from` is the known birth time, 2026-10-10 22:38:28 UTC; `time_to` is the later finalized supply-snapshot time, 2026-10-11 08:10:00 UTC. The endpoint's example returns `price.highest.price`, `price.highest.time`, and a `marketcap` field, although the OpenAPI response schema only promises `price` and `time`. Treat `marketcap` as diagnostic until its supply basis is documented. One request to this endpoint costs one REST request under the published one-request/one-credit pricing model.

This route is preferable to downloading every 1-second candle when the immediate question is only the maximum price and its timestamp. It does **not** return the price path, event order, or enough information to establish exact supply at the high. At second precision, multiple supply-changing instructions may share the reported timestamp. Since this mint had 30 burns in the audited window, the high and supply must not be treated as simultaneous without an ordered event match.

## Candle alternative and documented limits

The official OHLCV route is `GET /chart/{token}`. It accepts `type=1s`, `time_from`, and `time_to` in Unix seconds, and can return USD price candles; the token/pool variant `GET /chart/{token}/{pool}` is scoped to one pool. For token-wide dynamic pool selection, `dynamicPools=true` is documented as selecting the main pool over time for a consistent chart; this is not a promise that every pool's maximum is included. `removeOutliers` defaults to true, so a maximum-price audit should explicitly request `removeOutliers=false`. `marketCap=true` switches the chart to market-cap values, but the docs do not specify whether supply is historical, current, or otherwise adjusted.

The published OpenAPI definitions document neither a candle-count maximum, nor pagination/cursor behavior, nor a maximum time span/retention period for either chart route. They document an OHLCV array with `time`, `high`, and other candle fields, but no candle sub-event timestamps. Therefore a birth-to-anchor `type=1s` request is syntactically supported, but the maximum payload, truncation behavior, and complete retention cannot be assumed. If used, request a single explicit range and validate that returned first/last buckets bracket the intended period; this check still cannot prove provider coverage before its own indexing began.

## Free-plan and ATH semantics

The official pricing page currently advertises the Data API Free plan as 10,000 requests/month and 3 requests/second, with REST available on Free. The owner reports that this project's actual Free key is limited to 2,500 requests/month; use that account-specific 2,500 figure for budgeting. Four Solana Tracker requests had already been made in the session before this documentation pass. No new key or quota was used here.

The dedicated `GET /tokens/{tokenAddress}/ath` page explicitly defines its result as the all-time high **since the Data API started recording**. Its example contains `highest_price`, `highest_market_cap`, millisecond `timestamp`, and `pool_id`, but the response schema only guarantees `highest_price` and `timestamp`; the docs do not define the `highest_market_cap` supply convention. It is not a documented launch-to-now lifetime guarantee, and its ATH market-cap field cannot be equated with the project's historical price-times-supply maximum without clarifying the denominator.

## JC application to the existing supply evidence

The existing [supply reconciliation](2026-10-11-solana-ath-supply-ledger.md) establishes a one-time birth mint of 1,000,000,000 tokens, revoked mint authority, 30 burns totaling 34,238,910.728744 tokens, and reconciled supply of 965,761,089.271256 tokens through the later finalized anchor. The saved price windows currently agree at the daily/1-second maximum, but no minute/second market-history request should be inferred from those records.

After the one range request, a conservative bound over the **provider-returned interval** is `returned_highest_price × 1,000,000,000`, because the audited supply never exceeded birth supply. It is an upper bound, not the exact price-times-supply value at the high and not a canonical lifetime ATH: the provider's coverage start and range completeness remain undocumented. For a tighter time-aligned observed-window bound, use returned `highest.time` to select the corresponding second and multiply by the maximum proven supply state in that second, preserving transaction order where available. Do not use the optional `marketcap` field as proof of that calculation.

The one-call request can therefore answer a narrow operational question—Solana Tracker's highest recorded price in the specified range and reported timestamp—while the existing complete mint ledger supplies a conservative denominator ceiling. It cannot by itself prove a global lifetime price maximum, a complete multi-pool history, sub-second high/supply ordering, or an exact variable-supply ATH FDV.

## Primary sources

- [Solana Tracker OHLCV chart for a token](https://docs.solanatracker.io/data-api/chart/get-ohlcv-data-for-a-token) — route, 1-second interval, seconds-based bounds, `dynamicPools`, `removeOutliers`, `marketCap`, response fields.
- [Solana Tracker OHLCV chart for a token/pool](https://docs.solanatracker.io/data-api/chart/get-ohlcv-data-for-a-tokenpool-pair) — per-pool route and the same documented time controls.
- [Solana Tracker price high/low over a time range](https://docs.solanatracker.io/data-api/price/get-lowest-and-highest-price-in-time-range) — exact parameters and example response containing high price, timestamp, and marketcap.
- [Solana Tracker all-time-high price](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) — `since the data API started recording` scope and ATH response example/schema.
- [Solana Tracker pricing and limits](https://docs.solanatracker.io/pricing) — published Free REST allowance and rate limit; account-specific allowance is recorded separately above.
- [Solana Tracker documentation index](https://docs.solanatracker.io/llms.txt) — first-party endpoint catalog.

**Chosen follow-up:** the existing direct ATH response already supplies a maximum price; another range maximum would still omit the joint price/supply path. Use the already-authorized key for one short `type=1s` chart covering birth through two seconds after the final burn, with `dynamicPools=true`, `removeOutliers=false`, and `marketCap=false`. Bound it to2MiB/2,000 candles,20s and one attempt, with session ceiling5 Tracker requests. Compare every returned high with per-second supply bounds rather than assigning unknown intrasecond ordering. This does not change or reset the owner-reported2,500/month allowance.
