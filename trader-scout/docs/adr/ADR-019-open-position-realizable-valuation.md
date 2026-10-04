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

## Implementation evidence (2026-10-03)

- Swap math: `crates/scout-dex-solana/src/swap_math.rs`; exact reproduction on the committed fixtures in
  `crates/scout-dex-solana/tests/swap_math_evidence.rs` (run with `--nocapture` for per-signature rows).
  Rules: every fee is `ceil(gross * bps / 10_000)` on the GROSS quote amount; constant-product output is
  `floor(in * reserve_out / (reserve_in + in))`; PumpSwap runs on `pool_quote_vault + virtual_quote_reserves`
  (the event's pre-trade reserves); curve `TradeEvent` reserves are POST-trade (`sol_amount` is gross; a sell pays
  `sol_amount - fee - creator_fee`).
- State: `getMultipleAccounts` (base64, `confirmed`), decoders in `crates/scout-dex-solana/src/pump_accounts.rs`.
- Valuation: `crates/scout-engine/src/solana_open_valuation.rs`; venue accounts come from the decoded instruction
  (`bonding_curve` / `pool` accounts), no PDA derivation is needed.
- Residual (not a sell path): the fee-exempt mayhem-agent curve buy `2W63KrJw` does not follow the curve formula.


## Amendment 1 (draft, 2026-10-04): EVM open positions (on-chain quote)

Status: Draft for review. Extends this ADR to Robinhood, Base and BSC wallet ledgers (ADR-020).

1. **Same contract, different source.** An EVM open position (`(wallet, token)` FIFO remainder) is valued
   only on a live run (window ends at `as_of`); a historical window is `unvalued { historical_window }` and
   nothing is read. The value is what selling the WHOLE remainder into the pool's other asset returns NOW,
   asked from the venue itself, not a spot price.
2. **Venue = the pool the wallet last traded the token on** (ledger audit trail: venue label, swap-event
   emitter = pool / v4 PoolManager, v4 poolId). Quote paths: Uniswap v3 / Pancake v3 / Slipstream
   `QuoterV2.quoteExactInputSingle`; Uniswap v4 `V4Quoter.quoteExactInputSingle` with the PoolKey rebuilt from the
   pool's `Initialize` log and verified by `keccak256(abi.encode(key)) == poolId` (else `pool_key_unknown`);
   Uniswap v2 / Pancake v2 `getReserves()` + exact constant product with the factory's fee (Uniswap 997/1000,
   Pancake v2 9975/10000; an unpinned v2 factory is `venue_not_supported`); Aerodrome v2 pool `getAmountOut`.
3. **Pinned quoters only.** Addresses come from the research doc 2.2-2.4 (Uniswap v3 QuoterV2 Base/Robinhood,
   V4Quoter Robinhood). Everything else is `venue_quoter_unpinned` (invariant 16); an SDK-only override exists
   for tests and is reported as `quoter_source=override`.
4. **Pinned block, bounded, counted.** One `eth_blockNumber` fixes `state_block`; every `eth_call` uses it, goes
   through the run's request budget and limiter and is cached per run. A revert is `quote_reverted`, an unreadable
   answer `quote_response_invalid`, a spent budget `request_budget_exhausted`; never zero.
5. **Quote asset and unit.** The pool's other asset must be native (address(0) / WETH / WBNB merged) or a pinned
   quote token (USDG, Base USDC, Binance-Peg USDT/USDC); otherwise `quote_asset_unsupported`. Raw output is in
   that unit; totals are per unit, never summed across units.
6. **Impact and caveats.** `price_impact_bps` compares the full sale with a probe of 1/1000 of the position (pool
   fee cancels). The quote ignores transfer tax and exit gas; a token whose venue-side flow differed from the
   wallet's own delta in the run is labelled `transfer_tax_not_modelled`.
7. **Unrealized and USD.** `unrealized = realizable - known remaining basis` only when every open lot is known
   and in the same unit (`unknown_basis` / `basis_other_unit` otherwise). USD via ADR-018 at `as_of` (ETH-USD,
   BNB-USD, USDC/USDG par, USDT candles; Binance-Peg flagged `+binance_peg`).
8. **Ranking** is unchanged (realized-only); `--require-valued-open` and `--no-valuation` behave as on Solana.

Evidence: `crates/scout-engine/tests/evm_open_valuation.rs` (each venue path, revert, unknown PoolKey, budget,
USD, unrealized states, ranking, replays of one real Robinhood v4 wallet card with the fixture's real `Initialize`
log and one real Base USDC wallet card with mocked quoter answers).
