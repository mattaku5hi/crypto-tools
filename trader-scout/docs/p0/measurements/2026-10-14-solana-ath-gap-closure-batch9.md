# Solana ATH follow-up: current candidates and scoped signatures, batch 9

Seven native-SOL Mayhem curve marks were independently verified and written
with exact row/provider guards. The frozen 679-token cohort reaches
489 numeric / 190 unknown: 55 closures since the 434/245 checkpoint.
This is a source-scoped checkpoint, not annual all-market ATH completeness.

| Mint prefix | Birth-to-peak successful transactions | Stored FDV, USD |
|---|---:|---:|
| `CfK6U11L…` | 85 | 29,220.33 |
| `3Lwp8t4X…` | 157 | 122,778.98 |
| `6cMr3t2X…` | 106 | 31,899.85 |
| `Cor2UweU…` | 58 | 84,912.58 |
| `A8J63xvr…` | 76 | 12,727.74 |
| `AJgQSj7D…` | 294 | 17,387.69 |
| `CPH9cNxz…` | 221 | 68,718.94 |

All 53 positive neighboring TradeEvents match actual Pump program log
context and event-CPI bytes. Global maxima at both actual-minute SOL/USD
endpoints select the same signature zero to two seconds before the provider
peak, within the exact 2% gate. Each mint has two birth mint instructions
issuing 2B six-decimal tokens, authority revoked at birth, no burn through
the peak and current metadata-only extensions. Stored FDV uses exact saved
provider price × verified supply; independent FX bands corroborate the price
and are not maximum-FDV uncertainty bounds.

AJg's query contains 317 signatures: 294 successful overall but only 293
successful inside birth-through-peak. A successful row two seconds after
the peak and a failed row one second after it are explicitly retained.
The 293 in-range successful signatures plus exclusive-cursor birth equal
all 294 terminal TFA transactions. Other histories also reconcile. Standard
finalized recaptures match whole messages and selected metadata; native
System transfers within actual Pump buys match curve/fee/creator amounts.
Seven atomic guarded updates preserve holders/liquidity and `ath_at=NULL`.

The initial batch-9 selection was stale, included three already-known keys,
and sorted by the wrong field. It was stopped after 24 Helius methods and
ten Coinbase GETs, with no analysis/promotion. Immutable corrected-v2 input
uses exact current unknown membership and birth-to-peak duration ordering.
A separately recorded bounded extension includes the earlier attempts;
three exact-minute Coinbase responses are reused with hash provenance.
Corrected capture75 + stopped24 + canonical7 = **106 Helius methods** of
120; new7 + earlier10 = **17 Coinbase GETs** of22. No Solana Tracker/Codex
requests, retries, quota resets or new keys were used. AMM follow-ups require
their own separately reported accounting if performed.

Three selected AMM inputs are excluded from the curve formula. Their
nonterminal 800-TFA/1,000-signature captures cannot establish peak supply;
separate pool-price diagnostics do not silently close those gaps.

Root price/supply/native proof hashes:

- `a242ab6e35d5eb0ffa3033376b195eafe777961164b7c828dd4f1b4826419789`;
- `3591777dc1f9b001c222a512b387ef2cfc31d19243101ca057bfe43bdd86384f`;
- `8a8c0fcbb119f907d5f1eb35fa6fbbd82e48c05437f3a4ed24380a4ee39c2163`.

Separate worker proof:
`788684e4baeaf238b7566f672122820ab36174766912f13f56b3d83070c9890f`.
Evidence is under `.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch9-v2/`;
initial stopped captures remain separately retained. ADR-021 amendment38
and the shared captured-byte regression record the boundaries. Positive
maxima are explicitly restricted to -3..0 seconds; future negative controls
remain rejected. Full fmt, workspace Clippy and all workspace/all-feature
tests pass: 1,230 tests, with DB tests isolated in `scout_test`. Production
adapters and the live binary are unchanged. The 26-closure sealed package
predates this 55-closure checkpoint; no new full restore is claimed.
