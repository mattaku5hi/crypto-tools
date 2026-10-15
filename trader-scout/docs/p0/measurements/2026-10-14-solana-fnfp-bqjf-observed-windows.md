# Fnfp and BQJF observed PumpSwap windows — 2026-10-14

Two remaining Solana records have price and historical-supply evidence under
the existing `onchain-pool-window-supply-cex-reference-v1` source. These are
conditional selected-pool, captured-window FDV references. They do not establish
annual, lifetime, continuous-price, or all-market ATH coverage. Both lower
references were written atomically after the captured regressions and isolated
workspace checks passed. Frozen cohort: 527 numeric / 152 unknown; 93 new
references since the 434-numeric baseline (SOL79/Base2/RH12).

| Evidence | Fnfp | BQJF |
|---|---|---|
| Mint | `FnfpZNNNGHbZF9yARYj8583tQKARRZEozS9cWVjw73wQ` | `BQJFjhrDmCCov7WY5tjrx9zdRTNEBUb1iGVCYZ1Apump` |
| Selected pool | `GCvu7BpUG75vWUQPEMnKsH5vGDmoajvNT145xnhAfVWz` | `2mQBJRfTco9SmKhWe3WkyzEuBXpNS8LyxkTo4RwVomme` |
| Provider peak, UTC | March 30, 13:45:28.099 | April 26, 12:09:15.213 |
| Terminal captured ±3-second window | 128 successful transactions, 126 trade events | One successful transaction, one trade event |
| Actual PRE/POST vault marks | 252 | 2 |
| Global observed price vs saved Solana Tracker price | +1.2486% to +1.4050% | −0.01539% |
| Historical raw supply at every mark | `1000000000000000` | `1028873600726182` |
| FDV lower floor / upper outward ceiling | **$35,050.32 / $35,104.46** | **$8,339.38 / $8,339.39** |
| Later current raw supply, excluded from historical calculation | `979689874421880` | `1028857388822682` |

## Price and supply

Fnfp was previously excluded by a curve-only verifier because its selected
market is an AMM. The February 17 historical PumpSwap IDL matches every selected
trade event without trailing bytes. Exact event CPI, instruction, vault owner,
mint, decimals and transaction balances bind 41 BuyEvents and 85 SellEvents.
All 252 marks use their own Coinbase SOL/USD minute low/high, not an execution
average. One unrelated transaction has truncated logs and no selected-pool trade;
it contributes no price mark. Both canonical winning/latest transactions match
the saved full transaction objects exactly.

A fresh finalized, below-limit mint census returns 289 unique signatures:
203 successful and 86 failed. Its successful set plus exclusive birth/latest
anchors equals the 205 full successful transactions in the history/window union.
Birth has InitializeMint2, one MintTo of 1e15 raw, then mint-authority revocation.
There is no additional target mint or burn through the latest price mark.

BQJF required a new price-only capture. Its two vault marks bind the dated
BuyEvent in the sole returned transaction. The same full transaction is already
present in the saved historical supply capture. Its Coinbase SOL/USD minute
low/high are both $86.42; this is a venue-specific observed minute, not a bound
on every venue's dollar reference.

BQJF's retained history has 311 unique signatures, including 57 failed, and
254 full successful transactions; successful sets match and pagination is
terminal. Two birth MintTo instructions issue 2e15 raw and authority is revoked.
Two burns before the price transaction remove 971126399273818 raw. A later
third burn removes 16211903500 raw and explains the lower current supply.
Neither price state contains a target burn; both use the same historical supply.
The complete existing history is reused rather than requested again.

Both finalized Token-2022 mint accounts have metadata-only extensions. Current
pool tails remain uninterpreted; the actual-vault ratio is explicitly a
conditional model. Price/reference agreement within 2% does not prove historical
virtual reserve semantics or bound model and market-coverage error. Keep `ath_at`
null and preserve holder/liquidity metadata. Both references are below the
$500k runner threshold and cannot add runner-list members by themselves.

## Request accounting and retained evidence

Fnfp uses three new Helius methods: one signature census and two canonical
transactions. Its existing price window, Coinbase minute and Solana Tracker
response are reused. BQJF uses three Helius price-pilot methods, one additional
canonical transaction and one public Coinbase GET. Neither phase uses Codex or
a new Solana Tracker request. Every attempt has a retained manifest; no retries.

Private Fnfp directory:
`.scout-data/diagnostics/solana-bqfp-fnfp-amm-offline-2026-10-14/`.

- Price-only proof v1: `ac0b94031f902f046680d96f23b488665610e9b0171e1e47d4c3c70267417907`.
- Root final certificate v2: `f06a8ff53cbee314a1d3a1f185926a3a2f75e34ed76bdd350823710f8d02840b`.
- Independent raw audit v1: `b3aec6f4b58485f1a9fecdedb8f0a0eee660fcf80261356dd908790343197d75`.
- Independent supply audit v1: `74ed8a8d6e32936c4f23a3e9efac034b8d2d7c4611339a42e01d21ef93549c44`.

Private BQJF directory:
`.scout-data/diagnostics/solana-bqjf-price-pilot-2026-10-14/`.

- Price-only proof v1: `cf7378100ee72d08350fcea88fb66fba3c2d7e62662116ecc6c04edc891371a7`.
- Root supply certificate v1: `c3ff1c9c08924b5f648a6dd4ec0b73f0dfddd645702e31fdbc5bd78efe229856`.
- Root final certificate v1: `8966371b13b4f6e24970bfd6597077e78550fbac89a27a040b7ce6bcbd334946`.
- Independent raw audit v1: `5f99dccb226889d61f302b158eb3fe023d21030db9245ef68c1a5850a26dc92f`.

Captured regressions are
`solana_tracker_fnfp_historical_supply_fdv_window_case.json`
(SHA `69d61816a09a73b988e306b0cb7a2c98a576ce8089f8199a40153f918ecc636f`)
and `solana_tracker_bqjf_historical_supply_fdv_window_case.json`
(SHA `fbc6880a13e5fd52f8ec73076e7d84a85905456f6a02de38a1922f6cc2134302`).
They replay exact integer/rational price, supply and outward rounding; BQJF's
later burn is excluded from its earlier price states.

Workspace format check, all-target/all-feature Clippy with warnings denied,
and all-feature tests passed: **1,246 tests**, 54.59 seconds. Database tests use
`scout_test`; no production database tests. Private check manifest:
`.scout-data/checks/2026-10-14/b84-gap-closure-round28-validation-manifest.json`.
The guarded two-row write retained both original holder/liquidity values and
null ATH dates. Private before/after/SQL manifest:
`.scout-data/diagnostics/solana-fnfp-bqjf-guarded-promotion-2026-10-14/atomic-two-row-write-v1/`.
No daemon restart or production adapter change was required.

Related negative cases stay unknown: BQfp's recovered 418 AMM marks disagree
by +17.43–17.61%; 69KX's missing PRE is recovered but the global price remains
about +6.90%; Bd55's previously unknown discriminator is the dated
CloseUserVolumeAccumulatorEvent, a nontrade event, and its observed price
maximum remains about 5.9% below the provider. Their original frozen artifacts
are preserved. No supply requests are spent on these price-gate failures.
