# Twelve conservative Solana FDV bounds — 2026-10-14

ADR-021 amendment 40 applies the owner's accepted 2% uncertainty to twelve
named provider-window observations. The stored scalar is the lower bound
rounded down to cents. Exact peak supply and lifetime/year/all-market ATH
are not established. No production adapter or ranking gate is changed.

| Token prefix | Conservative FDV, USD | Birth-supply upper cent floor, USD |
|---|---:|---:|
| `EwMi5eRi…` | 52,739.88 | 52,848.62 |
| `DWLAF5wr…` | 46,833.71 | 46,842.85 |
| `9T4B3ADi…` | 43,517.19 | 43,751.10 |
| `2x9t…` | 36,562.71 | 36,562.81 |
| `5Uinp…` | 95,055.61 | 95,055.81 |
| `8bp3…` | 76,164.06 | 76,164.07 |
| `9dZk…` | 82,205.24 | 82,205.27 |
| `jKWm…` | 32,368.79 | 32,368.80 |
| `EuW3…` | 78,338.53 | 78,338.57 |
| `GP7H…` | 176,774.93 | 176,798.89 |
| `Den6…` | 57,369.25 | 57,380.26 |
| `2YG2…` | 115,877.27 | 115,965.09 |

The upper column is rounded down for presentation only; the acceptance gate
uses its unrounded rational value. Both independent pool/FX/supply endpoints
also agree within 2% of the stored scalar. Thus total uncertainty includes
price corroboration, supply and cent rounding, rather than adding tolerances.
All stored values are below $177k. The `4evPrwwv…` and `vC95xGRq…` controls
have about 44% supply intervals and are not eligible.

Evidence covers full selected-pool ±3-second pre/post neighborhoods with
dated complete PumpSwap schemas and actual-minute SOL/USD. Nine cases have
a separate independent price replay; root replays strict birth/current mint
conditions for all fourteen including negatives. Twelve new standard
getTransaction confirmations match selected history messages and metadata.
The mint proof checks authority revocation, issuance, exact legacy/metadata-
only mint layouts and excludes close/reinitialization and supply-changing
extensions. It does not enumerate every historical burn.

Root proof hash: `d7a3d2c7a33a889d0375029399b46324a1092850ba087aceef58ad72e38ff802`.
Canonical recapture manifest hash: `7bc251534bdf93de79fedf7f31a2db8baf6b26da3771db39cabe6d74d1b2ec53`.
Public golden: `crates/scout-devtracker/tests/fixtures/solana_tracker_conservative_fdv_bound_cases.json`.
Private raw captures and SQL guards remain outside git. The separate canonical
phase uses 12 Helius methods, no Tracker/Codex/Coinbase calls or retries; earlier
capture phase counts are retained independently in their immutable manifests.

Full workspace fmt, all-target/all-feature Clippy with warnings denied and
1232 tests passed in 47.08 seconds, using isolated `scout_test`; no test
mutated the production database. A separate guarded atomic transaction
committed all twelve values after those checks; read-back confirms the exact
lower floors, explicit source, null ATH dates and preserved holder/liquidity
metadata. The frozen cohort is now 502 numeric / 177 unknown (68 closures
since 434 baseline). The live daemon remains active with zero restarts.

Changed files: the shared Birdeye regression test module, the fourteen-case
golden, ADR-021, this measurement, central closure measurement and tickets.
Next task: B8.4, remaining provider-window validation and historical gaps.
