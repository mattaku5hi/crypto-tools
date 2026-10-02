# 2026-10-02 — `buyer-intersect` live smoke (Solana, pump.fun bonding curve)

First end-to-end run of the real Solana path (P0.16): HeliusProvider paginated mint scan →
pump.fun decoder (all six IDL trade variants, ADR-009) → ADR-003 qualification → CLI.

## Setup

- Commit: `361e552` (`feat/p0-p1-foundation`), debug build, run at `2026-10-02T13:36:56Z`.
- Host: Intel i7-12700K, 16 threads. Provider: Helius mainnet, free tier, key from
  `SCOUT_HELIUS_API_KEY` (never printed; CLI output is key-redacted).
- Input: `solana:AB48pUATr4vEsxdAp54X9pvqyae2whMR2B52rEuJpump`,
  `solana:67266Ha2icrdCKHyrYKyG4oJyJ7RqheaGbuGd6vwXbLD` — the two mints already present in the
  committed `pump_bonding_curve_buy_probe.json` fixture. `--min-token-hits 1` (K=1, so the
  run reports every confident bonding-curve buyer of either token, not an intersection).
- Page budget: default 10 pages × 100 txs, `sortOrder: asc` (earliest history first).
- Calls: at most 10 + 2 `getTransactionsForAddress` requests. Credit cost per call is not
  measured here (P0.6/P4.7).

## Result: exit 3 (Partial), 121 confident buyers

| token | status | txs scanned | failed txs | decoded buys (verified variants) | qualified buyers | malformed | unknown disc. | IdlOnly would-qualify | positive delta, no instruction |
|---|---|---:|---:|---:|---:|---:|---:|---|---:|
| `AB48…pump` | truncated (budget) | 1000 | 650 | 176 | 115 | 56 | 0 | exact_sol_in 2, v2 2, quote_in_v2 10 | 159 |
| `67266H…bLD` | ok (full history) | 145 | 19 | 6 | 6 | 25 | 0 | exact_sol_in 5, v2 2, quote_in_v2 14 | 100 |

Totals: decoded buys 182, decoded sells 179, known non-trade instructions 504, unknown
discriminators 0. Incomplete reasons as printed: AB48 truncated; 81 malformed instructions;
35 IdlOnly buys (7 `buy_exact_sol_in`, 4 `buy_v2`, 24 `buy_exact_quote_in_v2`).

Wall time was not instrumented. Only one run; nothing here is a performance measurement.

## Findings

1. **The pipeline works on live data** and the exit-3 reasons are the intended honest ones
   (budget truncation, IdlOnly variants, malformed shapes). Unknown discriminators: zero, so the
   pinned IDL's instruction table covered everything seen in 1145 transactions.
2. **`buy_exact_sol_in` arg length differs from the IDL in successful transactions.** Malformed
   samples report 24 and 26 data bytes where the IDL implies 25 (`u64, u64, OptionBool`). Failed
   transactions are no longer decoded, so these are successful executions: the program accepts
   a missing `track_volume` byte (24) and apparently a 2-byte encoding (26). This is IDL-vs-live
   drift, the same class as the +2 trailing accounts. Follow-up: per-variant/length breakdown and
   golden fixtures (P0.17), then an explicit, test-backed arg-length policy — not a silent relax.
3. **High failure rate on a hot bonding curve:** 650 / 1000 early AB48 transactions failed
   (slippage/bot contention). Without the execution-status gate these would have been decoded as
   trades; balance deltas would have kept them out of the buyer set, but diagnostics would have
   been inflated.
4. **First P0.7 overlap signal.** `docs/p0/measurements/2026-10-02-gmgn.md` recorded GMGN's top-3
   traders by profit for AB48 and zero overlap with our then single-transaction sample. With the
   paginated scan, **all three** (`8rx3h89w…`, `TvFQYF1T…`, `cS4gyYGP…`) are in our 115 confident
   bonding-curve buyers of AB48, alongside the fixture's own buyer `HgwBZM6k…`. This supports
   the earlier explanation (sample scope, not data disagreement). It is three wallets on one
   token, not the P0.7 experiment.
5. `positive_delta_without_instruction` is large (259). These are owners whose token balance
   increased without a recognized buy instruction for them: transfers, LP/vault accounts,
   post-migration venues, IdlOnly variants that were not attributed. Not buys by design (ADR-003).
   It is a diagnostic, not evidence of missed buyers.

## What this does NOT establish

- Completeness of AB48's buyer set (truncated at 1000 txs; post-migration venues out of scope).
- Credit cost, latency, or rate-limit behaviour of the scan.
- Anything about profitability of the listed buyers — `buyer-intersect` lists who bought, not
  how well.
