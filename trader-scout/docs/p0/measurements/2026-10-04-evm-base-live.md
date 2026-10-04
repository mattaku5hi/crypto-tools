# 2026-10-04 — first live wallet-stats on Base (Alchemy free tier, alchemy_getAssetTransfers indexer)

Commit `e683d94` (planner fix: no sole-touch receipts / balance reads when the indexer's internal
transfers are complete). `wallet-stats --period 1d --concurrency 2 --rpc-cu-per-sec 250
--max-requests 5000`, 2 wallets from `evm_base_swaps_all_2026-10-04.json`; exit 0, 264 s,
3,582 requests (alchemy_getAssetTransfers 19, eth_getTransactionReceipt 3,472,
eth_getBlockReceipts 38, eth_getBlockByNumber 42 for window resolution, eth_call 9 incl. the
USDC `decimals()` preflight). Before the fix the busy wallet was refused with a planned ≤ 9,486.

- `0x81ef037c…`: 3 txs, 1 trade (Uniswap v3 ETH buy), native leg source `alchemy_internal`.
- `0xb0b21cef…`: 3,623 txs, **3,618 trades (1,830 buys / 1,788 sells), all priced exactly**
  (USDC-quoted, single token) — a market-making bot whose position never returns to zero in the
  window: 0 closed episodes, 1 open, 10 left-censored disposals; known disposal PnL inside the open
  episode −176.051437 USDC; no USD legs (USD is computed for closed episodes only).
- API key absent from all output.
