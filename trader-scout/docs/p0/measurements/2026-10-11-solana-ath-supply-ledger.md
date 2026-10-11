# 2026-10-11 — Solana supply reconciliation for three pump mints

This read-only check reconciles the finalized current mint supply to the mint's birth supply and successful Token / Token-2022 burn instructions through the UTC day of the Birdeye 1D high candle. It establishes supply changes for these mints; it does not establish a historical FDV at the candle's intraday high, prove trader-history completeness, or promote any ATH rows.

## Result

All three mints were born with exactly 1,000,000,000 tokens at six decimals. Their mint authorities were revoked in the creation transaction. Each current finalized supply equals birth supply minus the sum of supported `Burn` and `BurnChecked` raw amounts found in the complete paginated address-history response through 2026-10-10 23:59:59 UTC. The same holds against later finalized account snapshots, so there is no unexplained supply change through those anchors.

| Mint | Birth tx time (UTC) | Finalized account slot / raw supply | Burn instructions | Burn raw total = supply decrease | Burn time range (UTC) | Birdeye daily-high candle |
|---|---:|---:|---:|---:|---|---|
| `48vcicPG…` | 2026-10-10 22:04:09 | 455580287 / 951113325932600 | 28 | 48886674067400 | 22:04:26–22:10:01 | 2026-10-10 00:00:00, $0.00012179988071982598 |
| `81kiaJS…` | 2026-10-10 22:40:09 | 455580308 / 966411236949771 | 30 | 33588763050229 | 22:40:14–22:45:58 | 2026-10-10 00:00:00, $0.000009928178143700252 |
| `JCd8kQp…` | 2026-10-10 22:38:28 | 455580334 / 965761089271256 | 30 | 34238910728744 | 22:38:33–22:44:13 | 2026-10-10 00:00:00, $0.000008772215583865555 |

The price source is the existing `birdeye-ohlcv-1d-usd-outliers-v1` reference in `.scout-data/diagnostics/birdeye-runner-potential-fdv-reference-2026-10-11.json`. Its candle timestamp marks the start of a 1-day candle, not the minute when the high occurred. Every mint burned during that candle's UTC date. Exact supply at the intraday price high therefore remains unknown until the candle is narrowed to minute-level highs and matched to the ordered burn ledger.

## Evidence and method

The baseline [proof manifest](../../../.scout-data/diagnostics/solana-pump-mint-supply-proof-2026-10-11/manifest.json) records finalized Helius account reads and creation transactions captured at 2026-10-11 08:10 UTC. The mint accounts are owned by Token-2022. Their account type is `Mint` (`1`) and their complete TLV area contains only type `18` (64 bytes) and type `19` (variable metadata, 170–173 bytes), followed by zero padding. The official [`ExtensionType` enum](https://raw.githubusercontent.com/solana-program/token-2022/main/interface/src/extension/mod.rs) assigns these IDs to `MetadataPointer` and `TokenMetadata`. The enum includes `MintCloseAuthority` (3), `ConfidentialMintBurn` (23), and `ScaledUiAmount` (24); none is present. The official [Token-2022 metadata guide](https://solana.com/docs/tokens/extensions/metadata) describes pointer and token metadata as metadata storage/reference extensions.

I queried Helius `getTransactionsForAddress` for each mint, successful transactions only, ascending, full transaction details, from that mint's birth block time through the exclusive end of 2026-10-10 UTC. Full transaction detail pages use a maximum of 100 entries; the provider documents 1,000 only for signature-only pages. Pagination continued until `paginationToken` was null. The query used 10 HTTP attempts total (5, 3, and 2 pages), one-second minimum spacing, a 25-second timeout, and an 8 MiB response cap. All 578 rows had unique signatures, `slot`, `transactionIndex`, and `blockTime`; canonical order is `(slot, transactionIndex)`. Each versioned transaction's account indices were resolved using the message keys plus `meta.loadedAddresses` in writable-then-readonly order. Outer instructions and inner CPI instructions were both checked.

For Token and Token-2022 instructions, the parser read the base58 instruction bytes and exact account indexes. It recognized the SPL `MintTo`, `MintToChecked`, `Burn`, and `BurnChecked` layouts and summed the little-endian `u64` amounts only when the instruction's mint account matched the target mint. The official [SPL Token instruction source](https://raw.githubusercontent.com/solana-program/token/main/interface/src/instruction.rs) defines these instruction account positions and amount fields. Each mint has exactly one matching mint-to instruction: the birth mint of `1000000000000000` raw units. There are no later matching mint-to instructions. The transaction metadata logs show the birth-time metadata-pointer and token-metadata initialization/update-authority instructions; the current mint TLV likewise has no supply-changing extension.

The resulting arithmetic is exact:

- `48vcicPG…`: `1000000000000000 - 48886674067400 = 951113325932600`
- `81kiaJS…`: `1000000000000000 - 33588763050229 = 966411236949771`
- `JCd8kQp…`: `1000000000000000 - 34238910728744 = 965761089271256`

The provider's [pagination documentation](https://www.helius.dev/blog/introducing-gettransactionsforaddress) describes cursor pagination and the separate full-detail/signature-only page limits. This mint-address scan is used only to reconcile supply-changing instructions; it is not a trader or swap-history index. The balance reconciliation is the check against an omitted burn: with mint authority revoked, no supply-increasing extension present, and observed burns already equal to the full birth-to-anchor supply decrease, an additional unobserved burn would make the totals disagree.

## Artifacts and limits

Raw Helius page bodies and SHA-256 hashes are in `.scout-data/diagnostics/solana-pump-mint-supply-proof-2026-10-11/history-manifest.json`. Per-mint parsed burn ledgers and their page hashes are `supply-ledger-48vcicP.json`, `supply-ledger-81kiaJS.json`, and `supply-ledger-JCd8kQp.json` in the same directory. No API key or endpoint URL is present in these artifacts.

This reconciles supply through the high-candle day against later finalized anchors. It does not identify the time of the daily price high, estimate minute-level price or FDV, establish all-time ATH, or make any change to ATH policy or database rows.


## Cached trade-event and SOL/USD crosscheck

I decoded the three cached birth transactions and the cached transaction at the 48 mint's one-second price high with the existing deployment-gated `BondingCurveBuyDecoder` and `PumpAmmDecoder`. Each relevant PumpSwap event-CPI had exactly one account, `GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR`, matching the event-authority account in the repository's PumpSwap live fixtures. The parser scanned the cached mint-history pages; it made no RPC or market-data requests. Raw selected event details are in [the focused PumpSwap diagnostic](../../../.scout-data/diagnostics/solana-pump-mint-supply-proof-2026-10-11/pump-amm-event-crosscheck.json). The original Bonding Curve event crosscheck is preserved separately in that directory.

For `48vcicPG…`, the one-second Birdeye high is at 2026-10-10 22:04:54 UTC. In that exact cached transaction, the PumpSwap hop spent 50,000,000 raw wrapped-SOL units (0.05 SOL) for 3,139,649,581 raw `BzLqhz…` units, and the following target-token hop spent exactly those 3,139,649,581 units for 44,336,800,350 raw target units (44,336.800350 tokens). This gives an end-to-end execution average of about 0.0000011277 SOL per target token. The cached Kraken minute candle at 22:04:00 has SOL/USD low/high of $110.15/$110.23. Using wallet gross (0.05 SOL) gives $0.0001242196–$0.0001243098 per target token, 1.99%–2.06% above the Birdeye one-second high. The two raw pool-vault deltas give a separate pool-execution ratio: SOL pool credit 49,653,120 raw / 3,139,649,581 raw BzLq out, then 3,136,519,321 raw BzLq credited to the target pool / 44,336,800,350 raw target out. Intermediate quote-unit scales cancel. This yields $0.0001232348–$0.0001233243 per target token, 1.18%–1.25% above the Birdeye high. It still misses the 1% agreement test.

For the 48 pool-execution ratio, both legs were `buy_exact_quote_in`. The first BuyEvent records gross `quote_amount_in=50000000`, pool-vault credit `quote_amount_in_with_lp_fee=49653120`, `user_quote_amount_in=49554011`, LP fee 99109, protocol fee 24778, and creator fee 322102; its wSOL vault balance rises by exactly 49653120 while the BzLq vault falls by exactly 3139649581. The second records gross quote input 3139649581, pool-vault credit 3136519321, user quote input 3130258803, LP fee 6260518, protocol fee 1565130, and creator fee 1565130; the BzLq vault rises by exactly 3136519321 while the target vault falls by exactly 44336800350. This identifies the route's quote/base mint identities and checks the event amount against actual reserve-vault deltas. These pool-credit ratios are distinct from the wallet-gross ledger consideration in ADR-012; no wallet fee accounting or PnL definition changes here.

The 81 and JC birth trades use `87eaXH…` as their curve quote mint, not SOL. Their same-transaction PumpSwap hops exchanged 87 SOL for 6,679,286,924,241 and 15,982,492,950,660 raw `87eaXH…` units respectively. The curve events quote 6,553,804,468,470 and 15,709,976,680,992 raw units, so those hop outputs exceed curve quote consumption by 1.91% and 1.73%. A proportional allocation of the native leg would imply roughly $0.00001189 for 81 and $0.00001191 for JC using the matching Kraken minute high; these are indicative estimates only because the intermediate quote amounts do not reconcile exactly and the AMM swap is not linear. They differ from Birdeye's one-second highs ($0.000009928178143700252 and $0.000008772215583865555) by about 19.75% and 35.73%. The events therefore do not provide a 1% price corroboration for either token. I do not substitute SOL spot directly for either non-native quote.

I kept reserve-derived marks separate from execution averages. The PumpSwap BuyEvent reserve fields equal the pre-token-balance snapshots in these transactions. For 48, multiplying the two pre-trade pool ratios (raw target-pool quote/base `9660667619514 / 163579749845143`; raw SOL-pool quote/base `513928576897 / 33448474937373`, with the corresponding 6/9 decimals) gives about $0.000100024 per target token at the Kraken minute high, 17.88% below Birdeye. The actual post-transaction vault balances give a separate post-trade reserve spot of about $0.000100103, 17.81% below Birdeye. For 81 and JC, the birth Bonding Curve events report virtual quote/token reserves `8866766100956 / 279900000000000` and `21254324774561 / 279900000000000`; translating those quote-unit ratios using same-transaction pre-trade PumpSwap quote-pool ratios gives about $0.000052909 and $0.000057451 respectively. These cross-pool marks combine virtual curve reserves and an external quote-pool reserve ratio; they are only a consistency check. None of the reserve-derived values agrees within 1%.

The Kraken feed is one exchange's SOL/USD minute OHLC, not a global FX bound. These checks compare specific execution paths with the provider's candle highs; they do not establish the exact market-wide price at each second, reconstruct a post-trade virtual-reserve price, or promote any ATH/FDV record. Birdeye one-second high timestamps for 81 and JC precede their first observed burns by 5 and 5 seconds respectively; for 48, the high is after its first observed burn. The supply ledger is complete through the UTC day, but exact circulating supply at those intraday highs still requires the minute/second supply ledger.

## Subsequent intraday denominator reconstruction

The root agent subsequently saved three minute and three second-level price
windows with interval-specific source keys. An initial concurrent three-call
second-level pass produced one success and two429 responses; two explicit
sequential continuations succeeded. No automatic retries or key rotation.
The price high in every refined window agrees exactly with the saved daily
high. Timestamp/supply evidence is in
`.scout-data/diagnostics/solana-second-price-supply-crosscheck-2026-10-11.json`.

For81 and JC, the winning seconds are their birth seconds, strictly before
the first burns. With revoked mint authority, metadata-only extensions and
the reconciled monotone burn ledger, supply at those provider price highs is
exactly1B. Every other provider-observed price is no higher and supply never
increases, so no other observation in that window can exceed the resulting
FDV. Exact floored reference values are 992,817 and 877,221 cents.

For48, the high at22:04:54 follows burns. The full first-minute second candles
and full two-hour minute candles were archived in two additional bounded
requests, ten seconds apart. The winning second contains no supply change;
its raw supply is **997060502547477**, giving **12,144,185 cents**. For each
other second, the conservative supply before any burn in that second yields
an FDV upper bound no greater than this value. Upper bounds for the other
returned minute candles likewise do not exceed it. This establishes the
maximum observed-window FDV without guessing intrasecond transaction order.
The complete comparison is
`.scout-data/diagnostics/solana-48-variable-fdv-2026-10-11.json`; raw candle
response hashes are in `solana-48-full-candles-2026-10-11/manifest.json`.

These calculations resolve the denominator/time gap for this provider
window. Independent USD market corroboration above still fails the existing
1% test, so **none of these three values was promoted to ATH**. The references
are not new numeric database observations or canonical lifetime ATH claims.

## GeckoTerminal pool-history cross-check

A keyless GeckoTerminal v2 pass queried `GET /api/v2/networks/solana/tokens/{mint}/pools?page=1&include=base_token,quote_token` for the same three mints. All three returned HTTP 200 with an empty `data` array; therefore no OHLCV calls were made and neither peak-day nor known-pool maximum comparisons could be computed. This is a price coverage gap, not a zero price and not evidence that the tokens had no trades.

| Mint | Saved Birdeye one-second high | Peak timestamp (UTC) | Pool-list result | 1% comparison |
|---|---:|---|---|---|
| `48vcicPG…` | `0.00012179988071982598` | 2026-10-10 22:04:54 | empty page-1 listing | unavailable |
| `81kiaJS…` | `0.000009928178143700252` | 2026-10-10 22:40:09 | empty page-1 listing | unavailable |
| `JCd8kQp…` | `0.000008772215583865555` | 2026-10-10 22:38:28 | empty page-1 listing | unavailable |

The [diagnostic manifest](../../../.scout-data/diagnostics/geckoterminal-solana-three-crosscheck-2026-10-11/manifest.json) preserves the three exact request URLs/parameters, timestamps, response bodies, hashes, and the supply/price input-manifest hashes. All requests were at least 15 seconds apart; none were retried. The cached supply proof and the 48 variable-supply reference were used only to identify the inputs and retain their provenance; no new supply or FDV inference was made. With no returned pools, there is no independent price-source confirmation within the existing 1% threshold, and none of these observations is promoted to ATH.
