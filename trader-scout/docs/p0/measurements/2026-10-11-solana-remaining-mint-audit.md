# Remaining Solana ATH-unknown mint audit

As of 2026-10-11. This audit covers the 100 Solana tokens that had zero-sentinel ATH rows in the current read-only database join of the 110-token eligible cohort. It audits current mint state for all 100 and exact birth receipts for nine bounded samples; it does not write ATH/FDV data.

## Cohort and current account state

The original eligible-cohort address list has 110 Solana mints. The read-only current database join found 10 numeric ATH rows and 100 rows with `ath_fdv_cents=0` and source `codex-missing`. Each of the 100 has a Pump launch row and creation signature. Holder counts are present for 97 (total 817, range 1–65); 3 are absent. The 100-token inputs, including launch timestamps, signatures, and available holders, are saved at `.scout-data/diagnostics/solana-current-mint-audit-2026-10-11/inputs/solana-ath-remaining-2026-10-11.json`.

One finalized `getMultipleAccounts` request returned all 100 mint accounts at context slot 455686275. All are initialized with 6 decimals, current mint authority absent, and freeze authority absent. Owners are Token-2022 for 88 and legacy SPL Token for 12. The Token-2022 accounts have only `MetadataPointer` and `TokenMetadata` extensions; the legacy SPL accounts have no extensions. No account failed the bounded mint-layout/TLV parse.

Current supply is nonconstant across the cohort: 55 current supplies are below 1,000,000,000 whole tokens (1e15 base units at 6 decimals), 45 are above, and none equal 1e15 base units. The observed raw range is 224,276,517,948,832 to 1,999,998,202,733,227. Current state alone does not establish each token's initial supply or historical burn amount.

## Birth receipt samples

Nine successful creation transactions were fetched, with selection ordered first by largest holder count and oldest creation time as tie-breaker; four additional samples were selected from current supplies above 1e15. In all nine, the token mint account is present in the transaction's resolved account keys and the matching Token-2022 `MintTo` instruction(s) target that mint. The same successful transaction sets mint authority type 0 to `None` (`SetAuthority` instruction bytes `06 00 00`).

Five sampled mints have one initial `MintTo` of 1,000,000,000,000,000 raw units (1 billion tokens at 6 decimals); four have two such instructions, totaling 2,000,000,000,000,000 raw units (2 billion tokens). For each of the nine, the current supply at the finalized account snapshot is below its receipt-proven initial mint total. Since the mint authority was revoked in that creation transaction, those nine supply reductions are consistent with irreversible burns and cannot be offset by later minting. The exact per-token initial amounts, current amounts, reduction amounts, creation receipt identities, and raw-response hashes are retained in the private result JSON.

This directly disproves a blanket 1-billion initial-supply assumption: the sampled cohort contains at least two constructor supply classes. The nine samples establish the behavior only for those nine mints. The remaining 91 birth transactions were not fetched within the 10-call limit, so their individual constructor totals and burn deltas remain unverified. All 100 current mint accounts are verified, but these observations alone do not justify classifying all 100 as invariant or burned.

## Scope and files

The audit used exactly ten read-only Helius requests: one `getMultipleAccounts` for 100 mints and nine `getTransaction` calls; there were no retries. Raw response bodies, full request parameters with redacted endpoint templates, HTTP statuses, result summaries, and hashes are saved under `.scout-data/diagnostics/solana-current-mint-audit-2026-10-11/`. Provider credential material is not stored there. No price/ATH write, application-code change, or external price request was made.
