# 1e6489 NVDA-quoted Robinhood token: bounded cross-market reference

As of 2026-10-14, this read-only check derives a candidate price reference for `0x1e6489ef6d75c6f785843669b01262a7562b1e18` from its configured initial v4 pool and a separate Robinhood v3 USDG/NVDA market. It does not write the ATH database and does not establish lifetime ATH or all-market completeness.

## Method and source evidence

The target pool is `0x7cdf69ad42307d838ca0e5e4a02b582267941dd223c845b2fcb733b14dd55acc`. The captured Initialize/registry evidence identifies the target token as currency0 and Robinhood NVDA (`0xd0601ce157db5bdc3162bbac2a2c8af5320d9eec`) as currency1; both have 18 decimals. The three target-pool swaps in the captured birth-to-anchor window are receipt-matched and header-matched. The highest cross-market reference is the first event, 2026-07-23 15:42:10 UTC, block 17,403,416, transaction `0x43baf9761365ab29b2c77b776ef8a5c9a0b2207d53887a4f9ac1b7d56eec5c57`, log 12.

The matching successful transaction contains a USDG/NVDA v3 swap at log 9. Its exact output of 252,919,262,788,927,982 raw NVDA equals the NVDA input to target-pool log 12. This provides a same-transaction cross-market execution reference for the winner. The event ratio is 0.00000009744713937898912236757236286 NVDA per target token; USDG/NVDA at the preceding v3 log is 207.3506004315927081703062841 USDG/NVDA. Applying the Kraken USDG/USD daily close of 0.9999 gives **$0.00002020370228828843925474632214 per token**. With the historically checked 1,000,000,000-token supply (18 decimals), the reference FDV is **$20,203.702288…; floor to cents = 2,020,370 cents**.

All three target events are converted in the private candidate sidecar, rather than selecting an event before conversion. The other two references are approximately $0.00001997345057 and $0.00001990181803 per token. For the winning point, retaining the same-day USDG daily low/high and the surrounding v3 execution prices gives a sensitivity envelope of about $20,181.44–$20,205.72 FDV. This is a sensitivity bound, not an event-minute confidence interval.

The USDG/NVDA pool is the official Robinhood v3 factory's fee-500 pool. A bounded log scan returned 763 swaps from 15:36:10 through 15:48:57 UTC, covering both target event groups; four nearest external neighbor swaps were receipt-checked. For the winning group, the same-transaction v3 leg supersedes a generic last-trade assumption and is directly amount-matched. The dated USDG/USD conversion uses the Kraken daily candle for July 23 (open 0.9999, high 1.0000, low 0.9998, close 0.9999; 562 trades).

Supply at the winning block is 1e27 raw with 18 decimals (1 billion tokens); the captured birth-through-peak transfer queries show one mint and zero burns. Robinhood's documented Chainlink NVDA/USD feed was also read at the target blocks, but its `updatedAt` ages were 244–361 seconds, so those stale oracle marks are excluded from this candidate.

## Scope and limitations

This is a pool-window cross-market execution-average reference, not wallet net consideration, not a direct fiat trade, and not proof that every target token trade or venue has been captured. The v3 scan is bounded to the surrounding minutes, not all USDG/NVDA history. The initial target v4 pool is the only target venue in scope. Therefore the result is not a lifetime ATH and should remain a candidate for independent review under the existing source-specific policy.

## Reproducibility

- Candidate values, exact fractions, all three event conversions, receipt links, supply proof and input hashes: `.scout-data/diagnostics/rh-1e6489-nvda-pool-window-2026-10-14/candidate-review.json` (SHA-256 77d19011f3c103ae03c90bde88264cb5744f2fb64ee8aaead04a82c600bf7e97).
- Target pool events, receipt/header, supply and mint/burn raw responses: `.scout-data/diagnostics/rh-1e6489-nvda-pool-window-2026-10-14/`.
- USDG/NVDA factory result, bounded swap scan, neighboring receipts and USDG/USD candle: `.scout-data/diagnostics/rh-1e6489-usdg-market-bracket-2026-10-14/`.
- Existing official token/quote deployment and source context: [Robinhood ATH supply/source audit](2026-10-11-robinhood-ath-supply-source-audit.md).
- No Codex, Birdeye, Solana Tracker or database writes were used. The additional Robinhood RPC receipt query is separately captured as `raw/11-v3-neighbor-receipt-3e9cf351.json`; it validates the nearest pre-event pool swap at the same second, while the winner itself uses the directly matching v3 swap in its own target receipt.
