# 48vcic Solana Tracker peak neighborhood — 2026-10-11

Correction: the original private calculation omitted `virtual_quote_reserves` from reserve-ratio spot prices. Its raw evidence and original report are preserved as `report-captured-v1.md` in the diagnostic directory. The invalid spot table has been removed below; use the [v2 reconciliation](2026-10-11-solana-virtual-reserve-price-reconciliation.md) and [v3 state reconstruction](2026-10-11-solana-virtual-reserve-price-reconciliation-v3.md) for corrected prices and limitations. Production decoding already exposes these reserves; this did not establish a production SDK bug, and no database write used the incorrect calculation.

This is a bounded, offline follow-up to the [three-token PumpSwap comparison](2026-10-11-solana-tracker-pump-swap-peak-validation.md). It scans every cached PumpSwap Buy/Sell event for the tracker-reported pool `wc4hQ7k6mZe1hLLfkARE7gDjK6toJDpY2YCPKj8UEr5` from `22:04:52Z` through `22:04:58Z` on 2026-10-10 and records the atomic BzLq/wSOL leg when present. No provider or RPC calls, database writes, or code changes were made. Full extracted events, vault balances, source hashes, and decimal calculations are in [`evidence.json`](../../../.scout-data/diagnostics/solana-48-st-peak-neighborhood-2026-10-11/evidence.json); its SHA-256 is recorded in [`manifest.json`](../../../.scout-data/diagnostics/solana-48-st-peak-neighborhood-2026-10-11/manifest.json).

The tracker returns `0.00013242878993078837` at `2026-10-10T22:04:55.327Z`. That millisecond timestamp does not identify a Solana transaction time. Solana block time in the cached records has whole-second precision; matching its integer second to the provider timestamp would overstate the association. The result below is therefore limited to the six-second pool neighborhood.

## Cached pool events and route prices

All six cached target-pool events in the interval have an atomic BzLq/wSOL swap in the same transaction. The event-side real pre-reserve fields match the raw pre-transaction token-vault balances for both pools in every event. Virtual quote reserves must also enter the effective reserve ratio; real vault balances alone are insufficient. The target pool quote is BzLq, and the second pool is BzLq/wSOL. SOL/USD uses the cached 22:04 UTC Coinbase/Kraken minute range `$110.14–$110.23`.

| UTC block time | Side | Signature prefix | Net route price USD/target | Required SOL/USD to equal tracker high |
|---|---|---|---:|---:|
| 22:04:52 | Sell | `4vBo6wNt…` | 0.0001229131–0.0001230136 | 118.6668 |
| 22:04:54 | Buy | `4EeoBCV1…` | 0.0001242083–0.0001243098 | 117.4294 |
| 22:04:54 | Buy | `LLtKSLhK…` | **0.0001293623–0.0001294680** | **112.7509** |
| 22:04:55 | Sell | `3Uv1xWxK…` | 0.0001257966–0.0001258994 | 115.9468 |
| 22:04:56 | Buy | `TT5fX8W…` | 0.0001210639–0.0001211628 | 120.4794 |
| 22:04:56 | Buy | `5MLWiCbH…` | 0.0001200337–0.0001201318 | 121.5135 |

The highest cached realized route is the 22:04:54 buy, not the 22:04:55 sell. Even its upper USD bound is 2.29% below the tracker value; its implied SOL/USD is `$112.7509`, above the cached same-minute range. The 22:04:55 sell route is roughly 5% below the tracker value. No event in this window reproduces the tracker high within the existing 1% cross-source threshold.

Corrected paired transaction-pre-state spots include real plus virtual quote reserves for both legs. The nearest cached pre-state is `3Uv1…`, `$0.000129143–$0.000129249` per target token, still 2.40–2.48% below Tracker. V3 additionally reconstructs intermediate/post-event virtual reserves using the pinned fee-transition rules and raw vault reconciliation. The best of its18 coherent states is `LLt…` after target log55, `$0.0001291558–$0.0001292613`, still2.39–2.47% below Tracker. Execution averages and reserve-ratio spots remain separate price kinds.

## Supply and limits

The cached mint supply ledger records no mint or burn between 22:04:52 and 22:04:58 UTC. Three earlier burns had already reduced the birth supply; its ledger-derived supply at the end of the interval is `997,060,502.547477` tokens. This is only the ledger’s bounded archived scope and does not identify the source’s price/supply convention.

The interval contains the nearest on-chain candidate trades, but cannot determine whether the tracker timestamp represents a trailing candle, a delayed indexer update, another transaction, or another price basis. It does not establish the global ATH, and it does not change the cross-source threshold or authorize an ATH/FDV promotion.
