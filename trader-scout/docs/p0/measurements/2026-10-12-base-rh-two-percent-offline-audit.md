# Cached Base/RH price-disagreement audit at 2% — 2026-10-12

This is a read-only follow-up using only saved evidence. The user authorized a ±2% comparison tolerance; the previous ±1% decision for 92d958 is superseded for this comparison. No price-kind, UTC-day, token-identity, supply, or observed-pool-scope rule was relaxed. This audit does not establish lifetime ATH or complete market coverage.

## First Base candidate: 92d958

For Base token `0x92d95883e79a695ca4afa40d4885b4ef4ac38b07`, the cached source has eight returned PoolManager pools, all eight initialized pools were Swap-scanned from initialization through the common Base anchor (block 52,456,236; hash `0x021113f4…d5e39b8`), and all eight have exact target-as-base USD GeckoTerminal histories. The full per-pool identities, history row counts/highs, Swap counts, and response hashes are in the accompanying [private audit JSON](../../../.scout-data/diagnostics/base-rh-two-percent-offline-audit-2026-10-12/audit.json).

The global observed USDC execution maximum is the receipt/header-verified event in pool `0x23d7f4…d135e736`: signed raw deltas are USDC `-45` (6 decimals) and token `+4,397,783,988,515,533,135` (18 decimals), block 45,500,972, transaction `0x83e23b…306f66e83`, log 365. Its exact quote/token fraction is `9,000,000,000,000 / 879,556,797,703,106,627` USDC/token. The peak falls on 2026-05-03, the same UTC day as Birdeye's `$0.000010050423098518516` and the eight-pool GeckoTerminal maximum `$0.0000100542474772537`.

Kraken's cached May 3 USDC/USD daily candle is low `0.9997`, high `0.9999` (raw candle indices 3 and 2). Exact multiplication gives `$0.00001022935644803808126496604620547071957…` to `$0.00001023140293327325943466995058602598029…` per token. Those endpoints are respectively **+1.74164174077% to +1.76199617545%** against the same-day GeckoTerminal high, and **+1.78035638665% to +1.80071856658%** against Birdeye, so both source comparisons fit the newly authorized ±2% threshold. This is dated daily FX sensitivity, not a minute-level quote.

The eighth pool is native-ETH-quoted. Its complete scan has ten Swap events; each event's signed raw amounts and `blockTimestamp` were decoded, assigned to its UTC day, and converted with the cached Kraken ETH/USD daily low/high. The maximum converted high is `$0.00000230529479701279422119598434096189479…` (May 3, block 45,502,641, log 650), about 22.93% of the GeckoTerminal high. The native pool's seven returned OHLC rows max at `$0.00000158747130534945` on May 3. Thus neither native-quoted execution nor its returned candle exceeds the USDC execution maximum.

Four other USDC pools have empty GeckoTerminal history and complete zero-Swap scans (0 logs in each). This is evidence of no observed swaps in those exact pools, not missing values treated as zero prices. The other USDC pools have 26, 157, and 1 returned Swap logs; their observed USD OHLC maxima are below the selected global maximum. GeckoTerminal's maximum and Birdeye comparison are same-day; this does not establish complete chain-wide or lifetime price discovery.

A separate post-swap marginal-spot observation is approximately `$0.00001369370172794236` (about 36.2% above GeckoTerminal). It is preserved as a different price kind. The selected source comparison is explicitly the maximum Swap execution ratio; under ADR-021 amendment 14, marginal spot is reported separately and does not replace or gate that execution measure.

## Other cached Base cases

- `00b0a0…9e3b07`: the expanded seven-pool GeckoTerminal maximum is `$0.0000142097655103013`, about 41.95% above Birdeye. A separately listed unsupported counterasset remains a scope gap.
- `3607f8…63b07`: the maximum USDC execution is `$0.00279998775925045214` on May 4, while GeckoTerminal's maximum is `$0.000388974286395251` on May 5. The larger execution maximum is preserved; the later marginal-spot match cannot stand in for it.
- `5b2a3c…f8cb07`: the maximum USDC execution is `$0.000100000000000000001` on May 2, against a GeckoTerminal maximum `$0.0000723003298273597`; this is not a dust exclusion.
- `fec3a8…f6cb07`: the all-history USDC execution maximum is `$0.0000111604903730992175…` on May 3; GeckoTerminal's maximum `$0.000011166419773745` is on May 4. The date mismatch remains unresolved, and Birdeye differs by more than 2%.

None of those cases enters the ±2% candidate set on the saved exact-day/source-scope checks.

## Cached Robinhood positive-window checks

The five existing WETH launch-pool checks (TST, BANANA, KANAPE, ERZHAN2, TT3) already have exact-minute Coinbase ETH/USD brackets. Both price endpoints are within 0.116% of each token's Birdeye launch-day high. The separate SMOGGY USDG/NVDA nearby-pool checks are within 0.111% using the dated Kraken USDG/USD range. These are positive corroborations for their observed launch/nearby pools, not complete all-pool/lifetime maxima; no new Robinhood source claim is made here.

The saved schema-10 restore manifest still records baseline 679, numeric 430, unknown 249, absent 3. This audit makes no database changes.

## Reproducibility inputs

The private JSON records source paths and SHA-256 values, all ten native event raw signed amounts/timestamps and exact ETH/USD fractions, and all eight 92d958 pool identities/history/Swap counts. Principal inputs are:

- `.scout-data/diagnostics/base-clanker-prior10-onchain-2026-10-11/analysis.json` and `manifest.json`;
- `.scout-data/diagnostics/base-prior10-spot-vs-execution-audit-2026-10-11/manifest.json`;
- `.scout-data/diagnostics/base-92d958-native-pool-completion-2026-10-11/manifest.json` and raw Swap response (SHA-256 `9d4040ce7721b62ccab205c5d3c6e1922406253dbdbbe74db9d938af3bc4cf00`);
- `.scout-data/diagnostics/geckoterminal-poolmanager-ohlcv-2026-10-11/manifest.json`;
- cached Kraken USDC/USD and ETH/USD daily responses; and Robinhood minute-FX audit manifests.
