# Base e465 cached route recheck

**As of:** 2026-10-14 UTC
**Status:** independent arithmetic recheck only; no database write or ATH promotion.

The cached evidence supports a source-specific, same-minute **post-swap spot reference** for Base token `0xe46504da7f7f9d9233d6118e27bdfbf27964a215`:

1. The target V4 pool’s Swap is at `2026-09-25T22:04:51Z`. Its same-transaction quote/ZORA Swap is earlier in log order (353 vs. target log 366), so the recorded quote-to-ZORA spot state precedes the target event.
2. Compose the target’s post-Swap spot ratio with that quote/ZORA state, a historical WETH/ZORA spot, and the cached Coinbase ETH/USD minute candle (`2026-09-25T22:04Z`, low `$2,682.44`, high `$2,684.08`, close `$2,683.13`). The target, quote, ZORA, and WETH decimals are each verified as 18 at the event block.
3. The source-matched supply evidence is 1 billion tokens at the event, with one creation mint and no burns through the event. The target pool’s captured initialization-to-cutoff scan contains one Swap.

For the active 3,000-fee WETH/ZORA parent pool, the reproduced reference is `$0.0000001126074421055` per token at the minute close, or `$112.6074421` FDV; the low/high sensitivity interval is `$112.5784837–$112.6473124` FDV. The 10,000-fee parent pool gives `$112.9661382` at the close, about `0.31854%` higher, and is retained as a corroborating venue rather than selected as the primary conversion. The original target execution-average ratio remains separate; it must not be substituted for this marginal-spot reference.

This closes the arithmetic and event-order check, not market completeness. GeckoTerminal returned HTTP 200 but no daily candles for the exact target pool. The certificate covers the captured target pool, not alternate pools or DEXes, and uses one Coinbase FX minute without a second FX venue. Thus this is not a lifetime ATH or a claim that the observed pool boundary is a canonical all-market maximum. It remains a candidate for root review under the existing source-specific reference policy.

The independent recheck is `.scout-data/diagnostics/base-e465-cached-route-recheck-2026-10-14/verification.json` (SHA-256 `56b2a7f03e4c64d6b3eb1c2dc8982d3e81aa4546cf5b7b69d34ea96909d5441a`). It re-derived the values from cached exact fractions and verified all 18 source hashes listed in the original candidate. No provider requests, RPC requests, or database writes were made in this recheck.

Root review additionally rechecked the exact scan request: birth block `0x314c7c0` through anchor `0x320a267`, with one successful target Swap. The old `raw/05` quote-initialization response was an RPC error and is excluded; successful `raw/01` supplies the actual Initialize. The old supply audit's `raw/06` no longer matches its recorded receipt hash and is excluded as an integrity gap. A fresh receipt was captured to a new path, status `0x1`, with the same target log and canonical block hash (SHA-256 `6233f14b75e01bfb109fe944e9691b5b9df4730e9f1977db28558de3a765b74f`). Attribution of the changed old file is not established. The raw files and old manifest were not rewritten to hide this gap. Root arithmetic recheck separately verifies supply, event order, positive liquidity and both parent prices. Parent V3 states are end-of-event-block references; they are not represented as pre-target transaction snapshots.

After this root review and the Amendment 27c golden passed, the exact unknown row was guarded-written at 11,260 FDV cents using `onchain-pool-window-supply-indirect-spot-cex-reference-v1`; `ath_at=NULL`, holders and liquidity preserved. Private before/after rows, guarded SQL and root verification are in the manual-candidate directory. This closes the reviewed source-specific scalar value, while the market-coverage limits above remain.
