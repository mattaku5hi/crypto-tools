# Base Clanker prior-six V4 post-swap spot audit — 2026-10-11

This offline pass recalculates post-swap spot from cached Uniswap V4 `Swap.sqrtPriceX96` for the six unresolved tokens in the prior-ten Clanker audit. It uses only the archived known-USDC PoolManager logs and cached Kraken USDC/USD daily candles; no RPC, market API, database, or application-code calls were made. The complete exact-rational event rows and input hashes are in `.scout-data/diagnostics/base-prior6-v4-spot-audit-2026-10-11/manifest.json`.

## Method and scope

The official [Uniswap V4 `IPoolManager` interface](https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol) documents `Swap.sqrtPriceX96` as the pool price after the swap, in Q64.96. The [PoolManager implementation](https://github.com/Uniswap/v4-core/blob/main/src/PoolManager.sol) emits this event before calling the `afterSwap` hook. This audit therefore treats the field as the core post-swap spot state, not the final wallet net price or necessarily post-hook accounting. The local [V4 decoder](../../../crates/scout-dex-evm/src/uniswap.rs) exposes it as an unsigned 160-bit value.

For each cached Initialize-verified pool, I computed `sqrtPriceX96² / 2^192` as the exact raw currency1/currency0 ratio, inverted it only when the verified currency order required, and applied the known 18-decimal token and 6-decimal USDC scales. Each event's spot was multiplied by that UTC date's cached Kraken USDC/USD low and high. The ranges are daily FX sensitivity bounds, not same-minute conversions. Only nonzero opposite-signed Swap deltas with a nonzero square-root price were retained. Exact rational numerator/denominator, raw signed amounts, block/log identity, source-response hash, and FX row values are preserved in the private manifest.

The scan covers the prior audit's known USDC PoolManager pools from each pool's Initialize block through Base anchor block 52,456,236. The archived query records are marked complete and the raw log responses are retained. This is not all Base DEX factories or all token pairs. Non-USDC pools are intentionally outside this comparison; several tokens below have such excluded pools.

## Results

| Token | Known USDC pools / logs | Birdeye high (UTC day) | Max post-swap spot across scanned USDC pools (UTC day; Kraken daily USD bounds) | Max spot on Birdeye day | GeckoTerminal high (UTC day); max spot on that day |
|---|---:|---:|---:|---:|---:|
| `00b0a0…9e3b07` | 7 / 108 (104 accepted) | `$0.000010010536877171716` (May 2) | `$0.00001529626803–$0.00001529932820` (May 2) | Same as global max; about 1.528× Birdeye | No recorded token-level maximum in the prior analysis |
| `3607f8…63b07` | 4 / 148 (146 accepted) | `$0.000010039829694162781` (May 3) | `$0.00039082745561–$0.00039196426566` (May 4) | `$0.00001000104027–$0.00001000304107`; 0.99614–0.99634× Birdeye | `$0.000388974286395251` (May 5); same-day max spot `$0.00016627186165–$0.00016632175818` |
| `5b2a3c…f8cb07` | 3 / 94 (93 accepted) | `$0.00005144453214016943` (May 2) | `$0.00010144607310–$0.00010146636840` (May 2) | Same as global max; about 1.972× Birdeye | `$0.0000723003298273597` (May 2); same-day max spot is about 1.403× this high |
| `67d5ab…9ab07` | 3 / 66 (66 accepted) | `$0.000010011587280260146` (May 1) | `$0.00289312583874–$0.00289428366799` (May 1) | Same as global max; about 289× Birdeye | `$0.000144069791573811` (May 1); same-day max spot is about 20.08× this high |
| `92d958…38b07` | 7 / 184 (183 accepted) | `$0.000010050423098518516` (May 3) | `$0.00001368959362–$0.00001369233236` (May 3) | Same as global max; about 1.362× Birdeye | `$0.0000100542474772537` (May 3); same-day max spot is about 1.362× this high |
| `fec3a8…f6cb07` | 4 / 69 (68 accepted) | `$0.000010019452563580683` (May 3) | `$0.00001235565038–$0.00001235812225` (May 3) | Same as global max; about 1.233× Birdeye | `$0.000011166419773745` (May 4); May 4 max spot is only about `1.02e-17` |

### Interpretation

The calculation changes the comparison because a post-swap marginal spot and the executed quote/base ratio are different price kinds. It does not make the six all-time prices agree:

- `3607f8…` has a promising **day-specific** Birdeye match on May 3. But its maximum post-swap spot over all scanned USDC pools is about `$0.00039196` on May 4—roughly 39× the Birdeye high. That spot-max event is the same event as the prior execution-ratio maximum and has an existing receipt/header check. The May 4 spot range is numerically within 0.77% of the May 5 GeckoTerminal high, but the dates differ; on May 5 itself the scanned spot maximum is only about 42.8% of the GeckoTerminal high. This may explain the magnitude of that provider disagreement, but it does not resolve the one-day discrepancy or establish a full-history maximum.
- `fec3a8…` does not repair the May 3/May 4 gap. Its receipt-checked all-time execution maximum and the largest post-swap spot are on May 3; its highest spot on the GeckoTerminal May 4 date is near `1e-17`, far below the reported `$0.0000111664` high. A May 3 spot near `$0.000012358` cannot be relabeled as corroboration of a May 4 high.
- The other four tokens' maximum known-USDC-pool spots also differ materially from both their Birdeye and same-day GeckoTerminal highs. The May 3 spot for `3607f8…` is the only Birdeye-day match in this cohort, but selecting it while ignoring the much larger all-history spot maximum would cherry-pick a date.

Post-swap `sqrtPriceX96` is a marginal pool-state price, not a guaranteed executable price for meaningful size. A thin pool can move sharply, and V4 hooks can apply additional accounting after the PoolManager emits the Swap event. This price-kind comparison does not change the existing ATH/FDV policy or authorize a record write; accepting it as an independent ATH comparator would require an explicit policy and golden evidence that compare the **all-history** maxima without relaxing the date/price gates.

Coverage remains bounded. `00b0a0…` has excluded native-ETH and other non-USDC quote pools; `67d5ab…` has a native-ETH pool and the prior report's GeckoTerminal empty-history gate remains; `92d958…` has a native-ETH pool. Other Base factories, later migrations, pools not present in the saved Initialize responses, and post-hook effective accounting are not proven complete here. The prior report's source/supply evidence is in [the prior-ten audit](2026-10-11-base-clanker-prior10-onchain.md); this note adds no FDV/DB changes.

## Provenance

Inputs and SHA-256 values are recorded in `.scout-data/diagnostics/base-prior6-v4-spot-audit-2026-10-11/manifest.json`. The cached Initialize identity source is `.scout-data/diagnostics/base-v4-initialize-pool-lookup-2026-10-11/manifest.json`; the Swap logs and comparisons come from `.scout-data/diagnostics/base-clanker-prior10-onchain-2026-10-11/analysis.json`; FX rows come from `.scout-data/diagnostics/kraken-usdc-usd-daily-2026-10-11/ohlc-raw.json`. Receipt/header validation exists for prior execution-maximum events; among these six, the global post-spot maximum event matches that receipt-checked execution event only for `3607f8…` and `fec3a8…`. The other spot-maximum logs were not given separate receipt/header checks in this offline pass.
