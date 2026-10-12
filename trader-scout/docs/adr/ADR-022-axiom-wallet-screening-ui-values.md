# ADR-022: Axiom wallet screening uses the displayed Performance metrics

Status: Accepted (2026-10-12)
Date: 2026-10-12
Scope: standalone `crypto-tools/axiom-wallet-pnl`; manual validation of discovery lists.

## Context

The owner specifies four Axiom Performance predicates: Total PnL and Realized PnL
for `30d` and `Max`. Manual inspection of those panels is the reference behavior.
The undocumented `/portfolio` response sometimes supplies `realizedUsdPnl` values
that differ from the panel beyond display rounding. The public frontend audit has
not established that these fields have the same accounting semantics. An API/UI
agreement requirement therefore adds a selection criterion the owner did not request.

## Decision

1. Read all four USD values from the selected wallet's Performance panel. Require
   the exact labels, dollar unit, selected period, matching wallet header, and a fresh
   `/portfolio` request bound to the exact wallet and chain-specific host/native unit.
2. Compare monetary thresholds with `Decimal`. Treat displayed values as intervals
   at their shown precision; a threshold crossing that interval is `UNKNOWN`.
   Missing, malformed, unbound, or wrong-period observations are also `UNKNOWN`.
3. Preserve API realized-USD values and their agreement with the display as diagnostic
   provenance. They do not replace panel values or add a profitability exclusion gate.
   Native API `totalPnl` is never interpreted as USD.
4. Require all four predicates. Defaults are $10,000 for each `30d` metric and $30,000
   for each `Max` metric. For the owner-requested current BSC run, both thresholds are
   halved to $5,000 / $15,000. SOL retains the defaults. Persist the actual thresholds.
5. Use a new source-policy/version fingerprint. Refuse ordinary resume with a different
   policy or thresholds. An explicit local migration may reclassify saved observations
   without provider calls, retaining raw UI, original API evidence, identity, and
   observation timestamps; archive the previous checkpoint first.
6. Keep PASS/FAIL/UNKNOWN in the complete report. Produce one deduplicated selected CSV
   per network for the merged input sets. Never overwrite the owner's source files.

## Consequences and evidence

These reports describe Axiom's displayed analytics at each row's `collected_at`.
The run `as_of` is a start/checkpoint marker, not a synchronized historical snapshot.
Axiom's accounting, history coverage, fees, and Max baseline remain unaudited.
The Rust ledger, normalization, accounting, and `wallet-rank` policies are unchanged.

Implementation and regressions live in `../axiom-wallet-pnl/axiom_pnl.py` and
`../axiom-wallet-pnl/tests/`. They cover threshold boundaries, API/display disagreement,
wallet and period binding, policy-sensitive resume, and offline checkpoint rescoring.
Private source observations and wallet results stay in ignored local storage.
