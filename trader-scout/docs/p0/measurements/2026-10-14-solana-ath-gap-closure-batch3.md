# Solana historical ATH gap closure — batch 3

This read-only screen tested five additional not-yet-probed rows from the frozen Solana unknown cohort. Selection prioritized exact WSOL pools and tight cached birth/current-supply FDV widths. The price gate was fixed in advance: exact pool's nearest preceding and first following successful swap within ±3 seconds; marginal spot from the event's pre-swap reserve values; same-minute Coinbase SOL/USD low/high; each endpoint must be within 2% of the Solana Tracker recorded-window peak. No threshold was relaxed.

| Mint prefix | Peak UTC | Cached birth/current FDV width | Prior bracket vs ST | Following bracket vs ST | Result |
|---|---|---:|---:|---:|---|
| A5Ewn | 2026-02-21 00:14:13.974 | $16.50 | −25.772% to −25.710% | −9.625% to −9.551% | Fail |
| D8uy | 2026-02-09 19:19:39.206 | $49.85 | −50.869% to −50.795% | −81.190% to −81.162% | Fail |
| Bd556 | 2026-01-27 23:43:20.656 | $62.73 | −15.888% to −15.828% | −18.887% to −18.830% | Fail |
| G3Hy | 2026-03-26 09:12:33.478 | $68.44 | −46.569% to −46.557% | −46.145% to −46.133% | Fail |
| 2YG2 | 2025-12-30 01:38:47.792 | $87.83 | −12.786% to −12.709% | −8.619% to −8.538% | Fail |

All five finalized PumpSwap account states decode to the exact ST base mint and WSOL quote mint. Both selected event reserve pairs match the transaction's pool-owned pre-token-balance values. These are price-gate failures, not evidence against a lifetime ATH. Supply history was not queried after price validation failed.

Raw windows, pagination continuations, request/response hashes, exact pool identities, Coinbase candles, and the normalized comparisons are indexed by `.scout-data/diagnostics/solana-gap-closure-2026-10-14-batch3/manifest.json` (SHA-256 `3a5ac52c6e7c53a9dda9cf52a696367ef82f178dd65729b7a33e899baf85f9a4`). The comparison JSON SHA-256 is `23f8852b91eee92eced5811b6e66b72eb7f3484ac0762b444864bf436e6fece8`. The batch used 13 Helius methods and the final 5/15 Coinbase public requests; response bodies stayed below 2 MiB, with no retries. No DB writes were performed. Further Coinbase price screening needs a new request budget.
