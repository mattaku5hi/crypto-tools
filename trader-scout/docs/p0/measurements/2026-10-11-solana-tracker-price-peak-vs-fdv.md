# JC price peak is not a proven FDV peak — 2026-10-11

## Decision

Do not promote the JC price observation into `ath_fdv_cents` as a maximum FDV. The evidence supports a precisely denominated FDV snapshot at the Solana Tracker price timestamp. A bounded chart control now supplies paired prices and supply bounds for 43 returned rows, but the sparse response, same-second burn ordering, and one-second sensitivity do not establish a maximum across the full interval.

The machine-readable calculations and frozen input hashes are in the [private audit sidecar](../../../.scout-data/diagnostics/solana-tracker-price-peak-vs-fdv-2026-10-11.json). This is a decision record only: it does not change the ATH policy, code, or database.

## Price and supply evidence

The saved Solana Tracker response reports `highest_price = $0.00023536979981208752`, timestamp `2026-10-10 22:44:14.212 UTC`, pool `4QnxqajHAqiKuTZjfsfkBA8qSxb7AY4p8axVZeNmVNuq`, and `highest_market_cap = $227,310.99`. The endpoint documentation describes its result as the token's all-time high price **since the Data API started recording**; it does not promise chain-lifetime coverage or define the historical supply basis of `highest_market_cap`. The returned pool ID is the JC/87 pool used by the independent route below.

The complete saved mint-address history contains 87 transactions, 30 supported burns, and no post-birth mint. JC was born with 1,000,000,000 tokens at 22:38:28 UTC; the last burn was 1,188,170.011339 tokens at 22:44:13 UTC. The finalized anchor supply is 965,761,089.271256 tokens, exactly equal to birth supply minus all 30 burns. Mint authority was revoked at birth and the Token-2022 mint has metadata-only extensions. The ledger therefore establishes that supply at the Tracker timestamp was 965,761,089.271256 tokens.

Multiplying the reported price by that timestamp's proven supply gives `$227,310.99424807910909633689232512`, or 22,731,099 cents when floored. This agrees with the response's `$227,310.99` `highest_market_cap` to the cent. It is an arithmetic **snapshot FDV under the reported timestamp**, not a same-event chain price/supply observation or a demonstrated all-time FDV maximum. The later chart sensitivity preserves the possible source-clock alignment gap.

An independent finalized, successful route at 22:44:16 UTC used the same JC/87 pool and atomically sold JC through 87ea into wSOL. Its exact user-output route price, converted with the cached Kraken SOL/USD candle, was `$0.0002343377102053727–$0.0002344864151252746` per JC. Both endpoints are within 1% of the Tracker price: 0.4385% and 0.3753% lower. This corroborates the reported price level in the identified pool shortly after its timestamp. It does not make the two observations simultaneous or prove a chain transaction at the Tracker millisecond. See the [event audit](../../../.scout-data/diagnostics/solana-quote87-pool-discovery-2026-10-11/quote87-jc-event-audit.json) and [timestamp bracket report](2026-10-11-solana-quote87-jc-bracket.md).

## Why that snapshot cannot stand in for maximum FDV

Before the final burn, supply was 966,949,259.282595 tokens. At that supply, a price only 0.1229% below the Tracker peak would already produce an FDV equal to the later snapshot. For example, an earlier price at 99.9% of the Tracker peak—`$0.00023513443001227543248`—would produce `$227,363.0629322049`, about `$52.07` above the snapshot. This is a sensitivity example, not evidence that JC traded at that price.

At the full 1 billion birth supply, the Tracker peak price would imply `$235,369.79981208752`. Since the observed price high bounds every price in the Tracker's own recorded scope and supply never exceeded birth supply, this is a conservative upper bound for that scope. The currently supported FDV snapshot is the lower bound. The remaining interval is too wide to identify the maximum.

Codex documents `athPrice`/`athPriceTimestamp` and `athFdv`/`athFdvTimestamp` as separate extrema fields. That contract does not say the FDV extremum is always the highest price multiplied by current total supply. ADR-021 amendment 4's earlier sentence that Codex ATH is the highest price times total supply is an informal simplification; it gives no temporal rule for variable supply. Using the JC price peak times current supply as `ath_fdv_cents` would silently substitute a price extremum for an FDV extremum.

## Evidence needed to close the gap

For a source-scoped FDV maximum, retain a price/supply history for the source's observation period and compare price times supply at every price observation, or prove conservative bounds for each interval between supply changes. If price and burn share a candle or second, establish their canonical order or use the higher pre-burn supply as the conservative denominator. The evidence must also record the source's pool-coverage scope: the current response names the winning pool, and the route validates that pool, but the saved artifacts do not enumerate every JC pool and its returned price maximum.

Until that joint history and scope are available, the evidence supports only the Tracker-observed price timestamp and its FDV snapshot. The other two candidate mints remain unpromoted as FDV maxima as well: 48 misses the existing 1% corroboration threshold, and 81's nearby quote evidence does not resolve a time-aligned USD price. No token is being represented here as having a proven canonical lifetime ATH.

### Follow-up chart check

The initial chart response was cap-scaled despite its manifest recording `marketCap=false`; its exact serialized query was not retained. A later control omitted that flag and returned price-scale rows with timestamps matching the original 43 rows. Their high values map to the original highs by the fixed finalized supply, and the final returned row's price high equals the Tracker ATH. Within returned rows, the exact-second conservative FDV upper maximum is the final snapshot. The ±1-second diagnostic allows a slightly higher value because the last burn occurred one second before that high; chart coverage also has gaps up to 75 seconds and the endpoint contract gives no completeness guarantee. See the [chart supply-bound report](2026-10-11-solana-jc-chart-supply-bounds.md) and [row-level sidecar](../../../.scout-data/diagnostics/solana-jc-chart-supply-bounds-no-timezone-2026-10-11/price-supply-bounds-sidecar.json).

## Sources

- [Solana Tracker: Get All-Time High Price](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) — endpoint scope and response fields.
- [Codex: TokenExtrema](https://docs.codex.io/api-reference/types/tokenextrema) — separate price and FDV extrema and timestamps.
- [Supply reconciliation and limits](2026-10-11-solana-ath-supply-ledger.md) — JC birth, burn ledger, and finalized anchor.
- [JC quote/price timestamp bracket](2026-10-11-solana-quote87-jc-bracket.md) — nearby same-route execution and FX evidence.
- [48 virtual-reserve price comparison](2026-10-11-solana-virtual-reserve-price-reconciliation-v3.md) — separate 48 candidate gap result.
