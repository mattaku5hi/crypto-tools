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

## Run C — live-verified provider defaults (commit `5b44c12`)

Same inputs and flags as run B (`--period 1d --max-pages-per-token 80 --max-requests 700`); new
defaults: page 500, `status=succeeded`, server-side `blockTime` window. 487 requests, 7 min wall
clock, exit 3. Transactions read: 6 × 40,000 + 727 + 1,396 ≈ 242k successful txs (≈24k credits by
the documented metering) vs 64k mixed txs in run B with about the same request count.

Matches K≥2: **609** (run B 137): K=2 435, 3 101, 4 35, 5 18, 6 10, 7 7, 8 3. Per-token wallets
with a qualifying operation: 484 / 873 / 629 / 2,022 / 1,109 / 308 / 1,905 / 467.

Coverage: the same 6 tokens still exceed **40,000 successful transactions in one UTC day** — the
window is not complete for them (named per token, exit 3). For such tokens a narrower window or a
larger page budget is the only honest option. 2 malformed PumpSwap trade instructions remain on
`HXxJdaQb…` (a further encoding variant; not yet captured).

## Run D — bounded concurrency + time slices (commit `a64bdd4`)

`--period 1d --max-pages-per-token 20 --slices 4 --concurrency 8 --max-requests 700` (same 80 pages
per token as run C, now spread over 4 equal sub-windows): **289 s wall clock** (run C: 424 s
sequential, i.e. ~1.5× faster), 417 requests, exit 3, no 429s. Matches K≥2: 671 (K=2 500, 3 109,
4 25, 5 24, 6 10, 7 1, 8 2). The speed-up is far below 8×: the run moves ~200k successful
transactions at ~16 KB each (≈3–4 GB of JSON), so it is bandwidth-bound, not request-latency-bound.
Slicing changes *which* part of an over-budget window is seen: every sub-window contributes its
newest transactions (a spread over the day) instead of only the newest hours; truncation is named
per slice. Further speed needs fewer bytes per transaction (no documented option to drop
`logMessages`/unused meta in `getTransactionsForAddress` full mode) or narrower windows.

Byte breakdown of a 1,000-tx full-mode page (busy pump token, 16.1 MB compact): `preTokenBalances`
28.9 %, `postTokenBalances` 28.9 %, `transaction` 18.1 %, `logMessages` 9.5 %, `loadedAddresses`
5.1 %, pre/post lamport balances 6.2 %, `innerInstructions` 2.9 %. The token balances (owner-keyed
deltas) are required by attribution, so request-side trimming could save ≲15–25 % at best
(e.g. binary `transaction` encoding); narrower windows remain the main cost/time lever.
