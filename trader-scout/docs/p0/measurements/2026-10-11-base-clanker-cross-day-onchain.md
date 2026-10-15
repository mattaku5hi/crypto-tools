# Base Clanker cross-day GT price corroboration

Captured 2026-10-11 UTC. This read-only audit examines two Base Clanker tokens whose cached GeckoTerminal (GT) all-history maximum falls on a different day from their Birdeye peak-day reference. The previously completed source audit establishes a fixed supply of 100,000,000,000 tokens at 18 decimals through Base block 52,456,236; see [the supply proof](2026-10-11-base-additional-clanker-supply.md). No Birdeye or GT request was made for this audit.

## Scope and method

The candidates are `0x613c9c27b25a269635ec5893fdb0dd19ac4eab07` (GROK) and `0xfa12411fff9cc0f69c57289ca646716141570b07` (SpaceX). Each cached page-1 GT listing returned two USDC pools. All four pools produced one decodable v4 `Initialize` event at the registered Base PoolManager, with the expected token and USDC currencies. The existing `scout-dex-evm::decode_v4_initialize` and `decode_v4_swap` SDK decoders were used.

Each pool's `Swap` history was scanned from its initialization through the verified Base anchor, block 52,456,236, hash `0x021113f4f7d0a2fe23e0036948b341ef7bbe5c75a604eb56f96c8a873d5e39b8`. The four queries returned 193 logs total (18–61 per pool); 178 decoded to nonzero opposite-signed deltas. For each token, the maximum exact quote/token execution ratio across both scanned pools was selected, then checked against its successful transaction receipt and block header. No wallet or trader identity is inferred.

GT's cached daily OHLCV requests used `limit=1000`, `before_timestamp=1791676799`, USD currency, base-token pricing, and `include_empty_intervals=false`. Every one of the four returned page-1 pools has at least one saved candle. The GT maximum below is the maximum across all nonempty candles saved for both returned pools. The candle sets are sparse because empty intervals were excluded; this audit does not infer missing-candle prices, unreturned pools, or whole-market completeness. Both GT maxima tie across more than one candle day, and the chosen on-chain event day is one of those maximum days.

The USD intervals use the already cached Kraken USDC/USD daily low/high for the event's UTC day. They are dated exchange observations, not a fixed USDC peg. No fresh FX request was needed. Both `decimals()` calls at the event blocks returned 6 for Base USDC. The derived intervals were normalized from the Kraken raw OHLC tuple (low at index 3, high at index 2); this corrects the prior close-as-low derivation while leaving archived API responses unchanged.

## Results

| Token | Cached GT all-pool max | UTC days with that GT high | Receipt-verified max execution | Dated USDC/USD | On-chain USD interval | Difference from GT max |
|---|---:|---|---|---|---:|---:|
| GROK `0x613c9c…ac4eab07` | `$0.000312766860035363` | May 16, June 3 | May 16 11:56:03Z; `$0.0003115923619305861375…` | `0.9996–0.9998` | `$0.0003114677249858–$0.0003115300434582` | `−0.415%` to `−0.395%` |
| SpaceX `0xfa1241…41570b07` | `$0.0000553764135797533` | May 5, May 21 | May 5 14:47:17Z; `$0.0000555764472102310881…` | `0.9997–1.0000` | `$0.0000555597742761–$0.0000555764472102` | `+0.331%` to `+0.361%` |

For GROK, the selected pool was `0xc2139a…b3367e0`; the successful event has token delta `−9,738,191,209,821,649,817,691` raw and quote delta `+3,034,346` raw USDC, at block 46,071,608, log `0x331`. The receipt has status `0x1`, contains exactly the expected PoolManager/pool/log event, and matches the fetched block hash. The 3.034346 USDC quote is a small execution, not evidence of meaningful depth.

For SpaceX, the selected pool was `0x55ce7a…b6d54f63d`; the successful event has token delta `−9,645,236,910,741,547,564,375` raw and quote delta `+536,048` raw USDC, at block 45,601,545, log `0x2e`. Its receipt has status `0x1` and matching pool/log/block identity. The 0.536048 USDC quote is also small. These ratios corroborate the provider highs within the unchanged ±1% threshold; they do not establish executable valuation or meaningful market depth.

Both candidates have tied GT maximum candles. The event dates, May 16 for GROK and May 5 for SpaceX, are among the dates with the reported maximum. The evidence supports those dates as validated-window winning dates, but it does not support a unique exact ATH date. No database or FDV write was made.

## Evidence and limits

- Full RPC parameters, statuses, dates, response sizes, SHA-256 hashes, and raw bodies for 16 calls are in `.scout-data/diagnostics/base-clanker-cross-day-2026-10-11/manifest.json`. The manifest also includes both pool-candle scopes, exact max-event amounts, receipt/header/decimals proofs, and dated FX comparisons.
- Cached provider source hashes and raw GT pool/OHLCV files, the source-supply proof, and Kraken FX inputs are indexed in that manifest. All RPC results were HTTP 200, under the 2 MiB body cap, with no retries; the largest Swap query returned 61 logs, under the 5,000-log cap. Total RPC use was 16 of 24. No public FX request was made.
- This covers the two cached page-1 pools per token. It does not verify other GT pages, pools outside the cached listing, venues, wallet attribution, lifetime market completeness, or trader PnL.
