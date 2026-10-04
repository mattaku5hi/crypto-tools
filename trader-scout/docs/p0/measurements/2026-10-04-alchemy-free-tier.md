# 2026-10-04 — Alchemy free tier, live capabilities (Robinhood, Base, BSC)

One Alchemy app, one API key, per-chain URLs (`robinhood-mainnet`, `base-mainnet`, `bnb-mainnet`
`.g.alchemy.com/v2/<key>`), configured as `SCOUT_{ROBINHOOD,BASE,BSC}_RPC_URL` (secret; never logged).
`eth_chainId`: 0x1237 / 0x2105 / 0x38 — match the chain profiles.

| Capability | Robinhood | Base | BSC |
|---|---|---|---|
| `eth_getLogs` | ≤ 10 blocks per request (HTTP 400, -32600 "Under the Free tier plan … up to a 10 block range", suggests a range) | same | same |
| Archive `eth_getBalance` (head − 2,000,000) | works | works | works |
| `debug_traceTransaction` | HTTP 400 "not available on the Free tier — Pay As You Go" | same | same |
| `trace_transaction` | "not available on ROBINHOOD_MAINNET" | — | — |
| `eth_getBlockReceipts` | works | works | works |

Consequences for ADR-020:
- Native ETH/BNB legs: the **archive balance-diff** path (W's balance at block−1 vs block, only when
  W has exactly one transaction in that block, gas added back when W paid it) is available on all
  three chains on the free tier; traces need Pay As You Go.
- Token-centric scans must use ≤ 10-block `eth_getLogs` windows on the free tier (Base ≈ 20 s,
  BSC ≈ 4.5 s, Robinhood ≈ 1 s of chain time per request) — feasible for short windows; long BSC
  windows need Pay As You Go or a different log source (the keyless Robinhood public RPC allows
  10,000-result queries without a tight range cap).

## Addendum: `alchemy_getAssetTransfers` (Enhanced API), measured by the owner 2026-10-04

| Capability | Robinhood | Base | BSC |
|---|---|---|---|
| categories `external`, `erc20` (`fromAddress`/`toAddress`, `pageKey`, `order`, `fromBlock`/`toBlock`, `withMetadata` block timestamps) | works | works | works |
| category `internal` | -32602 "The 'internal' category is not supported for this network" | works | -32602 (same) |
| Blockscout PRO, owner's key | works | HTTP 402 (paid plan) | not supported |

Used as the wallet-listing indexer where Blockscout is not available (ADR-020 amendment 4). Not verified: whether reverted
top-level calls (failed swap / approve with zero value) are listed; whether `internal` rows exclude reverted frames.
Base USDC (Circle official): `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, `decimals()` checked by each run's preflight.
