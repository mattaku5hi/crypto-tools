# 2026-10-06 — dev tracker (B0): measured event volumes and daily cost

Window: the last 24 h before the measurement (≈ 2026-10-05/06 UTC). Counts are successful events; one
request each unless noted. EVM via keyed Alchemy on Pay As You Go (unlimited `eth_getLogs` range),
Solana via the keyless public RPC (`getSignaturesForAddress`).

## Volumes per day

| Chain | Launches / day | Migrations (graduations) / day | Source of the count | Request time |
|---|---|---|---|---|
| Solana (pump.fun) | **53,312** (txs touching the `mint_authority` PDA `TSLvdd1p…`, seeds `mint-authority`, used by `create`/`create_v2` only) | **≈ 1,800** (successful txs signed by `39azUYFW…`; sample 24/24 successful = `Migrate`) | 57 + 3 signature pages | 129 s + 7 s |
| BSC (four.meme TokenManager V1+V2) | **9,087** `TokenCreate` | `LiquidityAdded` 3, `TradeStop` 3 — implausibly low: newer four.meme graduations go through another contract (to identify) | 1 `eth_getLogs` over 192,000 blocks | 3.3 s |
| Robinhood (Pons V2 factory) | **4,155** `TokenLaunched` | **35** `PoolGraduated` (0.8 %) | 1 `eth_getLogs` over 847,973 blocks (9.81 blocks/s) | 2.5 s / 0.7 s |
| Base — Zora factory `0x7777…baF3` | ≈ **611** factory events (top topic0 `0x2de43610…` 562, `0x74b670d6…` 32, `0xfb9e81c3…` 17: coin-creation variants, to pin) | no migration (no curve) | 1 `eth_getLogs` over 43,200 blocks | 2.2 s |
| Base — Clanker v4 `0xE85A…83a9` | ≈ **293** (`0xe80ed94c…`; `0x9299d1d1…` 157 to classify) | no migration | 1 `eth_getLogs` | 2.9 s |

Not yet counted: Bags (Robinhood), four.meme graduation contract, other Base launchpads.

## Prices used

- Alchemy PAYG $0.525 / 1M CU; `eth_getLogs` 60 CU; WebSocket subscriptions and webhooks billed by
  bandwidth, 0.04 CU per byte (≈ 40 CU per ~1 KB event).
- Helius free: 1M credits/month; `getTransactionsForAddress` full = 10 credits per 100 tx, signatures-only
  10 flat; WebSockets 20 credits per MB streamed + 1 credit per connection open; 5 connections on Free.

## Steady state per day (after the backfill)

Design: ingest **every** launch and migration event globally (the creator is in the launch event), so a
dev's launch/migration counts come from our own database; no per-dev polling.

| Chain | Ingestion option | Requests / day | Units / day | Per month |
|---|---|---|---|---|
| Solana | `getTransactionsForAddress` full on `TSLvdd1p…` and `39azUYFW…` every 15 min | ~ 600 | ~ 5.5k credits | ~ 170k credits (17 % of Helius Free) |
| Solana | `logsSubscribe` (mentions the same two accounts), ~5 KB of logs per create | 2 connections | ~ 5.5k credits (≈ 280 MB) | ~ 170k credits; public RPC WS as free fallback |
| EVM, 3 chains (≈ 6 filters incl. four.meme graduation, Bags) | `eth_getLogs` poll every 15 min | ~ 580 | ~ 35k CU | ~ 1M CU ≈ **$0.55** |
| EVM, 3 chains | same, poll every 5 min | ~ 1,730 | ~ 104k CU | ~ 3.1M CU ≈ **$1.6** |
| EVM, 3 chains | WebSocket `eth_subscribe` logs (≈ 14.4k events × ~1 KB) | — | ~ 580k CU | ~ 17M CU ≈ **$9** |
| ATH | free market-data API (DexScreener / GeckoTerminal; limits to confirm) for migrated tokens, batched | ~ 10k | 0 | $0 |

Conclusions:
- For EVM, **polling `eth_getLogs` every 1–5 min on PAYG is cheaper than WebSocket subscriptions** at
  these volumes (bandwidth billing makes a 1 KB event cost ~ as much as a whole 60-CU poll). Latency of
  1–5 min is enough for a 30–60 min category cadence. WebSockets pay off only for rare events.
- For Solana, polling and `logsSubscribe` cost about the same on Helius (~ 17 % of the free plan);
  WebSockets give lower latency; the public RPC WebSocket is a free fallback (no SLA; gap fill by polling).
- The steady state is ≈ $1–2/month on Alchemy PAYG plus ~ 17 % of Helius Free.

## One-time backfill (1 year)

- Solana: ≈ 19M creates + ≈ 650k migrations. Via Helius `getTransactionsForAddress` full on the two
  accounts: ≈ 1.9M + 65k credits (≈ 2 months of the free plan), ≈ 190k calls (≈ 5 h at 10 req/s). A
  shorter window (90 days ≈ 480k credits) or an analytics warehouse (Dune / Bitquery, terms to check) are
  the alternatives.
- EVM: one year of factory events is a few thousand `eth_getLogs` (split for the 150 MB response cap) —
  well under 1M CU (< $1) on PAYG.
