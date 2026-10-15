# Base e465 GeckoTerminal and quote-route cross-check

This read-only check used two authorized unauthenticated GeckoTerminal v2 GETs for `0xe46504da7f7f9d9233d6118e27bdfbf27964a215` and its configured Uniswap v4 pool `0xf1661e6dbbde59e76f9b620e859431180cfb107849a82d6eac12300758a6f0f5`. It made no database or ATH/FDV writes.

The pool metadata response returned HTTP 200 and identified the exact pool, `e465…` as its base token, `0x107fb51dbea200ae8e8d69ecf9afe13bfb6d3479` as quote token, and the DEX as `uniswap-v4-base`. The daily OHLCV request used `aggregate=1`, `limit=1000`, `before_timestamp=1791676799`, `currency=usd`, `token=base`, and `include_empty_intervals=false`; it also returned HTTP 200, but `ohlcv_list` was empty. GeckoTerminal therefore supplies no independent positive daily USD high for this pool in the queried history. Raw responses, exact requests, and SHA-256 hashes are in `.scout-data/diagnostics/base-e465-geckoterminal-check-2026-10-11/manifest.json` and `raw/`.

The onchain route was recomputed from the frozen event-ordered sidecar using exact fractions: target quote/token spot × the strictly prior quote/ZORA Swap spot × historical WETH/ZORA v3 `sqrtPriceX96² / 2^192` × the Coinbase ETH-USD 1-minute candle. The token, quote, ZORA, and WETH each have 18 decimals in the saved Multicall3 result. Coinbase’s 2026-09-25 22:04 UTC candle was low `$2,682.44`, high `$2,684.08`, open `$2,683.12`, close `$2,683.13`.

| Historical ZORA/WETH pool | Fee | e465 spot USD using minute low–high | Spot vs same-event execution ratio |
|---|---:|---:|---:|
| `0xa0ca5bebc42cdbf3623b1c09206ae4e3975b0fc7` | 3,000 | `$1.1257848371e-7–$1.1264731236e-7` | spot is `0.5453899%` below execution |
| `0x74f2b378e11112b64741f4b7adf757dbdb4f67ed` | 10,000 | `$1.1293708759e-7–$1.1300613548e-7` | spot is `0.5453899%` below execution |

The two pool spot ranges differ by about 0.32%; each produces a same-event spot/execution difference below 1%. This is a consistency check among the onchain event, pool-state observations, and one dated FX candle. It does not replace the empty GeckoTerminal candle result with independent market-history evidence. Exact fractions, minute OHLC values, derived low/high/open/close spot and execution values, and source hashes are in `e465-candidate-reconciliation.json` beside the raw responses.

The separate [e465 supply/source audit](2026-10-11-base-e465-supply-source-audit.md) verifies 1 billion tokens through the event. The frozen configured-v4 classification reports exactly one Swap in this pool through the cutoff. Other pools, DEXes, and migrations are not proven complete. The candidate therefore remains unpromoted: the authorized independent daily USD-high check returned no candles, and broader market-scope completeness is unknown. A separate [source-only FDV candidate certificate](2026-10-11-base-e465-onchain-fdv-certificate.md) preserves the exact onchain boundary, conversion, supply denominator, price bands, and cent floors for later price-policy review.
