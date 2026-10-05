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


## Amendment 1 (accepted, 2026-10-04): EVM open positions (on-chain quote)

Status: Accepted (2026-10-04). Extends this ADR to Robinhood, Base and BSC wallet ledgers (ADR-020).

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
   V4Quoter Robinhood) and, since ADR-020 amendment 7, from live-verified official pages (Pancake v3 Base/BSC,
   Uniswap v3 BSC, V4Quoter Base/BSC, Slipstream gen1-3 by the pool's `factory()`). Anything else is
   `venue_quoter_unpinned` (invariant 16); an SDK-only override exists for tests and is reported as
   `quoter_source=override`. A quoter answer of 0 is a valid quote of 0, never an error or "unknown".
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

**Amendment 1 note (2026-10-05): v4 PoolKey source and valuation cost plan.** The first live run (Robinhood wallet
`0x41de...`, Alchemy free tier, 178 open positions) spent 2,925 `eth_getLogs` on `Initialize` lookups (10-block range
cap) and left every position `request_budget_exhausted`. Changes: (a) the v4 PoolKey is read with one `eth_call`
`poolKeys(bytes25 poolId[0..25])` on the official PositionManager pinned per chain (Robinhood
`0x58daec31...4fa7`, Base `0x7c5f5a4b...9bdc`, BSC `0x7a4a5c91...f95b`; each `poolManager()` returned the pinned
PoolManager) and is accepted only when `keccak256(abi.encode(key)) == poolId` over all 32 bytes and
`currency0 != currency1` (a zeroed key = never registered); (b) fallback to the `Initialize` log goes through the logs
endpoint under a hard cap of 64 `eth_getLogs` requests per run, failed attempts included, and never spends more than
the budget left over by the plan; (c) before quoting, the valuation plans its requests (pool identity, PositionManager
lookups, 2 quote calls per position with the impact probe; cache-aware, counting what earlier admitted positions
fetch) and admits positions in order while they fit the remaining `--max-requests`; the rest are
`request_budget_exhausted` up front (no partial burn, run incomplete), positions that cost nothing (fully cached)
are always admitted. stderr: `valuation cost: positions=N planned_calls=M budget_left=K`; the diagnostics line
`requests_made=...` carries `(incl. valuation planned_calls=M)`. Quotes remain venue `eth_call`s pinned to one block.

**Amendment 1 note (2026-10-05): dead pools (illiquid / partially fillable / revert decoding).** The live rerun
(Robinhood, Alchemy) answered 163/163 open positions `quote_reverted`: the V4Quoter reverts with
`UnexpectedRevertBytes(bytes)` (`0x6190b2b0`) wrapping `NotEnoughLiquidity(bytes32 poolId)` (`0x7a5ed734`) and
`StateView.getLiquidity(poolId) == 0`: the last pool of abandoned memecoins is dead. Rules:

1. **Revert decoding.** Revert data (`error.data`) is unwrapped from `UnexpectedRevertBytes` (max 2 levels) and
   recognised as `NotEnoughLiquidity(bytes32)`, `PoolNotInitialized()` (`0x486aa307`; position `unvalued
   { pool_not_initialized }`) or any other selector (`unvalued { quote_reverted }` with the raw 4-byte
   `revert_selector` recorded). Selectors are computed by keccak in tests.
2. **Liquidity confirmation.** After `NotEnoughLiquidity` (v4) or a no-data revert (v3 / Pancake / Slipstream, an
   empty range) one liquidity read is made at the pinned block: v4 `StateView.getLiquidity(bytes32)` (StateView
   pinned per chain from developers.uniswap.org v4 deployments: Robinhood `0xf3334192...e673b`, Base
   `0xa3c0c9b6...7a71`, BSC `0xd13dd3d6...e0c4`; an unpinned chain stays `quote_reverted`), v3-family `pool.liquidity()`.
   A v3-family revert WITH data is never read as "no liquidity".
3. **`liquidity == 0` -> status `illiquid`** (reason `no_liquidity_in_last_pool`): `realizable_lower_bound_raw = 0`,
   caveat `other_pools_not_searched` (only the last-traded pool is asked, a better pool may exist), unrealized
   `lower_bound` = `-known remaining basis` (kept apart from the exact `unrealized_pnl`, which stays null).
4. **`liquidity > 0` but the whole amount does not fill -> status `partially_fillable`:** a bisection of the largest
   fillable amount (at most 12 quote calls, further limited to the budget slack of the cost plan; no slack = no
   search, lower bound 0) reports `realizable_lower_bound_raw` of that amount plus `unfillable_amount_raw`, same caveat.
5. **Lower bounds are never exact:** excluded from the exact `realizable` / `unrealized` totals (separate
   `illiquid` / `partially_fillable` counts and per-unit lower-bound sums), no probe, no USD (`lower_bound_not_priced`).
   Unvalued and illiquid positions now record the venue, pool address and v4 pool id.
6. **Cost plan** reserves one liquidity read per position (upper bound; made only after a revert); the bounded
   search is not planned, it spends slack only.
