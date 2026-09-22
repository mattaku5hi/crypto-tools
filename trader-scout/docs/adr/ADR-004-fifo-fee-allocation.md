# ADR-004: FIFO cost basis & fee allocation

Status: Accepted
Date: 2026-09-22

## Context

ARCHITECTURE.md §8 mandates FIFO cost basis with full lot preservation, exact fee allocation without
double-counting, and explicit handling of unknown-basis inventory from external transfers. ACCEPTANCE
C01-C14 give concrete numeric test cases the implementation must reproduce exactly.

## Decision

### Lot model

```rust
pub struct Lot {
    pub asset: AssetKey,
    pub acquired_at: CanonicalLocation,
    pub remaining_amount: RawAmount,
    pub remaining_basis: Money,       // capitalized acquisition cost, shrinks as consumed
    pub basis_status: BasisStatus,    // Known | Unknown { reason }
}
```

Lots are consumed strictly FIFO by acquisition order (`CanonicalLocation`, never wall-clock fetch
order — this is the same ordering contract as ADR-002's `CanonicalLocation`, reused here so ledger
replay is deterministic regardless of RPC completion order, satisfying E05/F01).

### Realized PnL formula (fixed, from ARCHITECTURE.md §8)

```text
realized_trade_pnl   = net_sale_proceeds - consumed_acquisition_basis
realized_net_pnl     = Σ realized_trade_pnl(in window) - attributable_expensed_trading_overhead(in window)
```

Where `net_sale_proceeds = actual_proceeds - allocated_sale_fees` and `consumed_acquisition_basis`
includes allocated acquisition fees capitalized into the lot at buy time. Fee allocation is
partial-consumption-proportional: selling 40% of a lot consumes 40% of that lot's remaining
capitalized fee-inclusive basis (C02's worked example: 100 units bought for 1000+fee10, sell 40 for
600-fee6 → consumed_basis=404, not 400 or 410 — verified as a golden test, not just described).

**A fee is allocated exactly once.** Acquisition fee is capitalized into the lot (increases basis);
sale fee reduces proceeds. Neither is ever subtracted a second time from `realized_net_pnl`
(C01/C08). `Overhead` in the second formula is *only* costs not already captured by basis/proceeds
(e.g. a confidently-attributed failed trade attempt's gas) — arbitrary wallet expenses are never
auto-classified as trading overhead.

### Unknown basis (transfers)

An incoming external transfer creates inventory with `BasisStatus::Unknown { reason }` unless a
provable lot lineage exists (explicit same-owner self-transfer group). Selling such inventory produces
known proceeds but the trade's PnL is `Statused::Unknown` and reported as unknown-subset — it is never
computed as if basis were zero (AGENTS.md invariant #6, C05). A wallet with material unresolved
PnL-critical unknown-basis inventory is excluded from strict `wallet-rank` eligibility (ARCHITECTURE.md
§10) but still appears fully in `wallet-stats`.

### Non-quote-asset swaps (C07)

Swapping token A for non-quote token B is modeled as a disposal of A (at a single consistent
consideration valuation) plus an acquisition of B — never as an "investment" event for whichever
intermediate route hops occurred. The combined fee for the swap is allocated across the disposal and
acquisition legs such that the sum of allocations equals the actual fee paid (no double count, no
under-count).

### PF and win-rate statuses (D06)

Profit factor and related ratios are `Statused<Money>` — an enum, not a raw float with sentinel
values:

```rust
pub enum RatioStatus<T> { Value(T), NoObservedLosses, Undefined }
```

Serialized as `{"value": ..|null, "status": "..."}`. No `Infinity`, `NaN`, or magic-number (e.g. `999`)
surrogate is ever produced (D06, AGENTS.md invariant #7's spirit extended to statistical output).

## Consequences

- `scout-ledger` owns FIFO consumption, fee allocation, and lot lineage; `scout-analytics` consumes its
  output and never re-derives cost basis independently — this keeps `wallet-rank` and `wallet-stats`
  guaranteed consistent (ARCHITECTURE.md §10, ACCEPTANCE D08).
- Every numeric example in ACCEPTANCE §C becomes a golden unit test in `scout-ledger` before the FIFO
  engine is considered done (AGENTS.md invariant #20: "заглушка не является поддержкой").
- Property tests required at ledger completion: inventory conservation
  (`opening + acquisitions - disposals = closing` for known flows), fee-allocation conservation
  (`Σ allocations + unallocated = actual fee`), idempotency (`apply(E); apply(E) == apply(E)`), and
  replay determinism (`canonical(snapshot+suffix) == canonical(full_history)`).

## Alternatives considered

- LIFO or average-cost basis: rejected for v1 — ARCHITECTURE.md fixes FIFO as the v1 policy; any future
  change requires its own ADR plus golden-fixture updates per AGENTS.md's "порядок разработки" clause.
- Representing PF as `f64::INFINITY` when there are no losses: rejected outright by D06 and by the
  workspace's `float_arithmetic` lint denial on ledger/analytics code paths.
