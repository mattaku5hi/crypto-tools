# Solana PumpSwap virtual-reserve state reconstruction — 2026-10-11

This cached-only v3 supplement resolves the specific post-event virtual-reserve gap recorded in [v2](2026-10-11-solana-virtual-reserve-price-reconciliation.md). It reconstructs coherent pre-transaction, after-first-leg, and after-second-leg cross-pool spots for the same six 48vcic routes. It supersedes v2 only for that state-reconstruction conclusion; v2's raw event arithmetic and 87ea buy-and-burn analysis remain unchanged. The [machine-readable v3 sidecar](../../../.scout-data/diagnostics/solana-virtual-reserve-price-reconciliation-v3-2026-10-11.json) contains raw amounts, event hashes, per-step balances, exact fractions, source paths, and output ranges.

## Scope and sources

The scope is the six cached transactions from 2026-10-10 22:04:52 through 22:04:56 UTC for target mint `48vcicPGmTpLVkJPHFeSprEEtuQNvCHJwRfAT997pump`, paired with its BzLq/wSOL quote pool. The analysis decodes 12 PumpSwap events in canonical log order and examines no other transactions. It uses the cached [transaction neighborhood](../../../.scout-data/diagnostics/solana-48-st-peak-neighborhood-2026-10-11/evidence.json), [Helius transaction history](../../../.scout-data/diagnostics/solana-pump-mint-supply-proof-2026-10-11/history-48vcicP-page-01.json), the pinned local decoder and IDL referenced by the sidecar, and cached Coinbase/Kraken minute FX observations. No API, RPC, provider, or database calls were made for this reconstruction.

Pump's public documentation was frozen from repository commit `2293f9a66c654e9fe82dc5e8f4618538f24bb35f`; the pinned [virtual-reserve fee adjustment](https://github.com/pump-fun/pump-public-docs/blob/2293f9a66c654e9fe82dc5e8f4618538f24bb35f/docs/VIRTUAL_QUOTE_RESERVES_FEE_ADJUSTMENT.md), [V2 trade semantics](https://github.com/pump-fun/pump-public-docs/blob/2293f9a66c654e9fe82dc5e8f4618538f24bb35f/docs/instructions/PUMP_SWAP_TRADE_V2.md), and [`multi_hop_swap` semantics](https://github.com/pump-fun/pump-public-docs/blob/2293f9a66c654e9fe82dc5e8f4618538f24bb35f/docs/instructions/MULTI_HOP_SWAP.md) describe the fee treatment used below. This pins current public documentation, not the historical deployment revision of the program.

## Reconstructing the intermediate states

For these pools, effective quote is `real quote vault + virtual quote reserve`. The pinned docs say V2 trades and `multi_hop_swap` retain protocol and creator fees in the quote vault and reduce the virtual reserve by the retained fee amount; the buyback portion is paid during the trade. Fee sweeps reverse that adjustment. V1 pays protocol and creator fees during each trade, so its virtual reserve does not change from fee retention. The same docs define effective quote as real quote plus boost reserves.

Each of the five `multi_hop_swap` transactions has no boost-buy-and-burn or fee-sweep operation in its cached instruction/event sequence, so the reconstruction uses zero **boost-reserve change** during that instruction. This does not assume the absolute boost reserve is zero. The remaining transaction is a V1 `buy_exact_quote_in`, with fees paid out and no virtual-reserve fee adjustment.

For each event, the decoded pre-state and exact event amounts determine the post-state. For V2, the virtual reserve decreases by retained protocol fee (`protocol_fee - buyback_fee`) and retained creator fee. Real quote changes by gross quote input less paid buyback on buys, or by net user output plus paid buyback on sells. For V1, the virtual reserve stays unchanged and the real quote vault changes by the event's pool quote credit/debit. Real base changes by the event base amount. All calculations use integer raw units and exact rational arithmetic.

The transaction has an ordered pair of pool events: after the first event, only that pool is advanced; after the second event, both pools are advanced. For each event, the reconstructed real base and quote balances match the cached transaction's final token-vault balances. Across all 12 events, the reconstructed effective-quote delta matches the event's net quote flow, and exact constant-product output floors differ from observed output by at most 64 raw units. These independent balance and event checks support the intermediate-state arithmetic for this bounded sample.

## Cross-pool spot comparison

The cross-pool spot is target-token price through BzLq/wSOL, converted by the cached SOL/USD minute range. The tracker candidate is `$0.00013242878993078837` at `2026-10-10 22:04:55.327 UTC`. Coinbase and Kraken overlap at `$110.15–$110.21` SOL/USD; the conservative outer envelope is `$110.14–$110.23`. Each table cell shows the full range from that outer envelope and the percent by which its low-to-high endpoints remain below the tracker candidate.

| Transaction / ordered event logs | Transaction pre-state | After first event | After second event |
|---|---:|---:|---:|
| `4vBo6wNt…` (22:04:52), target sell 15 → quote sell 29 | $0.0001248614–$0.0001249634 (5.71–5.64% below) | $0.0001225239–$0.0001226240 (7.48–7.40% below) | $0.0001217265–$0.0001218260 (8.08–8.01% below) |
| `4EeoBCV1…` (22:04:54), quote buy 75 → target buy 113 | $0.0001226874–$0.0001227877 (7.36–7.28% below) | $0.0001227105–$0.0001228108 (7.34–7.26% below) | $0.0001227771–$0.0001228774 (7.29–7.21% below) |
| `LLtKSLhK…` (22:04:54), quote buy 43 → target buy 55 | $0.0001287951–$0.0001289003 (2.74–2.66% below) | $0.0001288894–$0.0001289947 (2.67–2.59% below) | $0.0001291558–$0.0001292613 (2.47–2.39% below) |
| `3Uv1xWxK…` (22:04:55), target sell 15 → quote sell 29 | $0.0001291433–$0.0001292488 (2.48–2.40% below) | $0.0001247719–$0.0001248738 (5.78–5.70% below) | $0.0001232791–$0.0001233798 (6.91–6.83% below) |
| `TT5fX8WA…` (22:04:56), quote buy 43 → target buy 55 | $0.0001203503–$0.0001204486 (9.12–9.05% below) | $0.0001205297–$0.0001206282 (8.99–8.91% below) | $0.0001210545–$0.0001211534 (8.59–8.51% below) |
| `5MLWiCbH…` (22:04:56), quote buy 20 → target buy 32 | $0.0001193228–$0.0001194203 (9.90–9.82% below) | $0.0001195018–$0.0001195995 (9.76–9.69% below) | $0.0001200277–$0.0001201258 (9.36–9.29% below) |

The closest reconstructed state is `LLtKSLhK…` after target event log 55: `$0.0001291557961776–$0.0001292613347799`, or 2.47–2.39% below the tracker candidate. No one of the 18 reconstructed states is within 1%. The candidate's millisecond timestamp does not establish that any one of these second-resolution transactions is its underlying trade.

## Limits

This replaces the previous statement that after-first-leg and transaction-end spots could not be reconstructed from the cached evidence. It does not prove that this six-transaction neighborhood contains the exact transaction behind the tracker mark, and it does not establish full pool-history or lifetime price coverage. The two 87ea-quoted buy-and-burn examples in v2 remain buy-and-burn operations rather than user acquisitions; their USD conversion is unresolved, and no SOL or USD peg is assumed. The result therefore supplies no ≤1% spot corroboration for the candidate and makes no claim about the token's complete ATH.
