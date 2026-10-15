# Mobula and Moralis ATH alternatives — 2026-10-14

This primary-source follow-up checks whether Mobula or Moralis can add free historical price/ATH evidence for exact token addresses on Solana, Base, BSC, and public Robinhood Chain mainnet. It distinguishes a provider's direct ATH field, a historical point query, and OHLCV candles: none alone proves a complete lifetime maximum unless indexing starts at token/pool creation and covers relevant markets.

## Findings

| Provider | Documented route/data | Networks evidenced | Free access and decision |
|---|---|---|---|
| **Mobula** | Existing project research identifies `GET /api/2/token/ath` as a direct token ATH route. Its Swagger remains the canonical route reference. | Existing notes do not establish route-level token-address support for all four required networks or define whether ATH is all-market/lifetime, token-specific, or another aggregation. Current Mobula comparison page claims 88+ chains generally, which does not prove support for this endpoint/network combination. | The saved 2026-10-11 anonymous probe returned HTTP 403 with a response saying API access is unavailable on the Free plan. The current official comparison page advertises a $0 Free/Demo tier with 10,000 credits, but that allowance did not grant this endpoint in the captured probe. Do not spend another request on this route without a plan change or written endpoint entitlement. The existing 403 body and capture provenance are documented in `2026-10-11-free-ath-providers.md`; no new Mobula API request was made here. |
| **Moralis EVM** | Price API advertises exact-token historical price at a timestamp or block. Historical ERC-20 OHLC tutorials and the current API calculator describe pair candlesticks (`getPairCandlesticks`) as a separate route. Historical point price is not a time-series maximum; pair OHLCV `high` is only that pair's maximum over returned bars. | Official docs advertise BNB Chain/BSC and Base. General pricing says “all networks,” but the Price API's published chain list names Base and BNB and does not name Robinhood Chain. No source found that binds RH chain ID 4663 to either historical endpoint. | Current `moralis.com/pricing/` page says Starter costs $149/month billed annually. A Moralis developer pricing page and FAQ still show an older free Starter plan with 40,000 CUs/day. The official pages conflict, so current free entitlement for these historical endpoints is **unknown**; do not infer that a legacy free quota remains usable. API calls require an API key per published route examples. No API calls were made because no currently documented free entitlement is established. |
| **Moralis Solana** | Official Solana API Swagger lists `GET /token/{network}/{address}/price`, `/token/{network}/{address}/pairs`, and pair-scoped `/token/{network}/pairs/{address}/ohlcv`. The API docs list swaps separately. Current Solana docs describe OHLC candles, not a direct ATH field. | Solana mainnet is explicit in the Solana API. | The pair OHLCV route can provide pair-window highs if entitlement and range permit; it does not prove a token-wide maximum across pools, nor a lifetime maximum. Public docs reviewed did not state free access, maximum historical range, earliest indexed candle, or a max-bars cap. Entitlement and history depth remain unknown. |

## Historical price versus ATH

- Moralis historical point-price lookup returns a value at a requested timestamp or block; scanning sparse timestamps could miss an intraday peak.
- Moralis pair candlesticks include OHLC and therefore a per-returned-candle high. To compute a bounded-window high, the caller must enumerate the window, inspect response coverage, and retain the pair address/network. This is not a provider-native token-wide ATH.
- Pair-specific Solana OHLCV makes pool selection explicit. Aggregating all token pairs and migrations would require documented pair enumeration plus coverage and lifecycle checks; those guarantees were not found in reviewed sources.
- Neither provider's public material reviewed here proves a full-year range for arbitrary newly issued tokens on all target networks, and neither documents Robinhood Chain historical token OHLCV support in a way that can be relied on for mainnet chain ID 4663.

## Recommended bounded follow-up

1. Ask the project owner whether an existing Moralis API key is available and whether it has a verified no-charge entitlement; do not create an account, enable billing, or use a key until that is established.
2. If the owner confirms such access, make at most one GET per applicable documented route (maximum six total across both providers): one exact Solana pair OHLCV request and one exact Base/BSC EVM historical/candle request each, using known token/pair addresses and a fixed documented interval. Capture status, body up to 2 MiB, latency, exact query semantics, first/last returned timestamps, bar count, and an as-of marker. No retries or redirects, timeout 25 seconds.
3. For each response, record exact network and asset identity, route, plan/entitlement, earliest returned timestamp versus known pool/token creation, response pagination/range parameters, and whether the value is a point price, pair high, or direct ATH. Stop if it requires a paid plan or if entitlement is unclear.
4. Treat Robinhood mainnet as unsupported until official endpoint-level documentation or a bounded exact-chain request establishes it. Do not substitute brokerage-app data or RH testnet.

No credentialed or anonymous provider API calls, account creation, database writes, or software changes were made for this note. The only API evidence cited from prior work is the saved Mobula Free-plan 403 probe. Documentation facts were checked against first-party pages on 2026-10-14.

## Primary sources

- [Mobula API Swagger](https://api.mobula.io/docs) — route reference; interactive schema was not extractable in this review.
- [Mobula plan comparison](https://mobula.io/vs-coingecko) — currently advertises Free/Demo at $0 and 10,000 credits; general chain coverage is not an endpoint support matrix.
- [Moralis current pricing](https://moralis.com/pricing/) — current Starter pricing shown as $149/month billed annually, all-network plan text.
- [Moralis developer pricing](https://developers.moralis.com/pricing/) and [free request limits FAQ](https://developers.moralis.com/faq/what-are-the-limits-for-api-requests/) — still show free Starter and 40,000 CUs/day; these conflict with the current pricing page.
- [Moralis Price API](https://developers.moralis.com/api/price/) — historical point prices, candlesticks, all-major-EVM positioning, and published chain list including BNB Chain and Base.
- [Moralis Token API](https://moralis.com/api/token/) — ERC-20 current/historical pricing and pair/token data positioning.
- [Moralis Solana API Swagger](https://solana-gateway.moralis.io/api/) — current Solana routes including pair OHLCV, pair lookup, and token price.
- [Moralis Solana API help](https://api-help.moralis.io/en/articles/19194-using-the-solana-api) — Solana API overview and endpoint-dependent CU cost note.
- [Moralis historical ERC-20 OHLC tutorial](https://docs.moralis.com/get-started/tutorials/data-api/prices-and-charts/get-historical-erc-20-token-prices-ohlc-data) — pair-candle workflow.

## Open questions / limits

- Moralis's current free-plan status is internally inconsistent across first-party pages; current endpoint entitlement and endpoint-specific CU costs need confirmation against an existing project key/account.
- Moralis docs do not establish first candle time, history retention, arbitrary-token coverage, max candles/page, or complete market aggregation for the reviewed routes.
- No official RH Chain (4663) support for Moralis price/history routes was found.
- Mobula's 403 establishes the observed Free-plan rejection for one `/ath` request; no current all-network or exact-address support matrix was verified.
