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

## Amendment 1 (2026-10-04, step 2) — accepted

1. **Uniswap v4 on Robinhood is `FixtureVerified`** (invariant #16): 45/45 live `Swap` events match the PoolManager's ERC-20 net deltas exactly (`amount_i == -net_into_PoolManager`, i.e. swapper's view, negative = swapper pays); the native side is invisible in logs (`tx.value == -amount0` on the 26 ETH-paying swaps is corroboration, not proof). `active_from_block` is not pinned. Evidence: `docs/p0/measurements/2026-10-04-uniswap-v4-robinhood-verification.md`. ERC-721 `Transfer` logs (4 topics, no data; v4 PositionManager NFTs) are not fungible flows: counted, never a `malformed_log`.
2. **Native-leg sources** (replaces the single "trace only" rule of section 2): per native-quoted trade, in order (a) trace (`debug_traceTransaction` callTracer; value transfers of successful frames, reverted subtrees dropped), (b) archive balance difference `eth_getBalance(W, block-1)` vs `(W, block)` plus the fee (Base `l1Fee` included), valid ONLY when the block's receipts show W touched by no other transaction (not `from`/`to`, not in any other tx's log topics, not an emitter); it supersedes `tx.value` and internal transfers for W, (c) otherwise `Unknown { NativeLegNotObserved }`. Capabilities are detected once per run from the oldest affected tx; a per-tx failure degrades that tx only. The source used is recorded per trade (`trace`, `balance_diff`, `explorer_internal`, `logs_and_value_only`). Residual assumption of (b): native value pushed to W by ANOTHER transaction of the same block through an internal call with no log naming W cannot be excluded from receipts; trace has no such assumption and is preferred when available. Sides (buyer-intersect) never need the native leg and never resolve it.
3. **Quote units**: native (ETH, WETH merged) = `Wei` (18 dp, exact); USDG (Robinhood `0x5fc5360d0400a0fd4f2af552add042d716f1d168`, 6 dp, verified by the run preflight against `decimals()`, mismatch = refusal) = `UsdgUnits`, valued at par in USD and labelled `usdg_par_assumed`; ETH-USD from Coinbase `ETH-USD` (ADR-018). Base/BSC pin no quote assets until addresses are verified. No FX, units never summed.
4. **Gas**: fee payer only (the signer), capitalized once into the basis (buy) / deducted from proceeds (sell) of native-quoted trades; for USDG-quoted trades it is not mixed into the USDG basis (ADR-013 section 2) and is recorded as unexplained native flow. A missing Base `l1Fee` makes the basis `Unknown { FeeNotObserved }`. Failed transactions: counted, fee not attributed to trading (no log proves a swap attempt).
5. **Ledger**: the Solana ledger core is generalized (u128 token amounts, chain-supplied native unit/unit set/asset key; lot proration in 256-bit arithmetic) and drives both chains; reports carry `chain` and an EVM trade audit trail; one output stack renders both (EVM spelling `wei`/`eth` for the Solana-era `lamports`/`sol` JSON keys).
6. **Provider limits**: Alchemy free tier caps `eth_getLogs` at 10 blocks (the error's suggested range fixes the span for the rest of the scan); HTTP 4xx JSON-RPC error bodies are read, not discarded. RPC URLs with embedded keys are secrets (never printed, never in fixtures).

## Amendment 2 (2026-10-04, live Alchemy-free findings) — accepted

1. **Wallet-centric scans never use a window-wide `eth_getLogs`.** They list through the indexer (`txlist` of the wallet's own transactions incl. failed, `tokentx` of every ERC-20 transfer to/from the wallet incl. transactions it did not sign, `txlistinternal` only when the explorer reports completeness), restricted to the window's block range, deduplicated by hash; receipts come from RPC (`eth_getTransactionReceipt`, `eth_getBlockReceipts` only where several needed transactions share a block) and block times from the listings. A truncated listing (page cap) is a coverage gap, never silent. Without an indexer the CLIs refuse (exit 4) instead of attempting ~blocks/span requests.
2. **Per-method routing.** `eth_getLogs` may go to a separate endpoint (`SCOUT_<CHAIN>_LOGS_RPC_URL`, or automatically the keyless public Robinhood RPC when the keyed RPC is probed as capped at <= 10 blocks); receipts, archive state, traces and balances stay on the main endpoint; one exact request budget spans all endpoints. Scope/`run_meta` record endpoint classes (`logs_source`, `state_source`), never URLs.
3. **Client-side rate limiting** (invariant #19): one token bucket per endpoint in front of every HTTP attempt, optional approximate per-method compute-unit weights, 429 without `Retry-After` halves the rate for the rest of the run. Request-count and CU numbers are policy knobs, not claims about any provider's billing.
4. A token scan whose estimate (`tokens x ceil(blocks / known span cap)`) exceeds `--max-requests` (default 2,000) is refused up front (exit 4).

## Amendment 3 (2026-10-04, v2/v3 pool admission) — accepted

1. **Pool admission replaces "factory creation logs must be fed".** A swap-shaped v2/v3 log counts only at an emitter admitted by `SwapVenueGate::admit_pool` (invariant #16): its `factory()` is a pinned official factory of the chain and venue AND its address is the CREATE2 address of `(factory, token0, token1[, fee], init-code hash)` where the deployment pins a hash (pinned only if it reproduces the fixture pools), or the factory's own `getPool`/`getPair` answer is the emitter. Metadata is read with bounded, cached `eth_call`s that count against `--max-requests`; refused emitters stay coverage gaps and are not asked twice. No pinned factory for the venue on the chain = no lookup, no gate.
2. **Robinhood Uniswap v3 is `FixtureVerified`** (factory `0x1f7d7550b1b028f7571e69a784071f0205fd2efa`, canonical init-code hash `0xe34f199b...8b54`): 39 of 39 admitted swaps (23 pools) match the pool's ERC-20 net deltas exactly (v3 amounts are the pool's view). Evidence: `docs/p0/measurements/2026-10-04-uniswap-v2v3-robinhood-verification.md`. The pool metadata of those fixtures was derived (pinned factory assumed, CREATE2 equality as proof); the live `--swaps` capture re-checks it against recorded `factory()` values. Uniswap v2 on Robinhood: no official factory is pinned, v2 emitters are not gated.
3. `evm-capture --swaps <venue>` scans the venue's `Swap` topic0 (v2/v3 without an address filter, v4 at the PoolManager), keeps the newest `--max-txs` distinct transactions and records `pool_metadata` of each v2/v3 emitter.


## Amendment 4 (2026-10-04, Base enabled; Alchemy transfers as wallet indexer) — accepted

Evidence: `docs/p0/measurements/2026-10-04-alchemy-free-tier.md`, owner's live checks of 2026-10-04: Blockscout PRO with the owner's key serves Robinhood, answers HTTP 402 on Base (paid plan) and does not support BSC; `alchemy_getAssetTransfers` works on all three chains for categories `external` and `erc20`, `internal` only on Base (BSC/Robinhood: JSON-RPC -32602 "The 'internal' category is not supported for this network").

1. **Indexer choice per chain** (wallet-centric scans; replaces "no indexer = exit 4" for Base/BSC). In order: (a) Blockscout when the chain is Robinhood and `SCOUT_BLOCKSCOUT_API_KEY` is set; (b) otherwise, with a keyed RPC, `alchemy_getAssetTransfers` if the RPC answers it (one counted probe request); (c) otherwise Blockscout if a key is set (the plan may cover the chain; a 402 is an error card, not a silent gap); (d) otherwise exit 4 naming both options. Runs record `listing_kind` (`blockscout` | `alchemy_transfers` | `token_logs` for buyer-intersect) in `run_meta.scan` and stderr.
2. **Alchemy transfers semantics.** Per wallet `W` and the window's block range, bounded paginated streams (`maxCount 0x3e8`, `order asc`, `withMetadata`, `pageKey`, at most 20 pages per stream): `fromAddress = W` and `toAddress = W` for `external` + `erc20` (the `from` stream also asks for zero-value rows so signed zero-ETH contract calls are listed; zero-value ERC-20 rows are dropped), plus `internal` from/to where supported. Support of `internal` is feature-detected from the first internal request of a chain and cached; unsupported means `internal_transfers = None` (never "no internal transfers"). Amounts are read ONLY from `rawContract.value` (exact hex integers; the float `value` is never used); rows are deduplicated by `uniqueId`, hashes by transaction. A `pageKey` left after the page cap makes the listing incomplete (the wallet card is `Incomplete`, never silent). Requests go through the shared RPC client: ONE request budget and the endpoint's limiter; `alchemy_getAssetTransfers` weighs 150 CU in the approximate table (policy knob, not a billing claim); the Alchemy listing requests of concurrent wallets are NOT part of the per-wallet admission estimate (only RPC receipt/lookup requests are planned), a spent budget surfaces as the usual budget error.
3. **Signer knowledge and cost.** An `external` row sent by `W` proves `W` signed the transaction and carries `to`/`value` (no gas price: the receipt's `effectiveGasPrice` is used). A hash named only by token rows is "token-only": the receipt names the signer; if it is `W` (a token sell with no ETH value row) one `eth_getTransactionByHash` supplies `tx.value`, counted in the plan (`eth_getTransactionByHash(<=)`). Hashes with a token transfer from `W` are possible sells (whole-block receipts, as with Blockscout).
4. **Known blind spot (reported, not hidden).** A transaction signed by `W` that moved neither ETH nor tokens to/from `W` (a failed swap, an approve, an unsuccessful call) does not appear: failed-transaction fee overhead may be incomplete. Whether Alchemy lists reverted top-level calls was not verified. Trades are unaffected (a trade needs a token movement), and ADR-020 amendment 1 §4 already keeps failed fees out of trading figures. Reports carry `listing_kind=alchemy_transfers` and the sentence "a transaction signed by the wallet that moved neither ETH nor tokens to/from it ... failed-transaction fee overhead may be incomplete" in `run_meta.scan.coverage_notes` and on stderr; the wallet status is not downgraded for it.
5. **Native legs on Base.** With the `internal` category supported and its pagination finished, internal ETH transfers are an indexer-provided complete set: `internal_transfers = Some`, `native_source = alchemy_internal` (label in `ledger.evm.trades[].native_leg` and `trades_by_source`), exact like `explorer_internal`; the resolver then makes no trace/archive request for those transactions. Assumption (not verified live): the internal category lists value transfers of successful frames only (reverted subtrees absent). On BSC and Robinhood (`internal` unsupported) native sells use the archive balance difference when the endpoint serves historical state, else Unknown, as before.
6. **Base enabled.** Quote assets: WETH (`0x4200…0006`, merged with native ETH) and USDC (Circle, `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, 6 dp; the run preflight compares `decimals()` and refuses on mismatch) = `UsdcUnits`, valued at par (`usdc_par_assumed`); ETH-USD from Coinbase `ETH-USD`; USDbC (bridged) is not a quote asset. Gas includes `l1Fee` (unchanged). The `--allow-unverified-chain` gate now only applies to BSC (no pinned quote assets, venues unverified). Base venues: Uniswap v2/v3, Aerodrome v2 and Slipstream are FixtureVerified (`2026-10-04-base-venues-verification.md`); Uniswap v4 on Base stays IdlOnly, so wallets trading through it are `Incomplete` and buyer-intersect exits 3 for them.
7. **buyer-intersect on Base.** Token-centric over `eth_getLogs`; the keyed Alchemy RPC caps it at 10 blocks (1 h = 1,800 blocks = 180 requests per token). The up-front estimate `tokens x ceil(blocks / 10)` against `--max-requests` (default 2,000) is kept. There is no automatic public-RPC routing for Base (the 2,000-block public cap and its reliability are not what the scope text promises): only `SCOUT_BASE_LOGS_RPC_URL` redirects `eth_getLogs`.
8. **Replay evidence** (offline, committed fixture `evm_base_swaps_all_2026-10-04.json`): `alchemy_getAssetTransfers` is replayed from the fixture's receipts/transactions (`evm_replay::AlchemyReplay`). The fixture holds no traces or historical balances, so the replay has `internal` unsupported (like BSC/Robinhood) unless a test scripts rows; scripted rows are SYNTHETIC and labelled so.
