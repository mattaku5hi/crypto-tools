# ADR-011: Analysis window and left-censoring for the Solana wallet ledger

Status: Accepted
Date: 2026-10-02
Amends: ADR-010 §6 (inventory continuity) for windowed runs; ADR-005 (coverage) for wallet scans.

## Context

`wallet-stats`/`wallet-rank` (Solana slice) currently treat a wallet as coverage-complete only when
its **entire** address history was read. The live run of 2026-10-02
(`docs/p0/measurements/2026-10-02-wallet-rank-live.md`) shows this is unreachable for active
wallets at any sane budget: 150 of 150 P0.7 wallets still had history left after their newest 300
transactions, so every wallet was excluded as `incomplete_coverage` and no ranking was possible.

ARCHITECTURE §10 already defines the intended cohort: "fully observed episodes opened and closed
inside the reporting window; left-censored and still-open counts are shown separately". CLI.md
§2/§4/§5 define `--period`/`--since`/`--until`. What is missing is a precise rule for (a) when a
windowed wallet scan is complete, and (b) how inventory that predates the window is booked.

## Decision

1. **Window.** A run has an explicit half-open UTC window `[since, until)` in unix seconds, from
   `--since/--until` or `--period <N>d` (= `[as_of - N·86400, as_of)`, `as_of` = run start, pinned
   once per run and printed in `run_meta`). No window = today's behaviour (full history required).
   `until > as_of` (a window reaching into the future) is a usage error (exit 2).
2. **Windowed scan completeness.** The scan reads the wallet history newest-first and stops after
   the first page that contains a transaction with `blockTime < since` (or when history ends).
   Coverage is **complete for the window** iff that boundary was reached; running out of page or
   request budget before it is `incomplete` (exit 3), exactly as today. Transactions with
   `blockTime` outside `[since, until)` are dropped before the ledger. A transaction without
   `blockTime` inside the scanned range is a coverage gap (`incomplete`), never silently placed.
   `until < as_of` still requires reading from the newest end down to `until` (no provider time
   filter is assumed until one is live-verified); that cost is reported, not hidden.
3. **Left-censoring (amends ADR-010 §6 for windowed runs).** Inside a window, a disposal of a mint
   for which the window holds insufficient observed inventory is not an unexplained movement: the
   inventory predates the window. The shortfall is booked as a lot with
   `BasisStatus::Unknown { LeftCensored }` (distinct from `InventoryNotObserved` and from
   unexplained inbound transfers), and the episode it belongs to is **left-censored**. Same for
   positions the wallet already holds at `since`: they are not visible and are not invented.
4. **Episode classes.** `ClosedKnown` / `ClosedUnknown` (unknown basis or proceeds from causes inside
   the window: unpaired trades, unexplained transfers, unsupported quote, router forwards) /
   `LeftCensored` (any consumed lot is `LeftCensored`) / `Open`. Win rate, profit factor, realized
   PnL sums, holding times and `realized_cost_roi` use `ClosedKnown` only (unchanged). Left-censored
   episodes are counted and listed, never valued as 0 and never folded into the known sums.
   Precedence: an episode with any in-window cause is `ClosedUnknown` even if it also consumed
   left-censored inventory; `LeftCensored` counts only purely left-censored episodes. The
   censored raw amount is reported either way (`left_censored_amount_raw`).
5. **Quality gates.** "Material unknown basis" (ARCHITECTURE §10, `exclude_unknown_basis`) means
   `ClosedUnknown` episodes or unknown-basis inventory from in-window causes. Left-censoring alone
   does not exclude a wallet; it is reported (`left_censored_episodes`, raw amounts). Activity
   gates (P4.3) are evaluated over the window: trades / active UTC days and distinct mint-days /
   active UTC days, exact integer comparison.
6. **Activity evidence on incomplete scans.** Without a complete window, the observed days other
   than the oldest observed UTC day are fully observed, so a per-day count on those days is exact.
   If the activity ceiling is exceeded on fully observed days alone, the wallet records the ceiling
   reason in addition to `incomplete_coverage` (evidence, not a complete-window metric). It never
   makes an incomplete wallet eligible.
7. **Reproducibility.** `run_meta` carries `since`, `until`, `as_of`, the window source
   (`period`/`explicit`/`none`) and the ledger version; the ledger version is bumped.

## Consequences

- `wallet-rank --period 30d` becomes satisfiable for discretionary wallets with a page budget
  proportional to their 30-day activity; HF bots still exhaust budgets and stay `incomplete`, but
  now also carry activity-ceiling evidence.
- Realized PnL in a window excludes the PnL of positions opened before it — by design and named so.
- A provider-side time filter (Helius `getTransactionsForAddress` filters) can later replace the
  newest-first walk for `until < as_of` once live-verified; the completeness rule stays the same.
