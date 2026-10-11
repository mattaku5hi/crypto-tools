# Base Zora 14-token trade-boundary audit

**Scope:** fourteen previously selected Base Zora token/pool associations, their archived configured-v4 `Initialize` plus `Swap` logs, the cached quote/ZORA/WETH route and Coinbase ETH/USD minute candles. This is an event-boundary price reconstruction, not a token lifetime ATH, complete-market price, or FDV promotion.

## Result

All fourteen selected target pools have one archived `Swap` after their `Initialize` in the audited range: thirteen on 2026-10-09 03:49:39–03:50:33 UTC and e465 on 2026-09-25 22:04:51 UTC. There are no additional target-pool swap events in these archived queries. Thus the target-pool pre/post boundary is recoverable from the matching `Initialize.sqrtPriceX96` and that one `Swap.sqrtPriceX96` without more target-pool calls.

The rest of the selected event-time route is already cached for all fourteen: each row has a strictly prior 0x107f/ZORA Swap by `(block, transactionIndex, logIndex)` from an 808-event pool archive, two positive-liquidity ZORA/WETH v3 spot candidates (fee 3000 and 10000), and an ETH/USD Coinbase 1m candle. The two successful FX requests cover 2026-10-09 03:49–03:52 and 2026-09-25 22:04–22:07. There is no missing stored quote or ETH/USD minute input for these selected Swap timestamps. The external route and FX are evaluated at Swap time for both target-pool boundaries; the pre-swap mark is therefore not a launch-time price.

For the thirteen Oct. 9 tokens, the initial target-pool sqrt is `82,502,871,833,254`. The one Swap moves it to approximately `2.289e35` for eleven tokens and `5.173e35` for two. The event’s signed target-side delta is approximately `+990,000,000` tokens for every one, while its quote-side delta is approximately `-1,932.18` to `-1,935.26` quote tokens. The source-matched Zora `ContentCoin` implementation defines a 1B total supply and initially allocates 990M to market liquidity. Its pool distribution and this near-990M event delta give a concrete reason to treat the two numbers as different price kinds: the signed event amounts are caller-side net amounts, while `sqrtPriceX96` is the pool’s post-swap marginal state. A single near-full market-allocation-sized event can move the marginal state dramatically; this is consistent with the data and does not imply an SDK decoder defect. It also makes the post-state marginal marks poor standalone ATH candidates without liquidity/route-quality gates.

The exact offline composition gives an Oct. 9 post-swap spot range of roughly **$82.5M–$422.3M per token** across these thirteen events and the two cached ZORA/WETH venues. Their post-spot/raw-execution ratios are about **4.27×10¹⁸** for eleven tokens and **2.18×10¹⁹** for two. For e465, the same ratio is 0.99455 and the post boundary is approximately `$1.13e-7` per token. These are calculated boundary marks, not vetted market prices. The initial-price boundary for the 13 Oct. 9 pools is around `1.07e-35` USD/token when marked with Swap-time external rates; that extreme value also shows why the pool’s initialization state should not be presented as a historical USD quote.

## Supply proof for the thirteen Oct. 9 tokens

A bounded read-only Base RPC batch used 43 calls: two preflights, code/supply/decimals at each token’s own Swap block (39 calls), and two grouped zero-address `Transfer` filters from the earliest selected Initialize block to the latest selected Swap block. Every token’s launch timestamp equals its matching pool Initialize timestamp; each observed mint is in that same block and transaction. All thirteen runtimes match the exact EIP-1167 minimal-clone bytes pointing to the previously Sourcify-verified Zora implementation `0x5dbd43785954d43c1643a0caf2ecef9e0056ff13`. At each Swap block, `totalSupply=1,000,000,000e18` and `decimals=18`. Each token has one mint of exactly that amount and zero burn logs through its selected Swap. The implementation has a one-time initializer, performs the initial distribution in `ContentCoin`, and exposes user burn; the zero-address log audit is therefore material and is scoped only through each observed event.

The e465 source/supply proof is reused from [the separate e465 certificate](2026-10-11-base-e465-onchain-fdv-certificate.md); it is not counted as one of the thirteen new clone checks.

## Runner and completeness gates

The saved candidate joins contain `holders=null` and `max_liquidity_cents=null` for all fourteen. None appears in the saved 24-token source-qualified threshold-candidate list. Therefore this subset has **zero confirmed tokens** meeting both `holders >= 100` and a source-qualified `$500k+` candidate condition. The enormous calculated post-state marks above are not source-qualified candidates.

The archive covers the selected configured v4 pools, not every DEX, pool, migration, or route for each token. A one-Swap target-pool history cannot establish a market-wide/lifetime maximum. The report does not promote any value to the ATH/FDV ledger.

## Reproducible evidence

- New raw RPC and result manifest: `.scout-data/diagnostics/base-zora-14-supply-proof-2026-10-11/manifest.json` (43 requests; no URL saved), including raw responses and per-token event-block runtime, `totalSupply`, `decimals`, mint and burn logs.
- New exact arithmetic sidecar: `.scout-data/diagnostics/base-zora-14-supply-proof-2026-10-11/boundary-prices.json` (SHA-256 `5953cc01b169c1c74c5d50d5e1ea37ed8f7fd39a5adb6f9d726710e1a1432b18`). It includes matching `Initialize`/`Swap` raw paths and hashes, exact rational sqrt and route factors, FX candle provenance, and supply status for each token.
- Existing selection/log source: `.scout-data/diagnostics/empty-cohort-v4-scope-2026-10-11/annual-v4-pool-classification.json`.
- Existing event-ordered route/decimals: `.scout-data/diagnostics/empty-cohort-v4-quote-route-audit-2026-10-11/event-ordered-pricing.json`.
- Existing dated Coinbase candles: `.scout-data/diagnostics/empty-cohort-v4-quote-route-audit-2026-10-11/coinbase-eth-usd-fx/manifest.json`.
- Existing exact Sourcify source match and code: `.scout-data/diagnostics/base-zora-ath-source-audit-2026-10-11/sourcify-summary.json` and `source-files.json`; the source declares `MAX_TOTAL_SUPPLY = 1_000_000_000e18`, distributes 990M market supply, and includes an explicit burn path.
- Primary protocol source included in the Sourcify snapshot: `node_modules/@zoralabs/coins/node_modules/@uniswap/v4-core/src/interfaces/IPoolManager.sol` and `types/BalanceDelta.sol`.

No database writes, application-code changes, Codex calls, or additional price-provider calls were made in this stage. The useful next decision is whether the project’s price policy can accept an event-time marginal v4 spot as a scoped observed mark; regardless of that decision, a full ATH claim still needs broader pool/venue coverage and independent price quality evidence.
