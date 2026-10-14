# JC cross-quote observed FDV reference — 2026-10-14

JCd8kQpU6RpSNASnxwF8i8vGdf8FNVtAVCiCi1uUpump has a saved peak
price but a volatile87ea… quote token. Execution amounts are not marginal
spot prices. The recovered quote/WSOL pool allows reserve-based conversion
without assuming a dollar peg or using a later current price.

Six terminal pool/vault histories cover the seven-second provider window,
with two target transactions and one quote transaction. Five state pairings
follow actual event order. Token2022 transfers and event CPI payloads match
the raw receipts; the Boost buy burns target supply before its BuyEvent CPI.
The87-row mint replay contains30 burns, a1B-token birth mint and authority
revocation, no subsequent issuance, and exact current-supply reconciliation.
Historical supply is attached to each mark rather than substituting today's
balance for PRE supply.

The documented virtual-reserve fee model predates the event. Adjacent target
states support PRE-event semantics; POST-field interpretation is also computed
as sensitivity. Both models retain raw reserves and exact fractions. The
quote pool has only one event, the deployed revision is unknown, and its
current account has14 zero bytes beyond the pinned IDL. These are explicit
conditions, not proven away by the small resulting reference interval.

Across both models, the conservative observed maximum reference is
$227,079.92–$227,224.06. Its FX/model/cent width is0.0634328423%; that width
does not bound unknown market or deployed-schema errors. Model-wise global
prices agree within2% with the original Solana Tracker observation. The
window is [1791672251,1791672258), not lifetime/all-market/annual coverage.

Source: `onchain-selected-pool-cross-quote-observed-fdv-window-vq-model-cex-v1`.
The independent root arithmetic and a byte-identical raw-verifier replay
check the five marks in both models. The conservative lower rule is the
minimum of model-wise maximum lower bounds, avoiding the erroneous
$227,079.95 obtained by taking the maximum instead.

Private evidence SHA-256:

- Root certificate: `9d968b747ca3d5522eb56a5020968a605e6a6b3e510daa627c259f017562d2e7`.
- Independent raw gate: `e2da976cea2b78e9a061da004308c5695855f4f72cf0a8c53b7a8bd047ebfb2d`.
- Exact model replay v3: `c3ff61d31681134b71d76e38636934aba14e374d8f59a3afcdd24d976fd52fdb`.
- Root arithmetic: `651bdd167428abfc3e3935a98a0f3c76742fd0c2ae89b509314f37d1938203af`.

New captures used14 Helius methods (six quote-vault/canonical/account methods
and eight pool/target-vault history methods). FX and supply evidence were
already cached; no new Solana Tracker, Codex or market-provider calls.
No production adapter or runner gate changes. Next task B8.4: remaining
historical price/supply/schema gaps; annual ATH remains incomplete.

Golden: `solana_tracker_jc_cross_quote_fdv_window_case.json`, SHA-256
`ea4c7d2bf91249a1b15d7f477b262d9755bf2bea028ebd0a4c17a2f172a50bbd`.
The regression checks raw account/birth/source hashes,30 ordered burns,
actual mint metadata, five state pairings in both VQ models, exact2% gates,
scaling, minimum-of-maxima lower bound and cent rounding. Workspace fmt,
all-target/all-feature Clippy and1236tests passed against isolated scout_test;
incremental compilation was disabled. The earlier Clippy type-complexity
failure is preserved separately; the local type aliases resolve it.

The atomic guarded write stores22707992cents, preserves holder/liquidity,
and keeps ATH date null. The fixed679-key cohort is515numeric/164unknown,
81new references since434 (SOL67/Base2/RH12). This is a conditional observed
reference, not a claim of annual ATH completion. The latest sealed80-closure
package predates this write. A new full bundle is deferred to a completed
milestone rather than duplicated for every reference update.
