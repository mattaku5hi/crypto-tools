# JC chart and supply comparison — 2026-10-11

## Result

The final control request returned the Tracker ATH price in USD per token and timestamps inside the requested interval. Across the 43 rows it returned, the maximum candle high multiplied by the conservative supply bound for that second is `$227,310.99424807910909633689232512`, at `2026-10-10 22:44:14 UTC`. That is the same-second FDV snapshot already reconciled to the Tracker ATH response.

This does **not** prove a maximum over the full requested period. The API returned only 43 rows across a 342-second span, with gaps up to 75 seconds; its published response contract does not explain whether omitted timestamps indicate no trade or missing chart data. Candle timestamps also do not establish whether a high occurred before or after a burn in the same second. A ±1-second sensitivity bound around the final high reaches `$227,590.65358579069528504455271440` because the last burn was one second earlier. The evidence remains insufficient to promote the snapshot as a historical FDV maximum.

## Requests and unit control

Three bounded chart responses are retained under `.scout-data/diagnostics/solana-jc-chart-supply-bounds-*`:

- The initial request recorded `marketCap=false` in its typed manifest, but the exact wire query string was not saved. Its 43 highs were in market-cap scale: each is the control chart's corresponding USD price high multiplied by the finalized supply `965,761,089.271256`, within JSON number precision. The response's final high was `227310.9942480791`.
- A control request omitted `marketCap` and explicitly set `currency=usd&timezone=UTC`. It returned price-scale values, but every timestamp was exactly 7,200 seconds earlier than the requested interval. Those timestamps are not used for supply alignment.
- The final control omitted both `marketCap` and `timezone`, explicitly set `currency=usd`, and persisted its exact query string before sending: `type=1s&time_from=1791671908&time_to=1791672255&currency=usd&dynamicPools=true&removeOutliers=false`. Its 43 timestamps exactly match the initial response and fall inside the requested interval. Its highest price is `0.00023536979981208752`, equal to the ATH response's `highest_price`.

The first-party [chart contract](https://docs.solanatracker.io/data-api/chart/get-ohlcv-data-for-a-token) says `marketCap` defaults to false and returns price instead of market cap; it defines `time_from` and `time_to` as Unix seconds and `currency` as USD by default. The original request's lost wire serialization prevents a definitive explanation of why its `marketCap=false` result was cap-scaled. The final control avoids that ambiguity by omitting the flag. The explicit-UTC request's two-hour timestamp shift is also unexplained, so its timestamps are excluded from analysis.

## Supply bounds for returned highs

The cached complete mint-account ledger establishes birth supply of 1 billion tokens, 30 supported burns, and finalized anchor supply `965,761,089.271256`. Assuming each returned Unix `time` labels its corresponding one-second UTC bucket, the conservative upper bound subtracts burns with timestamps `< t`; the lower bound also subtracts burns at `t`. This treats a same-second high as potentially preceding that second's burn. The docs do not define finer event timing within a bucket. The separate ±1-second interval is only a clock-sensitivity diagnostic; it is not a documented provider guarantee.

| Returned high | Price high | Exact-second supply upper | Exact-second FDV upper | ±1s supply upper | ±1s FDV upper |
|---|---:|---:|---:|---:|---:|
| Final returned row, 22:44:14Z | `$0.00023536979981208752` | `965,761,089.271256` | `$227,310.99424807910909633689232512` | `966,949,259.282595` | `$227,590.65358579069528504455271440` |
| Largest earlier returned high, 22:44:02Z | `$0.000232153902551083` | `967,630,909.557580` | `$224,639.291882846235208879859140` | `967,630,909.557580` | `$224,639.291882846235208879859140` |

The final row is one second after the last burn at `22:44:13Z`; the exact-second supply therefore equals the finalized anchor. Its high agrees with the independently reported Tracker price. However, its low, close, and volume fields are zero while the high is positive. Those fields are retained as returned and are not interpreted as zero market activity or a complete OHLC candle.

The row-level exact-decimal calculations and both diagnostic supply intervals are in [the control sidecar](../../../.scout-data/diagnostics/solana-jc-chart-supply-bounds-no-timezone-2026-10-11/price-supply-bounds-sidecar.json). The request manifests and raw responses are hash-pinned beside it. No response contains a configured secret.

## Limits

The candidate FDV comparison covers only the returned high values. No completeness, gap-filling, or candle-ordering guarantee was found in the published contract. Missing intervals are unknown, not zero. Same-second burn order is not recoverable from a candle timestamp, and the ±1-second sensitivity demonstrates that a small timing discrepancy can change the conservative maximum. This chart therefore narrows the observed-row comparison but does not close the historical FDV maximum gap or establish canonical lifetime ATH.
