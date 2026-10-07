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

### BSC addendum (2026-10-07): Flap is the dominant BSC launchpad

- Pancake v2 `PairCreated`: **17,587 / 24 h**; in a random sample of 120 pairs, 112 contain a token whose
  address ends in `…7777`, and the most frequent transaction target is `0xe2cE6ab8…9De0` (34/120) =
  **Flap Portal** (docs.flap.sh deployed addresses; VaultPortal `0x90497450…4C06` is the second, 9/120).
  `…7777` is Flap's vanity suffix for tax tokens (`…8888` standard). **All four owner BSC tokens are Flap
  tax tokens** (no four.meme `TokenCreate`; their first Pancake pair is created in Flap transactions).
- Flap Portal events over 24 h (1.45M logs, 6,000-block windows), topic0 verified by keccak of the
  published signatures: `TokenCreated(uint256,address,uint256,address,string,string,string)` `0x504e7f36`
  **15,883**; `LaunchedToDEX(address,address,uint256,uint256)` `0x6e4f4763` **74** (0.47 %);
  `TokenBought(uint256,address,address,uint256,uint256,uint256,uint256)` `0xa800a203` 205,740;
  `TokenSold(…7 fields…)` `0x03a4693e` 134,773. VaultPortal: 1,587 logs.
- four.meme graduation is still unresolved (managers' `LiquidityAdded`/`TradeStop` 3/day vs 9,087 launches).
- Consequence for wallet discovery: Flap bonding-curve trades (`TokenBought`/`TokenSold` at the Portal) are
  not a verified venue yet, so `buyer-intersect` saw only the Pancake-pool phase of the owner's BSC tokens.

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


### Addendum B0.2 (2026-10-07): creation events pinned, more launchpads counted (24 h)

All topic0 values below equal the keccak of the signature.

| Chain | Launchpad (emitter) | Creation event | /24 h | Graduation | /24 h | Dev field |
|---|---|---|---|---|---|---|
| Base | Zora factory `0x7777…baF3` | `CoinCreatedV4(address indexed caller, address indexed payoutRecipient, address indexed platformReferrer, address currency, string uri, string name, string symbol, address coin, PoolKey, bytes32 poolKeyHash, string version)` `0x2de43610` | 562 | — (no curve) | — | `payoutRecipient` (creator rewards) / `caller` |
| Base | Zora factory | `CreatorCoinCreated(…same…)` `0x74b670d6` | 32 | — | — | same |
| Base | Zora factory | `TrendCoinCreated(address indexed caller, string symbol, address coin, PoolKey, bytes32, bytes poolConfig, string version)` `0xfb9e81c3` | 17 | — | — | `caller` |
| Base | Clanker v4 `0xE85A…83a9` | `TokenCreated(address msgSender, address indexed tokenAddress, address indexed tokenAdmin, string×5, int24 startingTick, address poolHook, bytes32 poolId, address pairedToken, address locker, address mevModule, uint256 extensionsSupply, address[] extensions)` `0x9299d1d1` | 157 | — (no curve) | — | `tokenAdmin` |
| Robinhood | Flap Portal `0x26605f32…eb09` (Bitquery) | `TokenCreated` `0x504e7f36` (BSC layout) | 2,195 | `LaunchedToDEX` `0x6e4f4763` | 0 | `creator` |
| Robinhood | Doppler Airlock `0xeb7c0347…0862` (Bitquery) | `Create(address,address,address,address)` `0x68ff1cfc` | 1,404 | `Migrate(address,address)` `0x2a05bb71` | 0 | to resolve |
| Robinhood | Pons V2 factory | (see table above) | 4,155 | `PoolGraduated` | 35 | `deployer` |
| Robinhood | Bags factory `0xe8Cc…Cb37` | — | 0 logs | — | — | inactive |
| Robinhood | Clanker `0xd3f2…9a94`, Virtuals, Klik, Ape.store (Bitquery list) | — | ≤ 18 logs each | — | — | near-inactive |

Corrections: Clanker's frequent topic `0xe80ed94c` is `ExtensionTriggered(address,uint256,uint256)` (293/day), not a launch; Clanker v4 launches are 157/day. Zora ≈ 611 coins/day across three event kinds.

### Addendum B0.3 / B0.4 (2026-10-07): ATH source and backfill source

**ATH — Codex (key present).** `filterTokens(tokens: ["addr:networkId", … up to 200])` returns
`token.extrema { athPrice athFdv athCircMc + *Timestamp }` for all four chains (network ids: Solana
1399811149, BNB 56, Base 8453, Robinhood 4663); e.g. Bicat (BSC) athFdv $7.19M, JEANPHIL (Solana)
$10.99M. Plan: free "Almost Free" tier = 10,000 requests/month, 5 req/s (one-time $1 activation); the next
plan is a $350/month subscription (no PAYG) → stay within the free tier by batching 200 tokens per request
(≈ 2M token lookups/month). GeckoTerminal (keyless, ~10–30 calls/min, daily OHLCV only ~6 months back) and
DexScreener (current values only) are fallbacks, not ATH sources.

**Codex is not a source of truth for counts.** Same settled 24 h window: Codex lists 1,189 Pump.fun tokens
with `launchpadMigrated` (1,061 creators; `PumpMayhem` 0) vs **1,694** successful migrate transactions signed
by `39azUYFW…` on chain (≈ 70 % coverage). Codex also has no Flap and no Pons protocol (its launchpad list:
ArenaTrade, Baseapp, BaseappCreator, BonadFun, BoopFun, Clanker, ClankerV4, Doppler, EgoTech, Flaunch,
FourMeme, HeavenAMM, Kumbaya, Liquid, MeteoraDBC, Moonit, NadFun, Printr, Pump, PumpMayhem, Rainbow,
RaydiumLaunchpad, TokenMillEVM, TokenMillV2, Vertigo, Virtuals, ZoraCreatorV4, ZoraV4). Codex does expose
`creatorAddress`, `launchpad { completed migrated completedAt migratedAt }` and a `creatorAddress` filter
(a hint for a dev's token list, to be cross-checked).

**Backfill design (B0.4).** Launch and migration FACTS come from chain data (launchpad events on EVM —
creator inside the launch event; Solana: migrate transactions of `39azUYFW…` and creates via the
`mint_authority` PDA). A dev can only qualify with ≥ 3 migrations (or ≥ 3 runners), so the full launch
history (the migration-rate denominator) is fetched only for creators with ≥ 3 migrations in the window,
from their own wallet history (Helius / Ankr), not for all 19M yearly pump.fun creates (≈ 1.9M Helius
credits). EVM backfill is a few thousand `eth_getLogs` on Alchemy PAYG (< $1). ATH from Codex in batches.