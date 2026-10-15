# Base Zora candidate supply audit (2026-10-11)

## Result

The six remaining positive-window Base Zora candidates share one exact EIP-1167 minimal-proxy runtime and one fixed implementation. The implementation has a Sourcify `exact_match` source/runtime match. Its verified source initializes each coin once and mints a constant 1,000,000,000 tokens; its only later supply-reducing method is a holder burn. At the pinned Base block, every candidate has `totalSupply = 1,000,000,000 × 10^18`, 18 decimals, one zero-address mint at its recorded deployment block for exactly that amount, and no zero-address burn from the earliest candidate deployment through the pinned block. This supports a historically invariant 1B supply **through that block for these six contracts**.

This closes only the denominator question for the six addresses. Their price input is a sparse Birdeye daily-candle observation (one to four bars per token), was not independently corroborated here, and is not a lifetime ATH proof. In particular, the `0x0261…6a` observed price of `2,689,264,895.1770654` USD is an extreme outlier and should remain unpromoted pending price/pool validation. This audit made no database or ATH/FDV writes.

## Source and runtime proof

The source/runtime lookup was one unauthenticated GET to [Sourcify v2](https://docs.sourcify.dev/docs/api/), for Base implementation `0x5dbd43785954d43c1643a0caf2ecef9e0056ff13`. Sourcify returned `creationMatch=exact_match`, `runtimeMatch=exact_match`, `proxyResolution.isProxy=false`, verified at `2026-04-01T02:07:13Z`; the runtime hash independently matched the same-block RPC bytecode (`51caba906ac5b816aaf91fb17bfa6cfe3d84211e2ea1af39bc2bbdf9ac8d8373`). The raw response is 1,463,640 bytes with SHA-256 `3431fd542110740f29b49e14b0d16274e595d48c79c1ad693342599382074409`.

All six token contracts have the same 45-byte minimal-proxy runtime, with the EIP-1167 delegate target fixed to that implementation address. No token-specific implementation or mutable proxy-admin slot is used by this bytecode. The [EIP-1167 specification](https://eips.ethereum.org/EIPS/eip-1167) describes this fixed-target clone pattern.

The exact matched source bundle is preserved at `.scout-data/diagnostics/base-zora-ath-source-audit-2026-10-11/source-files.json`; the complete Sourcify response and per-file hashes are beside it. Relevant source evidence:

- `node_modules/@zoralabs/coins/src/BaseCoin.sol`: `initialize` is guarded by OpenZeppelin's `initializer` and calls `_handleInitialDistribution` once (lines 122–190); `burn(amount)` burns only `msg.sender` (lines 199–204). `_update` calls the inherited ERC-20 update before emitting the additional `CoinTransfer` event (lines 284–289).
- `node_modules/@zoralabs/coins/src/ContentCoin.sol`: `_handleInitialDistribution` mints `CoinConstants.MAX_TOTAL_SUPPLY` once (lines 53–64).
- `node_modules/@zoralabs/coins/src/libs/CoinConstants.sol`: `MAX_TOTAL_SUPPLY = 1_000_000_000e18` (line 22).
- The matched OpenZeppelin `ERC20Upgradeable.sol` in that bundle implements mint/burn through the ERC-20 update path, which emits the standard `Transfer` event. Because `BaseCoin` permits burns, the zero-address burn-log audit is necessary; “fixed cap” alone would not prove constant realized supply.

## Read-only chain evidence

The RPC audit verified `eth_chainId = 8453` and the hash of block `52,456,236` (`0x021113f4f7d0a2fe23e0036948b341ef7bbe5c75a604eb56f96c8a873d5e39b8`) before token reads. At that same block it made one `eth_getCode`, `totalSupply`, and `decimals` read per token, two `eth_getLogs` calls for zero-address `Transfer` mint/burn filters over the six-address array, plus one implementation `eth_getCode` read: 23 logical RPC calls in total. Every successful raw response and its exact parameters are retained in the private manifest; no RPC URL or credential is recorded.

| Token | Launch block | Birdeye window high (USD) | High day | Returned bars | Same-block supply |
|---|---:|---:|---|---:|---:|
| `0x010e1320…80ebd3` | 52,357,520 | `0.000000000019872339620321878` | 2026-10-09 | 1 | 1,000,000,000 |
| `0x0261c9f8…74ea6a` | 52,357,594 | `2689264895.1770654` | 2026-10-09 | 1 | 1,000,000,000 |
| `0x058e72b9…9ac738` | 51,712,311 | `0.0014819050096223831` | 2026-09-27 | 3 | 1,000,000,000 |
| `0x101087a0…7248eff` | 51,745,697 | `0.0039335034268084485` | 2026-09-26 | 4 | 1,000,000,000 |
| `0x065a4fed…520d9a4` | 51,922,047 | `0.000014943220541173769` | 2026-09-29 | 2 | 1,000,000,000 |
| `0x109585ed…70c9e474` | 51,709,866 | `0.0000008111506335367991` | 2026-09-24 | 2 | 1,000,000,000 |

Every returned mint log is from the zero address, to the token itself, at the recorded launch block, and has raw amount `1000000000000000000000000000`, exactly matching same-block `totalSupply`. The batched burn filter returned zero logs. The range begins at the earliest of the six launch blocks (`51,709,866`) and ends at the pinned block. The source’s standard ERC-20 update path makes these standard `Transfer` events the supply mutation evidence for this implementation.

## Evidence files and remaining gaps

The private evidence directory is `.scout-data/diagnostics/base-zora-ath-source-audit-2026-10-11/`:

- `manifest.json` — candidate input hash, pinned chain/block, 23 RPC calls, implementation linkage, Sourcify result, per-token supply proof, and the explicit scope boundary.
- `raw/` — individual raw RPC responses and SHA-256 values.
- `sourcify-implementation.json`, `sourcify-summary.json`, and `source-files.json` — raw verification response, compact verification summary, and extracted matched source bundle.

Remaining gaps are price-side: Birdeye returned only one to four daily bars; the displayed high was not checked against GeckoTerminal or another pool source; and the complete historical pool/migration universe was not established. A supported denominator does not convert these highs into canonical or lifetime ATH values.

## Independent price cross-check (GeckoTerminal)

A separate unauthenticated GeckoTerminal pass queried the page-1 token-pool listing for each of the six source-proven tokens and every matching returned pool (all four indexed tokens returned exactly one; two listings were empty). It then requested daily USD OHLCV for each of the four listed pools. All 10 GETs returned HTTP 200, the minimum request-start gap was 15 seconds, response hashes verify, all four OHLCV responses had nonempty in-window candles and exact base/quote identity, and none of the six listings returned a next-page link. The exact URLs, params, raw bodies, candle rows, and comparisons are preserved in `.scout-data/diagnostics/geckoterminal-base-zora-six-crosscheck-2026-10-11/manifest.json`.

| Token | Birdeye observed-window high | Birdeye peak day | GeckoTerminal same-day high | Same-day and observed-pool max difference | Result |
|---|---:|---|---:|---:|---|
| `0x010e1320…80ebd3` | `0.000000000019872339620321878` | 2026-10-09 | no indexed pool | N/A | unresolved: empty page-1 listing |
| `0x0261c9f8…74ea6a` | `2689264895.1770654` | 2026-10-09 | no indexed pool | N/A | unresolved: empty page-1 listing; price outlier remains unvalidated |
| `0x058e72b9…9ac738` | `0.0014819050096223831` | 2026-09-27 | `0.00134982425679485` | `-8.912903%` | disagreement |
| `0x101087a0…7248eff` | `0.0039335034268084485` | 2026-09-26 | `0.00259858926544851` | `-33.937028%` | disagreement |
| `0x065a4fed…520d9a4` | `0.000014943220541173769` | 2026-09-29 | `0.0000148393140117083` | `-0.695342%` | one observed-window cross-check candidate |
| `0x109585ed…70c9e474` | `0.0000008111506335367991` | 2026-09-24 | `0.000000054835582895948` | `-93.239778%` | disagreement |

For each listed token the same-day maximum and maximum across the known pool's in-window bars are the same because only one matching page-1 pool was returned. The [aligned candidate file](../../../.scout-data/diagnostics/geckoterminal-base-zora-six-crosscheck-2026-10-11/aligned-candidates.json) contains the sole within-1% case. This is a price-source agreement for the observed window and listed pool only: no next page was advertised, but provider pool discovery, alternate venues, earlier pools, and migration coverage are not proven complete. The two empty listings and three price disagreements remain unresolved. No FDV/ATH database writes were made.

## Quote-asset identity for the three price disagreements

The cached GeckoTerminal listings identify the pairs as DVA/MATRY, TWILIGHT/SUNLIT, and QSNAG/SNAGETH. The names are provider labels; the address and runtime checks are the identity evidence. At the pinned Base block, the exact quote addresses `0x364863eece7fd2e6c5f3dc13a3f043521f77ec49`, `0x3a3a6b2e1624c66e470460da92e86246ef2ece48`, and `0x3c725423e2cad7239ef3c016a19f7a2d1c4aac37` all have the same 45-byte EIP-1167 runtime as the audited Zora coins. Each runtime hardcodes implementation `0x5dbd43785954d43c1643a0caf2ecef9e0056ff13`, whose Sourcify source/runtime is an exact match for Zora `ContentCoin` (see the source proof above). These are source-matched Zora ContentCoin clones, not WETH, USDC, or a verified ZORA/USD instrument.

I also checked the pool identity rather than inferring it from the 64-byte provider pool IDs. One bounded `eth_getLogs` query at the Base Uniswap v4 PoolManager `0x498581ff718922c3f8e6a244956af099b2652b2b`, filtered by the repository's `Initialize` topic and the three exact pool IDs, returned all three initialization events. Their emitted `currency0`/`currency1` addresses exactly match the cached GeckoTerminal pairs; each has dynamic-fee marker `0x800000`, tick spacing 200, and hook `0x0469a4bd3724dc86c9542f4694c976da13c450c0`. The registry address is present in `crates/scout-dex-evm/src/gate.rs`; Base v4's deployment and fixture verification are documented in [the Base venue verification](2026-10-04-base-venues-verification.md). The event signature and ABI decoder are pinned in `crates/scout-dex-evm/src/uniswap.rs` (`V4_INITIALIZE_TOPIC0`, `decode_v4_initialize`).

Because these quotes are other ContentCoins and no exact, independent USD price instrument for those quote addresses was established, I stopped before querying swap receipts or market FX. No symbol-only conversion, ZORA/USDT price, or `ZORA` ticker substitution is used. Therefore the three prior Birdeye-versus-GeckoTerminal disagreements remain unresolved; this work does not corroborate an alternate price within 1%.

The separate [quote identity manifest](../../../.scout-data/diagnostics/base-zora-quote-asset-audit-2026-10-11/manifest.json) preserves the six read-only RPC requests, exact filters, raw response hashes, quote runtimes, and decoded Initialize events. No swap-log, receipt, FX, Birdeye, GeckoTerminal, or Codex calls were made for this check; no database/FDV rows were changed.
