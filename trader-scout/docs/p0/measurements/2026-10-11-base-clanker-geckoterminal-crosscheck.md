# Base Clanker cohort: GeckoTerminal price cross-check (2026-10-11)

## Result

The 53 Base Clanker tokens with source-proven fixed 100B supply were cross-checked against keyless GeckoTerminal pool candles. The bounded job made 153 public GETs: one page-1 pool listing per token and 100 daily-OHLCV calls (up to two pools per token). All 153 returned HTTP 200, token-side identity matched for every candle response, and the minimum start-to-start interval was 15 seconds. Request bodies, URLs, provider headers, exact candle rows, and SHA-256 hashes are preserved in [the private manifest](../../../.scout-data/diagnostics/geckoterminal-base-additional53-2026-10-11/manifest.json).

30 of the 53 candidates pass both comparisons: GeckoTerminal's maximum USD high on the Birdeye peak date and its maximum high across the queried pool histories are each within 1% of the Birdeye high. The machine-readable [aligned candidate list](../../../.scout-data/diagnostics/geckoterminal-base-additional53-2026-10-11/aligned-candidates.json) records their exact decimal values and queried pools for review. These are **cross-check candidates only**, not production ATH values.

The remaining 23 are excluded from the aligned list: 10 have more than two matching page-1 pools, so at least one returned pool was not queried; 13 have a price disagreement greater than 1% on the same-day or queried-pool maximum. Several disagreements are extreme. For example, `0xfa1241…` agrees on the Birdeye peak day by +0.226% but the maximum among its two queried pools is +1,410.95% from Birdeye; `0xe18f80…` agrees on that date by +0.293% but its queried-pool maximum differs by +24.92%. A same-day match alone is therefore insufficient.

## Method and scope

The input was [the 53-token Base Clanker candidate file](../../../.scout-data/diagnostics/base-additional-clanker-input-2026-10-11.json), ordered descending by its exact Birdeye USD high so the largest candidates were examined first. Its high candle dates and values are used only as cross-check references. The Birdeye input contains sparse windows: 29 tokens have one returned daily candle, 14 have two, six have three, two have four, one has five, and one has seven.

For each token, the job queried `GET /api/v2/networks/base/tokens/{address}/pools?page=1&include=base_token,quote_token`. It recorded every matching pool returned but requested OHLCV for only the first two in provider response order. Ten listings had more than two matching pools; the remaining pool addresses are retained and those tokens are excluded. No page-1 response had a `links.next` value, but one page of this provider index does not prove complete historical pool discovery.

Each requested pool used `GET /api/v2/networks/base/pools/{pool}/ohlcv/day` with `aggregate=1`, `limit=1000`, `before_timestamp=1791676799`, `currency=usd`, the token's actual `base` or `quote` side, and `include_empty_intervals=false`. All 53 target tokens were the base token in these results. Only records timestamped inside `2025-10-10T00:00:00Z` through `2026-10-10T23:59:59Z` were used in comparisons. Numeric JSON values were parsed as decimal values, not binary floats. The difference is `(GeckoTerminal − Birdeye) / Birdeye × 100`.

The exact request parameters and candle timestamps are in the manifest. All pool metadata and candle response token addresses were checked against the requested target and pool relationship before their highs were considered. No empty-interval candle was synthesized. The 100 queried pool histories produced at most eight returned daily records each; they are sparse and pool-scoped.

## Interpretation

The source proof for these 53 token implementations and fixed supply is recorded in [the Base supply-proof manifest](../../../.scout-data/diagnostics/base-additional-clanker-proof-2026-10-11/manifest.json). That closes the denominator question for these candidates, but it does not make either price source exhaustive.

The aligned list says only that the two vendors' observed high values agree for the specified day and queried pool rows. It does not establish complete historical pool coverage, raw-swap coverage, every migration market, or a lifetime ATH. The rejected cases should remain unresolved; do not replace them with a current quote or accept a same-day match while a queried-pool maximum disagrees.

## Primary source

- [GeckoTerminal API reference](https://api.geckoterminal.com/docs/index.html) documents the public token-pool and pool OHLCV routes. The official public endpoint is keyless; its limits/history behavior and earlier single-pool age probe are summarized in [the provider research note](2026-10-11-free-ath-providers.md).
- Direct request URLs and source response hashes for this pass are listed per call in the private manifest; no account or API key was used.

## Page-1 pool-scope follow-up

A follow-up queried the 14 previously unqueried pool IDs for nine of the ten candidates that had more than two pools in their cached page-1 listing. The pool IDs and token-side parameters were resolved from SHA-verified cached listing responses; the original 53-token manifest was not changed. All 14 requests returned HTTP 200, the minimum request-start interval was 15 seconds, and each candle response matched the listed base/quote token identities.

Eight of these nine candidates now pass the same two checks across every pool returned in that cached page-1 listing: the maximum USD high on the Birdeye peak day and the maximum high in the observed pool histories are each within 1%. This adds eight cross-check candidates, for 38 aligned candidates out of 53. The [combined 38-candidate list](../../../.scout-data/diagnostics/geckoterminal-base-pool-scope-completion-2026-10-11/aligned-candidates-combined.json) points to both manifests; the [eight newly aligned entries](../../../.scout-data/diagnostics/geckoterminal-base-pool-scope-completion-2026-10-11/newly-aligned-candidates.json) are also available separately. The ninth token, `0x92d146…`, agrees on the Birdeye day (+0.012%) but its combined page-1 pool maximum is +109.78%; it remains excluded. One of the original ten pool-scope cases was not in this nine-token follow-up and remains unresolved. The 15 candidates outside the final aligned set therefore include that unqueried scope case, the observed-maximum disagreement, and the prior price-disagreement cases.

This closes only the observed page-1 pool-listing gap for eight candidates. It does not establish that the provider returned every historical pool, cover pools on another venue or before a migration, or prove lifetime ATH completeness. The exact additional requests, raw response hashes, candle identities, and per-token combined comparisons are in the [separate follow-up manifest](../../../.scout-data/diagnostics/geckoterminal-base-pool-scope-completion-2026-10-11/manifest.json); the initial 153-request manifest remains frozen.
