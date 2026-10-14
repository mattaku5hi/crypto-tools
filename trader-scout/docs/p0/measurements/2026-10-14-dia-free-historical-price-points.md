# DIA free historical price-point source

## Finding

DIA documents an unauthenticated historical chart-points API that is a new candidate for independent price cross-checks on Base, BNB Chain, and Solana. It is **not** an ATH endpoint or OHLC feed: it returns sampled, methodology-filtered prices, so its maximum is only the maximum of those observations and can miss intraperiod highs.

The documented REST shape is:

```text
GET https://api.diadata.org/v1/assetChartPoints/{filter}/{blockchain}/{address}
    ?starttime={unix_seconds}&endtime={unix_seconds}&scale=5m
```

The docs list filters `MA120`, `MEDIR120`, `VOL120`, and `MAIR120`; chart scales start at five minutes and include `30m`, `1h`, `4h`, `1d`, and `1w`. Responses contain timestamped `value` points. DIA says the free API needs no registration or API key, applies rate limits, and is intended for testing/evaluation. Its free tier advertises 3,000+ assets and names Solana, BNB Chain, and Base. DIA’s public feeds show chain labels `Solana`, `BinanceSmartChain`, and `Base`; I found no Robinhood Chain listing.

The data is derived from trades across multiple CEX/DEX sources and DIA documents per-point pool/pair provenance in GraphQL. This offers an independent, chain/address-level point series that may help check an existing peak date or bound a missing point. DIA’s marketing and feed pages present prices in USD, but the chart-point response schema documents only a generic `value` field; USD units and exact-token availability should be verified in a small response before use.

## Limits and next bounded validation

- A five-minute minimum sampling interval and MAIR/MA/median-style filters do not establish a trade price, candle high, or lifetime high. Never promote `max(value)` as canonical ATH without separate coverage and price-kind evidence.
- The docs give `starttime`/`endtime`, but do not state maximum history retention, maximum response size, pagination, or per-token coverage. The free tier’s rate limit is unspecified.
- DIA says free access is for testing/evaluation. The hosted REST v1 and GraphQL APIs are scheduled for discontinuation on **2026-12-20**; continuing use requires following DIA’s migration path to Lumina.
- Robinhood Chain support was not established. Do not infer it from the separate Robinhood stock feed.

The smallest useful test is one exact-address query for a known Base or Solana candidate and a narrow, already-known UTC interval. Record the exact chain label, status, response hash, `value` denomination, first/last timestamp, number of points, and any returned source pools/pairs. If the candidate is absent or the endpoint rejects the chain/address, stop; do not widen to symbol search. A positive result validates only that token and interval, not the full cohort or lifetime history.

## Bounded no-key probe (2026-10-14)

Three GETs were used, with no authentication, retries, or redirects. Each response was capped at 1 MiB and saved with its hash in [the private capture manifest](../../../.scout-data/diagnostics/dia-ath-probe-2026-10-14/manifest.json).

The known Base control `0xB1a03EdA10342529bBF8EB700a06C60441fEf25d` returned HTTP 200 from both the address quotation and the `MAIR120` 5-minute chart. The quotation price and the chart `value` at `2026-10-14T14:29:55Z` matched exactly as decimal JSON values: `0.001473336872684133`. The feed page identifies the Base token price in USD, so this control supports interpreting the chart value in the same USD units as DIA’s quotation. The live response’s `Series.columns` were `time,address,blockchain,exchange,filter,symbol,value`, which differs from the shorter example schema in the docs.

The third request queried unresolved Base token `0x00b0a07ec58d87dc09606e3166e1312f679e3b07` around midnight on May 2 (`23:50Z` on May 1 to `00:10Z` on May 2). That did **not** include the saved on-chain mark at `2026-05-02T04:14:31Z`, so it was not a peak-window check. It returned HTTP 200 and valid JSON with `Series:null` and zero points. The first capture script incorrectly classified the null series as a JSON parse failure; it had parsed successfully.

A separately budgeted fourth GET corrected the window to `2026-05-02T04:04:31Z–04:24:31Z`, centered on the saved event (Base block 45,452,962; tx `0x0ed48eb07da0d1d07049e0f8a91211d78aede58d31f86bf80883e3eac83fcd46`). It again returned HTTP 200, valid JSON, and `Series:null` (zero points). This establishes no DIA point coverage for this particular peak window; it does not establish absence of other token history or lifetime coverage. The corrected request and parse status are in the new [correction manifest](../../../.scout-data/diagnostics/dia-ath-probe-2026-10-14/manifest-v2.json), which pins the original manifest and raw response without modifying them.

Capture hashes: quotation `ee2ad920da1977955ba766bd91403571b06452ecd247ba92e76313cb12873e48`; control chart `ef6e1004418427744f15132d4db13f8df91b31dff95910ae00d7cc0d0d4607e2`; midnight target chart `7a4715ec88bfb1155200bd66f081ed6c228eed8f7f9611b35574ee5ce0ea4b12`; corrected target-window chart `7a4715ec88bfb1155200bd66f081ed6c228eed8f7f9611b35574ee5ce0ea4b12`. Original manifest SHA-256: `92360c99cc24c4f92b2ad63818d9e8c81f7e12ce58fefa4111e309ac0651a534`; correction manifest SHA-256: `c32a565cd721685daf38ac91e25fb8ca39c0980379b6267a55af56e55bdd8131`.

## Primary sources

- [DIA free Crypto API: access, coverage, rate-limit qualification, and intended testing scope](https://www.diadata.org/free-crypto-api/)
- [DIA Asset Chart Points API: endpoint, filters, timestamps, scale, and response shape](https://www.diadata.org/docs/reference/apis/token-prices/api-endpoints/asset-chart-points)
- [DIA GraphQL token price feeds: address-based selection, time range, source-pair filtering, and response fields](https://www.diadata.org/docs/reference/apis/token-prices/graphql)
- [DIA supported blockchains](https://www.diadata.org/docs/reference/apis/token-prices/api-endpoints/blockchains)
- [DIA API deprecation notice](https://www.diadata.org/docs/reference/apis/token-prices)
- [DIA Base asset feed example](https://www.diadata.org/app/price/asset/Base/0xB1a03EdA10342529bBF8EB700a06C60441fEf25d/)
- [DIA Solana asset feed example](https://www.diadata.org/app/price/asset/Solana/DUSTawucrTsGU8hcqRdHDCbuYhCPADMLM2VcCb8VnFnQ/)
