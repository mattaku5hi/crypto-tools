# Fht selected-pool maximum — 2026-10-14

The cached FhtWtm6M window has two successful buys and four actual vault
PRE/POST states. Its maximum is708ms before the saved provider timestamp.
The older strict-neighbor diagnostic rejected it because there was no later
trade; the complete terminal window satisfies the global maximum gate from
ADR-021 amendments40–42. A second trade after the provider timestamp is not
needed to establish that captured-window maximum.

Provider price is$0.001339448811338171 per token. Verified1B birth issuance
and later current supply999996024797032raw at6decimals give a provider-price
supply interval$1,339,443.4846…–$1,339,448.8113…. The conservative lower
reference is$1,339,443.48. All four real-vault spot states are independently
multiplied by raw Coinbase SOL/USD minute low/high; the global endpoints
agree within0.025% of the provider reference. The combined stored-scalar
gate includes FX, supply uncertainty and cent rounding without stacking
separate2% allowances. Exact peak supply and a full burn ledger are not claimed.

Both instruction wire shapes and complete BuyEvent payloads match the
February17 IDL, before the February18 price peak. Pool, mint, vaults, user,
authority, event-CPI parent and raw reserve deltas reconcile. Birth issuance
and ordered authority revocation are independently checked from actual SPL
bytes; the current mint has only metadata-pointer/token-metadata extensions.
Its original request includes duplicate unrelated addresses, but Fht occurs
once and its exact index/account bytes are verified.

Root certificate SHA-256:
`abdb98ca0674c9f2b04fc3eabb3c0d1d95f3b3341a0f8ee3dd64218b792bcd33`.
Corrected dated-IDL replay:
`3c117a33dc61b99f6034a712fa385690ea31599eed79f8cce1fddd5fced0553e`.
Two finalized standard transaction reads exactly match the original birth
and winning-swap captures; manifest:
`aa5e97995587ae83a97ccdeeda9b225a3c8169c42a9c6cd5984043c466596030`.
The price/supply proof uses cached data; only those two new Helius methods
were used in confirmation, without retries or new Tracker/Codex/FX calls.

The source remains
`solana-tracker-window-supply-lower-bound-2pct-pool-fx-validated-v1`,
with null ATH date and preserved holder/liquidity values. Saved holders12
remain below the50-holder runner gate. Neither a list-membership change nor
annual/all-market ATH completeness is inferred from this value.

The separate6SFB/7rWY windows have acceptable global price maxima, but18/25
and29/41 trade instructions have unresolved byte/account variants. They
remain unpromoted. The earlier offline alias pointed at January9 rather
than November15; its immutable proof is retained and corrected evidence
uses the actual November15 IDL. An audit of previously accepted older
windows finds their dated-IDL references correct.

Golden: `solana_tracker_fht_conservative_fdv_bound_case.json`, SHA-256
`3c8cdf1ac1250e2b266f68738ea3b0e201c8a6a85d632d2681baf8674ae58537`.
It extends the existing exact lower-bound regression with actual vault states,
dated wire/event bytes and raw capture hashes. Full workspace fmt, all-target/
all-feature Clippy and1234tests passed in47.56seconds against isolated
scout_test. A subsequent guarded transaction wrote the lower floor and
verified unchanged holder/liquidity and null ATH date. Fixedcohort512numeric/
167unknown,78closures since434. Main3 certificate-v4 capture-hash regression
passes in the same checks; its monetary results remain unchanged.

Changed files: shared birdeye.rs regression, new Fht golden, corrected main3
golden/provenance, ADR-021, this and central measurement, tickets and source
research. No production binary/adapter or runner-gate change.
Next task B8.4: remaining instruction-schema, supply and price-history gaps.
