# Solana 9Qv7: canonical observed-window FDV

The guarded write stores **$18,735.09** for
`9Qv7rMQnXh8nGgoedmxAnp8RxbwRKrVtkK9nrYUppump`, using existing source
`onchain-pool-window-supply-cex-reference-v1`. The observed reference interval
is $18,735.09–$18,745.91 with outward cent rounding. It is a conditional
selected-pool window reference, not complete annual, all-market or lifetime ATH.

All 32 captured PumpSwap events bind the exact selected pool and base/WSOL
vaults; all 64 PRE/POST balance states bind mint, owner and decimals. The dated
February 17 IDL and actual event-CPI bytes are retained. Own-minute Coinbase
SOL/USD is $86.61–$86.66. Global price maxima at both endpoints differ from
the saved provider price by only 0.0130%/0.07075%. All captured states are kept,
including those after the provider's peak second. Maximum observed FDV is a
PRE state in second 1777066195; the provider's millisecond timestamp is not an
exact chain maximum timestamp. Internal virtual-reserve equivalence is unknown.

The apparent old supply-history mismatch came from request boundaries.
Historical TFA excludes second 1777066196, and the signature census excludes
the explicit birth anchor. Four apparently missing successes are already in
the terminal price-window capture. A fresh census through its latest anchor
has 482 unique rows: 249 successful, 233 failed, below the 1,000-row cap.
Its successful set plus birth/latest excluded anchors equals 251 full captured
successful transactions. Two standard finalized transactions match the old
winning/latest TFA objects exactly.

All target supply instructions in that complete set are classified: two
birth MintTo operations issue 1e15 raw each, then authority is revoked; no
target burn occurs through the latest captured state. An unrelated LP-mint
burn is retained and excluded from target supply. All states therefore use
2e15 raw at six decimals. The later current supply of 1021739553566068 raw
is a separate snapshot. Replacing historical supply with it would be wrong.

Three new Helius methods were used against an eight-method cap. No new Codex,
Solana Tracker or Coinbase request was made. Independent audit and root raw
replay agree; all source hashes remained unchanged. Golden regression verifies
exact fractions, supply instruction account roles, census boundaries and the
unrelated-mint burn. Full fmt, workspace/all-target/all-feature Clippy and
workspace/all-feature tests passed: **1,242 tests**, in **56.37 seconds**;
database tests used `scout_test`, not production. `CARGO_INCREMENTAL=0`.

The one-row atomic update guards the original provider observation, missing
row/version, source and certificate/fixture equality. ATH date stays null;
holders 3 and recorded max liquidity $383 remain unchanged. This token does
not qualify as a runner and does not add a dev-list member.

Frozen 679-key checkpoint: **524 numeric / 155 unknown**, 90 new scoped values
since 434. New values by chain: SOL 76, Base 2, Robinhood 12. Unknown by chain:
SOL 20, Base 100, Robinhood 35, BSC 0. Numeric counts do not imply complete
annual ATH coverage. The sealed 80-value package predates ten latest writes;
no new full restore or deployment is claimed.

Private immutable provenance:

| Evidence | SHA-256 |
|---|---|
| Raw replay v2 | `3b19038e88487c1f701c041e93438899aa6808517fe23f381a4c538a00186e1b` |
| Independent raw audit v5 | `3768934269820dd9063ffae886318555b886b4d4a73c59f3fb5d4721a694033e` |
| Root certificate v1 | `2f56c6a7a863fee1293255d7635a5d031800bbcce5505c8de2ad53888da2acd8` |
| Golden fixture | `cfb41156ea2048ec76ef9bd303f02e600f213c177585c7b025c41d4c2553f095` |

Producer/canonical capture and guarded write:
`.scout-data/diagnostics/9qv7-canonical-census-2026-10-14/`.
Independent audit: `.scout-data/diagnostics/solana-9qv7r-independent-raw-audit-2026-10-14/audit-v5.json`.
Workspace checks: `.scout-data/checks/2026-10-14/b84-gap-closure-round26-validation-manifest.json`.
ADR-021 amendment 53; next task **B8.4** remains historical price/supply coverage.
