# Solana PumpSwap virtual-reserve price reconciliation — 2026-10-11

Later [v3 state reconstruction](2026-10-11-solana-virtual-reserve-price-reconciliation-v3.md) resolves the post/intermediate-state gap stated in this v2 report using pinned fee transitions and exact vault reconciliation. V2 event arithmetic remains valid; v3 supersedes only the state-availability conclusion. None of the18 reconstructed states corroborates Tracker within1%.


This cached-only v2 recomputes the six 48vcic PumpSwap target-pool events from 22:04:52 through 22:04:56 UTC, their six same-transaction Bz/wSOL legs, and the two 87ea-quoted buy-and-burn examples. It supersedes the earlier real-vault-only spot interpretation (about $0.0001054 for 48vcic): PumpSwap’s event carries `virtual_quote_reserves`, and omitting that field understates the effective quote reserve. The original raw evidence and the immutable [v1-derived FDV artifact](../../../.scout-data/diagnostics/solana-48-variable-fdv-2026-10-11.json) remain unchanged for audit; v2 is the accepted calculation. This identifies an omission in the earlier private v1 price calculation, not a production SDK bug: the SDK event type already exposes `virtual_quote_reserves`.

The machine-readable calculation is [the v2 sidecar](../../../.scout-data/diagnostics/solana-virtual-reserve-price-reconciliation-v2-2026-10-11.json). Inputs are the cached [six-transaction evidence](../../../.scout-data/diagnostics/solana-48-st-peak-neighborhood-2026-10-11/evidence.json), [pinned Pump AMM decoder](../../../crates/scout-dex-solana/src/pump_amm_event.rs), [pinned PumpSwap IDL fixture](fixtures/pump_amm_idl_e0687ae9.json), and cached Coinbase/Kraken minute candles. No API, RPC, paid provider, or database was used.

## Exact event arithmetic and fees

All arithmetic uses integer raw units and exact fractions. The target token and Bz quote token use six decimals; wSOL uses nine. For a buy, the event’s net `user_quote_amount_in` is used in `floor(X * input / (Y_real + Y_virtual + input))`. For a sell, the gross event output is checked against `floor((Y_real + Y_virtual) * base_amount_in / (X + base_amount_in))`. Across all twelve events, the decoded output differs from the computed floor by at most 64 raw units; every event’s gross-to-net fee identity reconciles.

| Transaction (UTC second) | Direction | Target / quote event indices | Target leg output residual | Quote leg output residual | Route execution USD/token |
|---|---|---:|---:|---:|---:|
| 4vBo6wNt… (22:04:52) | target → Bz → wSOL | 15 / 29 | 0 raw | 0 raw | 0.0001229131–0.0001230136 |
| 4EeoBCV1… (22:04:54) | wSOL → Bz → target | 113 / 75 | −14 raw | −64 raw | 0.0001242083–0.0001243098 |
| LLtKSLhK… (22:04:54) | wSOL → Bz → target | 55 / 43 | −14 raw | −61 raw | 0.0001293623–0.0001294678 |
| 3Uv1xWxK… (22:04:55) | target → Bz → wSOL | 15 / 29 | 0 raw | 0 raw | 0.0001257966–0.0001258994 |
| TT5fX8WA… (22:04:56) | wSOL → Bz → target | 55 / 43 | −15 raw | −62 raw | 0.0001210639–0.0001211628 |
| 5MLWiCbH… (22:04:56) | wSOL → Bz → target | 32 / 20 | −15 raw | −63 raw | 0.0001200337–0.0001201318 |

Each route row is realized two-hop output divided by input, calculated from the archived raw route amounts. It is execution price, not an instantaneous spot. For example, in 4vBo the target-pool sell emits 109,723,992,123 Bz raw gross, with 219,447,985 LP fee and 54,861,997 creator fee, leaving 109,449,682,141 for the second leg. The Bz/wSOL sell consumes exactly that amount and emits 1,719,389,439 lamports gross; after its 859,695 protocol fee, 1,718,529,744 lamports remain. The other event pairs reconcile by the same raw event accounting. Buy-route event fees likewise equal gross quote input minus pool quote credit.

## Spot comparison with the tracker candidate

The archived 48vcic tracker candidate is $0.00013242878993078837 at 2026-10-10 22:04:55.327 UTC. Coinbase and Kraken’s same-minute SOL/USD ranges overlap at $110.15–$110.21; the conservative outer envelope is $110.14–$110.23.

The table below gives a coherent **transaction-start** cross-pool spot for each transaction: both pools use their transaction pre-vault balances and each pool’s virtual reserve from that pool’s event pre-swap snapshot. It is distinct from the route execution prices above. No fully supported transaction-start spot is within 1% of the tracker candidate.

| Signature | Slot / transaction index | Paired target / quote event | Transaction-start spot, outer SOL/USD envelope | Difference from tracker at envelope endpoints |
|---|---|---:|---:|---:|
| 4vBo6wNt… | 455413619 / 490 | 15 / 29 | $0.0001248614–$0.0001249634 | 5.71%–5.64% |
| 4EeoBCV1… | 455413625 / 157 | 113 / 75 | $0.0001226874–$0.0001227877 | 7.36%–7.28% |
| LLtKSLhK… | 455413629 / 679 | 55 / 43 | $0.0001287951–$0.0001289003 | 2.74%–2.66% |
| 3Uv1xWxK… | 455413630 / 837 | 15 / 29 | $0.0001291433–$0.0001292488 | 2.48%–2.40% |
| TT5fX8WA… | 455413634 / 131 | 55 / 43 | $0.0001203503–$0.0001204486 | 9.12%–9.05% |
| 5MLWiCbH… | 455413637 / 543 | 32 / 20 | $0.0001193228–$0.0001194203 | 9.90%–9.82% |

3Uv is the nearest fully supported transaction-start spot, still 2.40% below the tracker candidate at the outer high endpoint. The API’s millisecond timestamp is not treated as proof that one of these second-resolution transactions is the exact underlying trade.

The cached transaction records include post-swap real token-vault balances but not post-swap virtual reserves. Therefore, an after-first-hop or transaction-end cross-pool spot cannot be reconstructed exactly. The sidecar leaves those cross-spots unset rather than combining post-vault balances with stale pre-swap virtual reserves. This is a data gap, not a zero or a claim that no later spot matched.

## 87ea-quoted buy-and-burn events

Two nearby cached transactions for 81kiaJS and JCd8kQp contain a PumpSwap `BuyEvent` followed by an explicit `BoostBuyAndBurnEvent`. The output amount equals the burn amount in each transaction, so these are **buy-and-burn operations, not user acquisitions**. Their event-snapshot quote-denominated spots and exact CPMM outputs are:

| Mint | Signature / slot / transaction index | Buy / burn program-data log indices | Effective quote reserve raw | Base reserve raw | Quote per target token | Base output residual |
|---|---|---:|---:|---:|---:|---:|
| 81kiaJS… | 1NiE1rJs… / 455424899 / 355 | 14 / 18 | 7,876,050,948,295 | 172,224,181,607,503 | 0.04573138844 | −21 raw |
| JCd8kQp… | cvaCJvo1… / 455424461 / 321 | 14 / 18 | 19,165,013,916,941 | 169,604,234,017,085 | 0.11299844033 | −8 raw |

Both target and 87ea quote tokens have six decimals in these event records. No USD conversion is applied: cached 87ea/WSOL executions show materially different quote/USD values near the respective timestamps, and the bridge’s instruction coverage remains incomplete. These quote-denominated values do not corroborate a USD ATH.

## Result and limits

The apparent ~$0.0001054 real-vault-only 48 spot is invalid because it omits the event’s virtual quote reserve. Including that reserve makes the twelve raw event outputs agree with the PumpSwap constant-product calculation to at most 64 raw units, and the route fees reconcile. The best fully supported paired transaction-start spot remains more than 1% from the tracker candidate. Exact post/intermediate cross-spot matching remains unresolved because post-event virtual reserves are absent from the cached evidence. This bounded neighborhood does not establish full historical event coverage or lifetime price completeness.
