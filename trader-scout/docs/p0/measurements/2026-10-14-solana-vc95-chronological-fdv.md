# vC95 chronological FDV reference — 2026-10-14

The saved Solana Tracker peak price alone cannot resolve vC95's large supply
interval. Birth issuance is2B tokens, but four burns reduce supply to
1.116992414701072B tokens. We replayed the captured history in canonical
transaction/instruction order and maximized price times contemporaneous
supply over the curve and one selected PumpSwap pool.

The terminal mint-address history has616 successful transactions, with112
failed signature records retained separately. Six actual SPL instructions
account for two birth mints and four burns. Three burns are outer instructions,
so examining only inner instructions would produce an incorrect ledger.
Mint authority is revoked at birth; the net ledger matches the verified
metadata-only current mint. Current supply does not substitute for the
supply assigned to earlier price states.

There are1219 physical marks and1661 endpoint rows: Create,442 curve PRE/POST
pairs with interval-valued PRE quote reserves, and334 selected-pool PRE/POST
vault marks. All35 Coinbase event-minute candles are parsed as exact Decimal
lexemes. Root and an independent worker recomputed every rational USD bound,
all442 integer quote inversions and the ordered supply ledger.

The full captured maximum is$497,125.96 lower floor to$497,905.92 upper
ceiling. Both maxima occur at the selected-pool POST state on2026-04-01
12:43:06UTC,1.330seconds before the saved Tracker timestamp. Combined
uncertainty including cent rounding is below0.157%, inside the2% gate.
Multiplying the saved peak price by current supply instead yields a different
reference ($497,612.32 floored); that scalar is not the selected global FDV.

All442 curve events have exact scoped SPL transfer amounts.233 buys have
scoped user-to-curve System inputs;209 sell outputs use the whole-transaction
curve balance delta and are explicitly labeled that way. One incomplete log
stack is recovered from a unique byte-exact CPI with the correct event
authority and sell-parent boundary. Three finalized standard transaction
reads confirm birth, that recovered event and the winning swap.

A raw-flow audit finds initial curve-account rent of1,468,560lamports and
one extra473,280lamport user-to-curve transfer beyond that transaction's
33,977,059lamport event input. The extra transfer is retained without an
invented fee/Mayhem meaning; it is not folded into event.sol_amount. All209
sell curve balance deltas match event output, with no scoped System curve
flow. The selected pool's334 real-vault balance marks are independently
bound to their actual account-index/mint/owner/decimal rows.

The PRE equation references remain conditional on the dated SDK model.
SDK1.31 declares25-byte buys, including track_volume: OptionBool, and24-byte
sells. The captured25th buy byte is a validated schema-defined Boolean;
52 shorter buys omit that declared field. Those legacy shapes, appended
account roles and Mayhem wrapper semantics are unparsed coverage gaps; the recovered event does not make the original log
stack complete. Selected-pool scope and minute FX are reference observations,
not proof of all-market or lifetime ATH. The upper endpoint below$500k
applies only to that captured model. Existing holder count5 remains below50.
No runner thresholds or generic production fallback change.

Private immutable evidence SHA-256:

- Root certificate: `bb8991d7c5d1687a3bc0feb374a33259a71c1e70b65c9bf0c1b946cba0e56c26`.
- Native endpoint replay: `9b4a180acf84eed0b40c12010f11701d7c2ba410a91454f16da1d625327458b5`.
- Global summary: `f77d6b50da5bd2f93c568573cf98d1c9e7626de090f91ffde1615504150be9bb`.
- Canonical confirmation: `aa0ebd8bdb81f891f96bc5e9b748a5c8f68711fe0eb9056685c1752093860920`.
- Independent raw pool/curve-flow audit: `b73417d3d66a64ab8c5f512a3bac61d4c7fda792a9637cd1ad4eecd4f9e1f485`.
- Corrected dated wire audit: `9ae25d1eeeaf46e0ffda5acf38f37d858b196cb754559523b1b17914e62bd784`.
- Independent audit: `f99bb4dc20efa8101dc1fa6652ebea072115554180f5a3152aaf6468e5f2af52`.

The frozen native replay mislabeled1661 endpoint rows as total reference
marks; the global summary distinguishes1219 physical marks correctly.
Original artifacts remain immutable; later canonical confirmation supersedes
the summary's statement that no standard recapture had yet occurred.

Next task B8.4: remaining price/supply/schema gaps and annual coverage.

Golden: `solana_tracker_vc95_chronological_fdv_window_case.json`, SHA-256
`a402d30980b047e03811c84438d6f427c012aa3ab9e246ea2035b420c5425cc9`.
Full workspace fmt, all-target/all-feature Clippy and1234tests passed
in48.16seconds against isolated scout_test. The atomic guarded write
required the exact original provider observation and old missing-row version;
all metadata was preserved and ATH date remains null. Fixed679-key cohort
is now513numeric/166unknown,79new references since434 (SOL65/Base2/RH12).
This count measures numeric scoped references, not annual ATH completeness.

Changed files: shared exact-bound regression and new vC95 golden, ADR-021,
AGENTS dated-source pitfall, source-research measurements and central status.
No production adapter/binary or runner gate change.
