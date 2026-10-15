# Solana five low-burn peak price corroboration

Read-only evidence as of 2026-10-11. This audit tests whether historical PumpSwap state independently corroborates the provider-reported price level near each peak. It does not claim exact millisecond pricing, provider-window completeness, canonical ATH time, wallet attribution, or realized PnL. No database rows were written.

## Method

Five cached PumpSwap pools were identity-checked against target mint and WSOL using one `getMultipleAccounts` response. Successful swap receipts in bounded ±3-second histories were decoded using the pinned Pump AMM IDL commit `2293f9a66c654e9fe82dc5e8f4618538f24bb35f` and the repository event/account/swap-math modules. Event pre-reserves matched actual transaction pre-vault balances. Since these legacy event layouts omit `virtual_quote_reserves`, effective quote reserves were inferred as exact integer intervals from the event variant and CPMM rounding; the current account field was not treated as historical state. Coinbase SOL-USD one-minute low and high were applied to both interval endpoints, and both ends had to fall within 1% of the provider peak.

## Results

| Token | Provider peak UTC / USD per token | Selected receipt state | Dated price interval USD/token | Result |
|---|---|---|---|---|
| 5b2 (`5b2BsBEVvsjBegVaKik9vByFJ8YGcDbk8R3A96RTpump`) | 2026-03-14T21:07:16.990000+00:00 / 12133750413201461/100000000000000000000 | post state, 1773522435 UTC second, slot 406434614 tx index 596 log 60 | 0.000121292252271056–0.000121320161748501 | PASS |
| 7DGZ (`7DGZSvNASBwjVZbFL9UwGtimdgitQu7Pq6RVb23pump`) | 2026-01-08T15:51:09.699000+00:00 / 59125282926240427/1000000000000000000000 | pre state, 1767887469 UTC second, slot 392161377 tx index 988 log 39 | 5.91236499639654e-05–5.91759216691619e-05 | PASS |
| 8bp3 (`8bp3yea4ZyT8KkHJQAQCHEMC1AhRHx7bYQfJD2uye6Bd`) | 2026-02-05T00:51:05.146000+00:00 / 3808203647341843/50000000000000000000 | pre state, 1770252665 UTC second, slot 398115049 tx index 521 log 39 | 5.2233651806365e-05–5.23475498743153e-05 | GAP |
| jKWm (`jKWmQJgLfTRp2VeXHKtGK9DxJReBuuocus7ZU6Kpump`) | 2026-02-25T00:28:33.268000+00:00 / 1294752145935567/40000000000000000000 | pre state, 1771979313 UTC second, slot 402515541 tx index 13 log 37 | 1.17171165260779e-05–1.17260438745028e-05 | GAP |
| 9dZk (`9dZkVHy3byQvSxw7h7gk5tScp9zfg8rizYDKq9XSpump`) | 2026-01-25T11:26:50.559000+00:00 / 8220527118895571/100000000000000000000 | post state, 1769340410 UTC second, slot 395826048 tx index 792 log 37 | 6.39846455490349e-05–6.39998042277988e-05 | GAP |

5b2 uses the successful post-state of its latest Swap at 21:07:15Z, 1.990 seconds before the provider timestamp 21:07:16.990Z. Its event reports gross quote input 227,907,305, quote input with LP fee 225,655,255, user quote input 225,204,845, LP fee 450,410, protocol fee 112,603, and creator fee 2,139,447; fee components reconcile exactly to gross input, and the quote-vault delta is 225,655,255. The legacy `buy_exact_quote_in` reserve equation is evaluated for both `q=user_quote_amount_in` and `q=user_quote_amount_in−1` to avoid assuming the one-lamport convention: the union of integer effective-quote bounds is 157,497,770,361–157,497,771,060 raw lamports (width 699), not a uniquely known reserve or a direct historical virtual-reserve field. Propagating that interval over the observed vault delta keeps both USD endpoints within 1% of the provider peak. Coinbase minute low/high were $86.92/$86.94 per SOL. The resulting token-price endpoints differ from the provider peak by −0.037295% and −0.014293%.

7DGZ uses the highest pre/post verified state among 16 successful swaps in the exact provider whole-second (15:51:09Z). The selected state is a SELL pre-state at slot 392161377, transaction index 988, log 39, signature `2s2B5L4oz5K8tvySLq6VZUEY6sEzbCDTpfoUDon1opWCMo6giopUVvLzoUPQLYfH2DGpyMLXJ7KmRZuiunc71r3h`. Its effective quote reserve interval is 87,520,405,888–87,520,405,894 raw lamports (width 6). Coinbase minute low/high were $135.73/$135.85 per SOL; the converted endpoints differ from provider peak by −0.002762% and +0.085647%. This is same-second level corroboration only; the provider timestamp has millisecond precision, while the chain events provide whole-second block time.

The other three remain price gaps under the unchanged 1% criterion. Their highest verified event states in the provider peak whole-second are 8bp3: $0.0000523475498712/token (31.27% below); jKWm: $0.0000117260438745/token (63.77% below); and 9dZk: $0.0000639998042278/token (22.15% below). Matching maxima found elsewhere in the ±3-second diagnostic window for 8bp3 and jKWm are not used because they are not timestamp-adjacent to the provider’s reported whole-second peak. No tolerance or timestamp window was widened.

## Supply and coverage

The supply audit derives each peak supply as birth raw supply minus the reconstructed burn amount at or before that provider timestamp. 5b2, 7DGZ, and jKWm had no burns by the peak, so their peak supply is 1,000,000,000 tokens. 8bp3 and 9dZk each had one raw-unit burn by the peak, so each peak raw supply is 999,999,999,999,999 (decimals 6), distinct from the lower current snapshot supply. For every token, reconstructed burn sums equal birth-minus-current supply at the finalized snapshot.

The bounded RPC work used 15 logical calls (one account identity call and 14 pool-history/continuation calls) of the 25-call cap, and five Coinbase candle GETs of the five-call cap. All saved response receipts indicate HTTP 200, no automatic retries, and bodies within the 2 MiB limit. A provenance limitation remains: the original JSON-RPC request bodies for pool history were not retained. Response receipts preserve pool, window/page context, status, response size, and SHA-256; the exact request body was not fabricated or replayed.

Private raw evidence and normalized results: `.scout-data/diagnostics/solana-five-low-burn-price-corroboration-2026-10-11/`. Final normalized rows are in `final-eligibility-analysis.json`; call receipts, raw hashes, source pins, and limitations are in `final-audit-manifest.json`.
