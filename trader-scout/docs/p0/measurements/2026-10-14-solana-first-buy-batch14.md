# Four further PumpSwap first-buy recoveries — 2026-10-14

The dated full-IDL replay identifies pool-creation events separately from
trades. Exact transaction-local initialization/funding/buy flows fill four
missing pre/post states under ADR-021 amendment42 and the same conservative
combined2% lower-FDV policy as amendments40–41.

| Token | Stored lower FDV USD | Complete pre/post states |
|---|---:|---:|
| `D8uyqvds…` | 44,222.73 | 230 |
| `gLTRxnhz…` | 44,069.19 | 98 |
| `A5Ewn6Xu…` | 40,963.59 | 360 |
| `NDt3PMmQ…` | 34,536.17 | 278 |

All966 states participate in full ±3-second endpoint maxima. Winning states
are also within the −3..0 second gate. No future higher price is filtered
before selecting maxima. Saved provider price times current supply, rounded
down to cents, lies within2% of the unrounded birth-supply upper bound and
both independent global-price/FX/supply endpoints. Exact peak supply, full
burn history and annual/all-market ATH are not asserted. The explicit source
is `solana-tracker-window-supply-lower-bound-2pct-pool-fx-validated-v1`.

Original A5Ewn/NDt3 captures were nonterminal. The final combined cursor chains
are terminal and contain224/158 successful transactions; the old pages and
failed intermediate audit remain preserved. Diagnostic page suffixes on
gLTR/NDt3 do not alter mint identity. The four334-byte CreatePoolEvents
fully decode under the dated IDL and match pool/base mint, event-authority
and create-parent invocation. Separate byte-bound BuyEvents plus ordered
SPL ATA/funding/transfers prove the actual intra-transaction vault states.

Bd556n65’s separate volume-accumulator close is a known non-trade. Its
associated buy/vault deltas reconcile, but the full price maximum still
differs5.9% from the reference; it remains unknown. F4es’s all-zero16-byte
suffix matches a later cashback schema, but the dated pre-peak release
does not declare those fields; the earlier schema gap remains explicit.

Root certificate SHA-256:
`b101c28dc5e41cce6636a3dc509e65491b44a9fe8973b75b04394910df18a0f5`.
Independent private price/binding proof SHA-256:
`33d8e5b4f841fb6f147dccbf294951ee20b4bec732c1250425c5e0fe225d5895`.
Eight standard finalized recaptures independently match messages and
selected metadata for each recovered buy and winning-price transaction.
The phase budget is8Helius methods, no retries or new Tracker/Codex/FX calls.

Golden: `solana_tracker_pumpswap_prebalance_batch14_cases.json`, SHA-256
`ed6cd55e49bcafaefc8819fbad1071b5724034810de3884cae0b4a49a52a6747`.
Shared regression keeps original negative price and malformed binding
controls, and checks the dated CreatePool OptionBool shape explicitly.
Full workspace fmt, all-target/all-feature Clippy and1233tests passed in
45.81seconds against isolated scout_test after adding global winner time
assertions. A separate atomic guarded transaction wrote all four exact lower
floors only after checks; read-back verified source, null dates and unchanged
holders/liquidity. Current fixed679cohort is508numeric/171unknown,74new
values since434baseline. No generic adapter or daemon binary change.

Changed files: shared regression and dated-schema flags, four-case golden,
ADR-021, this and central measurement, tickets and state-checkpoint docs.
Next task B8.4: chronological historical FDV maxima and remaining gaps.
