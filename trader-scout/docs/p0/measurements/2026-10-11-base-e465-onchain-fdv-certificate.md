# Base e465 onchain FDV candidate certificate

This is a source-only certificate for one observed Base token price candidate. It does not write the ATH/FDV database and does not assert an all-market or lifetime ATH. The candidate requires explicit price-policy review because the exact GeckoTerminal daily USD OHLCV response was empty and coverage outside the configured v4 pool is unknown.

## Result

For `0xe46504da7f7f9d9233d6118e27bdfbf27964a215`, the lowest-fee active ZORA/WETH parent pool gives a post-swap spot of `$1.12607442105546603896…e-7` per token using the Coinbase ETH-USD close for the same `2026-09-25 22:04 UTC` minute. The source-proven 1 billion supply through the event gives a candidate FDV of `$112.607442105546603896…`, or `$112.60` when flooring to cents. The same minute's Coinbase low/high bounds give floor-cent FDV bounds of `$112.57–$112.64`.

The pre-swap boundary is lower: `$1.11580330762305081749…e-7` per token and `$111.58` FDV at the same FX close. The only configured-pool Swap changes the boundary by about `+0.9205129%` for quote/token. The signed deltas and official Uniswap v4 settlement semantics identify quote as caller input and e465 as caller output. The event's `sqrtPriceX96` is the post-swap value.

| ZORA/WETH parent | Historical liquidity | Pre-swap FDV at FX close | Post-swap candidate FDV at FX close | Post-swap floor cents | Role |
|---|---:|---:|---:|---:|---|
| `0xa0ca5bebc42cdbf3623b1c09206ae4e3975b0fc7`, fee 3,000 | `14240401444882034149356` | `$111.5803307623…` | `$112.6074421055…` | `$112.60` | Primary: lowest fee with nonzero historical liquidity |
| `0x74f2b378e11112b64741f4b7adf757dbdb4f67ed`, fee 10,000 | `43820065584656535668` | `$111.9357551597…` | `$112.9661382304…` | `$112.96` | Corroboration; post estimate is `0.3185368%` above primary |

The event amount ratio composed through the same quote route, parent pools, and FX close is `$1.13224959601771479932…e-7` per token. The primary post-swap spot is `0.5453899%` below that event execution ratio. This is a sanity cross-check; it does not make the amount ratio a substitute price or prove the user-level net delta after hook adjustments.

## Boundary and conversion evidence

The exact configured pool is `0xf1661e6dbbde59e76f9b620e859431180cfb107849a82d6eac12300758a6f0f5`, with currency0 quote `0x107fb51dbea200ae8e8d69ecf9afe13bfb6d3479` and currency1 e465. The frozen configured-v4 classification contains one Swap from initialization through its cutoff. Its Initialize event at block `0x314c7c0` sets `sqrtPriceX96 = 759164761147273042673813471019`. StateView at block `0x316498f`, immediately before the only Swap, returns that same value. The Swap at block `0x3164990`, transaction index `0x62`, log index `0x16e`, has post-swap `sqrtPriceX96 = 755694595549977967291647672192`. The full configured-pool Swap scan is complete through the cutoff, so there is no earlier configured-pool Swap boundary to exceed the initialization/pre-event value.

Since e465 is currency1, the quote/token spot ratio is `Q192 / sqrtPriceX96² × 10^(e465_decimals − quote_decimals)`. Its quote conversion is the strictly prior quote/ZORA pool Swap in the same transaction: that parent event is log index 353, before target log index 366. The quote/ZORA pool scan retained 808 events. For the USD leg, both candidate v3 pools have nonzero historical liquidity; fee 500 is excluded because its liquidity is zero. There were no same-block Swap events in the selected parent pools. The fee-3,000 pool is the deterministic primary route by the lowest-fee/nonzero-liquidity rule; fee-10,000 is reported separately rather than averaged.

The historical target decimals and supply are 18 and `1,000,000,000 × 10^18` raw units, respectively. The quote, ZORA, and WETH each returned 18 decimals from three bounded `eth_call`s at the target event block. The token-specific audit verified the EIP-1167 implementation/source at creation and at the observed event, one creation mint, no burns through the event, and `totalSupply()` equal to `10^27` raw units. Supply proof ends at this event.

The Coinbase 1-minute ETH-USD candle is timestamped `2026-09-25T22:04:00Z`: low `$2,682.44`, high `$2,684.08`, open `$2,683.12`, close `$2,683.13`. The point estimate above uses close; low/high are retained as bounds. The exact calculation inputs, integer fractions, per-boundary prices/FDVs, cent floors, and file hashes are in `.scout-data/diagnostics/base-e465-fdv-certificate-2026-10-11/candidate.json`. The 3 decimals calls and pre-swap StateView call, with redacted endpoint metadata and retained raw responses, are in its `manifest.json` and `raw/` directory. Total Base RPC use for this broader route audit is 120/120; no further RPC calls were made.

GeckoTerminal's exact-pool metadata request returned HTTP 200 and verified the pool and token sides, but the authorized daily USD OHLCV request returned `ohlcv_list=[]`. Details and raw hashes are in `.scout-data/diagnostics/base-e465-geckoterminal-check-2026-10-11/` and [the cross-check note](2026-10-11-base-e465-gt-crosscheck.md).

## Scope and policy limits

The selected-pool configured-v4 maximum is bounded through the frozen cutoff, and supply is verified through the observed event. Other pools, other DEXes, migrations, and an independent positive USD candle are not established. This candidate is therefore not a canonical or lifetime ATH. Its onchain state spot can be considered under a future price policy for API-empty tokens, but it does not meet the current Birdeye-plus-GeckoTerminal corroboration path described by ADR-021 amendments 12–13.

The derivation uses exact integer fractions and `Fraction` arithmetic; decimal renderings are for review. FDV is `price × supply_tokens`, with the stated output cents floored only after exact multiplication. No amounts or prices were averaged.

Protocol references: [Uniswap v4 PoolManager source](https://github.com/Uniswap/v4-core/blob/main/src/PoolManager.sol) emits the core Swap result and post-swap sqrt price; [the v4 pool math](https://github.com/Uniswap/v4-core/blob/main/src/libraries/Pool.sol) updates that price during swaps; [Uniswap's flash-accounting guide](https://developers.uniswap.org/docs/protocols/v4/guides/flash-accounting) documents negative deltas as owed to the PoolManager and positive deltas as receivable by the caller. The 1% comparison is a consistency check only, not a canonical market-wide maximum claim.
