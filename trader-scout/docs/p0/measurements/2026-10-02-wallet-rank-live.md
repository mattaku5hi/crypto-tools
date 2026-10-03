# 2026-10-02 — first live `wallet-rank` run (Solana pump.fun slice)

Commit: `84dbdec` (release build). Provider: Helius `getTransactionsForAddress`, newest-first,
`--max-pages-per-wallet 3` (≤300 tx per wallet), `--profile insider`, no time window (not yet
implemented). Inputs: the two P0.7 K≥2 sets (`p0.7-2026-10-02/gmgn_k2_wallets.txt`,
`runB_ours_k2_wallets.txt`), prefixed `solana:`. API key never appeared in stdout/stderr (grep).

| Set | Wallets | Requests | Exit | Eligible | Excluded (primary) |
|---|---|---|---|---|---|
| GMGN K≥2 | 39 | 117 | 3 | 0 | `incomplete_coverage` 39 |
| Ours K≥2 | 111 | 333 | 3 | 0 | `incomplete_coverage` 109, `unknown_basis` 2 |

Observed in each wallet's newest 300 transactions (medians, min–max):

| Set | pump.fun bonding-curve trades | distinct mints | active UTC days | closed known episodes |
|---|---|---|---|---|
| GMGN K≥2 | 10 (0–83; 12 wallets with 0) | — | 1 (0–10) | 1 (0–25) |
| Ours K≥2 | 222 (19–298) | 77 (9–149) | 1 (1–9) | 102 (0–149) |

## Findings

1. **Full-history completeness is unreachable** for active wallets: 150/150 still had history after
   300 transactions. Ranking needs an analysis window with left-censoring → ADR-011.
2. **The P0.7 HF hypothesis is confirmed on our own data**: our K≥2 set trades a median 77 distinct
   pump.fun mints within one UTC day; the `insider` ceiling (≤10 mints/active day) separates them
   by almost an order of magnitude. ADR-011 §6 lets that evidence be recorded even on incomplete
   scans.
3. **GMGN leaderboard wallets mostly trade outside the bonding curve** (median 10 of 300 tx are
   bonding-curve trades, 12 wallets none): their PnL lives on PumpSwap AMM / other venues, which
   are not decoded (P4.1). Bonding-curve-only PnL is a small, biased subset for that population.

Raw JSONL/stderr were kept outside the repo (scratch); the numbers above are the full extract.

## Re-run with the ADR-011 window (commit `35f1fac`)

`wallet-rank --profile insider --period 3d --max-pages-per-wallet 5 --max-requests 250` over the
GMGN K≥2 set: 163 requests, exit 3, 0 eligible. Window coverage is now reachable: **15/39 wallets
complete for the window** (was 0/39 without a window). Primary exclusions: `incomplete_coverage`
24, `unknown_basis` 11, `no_pump_activity` 3, `metric_unknown` 1; any-reason also
`insufficient_closed_episodes` 11, `insufficient_active_days` 8, activity ceilings 3 (evidence on
incomplete scans, ADR-011 §6).

The 11 `unknown_basis` wallets (`wallet-stats --period 3d --detail full`, 2,646 tx in window):
`continuity_breaks` 112, `unknown_disposals` 105 vs `known_disposals` 28,
`out_of_scope_token_movements` 3,231. I.e. tokens bought on the bonding curve leave the wallet
through an undecoded venue (PumpSwap after migration), and most token movement is on mints never
traded on the bonding curve. The PumpSwap decoder (P4.1) is the binding constraint for this
population, not the window or the gates.

## Re-run with PumpSwap in the ledger (ADR-012, commit `5a72a68`)

Same command and set: 163 requests, exit 3, 0 eligible. Primary: `incomplete_coverage` 25,
`unknown_basis` 14; any-reason activity ceilings now 13 (was 3 — AMM trades make the activity
visible). The 14 `unknown_basis` wallets (2,994 tx in window, 1,073 decoded trades: curve 123,
PumpSwap 950): priced 501, `quote_funded_elsewhere` 286, `router_forward_trades_not_attributed`
214, `unreconciled` 158, `unsupported_quote` 128; `continuity_breaks` 356.

The PumpSwap decoder itself is not the problem (direct trades reconcile exactly); the blocker for
this population is **attribution through router/bot programs** — the PumpSwap `user` is a router
PDA, the wallet signs, pays and receives, often in multi-venue transactions with Meteora DLMM /
Jupiter and the unidentified `DF1ow4ts…` (P0.9). Pages of the two most affected wallets are
committed as fixtures (`router_wallet_{9oC3,tAwv}_page_2026-10-02.json`) for P0.18.

## Re-run with route swaps and quote units (ADR-013, commit `355bf6a`, 2026-10-03)

Same set/command, `--quote sol` and `--quote usdc`: 163 requests each, exit 3, 0 eligible; primary
`incomplete_coverage` 24, `unknown_basis` 14 (both units). In the 14 `unknown_basis` wallets
(3,192 tx in window) **priced trades rose from 501 to 1,502 of 1,528** (route swaps 970: 502 buys /
468 sells; PumpSwap direct 460; curve 98). Episodes: `closed_known` 300, `closed_unknown` 216,
`left_censored` 16, open 49. Remaining unknown causes: unexplained outbound 189 / inbound 66 token
movements (no FixtureVerified swap leg in the tx — swaps only on undecoded venues such as Meteora
DLMM / Raydium / Whirlpool, or transfers between own wallets), consumed unknown-basis lots 69,
unsupported quote 12, consideration unverified 8, cross-quote-unit 3.

Next levers, in order of measured impact: (1) verified swap evidence for aggregator routes
(Jupiter v6 route event) and the major undecoded venues (DLMM, Raydium CLMM/CPMM, Whirlpool), so
the ADR-013 route rule can book them; (2) an explicit, reported materiality policy for unknown
episodes in quality gates (today any `ClosedUnknown` excludes a wallet).

## Re-run with Jupiter + DFlow leg evidence and verified provider defaults (commit `6b4e870`, 2026-10-03)

`wallet-rank --profile insider --quote sol|usdc --period 3d --max-pages-per-wallet 5
--max-requests 250` (page 500, `tokenAccounts=balanceChanged`, server-side window): 157 requests,
exit 3, 0 eligible. Window-complete wallets 19/39 (was 15). Primary exclusions (sol):
`incomplete_coverage` 20, `unknown_basis` 17, activity ceiling 1, insufficient days 1; any-reason
activity ceilings **26/39** (trades/day 26, mints/day 24) — most GMGN leaderboard wallets are
themselves high-frequency (e.g. 257, 264, 137, 124 trades per active day).

Complete wallets now carry mostly known episodes, e.g. known/unknown closed: 48/2, 49/3, 52/1,
28/2, 28/4, 10/1, 57/5, 112/16, 97/19 (outliers 33/64, 3/6). The binding constraint for eligibility
is now the strict rule "any `ClosedUnknown` episode excludes the wallet" (P4.10), not decoding.
