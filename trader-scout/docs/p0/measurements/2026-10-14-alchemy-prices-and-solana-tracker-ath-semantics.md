# Alchemy historical-price pilot and Solana Tracker ATH contract — 2026-10-14

## Scope and result

This bounded follow-up used six Alchemy Prices API requests after the two earlier four-request pilots (`alchemy-prices-pilot-2026-10-14` and `alchemy-prices-pilot-v2-2026-10-14`). The combined total across all three pilots is **14 requests**. No Solana Tracker, Codex, or database requests/writes were made. The six exact request bodies, statuses, response hashes, and response files are in the private diagnostic directory [`alchemy-prices-semantic-pilot-2026-10-14`](../../../.scout-data/diagnostics/alchemy-prices-semantic-pilot-2026-10-14/manifest.json); credential values and full credential-bearing URLs are absent.

Alchemy's published SDK enum supplies the endpoint's canonical chain strings `solana-mainnet` and `bnb-mainnet`; the earlier `sol-mainnet` spelling was rejected. A 10-minute historical request returned three 5-minute points for wrapped SOL, WBNB, and CAKE. The same validated network parameters returned “Token not found” for the tested Solana low-cap mint and two Base candidates (the earlier e465 candidate plus current-window candidates 00b0 and 5b2a). Thus this is a working sampled-price supplement for supported/listed assets, but these tests did not find coverage for the tested runner tokens.

These responses contain point values, not OHLC highs, raw trades, or a provider lifetime maximum. An interval point series cannot establish a candle's intrainterval high, all-pool/migration coverage, or token supply at an extreme. Alchemy's FAQ describes address prices as volume-weighted averages of DEX prices, and its historical response example exposes `value`, `timestamp`, `marketCap`, and `totalVolume`; it does not document a historical maximum endpoint or candle-high field. The Prices FAQ states a Free limit of 300 requests/hour. The pilot's six calls are evidence of route behavior, not a coverage estimate for the project's universe.

## GoldRush OHLCV is not an annual-history fallback

GoldRush's first-party streaming OHLCV API takes a `timeframe` enum rather than arbitrary `from`/`to` dates; its documented enum runs through `THIRTY_DAYS` and has no 365-day or all-time value. The endpoint can return candle `high`, but that maximum 30-day lookback is too short to replace annual history or close older lifetime-ATH gaps. The streaming API documentation also requires a GoldRush API key. This is a documentation-level capability finding only: no GoldRush account or API request was made, and per-token/pool coverage was not tested. The official enum reference URL returned 404/403 from this review environment, so the exact enum values are recorded from the first-party reference but were not independently fetched here.

## Exact pilot outcomes

| Request | Network / address | Result | Meaning |
|---|---|---|---|
| SOL control | `solana-mainnet` / wrapped SOL | 200, 3 points | Canonical identifier works for a listed token. |
| SOL candidate | `solana-mainnet` / `88pqb…pump` | 400, `Token not found` | This runner mint was not returned; not a test of every Solana asset. |
| BNB controls | `bnb-mainnet` / WBNB and CAKE | 200 each, 3 points each | Canonical BNB identifier works for listed assets. |
| Base candidates | `base-mainnet` / `00b0…` and `5b2a…` | 400 each, `Token not found` | Both tested candidates were absent from this endpoint's index. |

All successes used `interval=5m` and the same ten-minute request shape. The control timestamps exactly bracket the requested interval. No inference is made about the API's maximum historical retention: this pilot tests a short, known historical slice only.

## Chain identifier evidence

Alchemy's archived first-party JavaScript SDK defines `Network.SOLANA_MAINNET = 'solana-mainnet'` and `Network.BNB_MAINNET = 'bnb-mainnet'`, and its Prices namespace accepts the general `Network` type for address-based historical prices. The SDK also warns that some functions are unavailable on some networks, so the enum alone is not proof of Prices-product coverage. The successful controls above independently verify these two identifiers for the Prices endpoint and these specific listed tokens. The API endpoint's current docs do not publish an endpoint-specific exhaustive network enum; chain support and per-token indexing must remain separate questions.

## Solana Tracker ATH field semantics

The first-party [Get All-Time High Price endpoint](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) describes its result as the high “since the data API started recording.” Its example contains `highest_price`, `highest_market_cap`, a millisecond `timestamp`, and `pool_id`; the formal response schema on the same page guarantees only `highest_price` and `timestamp`. The reviewed contract does **not** define:

- whether `highest_price` is a swap execution ratio, a post-swap pool spot, an OHLC high, or another mark;
- whether `pool_id` is the pool that established the high, a selected/current pool, or a representative pool among several;
- how multiple pools and bonding-curve-to-DEX migrations are aggregated;
- the exact supply snapshot used for `highest_market_cap`.

Prior saved responses showed `highest_market_cap` arithmetically matching `highest_price × current supply` for three burned Solana tokens, while their ATH-time supplies differed. That is a small observed sample, not a general provider guarantee; see [the keyed probe](2026-10-11-solana-tracker-ath-keyed-probe.md) and [price/FDV comparison](2026-10-11-solana-tracker-price-peak-vs-fdv.md). Cached Helius neighborhood checks disagree with Tracker highs for some tokens, but without a documented price-kind definition this disagreement alone cannot identify which observation semantics differ. No new Tracker calls were made here.

The strongest usable interpretation is limited: Tracker provides a provider-recorded-period maximum price and timestamp, plus undocumented example fields. It is not a documented launch-to-present chain-wide ATH or Codex-compatible FDV result. Validate a returned peak independently against same-pool state/trade evidence and contemporaneous quote conversion before treating its numeric value as a candidate price; preserve the provider scope and unknown price kind.

## Primary sources

- [Alchemy Historical Token Prices endpoint](https://www.alchemy.com/docs/data/prices-api/prices-api-endpoints/prices-api-endpoints/get-historical-token-prices) — request fields, interval response, and status contracts.
- [Alchemy Prices API FAQ](https://www.alchemy.com/docs/reference/prices-api-faq) — DEX aggregation method, token coverage claims, and Free request limit.
- [Alchemy SDK `Network` enum](https://github.com/alchemyplatform/alchemy-sdk-js/blob/master/src/types/types.ts) and [Prices namespace](https://github.com/alchemyplatform/alchemy-sdk-js/blob/master/src/api/prices-namespace.ts) — canonical network enum values and use of `Network` in historical address lookups. The repository is archived; API support was cross-checked only for the successful controls above.
- [Solana Tracker Get All-Time High Price](https://docs.solanatracker.io/data-api/tokens/get-all-time-high-price) — stated tracking scope, example fields, and formal response schema.
- [Solana Tracker public pricing](https://docs.solanatracker.io/pricing) — published general Data API limits; the project owner has separately reported a 2,500-request monthly allowance for its configured Free key, and this task made no Tracker requests.
- [GoldRush OHLCV token query](https://goldrush.dev/docs/api-reference/streaming-api/queries/ohlcv-tokens-query) and [OHLCV timeframe enum](https://goldrush.dev/docs/api-reference/streaming-api/types/ohlcv-time-frame) — OHLCV fields and lookback selection. The enum link was inaccessible (404/403) during this review; no live query was made.

## Diagnostics and limitations

Private response bodies are mode `0600`; the manifest stores only the API-key environment-variable name as credential provenance. The Alchemy base control and e465 request from the prior pilots are cited by their frozen manifests and were not repeated. The combined exploration made 14 Alchemy Prices requests across three manifests. No response was used as an ATH or written to production data. Coverage for Robinhood Chain, arbitrary BSC/Base contracts, historical maxima, OHLC highs, and per-token retention remains unverified by this pilot.

## GoldRush published timeframe clarification

One cached rendering of the first-party [OHLCV timeframe reference](https://goldrush.dev/docs/api-reference/streaming-api/types/ohlcv-time-frame) listed nine enum values, ending at `THIRTY_DAYS`; it listed no yearly or all-history value. Subsequent fetches returned 404/403, and no live GraphQL schema was tested. This resolves the older note’s enum question for that published reference, while leaving current runtime validity unverified. That documented query window does not supply a year of historical peaks. The previously documented historical price endpoint supplies sampled prices, not candle highs. The transcribed rendering facts and retrieval caveats are retained in `.scout-data/diagnostics/goldrush-timeframe-rendered-doc-facts-2026-10-14.json`.
