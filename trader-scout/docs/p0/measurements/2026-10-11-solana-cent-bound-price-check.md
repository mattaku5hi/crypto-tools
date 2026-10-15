# Solana cent-bound provider price checks — 2026-10-11 (v2)

## Result

Neither of the two Solana Tracker observations with equal-cent supply bounds is ready for source-price promotion. The added same-second reads complete all nine successful 8Cc pool signatures, but their event payloads omit `virtual_quote_reserves`; the historical deployed layout at those slots has not been independently established. Without that reserve value, the effective quote reserve and event-time spot mark remain unknown. The decoded trade execution averages are not spot marks and are not used to reject the provider price.

An independent Birdeye daily USD window did return for 8Cc. Its maximum high was `$0.000013419091705273695` on 2026-02-21 UTC, the same UTC date as the Solana Tracker peak, but 72.016% below the ST value `$0.000047952524582282125`. It is not within the existing 1% agreement threshold. Birdeye returned 232 exact-address, `1D`, USD candles, with `isScaledUiToken=false` and no multiplier. The PEC request was rate-limited with HTTP 429 and returned no price data; no retry or further Birdeye request was made.

| Token | Solana Tracker peak | Birdeye year-window high | Birdeye vs ST | Same-second event execution average range (diagnostic only) |
|---|---|---:|---:|---:|
| `8CcYvi…Vpump` | $0.000047952524582282125 at 2026-02-21 20:01:52.806Z | $0.000013419091705273695 on 2026-02-21 UTC | −72.016% | $0.00000278415–$0.00000366861 |
| `PECazc…3km` | $0.00006565114747464346 at 2025-11-09 03:48:35.868Z | unavailable (Birdeye HTTP 429) | unavailable | $0.00001791660–$0.00001889702 from three fetched events |

Event execution ranges use the exact minute's Coinbase SOL-USD low/high: $86.09–$86.14 for 8Cc and $155.66–$155.86 for PECaz. Buys use `quote_amount_in_with_lp_fee + protocol_fee + coin_creator_fee`; sells use `quote_amount_out_without_lp_fee - protocol_fee - coin_creator_fee`. These are trade averages, not spot-price measurements. Complete decoded fields and raw references are in `onchain-price-comparison-v2.json`.

## Evidence scope

- The 8Cc pool signature page returned 304 rows and included nine successful transactions in the reported peak second. All nine were fetched and each contains one matching event for the exact confirmed target/WSOL pool. Three transactions were at slot 401817025, four at 401817024, and two at 401817023. Because no block transaction-index query was made, relative ordering among transactions in each slot is unknown; the sidecar retains the signature-page order only.
- All nine 8Cc Buy/Sell events match the exact pool key. Their field prefix decodes through `cashback`, with no appended `virtual_quote_reserves` field. The current official reserve documentation describes effective quote reserve as vault quote amount plus signed virtual reserves, but the exact deployed event version at these earlier timestamps is not pinned here. No missing reserve value is assumed to be zero.
- PEC retains the prior three decoded same-second event samples. Its first signature page was capped at 1,000 rows; one authorized before-cursor page reached the peak second, but complete pool-window event coverage was not established.
- GeckoTerminal metadata succeeded for both pools. Its 8Cc daily OHLC request returned HTTP 401 because public history is restricted to the past 180 days. No retry or further GeckoTerminal request was made.
- Birdeye's 8Cc daily request covered `[2025-10-10T00:00:00Z, 2026-10-11T00:00:00Z)` and returned candles from 2026-02-21 through 2026-10-10. The PEC response body states “Too many requests” with HTTP 429; it is preserved, and the request was not retried.
- Solana Tracker history scope remains `provider-recording-window-unknown/not-ATH-FDV`. A Birdeye cross-check is available for 8Cc but disagrees materially; PEC lacks the second provider result. Neither token is promoted by this audit, and no ATH/FDV database writes were made.

The archived v1 report and sidecars are under `.scout-data/diagnostics/solana-two-cent-bound-pools-price-validation-2026-10-11/v1/`. The v2 manifest records each bounded request and raw response hash.

## Subsequent resolution

The v2 findings above describe the earlier sample and do not include the
later migration-state audit. Subsequent exact CreatePool/early-Buy receipts
corroborated both vendor points within 1% at both dated Coinbase endpoints.
After supply bounds, source review and workspace checks, 8Cc and PEC were
guarded-written at 4,795,252 and 6,565,114 cents under ADR-021 amendment19.
The contradictory Birdeye window and timing/coverage gaps remain preserved.
See [8Cc](2026-10-11-solana-8cc-migration-pool-spot-corroboration.md) and
[PEC](2026-10-11-solana-pec-migration-spot-corroboration.md); this resolution
does not claim lifetime or complete market coverage.
