# 2026-10-06 — live end-to-end pipeline on Robinhood (buyer-intersect → wallet-rank / wallet-stats)

Keyed Alchemy free-tier RPC (`SCOUT_ROBINHOOD_RPC_URL`, default limiter 250 CU/s), public Robinhood RPC
for `eth_getLogs` (auto-routed), Blockscout for wallet listings. Keys absent from all output (grepped).
Fixes made during the run: `bd83c4d` (P3.15), `86f93e5` (P3.16), `3080afe` (P3.17, ADR-020 amendment 9);
CI green on all three.

## 1. The 12-hour stall (P3.15)

First attempt (2026-10-05, `buyer-intersect`, 4 Pons tokens, 3 h, explicit `--rpc-cu-per-sec 250`): a
burst of 429s without `Retry-After` halved the limiter 250 → 7.8 CU/s (public RPC 250 → 125 /s), and the
limiter never grew back. The process ran ~12 h at 0 % CPU with no output after the last halving: one
`eth_getBlockReceipts` (500 CU) waited > 1 min. All Tokio workers were parked, one idle TLS socket — not
a deadlock, a crawl. The Alchemy quota itself was fine (`eth_blockNumber` 200 on all three chains).

Fix: additive recovery (+1/8 of the initial rate per 10 s without 429, capped at the initial rate; any 429
restarts the quiet period; one sleep capped at a step), `recoveries` in the end-of-run line, a restore
notice, and a URL-free `progress:` stderr line every 60 s in EVM runs.

## 2. buyer-intersect: block receipts were the bottleneck (P3.16)

Window `[2026-10-06T04:23:29Z, 07:21:29Z)` (blocks 81,347,514–81,450,959), 4 Pons tokens
(`0x268d81…`, `0xa7f4cf…`, `0x6effbf…`, `0x68f1b5…`), `--concurrency 1`:

| build | wall | waiting for limiter tokens | requests | 429 |
|---|---|---|---|---|
| `bd83c4d` (block receipts for every tx) | 497 s | 473 s | 824 | 0 |
| `86f93e5` (per-tx receipts for isolated txs) | 58 s | 17 s | 830 | 0 |

Per-token results identical: `0x268d81…` and `0x68f1b5…` no transfers in the window; `0xa7f4cf…` 15
transfer logs, 7 txs, 5 wallets; `0x6effbf…` 650 logs, 226 txs, 54 wallets, 33 swap-shaped logs at 9
refused emitters (exit 3, IncompleteCoverage). `K = 2`: 0 matches (only two tokens traded in the window);
`K = 1`: 59 wallets.

## 3. wallet-rank / wallet-stats over 12 of those wallets (1 day)

Selection: the 5 wallets of `0xa7f4cf…` plus the 7 most active (buy and sell) on `0x6effbf…`.
Window `[2026-10-05T07:40:30Z, 2026-10-06T07:40:30Z)`.

- `wallet-rank` (default `--profile quality`): 12 excluded — `min_closed_episodes = 20` /
  `min_active_days = 7` cannot hold in a 1-day window. Expected; use `--profile none` or a longer period.
- `--profile none`, before P3.17: 5 ranked, 7 excluded (5 `metric_unknown`, 2 `incomplete_coverage`).
  Cost ~735 requests, ~246 s; `eth_getBlockReceipts` = 90 (sell-side balance-diff checks) dominate the
  ~212 s waiting for tokens.
- Root cause of the Unknowns: Blockscout `txlistinternal` answers "not yet processed" for Robinhood
  (`internal_transfers_complete=false`), so sell proceeds come only from the archive balance diff, and
  bots send `approve` + sell in ONE block — the sole-touch rule refused them. Wallet `0xdd7a3c…`: 19/19
  sells `wallet_not_alone`, exactly the 19 blocks with two own transactions (approve to the token, then
  the sell via router `0x65050a9b…`).
- After P3.17 (ADR-020 amendment 9): **9 ranked**, 3 excluded; native legs by source
  `balance_diff 43, balance_diff_approve_companion 31, logs_and_value_only 98`; 720 requests, 230 s.

| rank | wallet | realized net PnL (ETH) | closed known | open | W/L |
|---|---|---|---|---|---|
| 1 | `0x3c9f87…` | +0.338017 | 5 | 0 | 4/1 |
| 2 | `0xe42242…` | +0.148122 | 1 | 2 | 1/0 |
| 3 | `0x351afe…` | +0.005784 | 3 | 0 | 2/1 |
| 4 | `0x0fbae5…` | +0.004477 | 1 | 1 | 1/0 |
| 5 | `0xdd7a3c…` | +0.002882 | 19 | 2 | 9/10 |
| 6 | `0x489674…` | +0.000496 | 3 | 6 | 2/1 |
| 7 | `0xc8d6ea…` | −0.000019 | 1 | 0 | 0/1 |
| 8 | `0xe4b7ab…` | −0.000616 | 1 | 0 | 0/1 |
| 9 | `0x1dd19d…` | −0.002605 | 10 | 1 | 2/8 |

Excluded: `0xfaf8fa…`, `0x86e038…` (`incomplete_coverage`: swap logs at unverified emitters),
`0xef0fef…` (`metric_unknown`, no closed episode).

## 4. Independent check of the companion balance diff

Archive balances fetched directly (`after − before + fees of both own txs`) against venue events in the
sell transactions:

- `0xdd7a3c…`, 9 Pons curve sells: proceeds = ETH released by the router × 0.99 (1 % router fee)
  exactly in 9/9 — 6 direct (`CurveSell.quoteOut × 0.99`), 3 where the curve's quote is the intermediate
  token `0x117cc2…`, swapped to WETH on v3 pool `0xddcbba…` and unwrapped (`WETH out × 0.99`).
- `0x3c9f87…` (rank 1), 8 sells: proceeds = WETH out × 0.99 exactly in 8/8 (1 curve, 7 Uniswap v4; the v4
  `amount0` is ~3 % above the WETH out — pool/hook fee before the router). Sells of 0.07–0.25 ETH make the
  +0.338 ETH plausible.

## 5. Open items

- `0x6effbf…`: 33 swap logs at 9 refused emitters (among them v3 pool `0xddcbba…`, the `0x117cc2…`/WETH
  pool the Pons router uses) — identify the factory/venue and decide admission.
- Blockscout Robinhood internals "not yet processed": re-check later; with them, sells would not need
  the 500-CU block receipts.
- Hooked v4 pools (Pons hook fee) are not modelled separately; the ledger uses the wallet's own deltas,
  so PnL is unaffected, only venue-level evidence.
