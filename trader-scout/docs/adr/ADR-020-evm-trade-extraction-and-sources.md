# ADR-020: EVM trade extraction and history sources (Base, BSC, Robinhood Chain)

Status: Accepted (2026-10-03)
Date: 2026-10-03
Builds on: `docs/p0/evm-p0-research-2026-10-03.md`; mirrors ADR-013 (wallet-side consideration),
ADR-011 (windows), ADR-014 (intersect sides), ADR-016/018/019 (unknowns, USD, valuation).

## Context (measured 2026-10-03)

- Chain ids verified by `eth_chainId`: BSC 56, Base 8453, Robinhood Chain 4663 (Arbitrum Orbit L2,
  ETH gas, mainnet live; genesis hashes in the research doc).
- Uniswap v4 `Swap` carries no user (sender = router/locker; PoolId hashed); v2/v3 `sender` is the
  router. Trader identity and amounts must come from the transaction, not from pool events.
- Sources: Blockscout key — Robinhood works (free), Base HTTP 402 (paid plan), BSC "Network not
  supported"; Blockscout `txlistinternal` on Robinhood reports "internal transactions … not yet
  processed". Robinhood public RPC: `eth_getLogs` (10,000-result cap), `eth_getBlockReceipts` ok; no
  historical state (`eth_getBalance` at past blocks fails), no `debug_trace*`. Base public RPC:
  2,000-block `eth_getLogs` range cap. BSC public RPCs: logs unusable. Etherscan V2 free tier excludes
  BSC/Base.

## Decision

1. **Data model.** `RawEvmTransaction { chain, hash, block_number, block_time, tx_index, from, to,
   value, status, gas_used, effective_gas_price, l1_fee (Base OP-stack `l1Fee`; Robinhood
   `gasUsedForL1` accounted in gasUsed), logs[], internal_transfers: Option<Vec<…>> }` — `None` means
   "native internal flows not observed", never "zero".
2. **Extraction = owner-keyed net flows (ADR-013 analogue).** Per transaction: ERC-20 `Transfer`
   deltas per (owner, token), WETH `Deposit`/`Withdrawal` (WETH9 semantics), native: `tx.value` from
   `from` plus internal transfers when observed. A trade of wallet W is booked only when:
   a. W = `tx.from` (EOA signer) — smart-wallet/AA ownership is out of scope until evidenced;
   b. at least one **verified venue swap event** in the tx involves token T (Uniswap v2/v3/v4 Swap,
      Aerodrome, PancakeSwap, launchpad curve events — each with pinned official ABI/addresses +
      activation block + live fixtures per ADR-009/invariant #16);
   c. W's net deltas are exactly one traded token T and one quote asset Q (native ETH/BNB with
      WETH/WBNB merged, USDC, USDT) with opposite signs;
   d. consideration = W's own net Q delta (exact); gas = fee-payer-only (ADR-010 §4 analogue, incl.
      the L1 data fee on Base).
   If Q is native and the native inflow could only arrive through an internal transfer that was not
   observed (`internal_transfers = None`), the trade is recorded with Unknown consideration
   (`NativeLegNotObserved`) — never priced from the pool event. A trace-capable source makes it exact.
3. **History sources (pluggable per chain).** Token-centric: `eth_getLogs` of the token's `Transfer`
   topic over the window (adaptive range splitting on range/result caps), then receipts
   (`eth_getBlockReceipts`/batched receipts) and transactions; wallet-centric: Transfer logs with W
   in topic1/topic2 + W's outgoing txs (Blockscout `txlist`/`tokentx` where available). Internal
   transfers: trace RPC (`debug_traceTransaction`/`trace_*`) or Blockscout `txlistinternal` **only
   when it reports complete processing**. Chain identity preflight (`eth_chainId` + genesis hash)
   before any scan (invariant #3/#4).
4. **Order of work.** Robinhood first (works with existing key + public RPC), then Base (public RPC
   with 2,000-block windows or a keyed provider), BSC when the owner provides a provider with logs.
5. **Versions and scope text** name chain, sources, venue ABI pins and whether native internal
   flows were observed.

## Consequences

- EVM trades are booked at the wallet's exact token/quote deltas, routers/aggregators need no
  special casing, and unverified venues never create trades.
- Native-ETH sells stay Unknown until a trace-capable source is configured — a visible, counted gap.
