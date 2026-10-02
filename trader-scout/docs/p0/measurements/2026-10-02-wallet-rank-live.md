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
