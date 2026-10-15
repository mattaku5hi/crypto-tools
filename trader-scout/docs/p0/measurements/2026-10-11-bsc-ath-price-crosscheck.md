# BSC unknown-ATH cohort: GeckoTerminal price cross-check (2026-10-11)

## Result

For all 13 BSC tokens in the unknown-Codex-extrema cohort, GeckoTerminal returned pool metadata and daily OHLCV without authentication. The bounded pass used 36 GETs total: 13 page-1 token-pool listings and 23 pool OHLCV requests. All returned HTTP 200; the minimum start-to-start request interval was 15 seconds. Token/base/quote identity matched for every OHLCV response.

For 9 of 13 tokens, GeckoTerminal's same-day USD candle high and its maximum high among all queried pool histories both agree with the Birdeye maximum daily high within 1%. Four tokens are materially discordant and should not be treated as cross-validated price values. Exact aligned and discordant candidate lists, all raw responses, SHA-256 values, request URLs, candle rows, and pagination metadata are in [the private manifest](../../../.scout-data/diagnostics/geckoterminal-bsc-crosscheck-2026-10-11/manifest.json).

These results are provider cross-checks, not ATH fills. The query examined only page 1 of GeckoTerminal's token-pool list (1–3 matching pools were returned per token; no `links.next` was present) and the returned candle rows. This does not prove GeckoTerminal indexed every historical/retired pool, every pre-migration market, or every venue. OHLCV used `include_empty_intervals=false`; no candle means no observation, not zero price. All Birdeye cohort maxima occurred in the first returned Birdeye candle, so their annual/lifetime completeness remains unresolved even where both vendors agree. Supply invariance for this 13-token cohort is separately documented in [the BSC supply audit](2026-10-11-bsc-ath-supply-source-audit.md).

## Method

The source cohort is the 13 rows in `.scout-data/diagnostics/bsc-ath-unknown-source-supply-2026-10-11/manifest.json`. For each token, the request was `GET /api/v2/networks/bsc/tokens/{token}/pools?page=1&include=base_token,quote_token`. Every matching pool on the returned first page was queried (none exceeded the four-pool cap) at `GET /api/v2/networks/bsc/pools/{pool}/ohlcv/day` with `aggregate=1`, `limit=1000`, `before_timestamp=1791676799`, `currency=usd`, `token=base`, and `include_empty_intervals=false`. All target tokens were the pool base token. For comparison, only daily highs whose UTC timestamp fell in `2025-10-10T00:00:00Z` through `2026-10-10T23:59:59Z` were considered. JSON decimal lexemes were parsed with decimal arithmetic; displayed percentages are `(GeckoTerminal - Birdeye) / Birdeye × 100`.

The official [GeckoTerminal API reference](https://api.geckoterminal.com/docs/index.html) documents the token/pool and pool-OHLCV routes. The keyless endpoint, chain support, rate-limit evidence, and prior history-window probe are summarized in [the provider research note](2026-10-11-free-ath-providers.md). Direct request URLs and response `Date`/content-type headers are preserved in the raw-request manifest.

## Per-token comparison

The `GT same-day high` and `GT queried-pool maximum` columns are identical for every token in this sample: the maximum among the queried candles is on the Birdeye peak day. “Pool count” counts the matching page-1 pools actually queried, not a claim that it is the complete market universe.

| BSC token | Birdeye peak day | Birdeye high USD | GT same-day high USD | Difference | Pools queried | Result |
|---|---:|---:|---:|---:|---:|---|
| `0x29b9f4e55b3c53b740597e69276a09885185ffff` | 2026-02-08 | 0.00037776923471387927 | 0.00037860709813585 | +0.2218% | 2 | Within 1% |
| `0x4353a83772f9ad97547801821687510d68c97777` | 2026-01-29 | 0.00010486682563776066 | 0.000104877591620805 | +0.0103% | 1 | Within 1% |
| `0x5b02f9454c368caf86869880d5db819809b9ffff` | 2026-02-08 | 0.00006741693698757898 | 0.0000673988652605198 | −0.0268% | 2 | Within 1% |
| `0x7e310f8e59885c17031de76dff29683e1f82ffff` | 2026-02-09 | 0.00008637865172507822 | 0.0000863131106424258 | −0.0759% | 2 | Within 1% |
| `0x7fca431f8a3b946f29e40f88cd29b9382530ffff` | 2026-03-25 | 0.000003082959590133124 | 0.000110028041110788 | +3468.9096% | 2 | Discordant |
| `0x8ce61d628a14f558e0eef44486ef7fd616bcffff` | 2026-03-25 | 0.000002696433914539064 | 0.00011347688748849 | +4108.4060% | 3 | Discordant |
| `0x8dbc967eec621908ed2b1c22e6b93fcb696bffff` | 2026-04-01 | 0.00008282339138022202 | 0.0000827554288100639 | −0.0821% | 2 | Within 1% |
| `0x8e43558cbad1e8f5928a2b88c52488ec76047777` | 2026-01-30 | 0.00009213849282293572 | 0.0000915746622302863 | −0.6119% | 1 | Within 1% |
| `0x8f4c3f63b27df01460841f9fb02d552432d2ffff` | 2026-02-08 | 0.0003417708256833763 | 0.00034408335792995 | +0.6766% | 2 | Within 1% |
| `0x930eeed71881738ecf972168d293891ac708ffff` | 2026-02-08 | 0.0003408746000562565 | 0.000358583090943082 | +5.1950% | 2 | Discordant |
| `0xbf6cd09edd81aaa181a784fb41d6dbfa07727777` | 2026-03-10 | 0.0000961713685983613 | 0.0000961870498613009 | +0.0163% | 1 | Within 1% |
| `0xfd74b6b00d620b1db96985f92be6a3b98e7d4444` | 2026-01-21 | 0.00010094846571056154 | 0.0000899802663406366 | −10.8651% | 1 | Discordant |
| `0xff48007f4560c3a3837b05a8614b4cc00da64444` | 2025-10-30 | 0.000046799291848223665 | 0.0000470078308970913 | +0.4456% | 2 | Within 1% |

The nine “within 1%” addresses are recorded under `crosscheck_aligned_candidates` in the private manifest as candidate corroborations. The four originally discordant addresses are under `crosscheck_discordant`. Their table values preserve the direct provider comparison. The bounded on-chain follow-up below supports GeckoTerminal for the first three; FD74 has daily FX sensitivity corroboration for the same-day high under ADR-021 amendment 14, but its instantaneous event-time FX remains unresolved because nearby populated minute candles do not bracket the swap. Neither list asserts lifetime ATH.

## Limits

- The public pool listing returned only page 1. Even without a `links.next`, the response is a provider index and does not prove complete historical pool discovery.
- The OHLCV request returns highs for the selected pool and requested time range, not raw swaps, token-level aggregated highs, or a guarantee that every migrated/retired pair is represented.
- The oldest token's returned candle history is sparse and its peak date is outside a 180-day lookback from the as-of date. The pass records what the keyless route returned for the `before_timestamp` query; it does not infer a universal retention policy from these 13 tokens or override the earlier single-pool old-history gate observation.
- A match within 1% validates only agreement of these provider observations at the same UTC day and observed returned-pool maximum. It is not evidence that either vendor's history is exhaustive.

## Subsequent on-chain source check

A separate bounded audit found receipt-verified admitted-pool swap evidence for all four rows. The first three maxima are PancakeSwap v2 swaps converted with exact-minute BNB/USDT candles and dated Kraken USDT/USD ranges; they agree with GeckoTerminal within 1% and remain far from Birdeye. The fourth maximum is a Pancake v3 swap at 2026-01-21 17:31:50Z. Its protocol-decoded ratio is `0.0001009556688883885308102701063 USD1/token`. KuCoin identifies the exact BSC USD1 contract, but its 17:31 UTC USD1/USDT candle has zero volume and turnover. A single one-hour window found no positive-volume candles bracketing the swap within five minutes on both sides (nearest: 17:27 before and 17:38 after). The separate closed daily candle has positive volume and turnover; its USD1/USDT daily low/high combined with the same-day Kraken USDT/USD low/high yields a daily sensitivity interval within 1% of Birdeye and more than 11% from GT. This matches the existing daily stablecoin FX method in ADR-021 amendment 14 and supports same-day provider corroboration only, not an instantaneous event-time quote. The zero-volume exact-minute candle and failed five-minute bracket remain excluded gaps. These event observations do not establish lifetime ATH. Full sources and limitations: [bounded on-chain audit](2026-10-11-bsc-discordant-ath-onchain.md).
