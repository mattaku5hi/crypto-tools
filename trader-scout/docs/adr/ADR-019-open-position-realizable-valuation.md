# ADR-019: Realizable valuation of open positions at as-of (pump curve and PumpSwap)

Status: Accepted (2026-10-03)
Date: 2026-10-03
Extends: ADR-018 (USD), ARCHITECTURE §8 ("show known remaining basis, amount, marked value, marked
unrealized PnL, price freshness and liquidity/sellability; spot ≠ realizable; missing quote = N/A;
paper profits of thin markets must not lift strict ranking").

## Decision

1. **What is valued.** Open positions (remaining raw token amount of a `(wallet, mint)` FIFO) at the
   run's `as_of`, only when the window ends at `as_of` (live runs). Historical windows
   (`until < as_of`) stay `unvalued` (no historical account state is read).
2. **Realizable, not spot.** Value = the quote amount the wallet would receive by selling its whole
   remaining amount now, computed with the venue's own swap math on live state:
   - pump.fun bonding curve (not `complete`): constant-product sell on the `BondingCurve` account's
     virtual reserves (account layout from the pinned IDL `e0687ae9`), minus protocol and creator fee
     bps;
   - PumpSwap pool: constant-product sell on the pool's base/quote vault balances (plus
     `virtual_quote_reserves` per the IDL), minus lp/protocol/creator fee bps.
   The swap math and the fee application are **verified against live paired events** before use:
   `TradeEvent` (curve) and `SellEvent` (PumpSwap) carry the pre-trade reserves and fee bps, so the
   formula must reproduce `sol_amount`/`quote_amount_out` and every fee exactly on the committed
   fixtures (ADR-009-style evidence). Fee bps at valuation time = those of the most recent observed
   event on the same curve/pool in the run; if none was observed → `unvalued { fee_unknown }`.
3. **Where the state comes from.** `getAccountInfo` / `getMultipleAccounts` at `as_of` (bounded,
   counted in the request budget) for the curve PDA or the pool (pool address = the last PumpSwap
   pool the wallet traded that mint on; a curve that is `complete` with no known pool →
   `unvalued { migrated_pool_unknown }`). Other venues → `unvalued { venue_not_supported }`.
4. **Labels and liquidity.** Each valued position carries `label = realizable_cp_quote`, the state
   slot, `price_impact_bps` (realizable vs. marginal price), the quote reserve, and USD via ADR-018
   at `as_of`. `unrealized_pnl = realizable_value − known remaining basis` (unknown if any remaining
   basis is unknown; never zero for missing data).
5. **Ranking.** Default ranking stays realized-only (ARCHITECTURE §10). The report shows open exposure
   valued/unvalued counts and totals; `--require-valued-open` excludes wallets with any unvalued
   open position (`open_exposure_unvalued`); unrealized gains never raise the realized rank keys.

## Consequences

- Open positions on pump.fun/PumpSwap get an honest, slippage-aware exit value; thin markets show a
  large price impact instead of a paper profit.
- Valuation depends on live state at run time (not reproducible later); `run_meta` records the slot
  and `as_of`.
