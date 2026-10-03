# 2026-10-03 — live `buyer-intersect --side any` (ADR-014) on the P0.7 runner set

Inputs: the 8 P0.7 runner tokens (`p0.7-2026-10-02/runner_set.txt`). Venues: pump.fun bonding
curve, PumpSwap AMM, ADR-013 route swaps. API key absent from all output (grep).

| Run | Commit | Window | Pages/token | Requests | Exit | Matches K≥2 | malformed / orphan |
|---|---|---|---|---|---|---|---|
| A | `5acced8` | `--period 1d` | 20 | ≤200 | 3 | 91 | 23 / 23 |
| B | `867e1e7` | `--period 1d` | 80 | 504 | 3 | 137 | 0 / 0 |

Run A exposed a PumpSwap client encoding (`track_volume` as 2 bytes `01 01`, 26-byte data) that
the decoder rejected; fixed with live evidence (ADR-009 amendment, 7/7 successful trades exact on
the base leg).

Run B, hit distribution: K=2 103, 3 22, 4 4, 5 4, 6 1, 7 2, 8 1 (one wallet traded all eight).
Per matched (wallet, token): buy+sell 163, sell only 121, buy only 52. Per-token wallets with a
qualifying operation: 48 / 230 / 119 / 318 / 551 / 320 / 480 / 477.

Coverage: 6 of 8 tokens still had window left after 8,000 transactions (newest-first) — one UTC
day of these tokens exceeds 8,000 transactions, most of them bot noise (run A counted 1,634
failed transactions in 16,000 scanned). Cost lever to evaluate next (P0.6/P4.7): provider-side
filters on `getTransactionsForAddress` (e.g. successful-only) once live-verified, so pages carry
trades instead of failed bot attempts.
