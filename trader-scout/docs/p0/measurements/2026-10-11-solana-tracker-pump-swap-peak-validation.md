# Solana Tracker ATH versus cached PumpSwap transactions — 2026-10-11

Correction: the original private reserve-ratio calculation omitted event `virtual_quote_reserves`; its raw evidence and original report are preserved as `report-captured-v1.md` in the diagnostic directory. Incorrect spot numbers have been removed here. The [v2 reconciliation](2026-10-11-solana-virtual-reserve-price-reconciliation.md) and [v3 state reconstruction](2026-10-11-solana-virtual-reserve-price-reconciliation-v3.md) supersede those calculations, separate price kinds and state boundaries, and qualify the two `BoostBuyAndBurn` operations. No ATH/FDV write used the erroneous prices; the production event decoder already preserves the virtual-reserve fields.

This is an offline comparison of three cached Solana Tracker ATH responses with existing Helius transaction archives. No new API/RPC calls were made. Raw extracted event evidence and source hashes are in [`evidence.json`](../../../.scout-data/diagnostics/solana-tracker-pump-swap-peak-validation-2026-10-11/evidence.json) with [`manifest.json`](../../../.scout-data/diagnostics/solana-tracker-pump-swap-peak-validation-2026-10-11/manifest.json). Prices use exact decimal arithmetic; no wallet attribution or hypothetical fee adjustment is applied.

Event decoding follows the checked PumpSwap `BuyEvent`/`SellEvent` layout in [`pump_amm_event.rs`](../../../crates/scout-dex-solana/src/pump_amm_event.rs) and the pinned [`PumpSwap IDL fixture`](fixtures/pump_amm_idl_e0687ae9.json). Pool account keys, event pool pubkeys, target/quote mint token balances, and the cached deployed PumpSwap program identity agree.

## 48vcic…

The provider reports a high of `$0.00013242878993078837` at `2026-10-10 22:04:55.327Z` in pool `wc4hQ7k6mZe1hLLfkARE7gDjK6toJDpY2YCPKj8UEr5`. A cached transaction in that pool at integer `blockTime=1791669895` (`3Uv1…Fp6k`) is a nearby sell candidate; the provider’s millisecond timestamp does not bind it to that chain transaction. A full cached `22:04:52–22:04:58Z` scan is recorded in [the 48-token neighborhood report](2026-10-11-solana-48-st-peak-neighborhood.md). It sells `2,840,079.914852` target tokens for `197,055.637706` BzLq. The same transaction then sells that exact BzLq output for `3.243801772` wSOL in the BzLq/wSOL pool. Thus the quote path is on-chain observable as target → BzLq → wSOL; it is not a direct SOL quote.

Using the cached 22:04 UTC SOL/USD minute range (`$110.14–$110.23`, Coinbase and Kraken), the realized two-swap output is `$0.0001257966–$0.0001258994` per target token. That is 3.28–3.37% above the cached Birdeye one-second and 1D highs (both `$0.00012179988071982598`; the 1D candle bucket starts at `2026-10-10T00:00:00Z`) and 4.93–5.01% below the Solana Tracker high. The corrected transaction-pre-state compounded spot is `$0.000129143–$0.000129249`, 2.40–2.48% below Tracker. Post-event virtual state is not archived, so post-vault balances alone do not prove a post-state spot. Neither the executed route nor the supported pre-state spot corroborates the provider value within the existing 1% check.

This establishes a real nearby transaction path and USD conversion, not that the tracker high was executable at that price, nor complete pool coverage or an ATH/FDV write authorization.

V3 subsequently reconstructed intermediate/post-event virtual state from documented retained-fee transitions and verified the resulting vault balances. Of18 coherent states across six cached routes, the closest is `LLt…` after target log55, `$0.0001291558–$0.0001292613`: still2.39–2.47% below Tracker. The missing literal post-virtual snapshot was resolved for this bounded sample through reconstruction, rather than by using stale virtual reserves with post balances.

## 81kia… and JCd8…

Both provider-reported pools match cached PumpSwap events, but their quote mint is `87eaXHno6VtRXUyuYpSVm8oYpSpHjLMQE9mvbcnKpump`, not SOL or a verified USD stable asset. The closest cached operations are `BoostBuyAndBurn`, one integer `blockTime` second before the provider timestamps: tokens are bought for burning, so these are not evidence of user acquisition.

| Mint | Tracker timestamp / price | Nearby buy-and-burn | Exact input ratio |
|---|---|---|---:|
| 81kia… | 22:45:47.033Z / `$0.00009431116993278875` | 22:45:46, `eqEm…Koz` | 37,201.767694 quote / 809,659.974884 target = 0.04594739625 quote/target |
| JCd8… | 22:44:14.212Z / `$0.00023536979981208752` | 22:44:13, `4Qnx…VNuq` | 135,208.567767 quote / 1,188,170.011339 target = 0.11379564076 quote/target |

Dividing each provider’s reported USD token price by the corresponding on-chain quote/target ratio would imply quote-token values of `$0.00205259` and `$0.00206836`. The Birdeye windows instead imply `$0.00021608` and `$0.00007709`, respectively. Those are derived implications, not independent valuations of `87ea…`; the stored evidence has no dated USD conversion for this quote asset. The discrepancies are about 9.50× and 26.83×, so neither token is validated by these transactions.

The frozen checkpoint [`birdeye-runner-potential-fdv-reference-2026-10-11.json`](../../../.scout-data/diagnostics/birdeye-runner-potential-fdv-reference-2026-10-11.json) records the exact same Birdeye 1D USD high as the one-second high for each mint (one daily candle returned per mint; its hash and matching rows are included in the proof JSON). This is an archived DB-window checkpoint, not a raw provider response. The independent read-only DB reread [`solana-birdeye-daily-vs-tracker-2026-10-11.json`](../../../.scout-data/diagnostics/solana-birdeye-daily-vs-tracker-2026-10-11.json) confirms the same three rows (zero provider calls, zero DB writes). Sparse one-second candle count cannot explain the 81kia/JCd8 gap; the mismatch also appears against the saved 1D maxima.

## Result

This initial cached comparison identifies the quote assets but supplies no1% price corroboration on its own. Later work changes that price conclusion: the [JC bracket addendum](2026-10-11-solana-quote87-jc-bracket.md) version-decodes a successful, same-pool JC→87ea→wSOL route and corroborates the price level within0.44%; the [81 nearby spot comparison](2026-10-11-solana-81-nearby-spot-comparison.md) gives a numerical match with an explicit separate-pool timing/direct-vault-transfer gap. Production instruction coverage is unchanged. All three FDV maxima remain unpromoted: a price maximum times later supply is not established as the historical FDV maximum when burns change supply. These checks do not change the existing FDV definition or authorize promoting `highest_market_cap` blindly.
