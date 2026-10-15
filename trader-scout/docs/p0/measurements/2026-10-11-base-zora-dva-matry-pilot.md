# Base Zora DVA/MATRY route-price pilot — 2026-10-11

## Result

This bounded audit found a strong same-transaction quote route for DVA's largest observed DVA/MATRY execution, but it does **not** validate the Birdeye daily USD high within the existing 1% comparison tolerance. The exact on-chain route implies a DVA execution price around **$0.0015197–$0.0015200** using Coinbase's ETH/USD one-minute candle, versus the Birdeye Sep 27 daily high **$0.0014819050096223831**. The discrepancy is about **+2.55% to +2.57%**. Keep this as a discordant observation; do not promote an ATH/FDV record from this pilot.

## Evidence and scope

- Chain/deployment: Base mainnet, chain ID 8453; the repository's Uniswap v4 gate records the Base PoolManager `0x498581ff718922c3f8e6a244956af099b2652b2b` as FixtureVerified (`crates/scout-dex-evm/src/gate.rs`). The event topic and signed `int128` amount decoding follow `crates/scout-dex-evm/src/uniswap.rs`.
- Target: DVA `0x058e72b9e99561844582b019ab91cdab6c9ac738`, pool ID `0xe5e1121b233d89988ecd0b5b9312d8919b81a8efe4dbd9a8c8336ea324189c54`, currency0 DVA / currency1 MATRY. Quote route: MATRY `0x364863eece7fd2e6c5f3dc13a3f043521f77ec49` / canonical Base WETH `0x4200000000000000000000000000000000000006`, pool ID `0x58798e9481475f7477dfbeff80f6fb305f08f20ac176d8cbfaf19c5caf1846ee`. Both pool identities were matched to their on-chain `Initialize` logs from that PoolManager.
- The scan covered 326 DVA/MATRY swaps from pool initialization through pinned block 52,456,236 and 519 MATRY/WETH swaps over the same anchored history. The largest absolute realized `MATRY per DVA` amount ratio was `1.356513539683119528770849846079331050388130285527...`, at block 51,869,488, log 153, transaction `0x5de8c05554f6c146f9e542587e2c125c8c4d666a1874dc354d34ca47c0d213dc`.
- In that same successful transaction, the MATRY/WETH pool delivered exactly `276.848739738952660574` MATRY (18 decimals), which the DVA/MATRY pool then spent for `204.088445592385538139` DVA (18 decimals). The WETH paid was `0.000115455870985401`. Thus the matched route cost was `5.657148823407404813699246029e-7 WETH/DVA`; this is transaction-level execution evidence, not a daily candle multiplication.
- The candidate event timestamp is 2026-09-27 16:58:43 UTC. Its block header and successful receipt were fetched at the same pinned chain state; both receipt logs match the exact PoolManager, pool IDs, transaction, and log indices. The nearest distinct MATRY/WETH transactions in the scanned pool were 38 seconds before (16:58:05 UTC) and 90 seconds after (17:00:13 UTC); their successful receipts and matching headers were also checked.
- Birdeye's saved closed-window data reports the DVA high as `0.0014819050096223831` USD on 2026-09-27 (`.scout-data/diagnostics/geckoterminal-base-zora-six-crosscheck-2026-10-11/manifest.json`). This is a daily candle date, not an exact intraday high timestamp.

## FX cross-check

One unauthenticated Coinbase Exchange candle request retrieved the ETH-USD 60-second bucket starting 2026-09-27 16:58:00 UTC. The returned low/high were **$2,686.29 / $2,686.84**. Applying this bucket range to the exact WETH/DVA route gives **$0.0015196742–$0.0015199854 per DVA**, 2.55–2.57% above Birdeye's date-level high. Coinbase documents candle records as `[timestamp, low, high, open, close]`, supports 60-second granularity, and cautions that history can be incomplete: [Get product candles](https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-candles). The returned bucket bounds the exchange price within that minute; it does not reveal the exact ETH/USD trade price at second 43.

This is a concrete cross-source discordance, not proof that Birdeye is wrong: the Birdeye comparator is a daily high with no stored intraday timestamp, and this scan covers these two identified pools rather than every possible DEX/pool or market. No Kraken request or additional FX venue was used. The result therefore fails the 1% validation gate and leaves the historical USD ATH unresolved.

## Reproducibility and limits

Raw bounded RPC responses, request/response hashes, headers, exact filter parameters, receipt evidence, and the Coinbase response are preserved under `.scout-data/diagnostics/base-zora-dva-matry-amm-pilot-2026-10-11/` (`phase1-manifest.json`, `phase2-manifest.json`, `coinbase-fx-manifest.json`, and their `raw/`, `phase2-raw/`, and `fx-raw/` files). The RPC endpoint value is intentionally omitted; only its environment-variable name and host are recorded. The pilot used 12 Base RPC calls total, one public FX GET, response caps of 2 MiB, no automatic retries, and no writes to the ATH database.

The observed maximum is a realized pool execution ratio, not a guarantee of a canonical market-wide ATH. The comparison does not establish pool-index completeness, migration history, or total market coverage. It also does not establish Birdeye's exact daily high timestamp. Further work would need exact-time price evidence or a comparable independently sourced daily USD bar before any record can satisfy the existing validation rule.
