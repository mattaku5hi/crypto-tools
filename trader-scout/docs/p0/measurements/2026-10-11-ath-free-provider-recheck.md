# ATH provider recheck: Moralis, Bitquery, DEXTools, Pump.fun — 2026-10-11

## Result

This pass checked whether four shortlisted sources materially close the remaining
Solana, Base, and Robinhood ATH gaps. No authenticated or live API calls were
made: each usable path either requires an account/key, has only a short free
window, or does not document an ATH field. It does not change stored ATH values.

**Best new lead is Moralis for bounded Solana/Base price-history reconstruction, conditional on resolving its current free-plan status.** Its first-party Price API page advertises historical point prices and OHLCV candles, full historical prices, and broad DEX coverage; its official API docs require an API key. The current `moralis.com/pricing` page lists Starter at $149/month billed annually, while the older official `developers.moralis.com/pricing` page lists a free Starter with 40,000 compute units/day and Price API access. These first-party pages conflict, so a free entitlement must not be assumed. No direct all-time ATH endpoint, historical depth bound, pool aggregation/migration guarantee, or Robinhood Chain support was established. The EVM API enum includes BSC and Base; Solana has its own Price API surface. Robinhood is not shown in the reviewed Moralis endpoint enums. [Moralis Price API](https://moralis.com/api/price/), [current pricing](https://moralis.com/pricing/), [older developer pricing](https://developers.moralis.com/pricing/), [key requirement](https://docs.moralis.com/web3-data-api/evm/get-your-api-key), [token-price API overview](https://api-help.moralis.io/en/articles/19193-get-any-token-price).

## Provider findings

| Provider | Free access and useful capability | Coverage and limitation for ATH work | Assessment |
|---|---|---|---|
| **Moralis Price API** | Official marketing documents point-in-time historical prices, OHLCV candles and “Full Historical Prices”; Price API is listed in the older free Starter tier. API requests require `X-API-Key`. | No reviewed endpoint promises a direct lifetime ATH. OHLCV `high` could reconstruct a bounded observed maximum, but exact retention, first tracked time, pool selection and migrations remain undocumented here. Official EVM token-price parameters enumerate BSC (`0x38`) and Base (`0x2105`); Solana has a separate surface. No Robinhood/4663 mapping found. Current pricing conflicts with older free-plan docs. | Worth one key-owner entitlement check and small fixed sample only if account UI confirms free Price API access. Not a demonstrated all-chain or lifetime ATH source. |
| **Bitquery** | The 7-day trial is 1,000 API points, requires free account/OAuth token, and is documented as real-time only. Crypto Price `Trading` OHLC supports 1-second/1-minute high data for the rolling ~30 days; full historical trades can be grouped into OHLC. | Official docs explicitly cover Solana, BSC, Base, and Robinhood Chain (`EVM(network: robinhood)`, chain ID 4663). For old history, `archive`/`combined` access is a per-chain paid add-on; RH and Solana have separate history products. The free trial does not include Robinhood archive; do not treat a “trial” as free annual backfill. The rolling Trading window is suitable for forward observation, not prior lifetime highs. | Strong technical coverage, including RH, but not a free historical fill for the remaining cohort. It could serve forward-only recent observations during trial, with exact start boundary and no lifetime claim. |
| **DEXTools API V3** | Free API tier documents 10,000 credits/month and 60 requests/minute. Every request requires an API key; setup requires creating an account, though no payment method is required. | V3 overview documents real-time and historical DEX data, but the reviewed public reference provides no direct ATH contract/field or explicit history retention. Existing token-metrics review identifies current price/short-window metrics, not ATH. Quick-start's example chain list is not exhaustive; RH support is unverified. | No reason to spend a key/signup on this gap yet. Do not reinterpret max/current metric as ATH. Existing report contains endpoint-level review. |
| **Pump.fun Coins v2** | The official agent skill documents `GET /coins-v2/{mint}` and a current response with `market_cap`, `usd_market_cap`, `total_supply`, and bonding-curve/pool metadata. | The official example and schema do not define `ath_market_cap` or its units, derivation, tracking start, migration, or supply denominator. A previous bounded public probe observed undocumented ATH-like fields on one response; that is runtime behavior, not a stable official contract. This is Solana/Pump-only. | Do not use the undocumented field as canonical ATH/FDV without first-party semantics and independently verified price/supply. Existing probe/report should remain the evidence record. |

Bitquery's own OHLC documentation says a candle's `High` is the highest price in
that interval and points to chain-level DEX trades for deep history. That is
reconstructible market history only within the dataset/market coverage the
query actually returns; it does not establish token-wide pool completeness or
historical FDV supply semantics. [OHLC candle docs](https://docs.bitquery.io/docs/trading/crypto-price-api/crypto-ohlc-candle-k-line-api/), [pricing/trial/archive terms](https://bitquery.io/pricing), [Robinhood chain history](https://docs.bitquery.io/docs/blockchain/robinhood/), [Robinhood Pons history details](https://docs.bitquery.io/docs/blockchain/robinhood/pons-api/).

DEXTools quota and authentication are stated in its [V3 reference](https://api-portal-dev.dextools.io/docs) and [quick start](https://api-portal-dev.dextools.io/docs/quickstart). Pump's official [Coins v2 documentation](https://github.com/pump-fun/pump-fun-skills/blob/main/swap/SKILL.md) shows current coin fields and requires callers to verify token-program identity on-chain; it does not define a historical ATH contract. The earlier [live field probe](2026-10-11-pump-coins-v2-ath-fields.md) is intentionally retained as an undocumented-field observation.

## Recommended next bounded step

1. Ask the owner to check the existing Moralis account's actual plan entitlement, without sharing a key. If the Price API endpoint is included at no charge, select a fixed set of already-priced unresolved mints across Solana and Base, including a migrated and an inactive-pool case; request documented OHLCV and record first/last candle timestamps, count, exact high/time, endpoint cost, and empty/error status. Do not bulk backfill until that pilot establishes history depth and actual plan billing.
2. Keep Bitquery as a possible trial-only live-forward source, not an annual history source. A one-month window beginning at subscription time cannot repair old ATH gaps.
3. Do not spend a provider request on DEXTools or Pump.fun until exact endpoint docs establish direct ATH or historical candle access and its free entitlement. Do not promote any `market_cap`, price ATH, or OHLC high to Codex `athFdv` without the project supply denominator and pool/launch coverage checks.

## Scope

Official documentation and pricing only; no provider API requests, account
creation, keys, DB access, code changes, or ATH writes. This recheck makes no
claim that any provider indexes all tokens, pools, launches, or migrations.
