# 2026-10-05 — live EVM open-position valuation and first multi-chain run

Commit `cd6222a`. `wallet-stats --period 1d --concurrency 2 --rpc-cu-per-sec 250 --max-requests 3000
--detail full` over a mixed input (`robinhood:0x41deea…`, `base:0x81ef03…`) — one multi-chain run,
exit 0, per-chain budgets: robinhood 2,165 requests (exit 0), base 51 (exit 0). Keys absent from all
output.

- Before the PositionManager fix (`a1c4d32`), the v4 PoolKey lookup via `Initialize` logs burned
  2,925 `eth_getLogs` under Alchemy's 10-block cap and the budget (all positions
  `request_budget_exhausted`). After: `log_calls=0`, PoolKeys from `PositionManager.poolKeys`.
- First rerun: 163/163 `quote_reverted`. Hand-decoded live: `UnexpectedRevertBytes(NotEnoughLiquidity(poolId))`;
  `StateView.getLiquidity` = 0 → dead pools (dead-pool handling added in `cd6222a`).
- Multi-chain run: Robinhood wallet (a sniper, 1,026 txs/day) has 164 open positions → **162
  `illiquid`** (`no_liquidity_in_last_pool`, lower bound 0, caveat `other_pools_not_searched`), 2
  `quote_reverted` (other selector). Sample of 25 illiquid pools: all plain Uniswap v4 pools, fee
  1 %, **no hooks**, liquidity 0 — i.e. liquidity was pulled (rugged launches); the zero is not an
  artefact of hook-held liquidity. Base wallet: `no_trade_activity` in the window.
- Planned valuation cost: 656 calls for 164 positions (491 used), budget left 1,329.
