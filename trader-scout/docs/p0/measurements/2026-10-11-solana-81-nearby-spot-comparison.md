# Solana 81kia nearby spot comparison

As of 2026-10-11. This is a bounded, offline comparison of cached finalized Solana transactions and cached Kraken SOL/USD minute data. It makes no ATH/FDV database change and does not call external APIs.

## Finding

The Tracker 81kia ATH candidate is `0.00009431116993278875` USD at `2026-10-10T22:45:47.033Z`, pool `eqEmQJtbfXvoaEkS3iJ1B1VZwRtE1Zns7ebGrb5dKoz`. Its cached successful transaction `1NiE1r...WFfjL5A` is at block time `22:45:46Z` (slot 455424899). The official Pump AMM IDL pinned at commit `2293f9a66c654e9fe82dc5e8f4618538f24bb35f` decodes an embedded `buy_exact_quote_in` followed by `BoostBuyAndBurn`. Both events name the same 81kia pool; the Boost event's post-state reserves agree with the transaction's post-token balances. This is a protocol buy-and-burn action, not evidence of a consumer acquisition or trader purchase.

The 81kia pool's quote mint is 87ea (both tokens have 6 decimals). The decoded prestate has 172,224,181.607503 81kia and 6,520,234.096188 87ea in real reserves, plus 1,355,816.852107 87ea virtual quote reserve. Its poststate has 171,414,521.632619 81kia and 6,557,435.863882 87ea in real reserves, with the same signed virtual quote reserve. Effective quote/base spot therefore moves from 0.04573138844256 to 0.04616442434760 87ea per 81kia.

For the quote bridge, the cached CF3 pool `CF3ML9g28j2bdcKZ4jDqtffomELUhcxPCSTtbcmpQ7JX` has a successful `SellV2` at `22:45:54Z`, 6.967 seconds after the Tracker timestamp. Its exact pretrade reserves and signed virtual quote reserve produce 0.00001851707940754 SOL per 87ea (6/9 decimal normalization). The cached Kraken SOL/USD 22:45 minute has positive trade volume and a low/high of $110.20/$110.35. Applying these same-minute endpoints to the CF3 prestate and then the 81kia poststate yields **$0.000094202300321567–$0.000094330524868284 per 81kia**, or **−0.11544% to +0.02053%** versus the Tracker ATH value. Both endpoints are within the existing 1% comparison threshold. The 81kia prestate sensitivity is $0.000093318654983130–$0.000093445676745811, which spans −1.05238% to −0.91770%; one endpoint falls just outside 1%. The poststate comparison is the one that matches at both endpoints.

## Scope and limits

The cached CF3 pool-address signature page has 1,000 rows and covers the interval, with its oldest returned block time at `22:36:43Z`. In the interval from the 81kia transaction at `22:45:46Z` through the CF3 sell at `22:45:54Z`, it contains no other CF3 pool-address signature; it does contain that sell. This does not rule out direct transfers to token vaults that do not reference the pool account, nor establish exact simultaneous state between separate pools. The comparison uses a later CF3 prestate as a nearby quote bridge, not an atomic price observation at the 81kia ATH instant.

The calculation is spot from reserve ratios, not executable market impact for a specified order. The Tracker ATH timestamp carries millisecond precision while the transaction and FX source are second/minute resolution. Pool state before and after BoostBuyAndBurn brackets the candidate event, and only the after-state conversion falls within 1% at both FX endpoints. These findings support a nearby numerical comparison; they do not prove a historical trade by a user or exact 81kia ATH execution price.

Private proof references and source hashes are in `.scout-data/diagnostics/solana-81-nearby-spot-comparison-2026-10-11/manifest.json`.
