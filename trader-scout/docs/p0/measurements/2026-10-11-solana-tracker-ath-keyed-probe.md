# Solana Tracker direct ATH probe for three Solana mints — 2026-10-11

## Result

The official Data API's documented direct ATH endpoint worked with the owner-provided Free-plan key for all three requested mints: exactly three authenticated GETs returned HTTP 200. The key was read only inside the probe process, sent only as `x-api-key`, and is not present in this report or the saved diagnostics. Every response body was checked for the exact key bytes before it was written; all three checks were false (key absent).

| Mint | `highest_price` USD | `timestamp` (ms, UTC) | Provider pool ID | Saved Birdeye 1s-window high | Relation |
|---|---:|---|---|---:|---|
| `48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump` | `0.00013242878993078837` | 2026-10-10 22:04:55.327 | `wc4hQ7k6mZe1hLLfkARE7gDjK6toJDpY2YCPKj8UEr5` | `0.00012179988071982598` at 22:04:54 | ST +8.73%; same sampled minute, but not the same candle second |
| `81kiaJSjaU9w6FVwRVT2K5TP1EhDghAXCm6Y4zmRpump` | `0.00009431116993278875` | 2026-10-10 22:45:47.033 | `eqEmQJtbfXvoaEkS3iJ1B1VZwRtE1Zns7ebGrb5dKoz` | `0.000009928178143700252` at 22:40:09 | ST is 9.50×, but about 5m47s after the saved 22:40 one-second window |
| `JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump` | `0.00023536979981208752` | 2026-10-10 22:44:14.212 | `4QnxqajHAqiKuTZjfsfkBA8qSxb7AY4p8axVZeNmVNuq` | `0.000008772215583865555` at 22:38:28 | ST is 26.83×, about 5m46s after the saved 22:38 one-second window |

The Birdeye comparison values above come from `.scout-data/diagnostics/solana-second-price-supply-crosscheck-2026-10-11.json`; that file records sparse one-second-window observations. A subsequent read-only check also confirmed that each saved closed-day 1D maximum has the exact same price as its one-second high: `.scout-data/diagnostics/solana-birdeye-daily-vs-tracker-2026-10-11.json`. The daily series returns only one candle per mint. The provider disagreement therefore also exists against these saved daily maxima; sparse one-second sampling alone does not explain it. Neither series proves full-lifetime coverage.

## Supply and market-cap interpretation

The saved Token-2022 birth/burn ledgers report an initial mint of `1,000,000,000` tokens for each mint (raw supply `1,000,000,000,000,000`, decimals 6). The returned `highest_market_cap` matches `highest_price × cached current supply`, rounded to cents, for all three:

| Mint | Returned `highest_market_cap` | `highest_price × current supply` | Supply at ST ATH from saved burn ledger | Current supply in saved proof |
|---|---:|---:|---:|---:|
| 48vcic… | `$125,954.79` | `$125,954.7868403…` | `997,060,502.547477` | `951,113,325.932600` |
| 81kia… | `$91,143.37` | `$91,143.3743929…` | `967,816,807.043853` | `966,411,236.949771` |
| JCd8… | `$227,310.99` | `$227,310.9942481…` | `965,761,089.271256` | `965,761,089.271256` |

The first two ATH timestamps precede later burns in the saved ledgers (25 post-ATH burn events for 48vcic…; one for 81kia…). For 48vcic…, multiplying the ATH price by supply at the ATH timestamp would yield about `$132,039.52`, rather than the API's `$125,954.79`. The returned market-cap field therefore behaves like peak price times the later current supply for that mint; the field is not a historical market-cap observation at the ATH timestamp. For the other two, the returned values also match current supply, though JCd8… has no post-ATH burn in this saved ledger. This arithmetic is strong evidence about these observed responses, not a general provider guarantee or an independent proof of the provider's supply algorithm.

Do not write `highest_market_cap` directly as Codex FDV without confirming the exact supply snapshot and semantics. Independent USD price corroboration remains necessary before choosing the applicable FDV denominator. All three disagree with the saved Birdeye daily highs. Pool IDs were not independently resolved during these three requests; later [cached transaction checks](2026-10-11-solana-tracker-pump-swap-peak-validation.md) and [quote-pool discovery](2026-10-11-solana-quote87-pool-bridge.md) verify their quote identities but do not authorize a numeric promotion.

## Provider contract and budget

Solana Tracker's [Get All-Time High Price](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) documents `GET https://data.solanatracker.io/tokens/{tokenAddress}/ath`, requires `x-api-key`, says the high is “since the data API started recording,” and examples show a millisecond Unix `timestamp`, `highest_price`, `highest_market_cap`, and `pool_id`. Per-token tracking start, all-pool aggregation, migration behavior, and supply denominator remain undocumented in the reviewed contract. The [public pricing page](https://docs.solanatracker.io/pricing) currently lists Free Data API at 10,000 REST requests/month and 3 req/s. The project owner reports this configured account's actual Free-plan allowance is 2,500 requests/month; this probe used 3 requests against that owner-reported allowance and did not infer or reset the quota.

## Reproducibility and limits

This report's request count is three. The subsequent quote-pool discovery used one additional TokenInfo GET, bringing this session's Solana Tracker total to **four requests** against the owner-reported 2,500/month allowance. No quota reset, remaining-account-balance assumption, or automatic full-cohort scan was made.

Subsequent verification used eight bounded Helius RPC calls in total, then cached-only calculations. A successful [JC atomic route](2026-10-11-solana-quote87-jc-bracket.md) corroborates the source price within0.44%; [81 nearby spot](2026-10-11-solana-81-nearby-spot-comparison.md) also numerically matches, with a separate-pool timing gap. These do not prove a historical FDV maximum over changing supply. The source price/supply distinction and next required history check are recorded in [the FDV decision note](2026-10-11-solana-tracker-price-peak-vs-fdv.md).

Three later bounded JC chart requests bring the session total to **seven Tracker requests** (3 ATH,1 TokenInfo,3 charts), with no automatic retries. The final price-scale, timestamp-aligned response gives43 rows; sparse gaps and same-second supply uncertainty keep the FDV maximum unproven. Unit/time controls and their immutable responses are in [the chart/supply report](2026-10-11-solana-jc-chart-supply-bounds.md). This report's original three-response evidence remains unchanged.

Raw bodies, exact public request URLs, status, response hashes, timestamps, safe response-header allowlist, parsed exact decimals, and the pre-persistence key-absence checks are in `.scout-data/diagnostics/solana-tracker-ath-keyed-probe-2026-10-11/`. The manifest contains only the API-key environment-variable name, never its value. No retries were made. No Codex, Birdeye, Helius/RPC, other market-data endpoints, database writes, or code changes were made. This is a provider ATH result since provider recording began, not yet a proven full-lifetime market-wide ATH or a validated FDV record.
