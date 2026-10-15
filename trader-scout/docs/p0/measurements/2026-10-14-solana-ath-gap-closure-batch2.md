# Solana historical ATH gap closure — batch 2

Read-only validation against the frozen 2026-10-13 unknown-priority cohort, using the cached Solana Tracker peak observations and the bounded local supply audit. The task selected ten not-yet-checked exact WSOL PumpSwap pools with comparatively narrow birth/current-supply FDV widths. The price protocol was fixed before review: within ±3 seconds of each provider timestamp, choose the nearest preceding successful pool swap and the first following successful pool swap by time/order; use the marginal pool spot from the event's pre-swap vault reserves and same-minute Coinbase SOL/USD low/high. Both FX endpoints must remain within 2% of the recorded ST price. No threshold was widened. No DB writes were performed.

| Mint prefix | Nearest prior price envelope vs ST | First following envelope vs ST | Result |
|---|---:|---:|---|
| 88pqb | −1.645% to −1.396% | −0.109% to +0.144% | Pass |
| 6nV9 | −11.320% to −11.299% | −11.604% to −11.584% | Fail |
| EuW3 | −0.393% to −0.291% | −7.842% to −7.748% | Fail |
| F2Rwc | −93.751% to −93.749% | −94.015% to −94.013% | Fail |
| gLTR | −3.633% to −3.589% | −55.424% to −55.404% | Fail |
| 4CX5 | +2.964% to +2.993% | −64.185% to −64.175% | Fail |
| BNrJ | −35.905% to −35.895% | −35.131% to −35.121% | Fail |
| GP7H | −0.474% to −0.157% | −3.927% to −3.620% | Fail |
| NDt3 | −11.410% to −11.210% | −13.131% to −12.935% | Fail |
| Den6 | −1.980% to −1.791% | −7.957% to −7.780% | Fail |

For all ten, finalized PumpSwap pool-account bytes decoded to the exact ST base mint and WSOL quote mint. Pool and event reserve identities were checked before comparison. The nine failures are price-window failures under the fixed two-sided gate, not evidence against lifetime price.

## 88pqb exact provider-window FDV candidate

Solana Tracker recorded a provider-window maximum price of `$0.00000960624821632361` at `2026-04-10T00:38:40.246Z`. The selected pool's closest preceding successful Buy is at `00:38:39Z` (slot 412180753, transaction index 977); the first following successful Sell is at `00:38:42Z` (slot 412180760, transaction index 373). No selected-pool swap appears in the provider's whole second. The event reserve fields on both brackets match the pool-owned pre-token-balance values exactly. Their real reserve marginal spots, converted with the Coinbase `00:38Z` SOL/USD minute low/high of `$82.94–$83.15`, are within 2% at both endpoints. This brackets the observed price level; it does not resolve an exact pool state at the provider's millisecond.

The token's validated birth receipt at `00:35:32Z` minted `2,000,000,000,000,000` raw units (six decimals) and set mint authority to `None`. The current finalized Token-2022 mint has no mint or freeze authority and only MetadataPointer/TokenMetadata extensions; no MintCloseAuthority extension is present. In the bounded successful interval `[1775781332,1775781521)`, 191 transaction signatures from the full mint-address signature index exactly match the 191 full transactions returned by the time-filtered address query. The signature index is terminal at 593 records and reaches the birth signature. Decoding the interval found no post-birth mint, no burn through the provider peak second, and no mint-close-authority initialization. Peak supply is therefore exactly birth supply. The ST price times peak supply floors to `1,921,249` cents; upper and lower supply-bound products have the same cent floor. This is a maximum within ST's recorded price window only; it does not establish lifetime or all-market ATH.

The provider also reports market cap `$9,606.25`; at the recorded price that corresponds to a one-billion-token basis. The validated mint supply is approximately two billion tokens, producing total-supply FDV `$19,212.49` and the proposed `1,921,249`-cent value. The provider market-cap field is retained as a distinct value and is not used as the FDV input.

Correction to the original layout interpretation: the final 16 bytes of the selected events are not `virtual_quote_reserves`. The full 439-byte BuyEvent and 376-byte SellEvent decode under the official 2026-02-17 IDL, commit `82dacacf15` (SHA-256 `5a15060f412974e53068bae7e89aa6004defbb70ef0c56e3902ce75d124accb6`); those bytes are the `cashback_fee_basis_points` and `cashback` fields, both zero. The selected calculation uses the actual pre-swap real quote-vault balance, and the previous suffix-as-VQ interpretation is withdrawn. The dated IDL does not prove the deployed binary activation date and does not establish a general missing-field default. Curve, fee, and pool-vault deltas still reconcile, and event amounts are not used as spot-price proxies. The source is reclassified as `solana-tracker-window-peak-supply-legacy-pool-fx-validated-v1`; this is a manual review, not an automatic source adapter.

Candidate: `.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch2/88pqb-candidate.json`, SHA-256 `ff7c37ef3c72ebac3af380f964de6eac86cf6724213040904756ec61fe8b82e4`. It indexes 39 evidence files with matching byte counts and hashes. The batch used 28/120 authorized Helius methods and 10/15 Coinbase requests, with no retries and response bodies below 2 MiB. No DB writes were made.
