# 2026-10-04 — first live EVM runs (Robinhood Chain, Alchemy free tier + Blockscout + public RPC)

## buyer-intersect (commit `afbd486`)
2 tokens (`Aiden` 0x15e853…, `PWPLT` 0x04cc67…), 1-hour window, `--min-token-hits 1`:
- via Alchemy free for everything: budget of 3,000 requests exhausted (36,000 blocks / 10-block
  `eth_getLogs` cap) — exit 3, nothing scanned for the second token.
- with auto-routing (logs → keyless public Robinhood RPC, receipts/state → Alchemy): **63 requests,
  5.5 s**, exit 3 only because 1 swap went through a Uniswap v3 pool (v3 not yet verified on
  Robinhood). Both tokens were quiet in that hour (0 and 9 transfer logs).

## wallet-stats (commit `981d01c`)
2 wallets from the Aiden fixture, `--period 1d --concurrency 2 --rpc-cu-per-sec 250
--max-requests 3000`: pacing held (0 halvings, no 429s).
- `0x41deea…`: **859 txs, ok** — 624 trades (335 buys / 289 sells, all Uniswap v4 route trades,
  all priced exactly); native legs of all 624 from Blockscout internal transfers (listing reported
  complete that day — balance-diff not needed); 148 distinct tokens, 148 open episodes (a sniper
  accumulating positions; cross-checked one mint against Blockscout `tokentx`: one buy, no sell in
  the window), 289 known disposals inside open episodes with known PnL +10.3697 ETH; 0 closed
  episodes in the window. Open positions unvalued (EVM valuation not implemented yet).
- `0x4e40ce…`: 968 txs → planned ≤1,567 requests did not fit the remaining budget → refused before
  scanning (`not_scanned`, honest exit 3).
- Run total 915 RPC requests (eth_getTransactionReceipt 570, eth_getBlockReceipts 289,
  eth_getBlockByNumber 46 for window resolution, 3 setup), 341 s.
