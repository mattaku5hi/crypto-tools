# BbM37 and D28 observed windows with bounded supply — 2026-10-14

Both candidates pass the existing observed-window price comparison after
replaying every captured PRE/POST state. Their canonical birth and later mint
accounts support narrow conservative supply bounds under the existing
`onchain-pool-window-bounded-supply-cex-reference-v1` source. Exact event-time
supply, deployed virtual-reserve semantics and annual/all-market ATH remain
unknown. Both conservative lower references were written atomically after
the captured regression and workspace checks passed. Frozen cohort: **529
numeric / 150 unknown**, 95 new references since the 434-numeric baseline
(SOL81/Base2/RH12); remaining SOL15/Base100/RH35/BSC0.

| Fact | BbM37 | D28 |
|---|---|---|
| Mint | `BbM37gLNqRLa2NYHmUGG6hc6XfzTNyXGTEwPngurpump` | `D28iEFyY4tFFG3y2Sw2BnT53Q5SxeQQuQ19PWgQPpump` |
| Pool | `9RSYQ6pGuRkL6Ng5grfPzXySJddVoBHNUVcyEHL8xaSD` | `6cLeP7ZEo5WU8LsTiFiNSJWARQvzLddToXeiwA2nnNLP` |
| Whole-second window `[gte, lt)` | `[1773544230, 1773544237)` | `[1769275220, 1769275227)` |
| Successful transactions / actual PRE+POST marks | 6 / 12 | 1 / 2 |
| Global price maxima vs saved Solana Tracker price | −0.02508% / +0.00893% | −1.98427% / −1.93796% |
| Supply raw lower / upper | `1999950095876448` / `2000000000000000` | `999998621881978` / `1000000000000000` |
| Supply interval width relative to birth | 0.00249521% | 0.000137812% |
| Observed FDV lower floor / upper outward ceil | **$12,738.20 / $12,742.86** | **$109,559.38 / $109,611.31** |

The old diagnostic classifier used a strict timestamp bracket for BbM37 and
only a PRE endpoint for D28. All captured states are now replayed using exact
pool-owned WSOL/base vault balances and their own Coinbase SOL/USD minute
low/high. Both selected markets are PumpSwap AMMs. Every trade event binds
its dated January 9 or February 17 IDL, parent instruction, pool and mint.
The empty final pages prove terminal pagination only for these captured
mint-address windows, not other pools or a whole year.

Both global maxima are POST states. BbM's chain blockTime shares the provider's
reported second, so order within its millisecond timestamp is unknown. D28's
only transaction precedes the provider timestamp by 2.25 seconds. These remain
observations at the stated offsets; neither is interpolated to the timestamp.

Canonical birth transactions issue two 1e15 raw MintTo operations for BbM and
one for D28, then revoke mint authority. The later finalized account supplies
the lower bound. BbM's Token-2022 mint has metadata-only TLVs; D28 is an 82-byte
classic SPL mint. No mint-close, confidential-supply or interest/scaling extension
is present. Historical supply can decrease by burns; complete burn histories
are not captured and an exact per-state supply is not claimed. Pair every
observed price with both conservative supply endpoints and retain their interval.

All three fresh finalized winning/latest transactions match the indexed full
transaction objects exactly. This phase uses three Helius methods, no retries,
and no new Solana Tracker, Codex, Coinbase or supply-history requests. Existing
canonical birth and current account captures are reused. The interval does not
bound error from the conditional vault model or unobserved markets.

The metadata-only correction from proof v2 to v3 fixes Coinbase candle tuple
labels: open is column3, close column4, volume column5. Low/high were correctly
column1/2 and all monetary calculations are unchanged. BbM's candle is
`[1773544200, 88.18, 88.21, 88.20, 88.19, 202.35848617]`; D28's is
`[1769275200, 126.99, 127.05, 127.01, 127.01, 109.98302212]`.
Old proofs/audits remain immutable. Snapshot slots distinguish the older
455686275 capture from the later batch5 account capture at456610930.

Private root certificate:
`.scout-data/diagnostics/solana-bbm-d28-canonical-phase-2026-10-14/root-final-certificate-v1.json`,
SHA `6f6f0a6b32e8ddad82abeb42d14d4b8cebabf786692b6071ae1f8ac5534b54b2`.
Corrected price proof v3 SHA:
`da0c0bf4fc7ea9c5e4a06692a2077f87effcd7f63cb0d6b544fdd6c454a795fe`.
Independent audit v5 SHA:
`f4c73fb7c7218510a691b9fe6259c3cc62c31548cbb30dbc2e8d0bb62a8b2777`.
Guard writes with these proofs, the captured fixture, isolated checks,
original provider observation and missing-row version. Preserve null ATH dates
and holder/liquidity metadata. Both values are below the $500k runner gate;
no new runner membership follows.

The paired fixture
`solana_tracker_bbm_d28_bounded_supply_fdv_window_case.json` has SHA
`13edc85cb2abb3323fbf984bcb75bb5950020c23ee42d26e81975a38d7e0fef7`.
Its exact U512 replay checks all 14 marks, raw mint/revoke ordering, conservative
supply bounds, both global price gates, corrected candle metadata and outward
rounding. Workspace formatting, all-target/all-feature Clippy with warnings
denied, and all-feature tests passed: **1,247 tests**, 60.94 seconds; database
tests use `scout_test`, not production. Private manifest:
`.scout-data/checks/2026-10-14/b84-gap-closure-round29-validation-manifest.json`.

The atomic guarded write retained holder/liquidity values and null ATH dates.
Before/after/SQL and validation hashes are retained in
`.scout-data/diagnostics/solana-bbm-d28-canonical-phase-2026-10-14/atomic-two-row-write-v1/`.
No daemon restart or live adapter change. The sealed80 state package predates
15 guarded writes; it is not the current95-reference checkpoint. A server
cutover still requires a fresh consistent snapshot and restore verification.
