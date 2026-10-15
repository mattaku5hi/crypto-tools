# Base Zora: five remaining price cases (2026-10-11)

I checked the exact addresses behind the three disputed Zora ContentCoin quote assets, then queried their returned page-1 external pools for daily USD OHLCV. The original pool `Initialize` identities are retained in the linked quote-audit manifest; the new external-pool identity evidence below is GeckoTerminal’s exact base/quote token relationships. Prices are provider-reported daily candles, not exact execution-time valuations.

## Three quote-asset cases

| Target pair | Exact quote address | Quote-asset USD pool candle on target peak day | Target pool high in quote units | What this establishes |
|---|---|---:|---:|---|
| DVA / MATRY, 2026-09-27 | `0x364863eece7fd2e6c5f3dc13a3f043521f77ec49` | MATRY/WETH high `$0.00112009594617818` | `1.35651353968312` MATRY per DVA | An address-matched provider USD path exists for MATRY on that day. The two daily highs are independently timed; their product is only a diagnostic upper-product (`$0.001519425316734876…`), not an observed DVA/USD price. GeckoTerminal’s target-pool USD candle high is `$0.00134982425679485`; Birdeye is `$0.0014819050096223831`. |
| TWILIGHT / SUNLIT, 2026-09-26 | `0x3a3a6b2e1624c66e470460da92e86246ef2ece48` | SUNLIT/USDC high `$0.0000486778775206468`; SUNLIT/WETH high `$0.00112678022847111` | `3.54003643190408` SUNLIT per TWILIGHT | Both exact-address page-1 routes return same-day USD candles, but the two SUNLIT highs differ by about 23.15×. The daily-high products are `$0.000172321459850854…` via USDC and `$0.003988843059536932…` via WETH; they are not exact same-instant prices and disagree with each other. Do not select either as the canonical conversion. |
| QSNAG / SNAGETH, 2026-09-24 | `0x3c725423e2cad7239ef3c016a19f7a2d1c4aac37` | SNAGETH/WETH high `$0.0000395777361006741`; SNAGETH/ETH has no candle on 2026-09-24 | `0.0607516672046502` SNAGETH per QSNAG | A WETH USD path exists on the day; the separate ETH path has only earlier observations. The daily-high product via WETH is `$0.000002404413452301623…`, but the maxima may have happened at different times. GeckoTerminal’s target-pool USD high is `$0.000000054835582895948`, versus Birdeye `$0.0000008111506335367991`; the conversion does not reconcile that difference. |

The three quote contracts are exact source-matched Zora ContentCoin clones, not WETH, USDC, or a ZORA/USD instrument. Here, “USD path” means GeckoTerminal returned a `currency=usd` OHLCV candle for the exact quote address as the base token in a separate listed pool. It does not mean the quote coin is a stablecoin, that its price is trustworthy, or that the candle’s high coincided with the target-pool high. The quote pools’ histories are sparse (one to eight returned daily rows) and the source listings cover only page 1. In particular, the two SUNLIT pools give materially different USD readings, so this path is not sufficient to resolve the target price.

## Two tokens with empty GeckoTerminal listings

| Token | GeckoTerminal page-1 listing | DexScreener documented `/token-pairs/v1/base/{address}` lookup | Result |
|---|---|---|---|
| `0x010e1320febbb89c1a48e9549e8ff45db180ebd3` | HTTP 200, zero matching pools, no next page | HTTP 200, zero pairs | No public pool or historical USD path found in these scoped lookups; Birdeye’s one-bar value remains unvalidated. |
| `0x0261c9f86d60fb46b4e668cb2fb265694874ea6a` | HTTP 200, zero matching pools, no next page | HTTP 200, zero pairs | No public pool or historical USD path found; the one-bar `$2,689,264,895.1770654` outlier remains unvalidated. |

An empty listing is limited evidence about those providers’ current indexes; it does not prove that no historical pool existed. No quote symbol or assumed peg was used to fill the gap.

## Request and evidence scope

The pass used 15 unauthenticated public GETs, below the 16-request bound: three GeckoTerminal quote-token pool listings, five daily USD histories for the quote-token/WETH, USDC, or ETH pools returned by those listings, three daily quote-denominated histories for the target pools, and two documented DexScreener `/token-pairs/v1/{chainId}/{tokenAddress}` lookups for the empty targets. Two earlier exploratory requests to a legacy DexScreener path are retained in the raw manifest but are not relied on for the empty-pair conclusion. GeckoTerminal requests were spaced at least 15 seconds apart, used a 2 MiB response cap, and had no automatic retries. Every raw response is SHA-256 checked in `.scout-data/diagnostics/base-zora-five-price-followup-2026-10-11/manifest.json` and its `raw/` directory.

Relevant first-party references: [GeckoTerminal public API docs](https://api.geckoterminal.com/docs/index.html) document pool OHLCV and USD/token currency selection; [DexScreener API reference](https://docs.dexscreener.com/api/reference) documents token-pair lookup. Prior evidence files: `.scout-data/diagnostics/base-zora-quote-asset-audit-2026-10-11/manifest.json` and `.scout-data/diagnostics/geckoterminal-base-zora-six-crosscheck-2026-10-11/manifest.json`.

No chain RPC, Birdeye, Codex, or database calls/writes were made. No ATH/FDV values were changed. The three quote contracts have address-matched daily USD observations, but no exact event-time conversion was established; the two tokens with empty listings remain unpriced by these public paths. Treat all five as unresolved for canonical ATH validation.
