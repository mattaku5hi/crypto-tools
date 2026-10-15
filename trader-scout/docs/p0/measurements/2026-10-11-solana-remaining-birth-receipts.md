# Solana remaining birth-receipt supply audit (2026-10-11)

This supplemental read-only phase resolves the 91 birth receipts left after the earlier nine transaction samples. It does not modify the original phase-1 manifest, report, source cohort, or current-account snapshot.

## Scope and method

The source is the existing 100-mint cohort and finalized current-mint account snapshot in `.scout-data/diagnostics/solana-current-mint-audit-2026-10-11/`. Four sequential Helius JSON-RPC batch requests contained 25, 25, 25, and 16 logical `getTransaction` calls. Each requested the cohort’s exact birth signature at `finalized`, `encoding=json`, `maxSupportedTransactionVersion=1`. All four returned HTTP 200 within the saved body cap; no retry or fallback was used. Each response is matched by JSON-RPC id and then checked against its requested signature and expected slot.

For each successful transaction, the local decoder resolves static and loaded account keys, recognizes SPL Token and Token-2022 opcodes, checks the target mint’s InitializeMint decimals and owner program, sums target MintTo amounts, compares that total to the transaction’s post-token balances, and verifies `SetAuthority(MintTokens, None)` occurs after minting. The archived response bodies and exact request arrays are preserved for independent review.

## Findings

All 91 responses passed those transaction and token-level checks. Combined with the nine already validated receipts, all 100 cohort mints now have a receipt-backed initial supply and a current supply from the preserved account snapshot. All use 6 decimals. Birth issuance was 49 mints at 1,000,000,000 UI tokens and 51 at 2,000,000,000 UI tokens. The current raw supply is retained per mint alongside initial supply so later price joins can bound FDV without assuming constant supply.

Eight of the 91 transactions do not invoke the Pump core program in their outer instruction list; they instead contain another outer program, while still passing the direct target-mint initialization, mint-to, balance, and authority-revocation checks. That distinction is recorded per mint. This evidence establishes token issuance details, not economic buyer attribution.

## Artifacts and limits

The phase-2 raw requests, responses, HTTP metadata, parsed 91-receipt results, and all-100 join are under `.scout-data/diagnostics/solana-current-mint-audit-2026-10-11/`. `phase2-all-100-supply-join.json` records input/output SHA-256 digests, batch identities, and per-mint values. Current-account snapshot provenance is inherited from the original manifest. No database writes, price calls, or production code changes were made.
