# 8Cc PumpSwap quote-reserve and local spot reconstruction (2026-10-11)

This read-only supplement revisits the 2026-02-21 PumpSwap spot disagreement for `8CcYviLTUTnWjoVpa2nW8bHS9mgqDumbqeVqUDfVpump`. It uses the nine already archived successful transactions for pool `CTNhVdyC93jLs9NxJ7f5asMWzbqap3fVruTdwQ45tpch`, the existing Coinbase SOL/USD minute candle, and the existing Birdeye one-year USD candle response. No RPC, Birdeye, Solana Tracker, or database calls were made.

## Fee-aware reserve reconstruction

Each transaction’s `pool_base_token_reserves` and `pool_quote_token_reserves` match its `preTokenBalances` for the pool-owned base and wSOL vaults. The base/quote post-vault changes also reconcile exactly to the event amounts: buys decrease base by `base_amount_out` and credit quote by `quote_amount_in_with_lp_fee`; sells credit base by `base_amount_in` and debit quote by `quote_amount_out_without_lp_fee`. Event fee fields reconcile the fee-inclusive and user-net amounts.

For the constant-product output floor, a normal `buy` uses `quote_amount_in`; `buy_exact_quote_in` uses `user_quote_amount_in` (the net curve input, also `quote_amount_in_with_lp_fee - lp_fee`); a sell uses gross `quote_amount_out`. Solving each integer floor equation for effective quote reserve yields narrow intervals. The inferred amount differs from the actual pre-swap quote-vault balance by at most 1,243 lamports across all nine events. That is a fee-aware output constraint, not an assertion that an omitted event field is zero.

The source timeline independently resolves whether a virtual reserve existed at these slots. Pump’s official developer update says the `Pool::virtual_quote_reserves` field shipped on 2026-07-15, and that its nonzero phase was scheduled later; this is months after the events on 2026-02-21. All nine February event payloads decode with `virtual_quote_reserves = None` and no unconsumed trailing bytes (buy payloads are 416 or 431 bytes; sell payloads are 368 bytes, excluding the event discriminator). They end before that subsequently appended event field. The applicable effective quote for this historical pool state is therefore its actual quote-vault balance. Current PumpSwap docs give the later formula as raw quote-vault balance plus signed virtual reserves. These sources establish a later feature’s introduction date and current semantics, not a claim that the 2026-10 public IDL is the deployed historical revision. [Pump Developer Updates](https://t.me/s/pump_tech_updates/36) · [official PumpSwap reserve docs at pinned commit `2293f9a66c654e9fe82dc5e8f4618538f24bb35f`](https://github.com/pump-fun/pump-public-docs/blob/2293f9a66c654e9fe82dc5e8f4618538f24bb35f/docs/PUMP_SWAP_README.md).

The transactions can be ordered by vault-state continuity: each event’s post-vault pair equals exactly one other event’s pre-vault pair, producing a single nine-event chain. This is a reconstruction from transaction token balances, not a fetched block transaction-index ordering. Each row below shows the pre- and post-spot ranges after multiplying the exact raw vault ratio by the cached 2026-02-21 20:01 UTC Coinbase SOL/USD interval `$86.09–$86.14`. Raw units are 6 decimals for base and 9 for wSOL.

| Transaction | Side | Pre-state spot USD | Post-state spot USD |
|---|---|---:|---:|
| `3VzuAxCPsz…` | sell | $0.0000028208–$0.0000028224 | $0.0000028180–$0.0000028197 |
| `3Jz1Z16Xak…` | buy | $0.0000028180–$0.0000028197 | $0.0000028645–$0.0000028661 |
| `5HiZSCD5c3…` | sell | $0.0000028645–$0.0000028661 | $0.0000028280–$0.0000028297 |
| `3UuJFNQo86…` | buy | $0.0000028280–$0.0000028297 | $0.0000028373–$0.0000028390 |
| `4tDHggB9v8…` | buy | $0.0000028373–$0.0000028390 | $0.0000028410–$0.0000028427 |
| `31uyanQiLz…` | buy | $0.0000028410–$0.0000028427 | $0.0000028436–$0.0000028452 |
| `3g12nE1wKJ…` | sell | $0.0000028436–$0.0000028452 | $0.0000028137–$0.0000028154 |
| `28PM9Sweeh…` | buy | $0.0000028137–$0.0000028154 | $0.0000035520–$0.0000035540 |
| `58bzCzSGLs…` | buy | $0.0000035520–$0.0000035540 | $0.0000036918–$0.0000036940 |

Across all 18 observed states, the maximum upper endpoint is `$0.0000036939687651` (the last linked buy’s post-state). The cached Birdeye response contains 232 exact-mint, daily USD candles; its high is `$0.000013419091705273695` on 2026-02-21, with reported daily USD volume `$28,739.571541376557`. The Birdeye high is about 3.633× the largest local on-chain spot upper endpoint; the observed local maximum is 72.4723% lower, so this neighborhood does not corroborate that daily high within the existing 1% threshold. The daily candle has no intraday timestamp, and these nine local events do not cover all trades or other pools that day, so this does not disprove the provider high.

## Supply-bound effect and conclusion

The archived birth/current supply evidence is 1,000,000,000 tokens at birth and 999,999,927.688911 tokens at the later finalized snapshot (6 decimals; burn difference 72.311089 tokens). Multiplying Birdeye’s selected high by both exact supply endpoints gives `$13,419.090734924560400792071496145` and `$13,419.091705273695`; both floor to `$13,419.09`. The supply interval therefore collapses to the same cent for this chosen price, but it does not validate that price as the token’s ATH. The separate Solana Tracker point mark is `$0.000047952524582282125`; its own supply endpoints also have the already-recorded same-cent result, and that different mark does not by itself invalidate Birdeye.

**Conclusion:** the supposed missing virtual-quote correction is resolved for these February events: virtual quote reserves were introduced later, and the fee-aware swap math and vault transitions are consistent with the real quote vault. The nine observed local states do not corroborate Birdeye’s same-day high within 1%. Full-day/all-pool price completeness and the canonical ATH remain unknown; no ATH/FDV promotion is supported by this bounded comparison.

The machine-readable calculation is `.scout-data/diagnostics/solana-8cc-quote-reserve-inference-2026-10-11/enriched-analysis.json` (SHA-256 `49eb8232a22fe45dcfbdf6e2dff5d5c8cd7880e0f5fd9e278ee0e538e36a1d37`); the raw transaction, event, FX, and Birdeye hashes and paths are retained in the referenced existing manifests and sidecars. The exact reserve intervals and all nine transaction response hashes are in `.scout-data/diagnostics/solana-8cc-quote-reserve-inference-2026-10-11.json`.
