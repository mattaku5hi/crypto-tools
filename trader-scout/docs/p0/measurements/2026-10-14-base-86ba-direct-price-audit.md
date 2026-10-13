# Base 86ba: two-pool observed-price audit

**As of:** 2026-10-14 UTC
**Status:** read-only candidate evidence; no database write or ATH promotion.

## Result

For Base token `0x86ba36b55623935a11c4c04f4128eeaeb5ab1b07`, the pinned Uniswap v4 PoolManager scan found two pools from the token’s creation block through block `52,456,236`. Each pool has one successful Swap event in that interval. Both swaps occurred on 2026-04-24 UTC. They produce very different price kinds, so neither can be substituted for the other:

| Pool | Swap time UTC | Core deltas | Execution-average FDV, 100B supply | Post-swap marginal-spot FDV |
| --- | --- | --- | ---: | ---: |
| `0xae9e786432f1bf7209718f9dfdc6ce795f4c209611158f870a17d30aa6fa962b` | 05:15:21 | 0.022768 USDC in; 99,999,956,652.066052549… token out | $0.0227612 at daily close (2 cents floored) | $51,592.66598 at daily close (5,159,266 cents floored) |
| `0x47478f769eb26900dadcf26569dad0a1cb649b406bb8ed1d20c40994cbd49ce1` | 05:16:43 | 0.000100 USDC in; 98,994.64211057612436515 token out | $100.9852633 at daily close (10,098 cents floored) | $99.9803591 at daily close (9,998 cents floored) |

The cached Kraken `USDC/USD` daily candle for 2026-04-24 is `[open 0.9998, high 0.9999, low 0.9997, close 0.9997]`. Applying its low/high to each event’s exact USDC/token ratio gives the following **venue-specific reference bands**, not a universal peg: execution-average FDV is $0.0227612–$0.0227657 for pool 1 and $100.9852633–$101.0054664 for pool 2; post-swap spot FDV is $51,592.66598–$51,602.98761 for pool 1 and $99.9803591–$100.0003612 for pool 2. The data are preserved without floating-point arithmetic in the candidate JSON.

The execution-average maximum across these two observations is the second pool; the post-swap marginal-spot maximum is the first. This price-kind divergence is material. There is no same-window Codex/Birdeye comparison in the saved evidence, and one swap per pool is sparse. The result is therefore a scoped observed-price candidate, not a token ATH and not ready for automatic promotion.

## Evidence and derivation

- The launch receipt contains a V4 `Initialize` for pool 1 at block `45,109,169`, with currency0 Base USDC (`0x833589fcd6edb6e08f4c7c32d4f71b54bda02913`) and currency1 the target token. The exact-token Initialize filters cover that creation block through the anchor and found pool 2 at block `45,109,213` with the same currency order. See `.scout-data/diagnostics/base-86ba-birth-init-2026-10-13/manifest.json` and `initialize-scan-manifest.json`.
- Separate Swap log scans for both pool IDs, each from its Initialize block through anchor `52,456,236`, returned one log each. The second Swap receipt is successful and has exactly one matching PoolManager/pool Swap with byte-identical event data to the cached log. Its canonical block header is `0xd14140de436f624fccc6066e17f5a59a61515ad36d0371dda666b14af3e66ff7` at `2026-04-24T05:16:43Z`. The first event’s receipt/header and log match are preserved in `supply-audit-manifest.json` and `block-header-manifest.json`.
- Both pools have positive post-swap liquidity and nonzero signed deltas. With currency0=USDC (6 decimals) and currency1=token (18 decimals), execution average is `abs(amount0)/amount1 × 10^12` USDC/token. The post-swap marginal spot is `(2^192/sqrtPriceX96^2) × 10^12` USDC/token. These are separate measurements. V4 `Swap.amount0/amount1` are caller-perspective signed deltas; the signs here mean USDC input and token output. See [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs).
- Sourcify v2 reports creation/runtime matches and `isProxy=false`; direct runtime code at the pinned anchor is byte-equal to the Sourcify on-chain bytecode. `totalSupply` is 100 billion (18 decimals) at both Swap blocks, the anchor, and the later queried head. The birth-to-head logs show one 100B mint at creation and no burns. The verified source permits bridge-authorized cross-chain mint/burn, so this is observed supply stability through the queried head, not an unconditional assertion that supply can never change.
- Kraken daily OHLC bytes are cached at `.scout-data/diagnostics/kraken-usdc-usd-daily-2026-10-11/ohlc-raw.json`, SHA-256 `e84624b906803de0f4f8bf9344d88314f6af34ab150b470ee02893caee343465`; the selected row is timestamp `1776988800` (`2026-04-24` UTC), pair `USDC/USD`. A Coinbase one-minute `USDC-USD` request previously returned HTTP 404 (`NotFound`); no retry was made.
- Raw source hashes, exact fractions, decimal outputs, daily FX row, and the review-only status are in `.scout-data/diagnostics/base-86ba-two-pool-candidate-2026-10-14/candidate.json` (SHA-256 `7e9da78a938a3f3dfc7e7b7b6a4cd4f03ee3e6473c2192dd61f1856f99b582c6`). Two additional Base RPC requests were made for this pass (second-pool header and receipt); total recorded calls across the existing evidence bundle are 22. Codex/Birdeye requests and database writes: zero.

## Limits / next useful evidence

The scan establishes only these two pools on the pinned Base V4 PoolManager, from token creation through the stated anchor. It does not establish absence of other DEXes, curves, or venues, and it does not provide a continuous or provider-window lifetime maximum. A same-day independent price source and a policy decision about which price kind represents the ATH are still needed before a numeric value can be stored. The separate candidate artifact intentionally leaves `ath_at` unset and records no promotion.
