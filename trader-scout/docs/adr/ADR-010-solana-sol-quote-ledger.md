# ADR-010: SOL-quoted ledger for pump.fun bonding-curve trades

Status: Accepted
Date: 2026-10-02
Closes: TICKETS P0.14 (base-unit vs `Money` scale boundary) for the Solana pump.fun slice.

## Context

`wallet-stats`/`wallet-rank` need realized PnL. ADR-001 fixes `Money` as `i128` at
`MONEY_SCALE = 8`, USD by default, with `quote_asset_policy` allowing other quote assets. pump.fun
bonding-curve trades settle in SOL (lamports, 9 decimals) — or, for v2 curves, in the curve's
`quote_mint`. No USD price source exists in this workspace yet (P5.2), and invariant #6 forbids
inventing one. A SOL-denominated ledger needs an exact consideration per trade, a single fee
allocation (invariant #8), and an explicit rule for the native flows that are not part of the trade.

Evidence (2026-10-02, 20 successful trades from `pump_bonding_curve_buy_probe.json` and
`pump_variants_live_2026-10-02.json`): pump.fun's `TradeEvent` (Anchor event CPI, discriminator
`bddb7fd34ee661ee`) carries `sol_amount`, `fee`, `creator_fee`. For transactions with no other
native activity, `user_lamport_delta + (sol_amount + fee + creator_fee) + tx_fee = 0` exactly for
buys, and `user_lamport_delta - (sol_amount - fee - creator_fee) + tx_fee = 0` for sells (residual 0
in the clean cases; residuals elsewhere are explained by token-account rent deposits/refunds
(~2,039,280), 10,000-lamport tips, ~1% trading-bot platform fees, or external top-ups of the user
account).

## Decision

1. **Quote unit = lamports, exact.** For SOL-quoted trades the ledger currency is the lamport.
   `Money` values in this ledger hold `lamports × 10^MONEY_SCALE` (i.e. one lamport is one `Money`
   unit at scale), so no rounding ever occurs (i128 headroom: ~1.7e38 vs. u64::MAX × 1e8 ≈ 1.8e27).
   The ledger/report carries an explicit quote-unit tag (`QuoteUnit::Lamports`); mixing ledgers of
   different quote units is a type error, not a convention. Conversion to SOL (÷ 1e9) happens only
   at presentation, as an exact decimal string.
2. **Consideration source = the paired `TradeEvent`.** A trade's consideration comes from the
   `TradeEvent` paired 1:1 with its decoded trade instruction (same top-level group, consistent
   `mint`, `user`, side). Buy cost = `sol_amount + fee + creator_fee`; sell proceeds =
   `sol_amount - fee - creator_fee` (checked arithmetic; underflow = malformed). Fields beyond
   `creator_fee` in newer event layouts (cashback, buyback, holder rewards) are recorded but do not
   change consideration until a fixture proves they are additional to `fee` — if a fixture shows a
   non-zero residual explained by them, this ADR is amended with that evidence.
   Unpaired or inconsistent event → the trade is recorded with `BasisStatus::Unknown`
   (`ConsiderationUnverified`), never a consideration computed from instruction args or balance deltas.
3. **Quote asset gate.** v1 instructions (`buy`, `buy_exact_sol_in`, `sell`) are SOL-quoted.
   v2 instructions are SOL-quoted only when `quote_mint` is wSOL
   (`So11111111111111111111111111111111111111112`). Any other quote mint (e.g. USDC-paired curves)
   → the trade is recorded but its PnL is `Unknown { UnsupportedQuoteAsset }` in this ledger; no FX.
4. **Network fee (invariant #8).** `meta.fee` is charged to the fee payer only. If the trading
   wallet is the fee payer, the fee is allocated across that wallet's trades in the transaction:
   capitalized into buy basis / subtracted from sell proceeds (ADR-004). Several trades in one tx →
   split proportionally to consideration, remainder lamports to the earliest trade by canonical
   location, so allocations sum exactly to the fee. Not the fee payer → zero fee cost (sponsored).
5. **Everything else is not trading cost.** Rent deposits/refunds, tips, bot-platform fees, top-ups
   and other native transfers are never auto-classified as trading cost (ADR-004 "overhead only when
   confidently attributable"). Per wallet, the report exposes `unexplained_native_flow_lamports`
   (signed sum of the wallet's native delta not explained by its consideration + allocated fee) and
   the count of transactions contributing to it, so a reader sees how much activity sits outside the
   modeled trade legs. It is a diagnostic, not a PnL adjustment.
6. **Inventory continuity.** Token inventory for `(wallet, mint)` changes only through decoded trades.
   Any token balance movement of the wallet for that mint not explained by its decoded trades in the
   same transaction (transfers, post-migration venue trades, router forwards) breaks continuity:
   inbound → lot with `BasisStatus::Unknown`; outbound → disposal whose PnL is `Unknown`; the
   position is reported as censored for episode statistics. Never a fabricated trade.
7. **Ordering.** FIFO consumes by canonical location `(slot, transaction_index, instruction order)`
   (invariant #12), independent of fetch order.

## Consequences

- PnL is reported in SOL for the pump.fun bonding-curve scope only. Wallets whose activity is mostly
  post-migration will show censored positions and low eligible-episode counts — honestly, not as
  zero PnL.
- Bot-platform fees are not deducted, so PnL of bot users is an upper bound on their trade-leg PnL;
  `unexplained_native_flow_lamports` makes the size of that gap visible.
- A future USD ledger (P5.2) converts lamport-denominated execution values with a timestamped
  SOL/USD price; it does not reinterpret this ledger.
