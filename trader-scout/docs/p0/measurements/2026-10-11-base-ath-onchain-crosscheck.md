# Base Birdeye high cross-check against Uniswap v4 Swap events — 2026-10-11

## Result

For three disputed Base tokens, a real Uniswap v4 PoolManager Swap event on Birdeye's reported UTC peak day has an exact USDC-per-token event-delta ratio within 0.12% of Birdeye's USD-per-token daily high. The sampled peak-day swaps are strong evidence that those three reported prices correspond to on-chain market activity, but this does not establish USD equivalence, a token-wide lifetime ATH, or wallet-net trade prices. No ATH/database write was made by this pass.

| Token | Birdeye high (USD/token) and UTC day | PoolManager event (block; UTC) | Event ratio (USDC/token) | Ratio vs Birdeye number |
|---|---:|---|---:|---:|
| `0x8e07d62d6c2c899641b60292e10e32e32ca1ab07` | 0.0098586356482647; 2026-04-26 | 45,190,272; 2026-04-26 02:18:11 | 0.0098473209208551119310… | 0.9988523029× |
| `0x6b7c8947cd9c7855e420d3ef8bf4c6f492237b07` | 0.009862311592031825; 2026-04-27 | 45,250,034; 2026-04-27 11:30:15 | 0.0098618946923455258317… | 0.9999577280× |
| `0xe961523fd436e42635a4fce0417cb9f9fe56ab07` | 0.0006667596952990955; 2026-04-25 | 45,171,214; 2026-04-25 15:42:55 | 0.0006667569161652386228… | 0.9999958319× |

The cross-unit ratios above are numeric diagnostics only: Birdeye labels its field USD/token, whereas the chain events use USDC, whose USD exchange rate was not established in this measurement. For each of these three tokens, the displayed event was also the maximum among the two known USDC-quoted pools queried for that token; the second-pool maxima were about 0.000001 USDC/token. This comparison is confined to those known pools and does not claim complete market coverage. The source data and exact arithmetic are in the private [RPC evidence manifest](../../../.scout-data/diagnostics/base-v4-swap-crosscheck-2026-10-11/manifest.json). The same-day event values are not converted to USD.

## Method and bounded scope

Used the locally pinned Base Uniswap v4 PoolManager `0x498581ff718922c3f8e6a244956af099b2652b2b` and `Swap` topic from [`gate.rs`](../../../crates/scout-dex-evm/src/gate.rs) and [`uniswap.rs`](../../../crates/scout-dex-evm/src/uniswap.rs). For each of six tokens, `eth_getLogs` queried each known initialized pool from its initialization block through Base block 52,456,236. The run made 30 JSON-RPC calls total: chain/anchor preflight (2), 6b7 pool initialization filters (2), pool logs (20), and one candidate event-block timestamp lookup for each token (6). Every call returned HTTP 200 within the 2 MiB response cap; there were no retries. The endpoint credential was kept private; evidence records host only. The full candidate set and per-pool query boundaries are in the manifest.

The pools were matched by exact token currency address and quote currency. Quote raw amounts used the USDC 6-decimal scale and token raw amounts used the verified 18-decimal scale. For a USDC/token event, the measured execution ratio is `abs(amount0) / 10^6 ÷ (abs(amount1) / 10^18)` when token is currency1, and reverses the amount roles when token is currency0. Values were computed with integer/Fraction arithmetic and archived as decimal strings. The manifest also records the independent post-swap square-root spot price; it is a different metric and was not substituted for the event amount ratio.

After the initial 30-call log-and-timestamp phase, three separately authorized receipt calls brought the total to 33. Each receipt returned success status and contained exactly one event matching the candidate log index, transaction hash, block hash, PoolManager address, and pool ID. Each transaction also contained two Swap events for that same pool and seven USDC Transfer logs. The full transfer log endpoints and exact raw amounts are archived, but are not attributed to a trader or treated as wallet consideration: these were multi-swap transactions, and transfer flow can include pool, hook, and router legs.

The 6b7 live Initialize lookup returned two USDC pools, consistent with the previously cached pool index. Other tokens had 2–9 known pools in the prior Initialize evidence. This is a bounded known-pool sample, not a proof that every pool or venue was found. A returned empty log result means no matching emitted Swap in that specific pool/range; it is not evidence that the token had no trading elsewhere.

## Interpretation limits

Uniswap's official [`IPoolManager` interface](https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol) defines Swap `amount0`/`amount1` as signed currency deltas and reports `sqrtPriceX96` as the post-swap pool price. The official [`PoolManager` implementation](https://github.com/Uniswap/v4-core/blob/main/src/PoolManager.sol) emits the Swap event before invoking `afterSwap` hooks. Consequently these event deltas are useful pool-execution evidence but are not automatically the final wallet net amounts: custom hook deltas, router legs, and other transaction flows can change wallet-side consideration. The indexed `sender` is not treated as an economic owner. This follows the quote-unit and wallet-side consideration distinction in [ADR-013](../../adr/ADR-013-route-swaps-and-quote-units.md).

The event agrees closely with the Birdeye daily high on the exact UTC date for these three examples, despite earlier GeckoTerminal token-candle comparisons missing comparable highs. That supports using independent swap-event evidence to diagnose sparse or filtered candles; it does not show that Birdeye captures all market venues, all intraday peaks, or the full token lifetime. The remaining three sampled tokens showed larger cross-pool event maxima or no accepted USDC ratio, so this report makes no corroboration claim for them. Exact per-pool maxima, event counts, invalid zero-delta counts, transaction hashes, post-swap spot values, and the additional three same-day probes are preserved in the manifest.

Receipt inclusion confirms that the sampled Swap events are present in successful receipts for the queried block hashes; it does not resolve the hook/router caveat or establish wallet-side consideration. A contemporaneous USDC/USD conversion remains a separate evidence gap; no peg is inferred here.

## Subsequent FX validation and guarded recording

The root agent subsequently resolved the quote-conversion gap using closed
Kraken USDC/USD daily observations: April25 low/high 0.9994/0.9998,
April26 0.9997/1.0000, April27 0.9997/0.9999. Pair identity and raw responses
are archived in `.scout-data/diagnostics/kraken-usdc-usd-daily-2026-10-11/manifest.json`.
The currently open candle was excluded, as required by the
[Kraken OHLC contract](https://docs.kraken.com/api-reference/market-data/get-ohlc-data).
These exchange observations are a sensitivity range, not global FX bounds.
At both ends of each range the exact Swap ratio stays within 1% of Birdeye:
maximum deviations 0.144735%, 0.034226% and 0.060417%, respectively.

Together with the previously verified invariant 100B supply, this satisfied
[ADR-021 amendment14](../../adr/ADR-021-dev-tracker-facts-and-storage.md).
Three guarded missing-only writes were then recorded, respectively
98,586,356,482 / 98,623,115,920 / 6,667,596,952 cents, with source
`birdeye-window-supply-swap-fx-validated-v1` and `ath_at=NULL`.
The actual monetary value remains the provider's USD high times proven supply;
the independently converted event price corroborates it. Receipt evidence
does not change the hook/router or market-coverage limitations above.
Write results and proof hashes are in
`inputs/results/birdeye-validated-fdv-swap-fx-2026-10-11.json`.
