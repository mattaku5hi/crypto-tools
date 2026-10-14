# Solana ATH follow-up: explicit signature time boundaries, batch 7

Seven more native-SOL Mayhem curve marks were independently verified and
written with exact row/provider guards. The frozen 679-token cohort reaches
474 numeric / 205 unknown: 40 closures since the 434/245 checkpoint.
This is not annual all-market ATH completeness.

| Mint prefix | Birth-to-peak successful transactions | Stored FDV, USD |
|---|---:|---:|
| `FgFgrJLZ…` | 6 | 5,354.28 |
| `EaJYZF47…` | 10 | 11,166.35 |
| `4gTnpCre…` | 47 | 24,749.33 |
| `7ry8NVmM…` | 14 | 6,020.43 |
| `HgWdqwnB…` | 70 | 6,836.96 |
| `8mbDAwne…` | 14 | 6,510.93 |
| `7ktfxYwn…` | 23 | 22,366.70 |

All 101 captured neighboring TradeEvents match actual Pump program log
context and event-CPI bytes. Their global window maxima occur at the provider
timestamp's second or one second earlier. Each passes the exact 2% price gate
at both dated SOL/USD minute endpoints. Two birth mint instructions issue
2B six-decimal tokens, authority is revoked at birth, and no burn occurs
through the price peak. Complete filtered histories reconcile with scoped
finalized signature queries plus explicit birth. Current mint extensions
are metadata only, with no close or supply-scaling authority.

The Fg signature query demonstrates why a cursor is not an exact timestamp
boundary: it retains one successful transaction two seconds after the peak,
and two failed transactions. The capture manifest's original mismatch status
is preserved. A separate proof explicitly filters to known birth-through-peak
timestamps and successful effects, retaining excluded rows and adding the
birth that `until=birth_signature` excludes. Its five in-scope successful
signatures plus birth match all six complete TFA transactions. A null time
would remain a gap. Raw captures were not rewritten.

Successful standard transaction recaptures verify entire messages, balances,
inner instructions and logs against TFA. Native System transfers are scoped
to each direct/nested Pump buy invocation; curve and fee/creator legs and
ceiling fee rounding reconcile. Fee-payer deltas are not substituted for
quote amounts. Known positive rows are preserved; all seven updates keep
holders/liquidity and `ath_at=NULL`. The source remains the explicitly scoped
Mayhem curve-mark reference from ADR-021 amendments 34–36.

The AiEpw negative control retains an earlier matching mark, but its global
captured maximum differs by about 8.6%. It remains unknown. The two AMM inputs
were not evaluated with the curve formula. No quality threshold was relaxed.

Capture accounting: 57 Helius methods, ten Coinbase GETs, then seven standard
finalized recaptures (64 Helius methods total of 70). No Solana Tracker/Codex
requests, retries or new provider keys were used. Root proof hashes are:

- price: `25be858f6306b51dfb8f3ae53982a888c47d07d015afacb849c8e920df12185d`;
- supply: `70398ca44cbf59fa6394dba162a703f261b597d1040751a2edb61ebae49040cf`;
- native: `b3831fbadb139490326f24ee657f15054252b04d1d9fa1839097894a86122a9c`.

The separate final worker sidecar is
`64546a46fc73a917e5fdfb4d4c138e86d549b793e4083722526b4bb0f54aaecd`.
An earlier strict-cursor analysis was revised before any write; its original
capture status is still retained. Evidence is under
`.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch7/`.

The existing fee-case regression now runs both its 36-event fixture and this
101-event fixture, including explicit out-of-scope/failed signature controls.
Full fmt, workspace Clippy and all workspace/all-feature tests pass: 1,230
tests, with DB tests isolated in `scout_test`. Production adapters and the live
binary are unchanged. The verified 26-closure state package predates this
40-closure checkpoint; a later package must state its own captured counts.
