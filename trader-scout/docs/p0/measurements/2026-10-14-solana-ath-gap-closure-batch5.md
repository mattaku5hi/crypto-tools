# Solana ATH follow-up: current unknowns, batch 5

The input selects ten currently unknown SOL keys from the immutable 679-token
diagnostic cohort. Earlier cached captures are reused when their identity and
time scope match; these are not necessarily ten never-before-queried tokens.
The worker reached its model limit before writing a final manifest. Root
retained its raw captures, completed two missing history pages, and recovered
the file-hash manifest without replacing responses.

For `8ihYkBBP…`, all 838 successful birth-to-peak transactions match the
837 successful signature-index entries plus birth. The signature cursor
explicitly excluded birth. Five burns remove 92,734,328 raw units before the
peak; no later mint appears, authority was revoked at birth, and the mint
cannot be closed/reinitialized under the observed extension layout.
Both adjacent PumpSwap event states pass complete dated-IDL parsing,
curve/fee and actual-vault checks. Their SOL/USD low/high endpoints agree
with the saved provider peak within 2%.

The peak-supply FDV and birth-supply upper bound differ by less than $0.01.
Their cent floors are 10,435,467 and 10,435,468. ADR-021 amendment 33 accepts
the conservative lower value with this explicit one-cent uncertainty;
the maximum FDV's exact cent remains unresolved. This provider-window value
does not establish all-market or lifetime coverage.

Three input rows have `pool_id == mint`, with a Token-2022 mint account rather
than a PumpSwap account. Their saved transactions contain pump.fun bonding
curve TradeEvents. They require a curve-specific check and are not classified
as invalid tokens merely because the AMM account check fails. Other rows
retain observed price disagreements or missing strict time brackets.

Recovered evidence is under
`.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch5/`:
`root-recovered-batch5-manifest.json`, `root-8ih-independent-proof.json`,
and the request/response captures. Across batches 4 and 5, recorded calls are
76 Helius methods (caps 66 plus 30 for the separate Mayhem supply pass) and 18 Coinbase GETs (cap 20); no new Solana Tracker
or Codex requests were made. Source prices keep their unknown recording
window, and monetary calculations use exact rational arithmetic.

The three curve cases are now independently verified and guarded-written.
All 27 captured TradeEvents match actual Pump event-CPI bytes in the program
log context; selected maximum states were recaptured through standard
finalized RPC, and native SOL/token deltas match emitted trade amounts. The
official Pump SDK formula establishes a curve-mark reference, while Mayhem
reserve updates need not equal ordinary user-trade deltas. The maximum of
every captured state at both dated FX endpoints agrees with ST within 2%.
Complete mint history sets contain 17/25/181 successful transactions, equal
to the finalized signature sets plus birth; each mint issued 2B tokens in two
birth instructions, revoked authority and had no burn through the peak.
The exact FDV floors are 1,430,309 / 1,567,048 / 5,643,677 cents, with
`ath_at=NULL`. Their provider-reported market caps used approximately 1B and
were not copied into the ledger. ADR-021 amendment 34 and a captured-byte
golden exercise the curve model, absent suffixes, FX endpoints and 2B supply.

Evidence: `root-mayhem-price-proof.json`,
`root-mayhem-supply/root-independent-supply-proof.json`,
`root-native-standard-recapture/manifest.json`,
`root-mayhem-guarded-promotions/manifest.json`, and the independent
`pump-mayhem-capture-offline-audit-2026-10-14/proof.json`. Production adapters
and runner thresholds are unchanged. Full fmt/Clippy/workspace checks passed
with 1,229 tests, using the isolated `scout_test` database.
