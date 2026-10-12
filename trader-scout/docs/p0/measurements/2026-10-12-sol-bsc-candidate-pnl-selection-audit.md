# SOL/BSC candidate PnL selection audit — 2026-10-12

This read-only audit checks the saved candidate lists and PnL exports against the user’s new Axiom screening rule: realized and total PnL must each meet $10,000 over 30 days, and each meet $30,000 over Max. No live Axiom data was fetched. The current artifacts can supply a 30-day **realized-net-USD** pre-screen, but they do not certify this compound rule: the saved tables have no Axiom-compatible total-PnL field or maximum-period PnL value. Open-position `value_usd` is gross realizable value, not unrealized PnL; some raw open-position metrics are unknown. Do not treat the candidate counts below as passes. Wallet addresses are abbreviated here; full lists remain in ignored local inputs.

## Why the former SOL “top 100” is not a profit filter

`inputs/results/solana_leaderboard_top100_early_k2plus.csv` is a 100-wallet discovery list. Its columns rank K≥2 wallets using early-entry counts, full cycles, list trades, recent transaction density, and related discovery fields; it has no PnL column. PnL was joined afterward. Among the 100 joined rows, 98 have numeric 30-day realized net PnL: **84 are negative (85.71% of known rows), 14 are positive, and two are scan errors with unknown PnL**. The separate 98-row `solana_leaderboard_top100_early_k2plus_pnl30d_by_pnl.csv` orders those realized values descending, while retaining the original discovery rank: the top row is original rank 69, not discovery rank 1. Sorting by PnL did not filter on profit.

The 58-row `solana_leaderboard_top100_early_k2plus_pnl30d_tpd100_by_pnl.csv` applies only an activity-density constraint: positive active days and no more than 100 buy-plus-sell trades per active day. It still contains 52 negative and 6 positive realized results. This is not a profitability filter. The saved SOL outputs have no Axiom total/max fields or quality-gate manifest. This behavior is consistent with current policy rather than a malformed filter: [ADR-019](../../adr/ADR-019-open-position-realizable-valuation.md) keeps default ranking realized-only, and [ADR-016](../../adr/ADR-016-unknown-episode-materiality.md) preserves unknown episodes and uses explicit lower bounds. The SOL exports rank `pnl30d_realized_net_usd` (known-subset net), do not expose the corresponding realized-PnL lower-bound field, and expose `pnl30d_open_value_usd` as position value rather than total/unrealized PnL. An `ok` scan status is not itself an unknown-episode or Axiom profitability gate.

The SOL K2+ top-100 scan used the 30-day window 2026-09-08 09:53:27Z through 2026-10-08 09:53:27Z, with a 30-page / 30,000-transaction per-wallet ceiling. These dates come from the source JSONL run metadata; the joined CSV itself omits per-row window fields. The K3 run used a different 30-day window: 2026-09-09 18:41:47Z through 2026-10-09 18:41:47Z, with the same stated page/transaction ceilings.

## Broader SOL K3 candidate source

`solana_k3_candidates.csv` has 2,972 K≥3 discovery wallets: 2,274 human-like, 426 suspect, and 272 bot-classified. The existing full PnL output covers the 2,274 human-like wallets, not every K3 candidate. Exact Decimal counts for that 30-day realized-net column are **1,780 negative, 444 positive, and 50 zero**. Thirty-six rows meet the realized-only $10,000 pre-screen (28 `ok`, 8 `incomplete`); eleven meet $30,000 (7 `ok`, 4 `incomplete`). These results explain why a broad K3 pass can surface additional high-PnL candidates despite most of the cohort losing money.

The seven `ok` rows with 30-day realized net ≥$30,000 are a useful Axiom lookup queue, not confirmed Axiom qualifiers:

| Wallet | Saved 30-day realized net | Status | Unknown episode share |
|---|---:|---|---:|
| `AxKt...xi45` | $74,756.17035333 | ok | 11.26% |
| `EzKD...uSLt` | $57,442.57010699 | ok | 26.28% |
| `HPZR...7SAU` | $47,655.91948802 | ok | 21.07% |
| `6yJ3...aL8AV` | $43,772.29799494 | ok | 8.37% |
| `3zFn...qfPQ` | $35,623.21007605 | ok | 40.24% |
| `DHDh...MaSe` | $35,440.03574919 | ok | 6.59% |
| `GyeW...DVMy` | $33,336.31738703 | ok | 9.41% |

Four further K3 human-like rows exceed $30,000 realized but are marked `incomplete`; they remain unknown-quality candidates rather than passes. The saved full K3 PnL table preserves all 2,274 human-like rows, including negative and zero results.

## K2+, K3, and BSC input choices

For broad SOL K3+ follow-up, use `inputs/results/solana_k3_candidates.csv` as the full 2,972-wallet input. If continuing the existing human-like PnL screen, use `inputs/results/solana_k3_human_like.txt` and its 2,274-row `inputs/results/solana_k3_human_pnl30d_by_pnl.csv` / `.json`. The human-like list is a behavior-class filter, not a profitability filter.

The only saved SOL K2+ candidate file in this result set is the early-ranked 100-wallet slice (`solana_leaderboard_top100_early_k2plus.csv` and its JSON). I found no full K2+ candidate list among the named result files. Within its 30-day PnL join, one `ok` wallet meets realized ≥$10,000: `xfFh...d57U` at $11,482.91450821. The largest row, `88pt...usbW` at $35,767.84866384, is `incomplete`; do not promote it from the number alone.

For broad BSC K2 follow-up, `inputs/results/bsc_k2_candidates_with_curve.csv` is the 732-wallet discovery input (355 human-like, 243 suspect, 134 bot). `inputs/results/pnl/bsc_k2_human_wallets.txt` and `inputs/results/bsc_k2_human_pnl30d_by_pnl.csv` are the existing 355-wallet human-like screen. In that full human-like PnL table, 69 are positive, 245 negative, and 41 zero; **none** reaches $10,000 30-day realized net. The strongest saved value is $7,439.73282469 and is incomplete, so this BSC set has no current realized-only candidate for the $10,000 30-day gate.

The BSC early-ranked top-100 PnL table is another selection slice, not a profit gate: 22 positive, 64 negative, and 14 zero/no-trade rows; none reaches $10,000. Its TPD100 variant has 92 rows (22 positive, 64 negative, 6 zero) and also has no realized value ≥$10,000. Its manifest explicitly records `quality_gates_applied: false`; its only additional predicate is the exact trades-per-active-day screen. The full BSC human-like export combines three source runs and retains per-wallet window and source ID: 298 rows use Sep 9–Oct 9, 56 use Sep 10–Oct 10, and one corrected row uses the latter window.

## Exact offline recheck

The deterministic offline check is `.scout-data/diagnostics/sol-bsc-candidate-pnl-selection-audit-2026-10-12/recheck.py`. It uses `Decimal`, verifies candidate/PnL wallet-set joins, checks the sorted realized PnL ordering and exact TPD100 integer predicate, and asserts the counts above. Command:

```sh
python3 .scout-data/diagnostics/sol-bsc-candidate-pnl-selection-audit-2026-10-12/recheck.py
```

Result: `PASS: exact Decimal counts, CSV/JSON row joins, source-list joins, PnL descending order, and TPD100 predicate`.

## Source hashes

Hashes are SHA-256 of the exact saved files read for this audit.

| Artifact | SHA-256 |
|---|---|
| SOL K2+ early top-100 discovery CSV | `e1e76e6aa74b961261d630c76a195ecc259455d091a0233a4bf6db3a0442f427` |
| SOL K2+ early top-100 discovery JSON | `79c9ddc674e90edc8cbb771d30efa539d97da43a744c4b60e86b952866e3edf7` |
| SOL top-100 30d joined CSV/JSON (100 rows) | CSV `6b4818cbb6cf00f33be808e96967a511e3ced2ef584a4ba8171c14b7312a1094`; JSON `5a361ed4117f2d144d91e9be26fd5f927a9264077c27b06203486823cc2ff873` |
| SOL top-100 30d PnL-sorted CSV/JSON (98 rows) | CSV `61b04c74182c58cc5c38a46b419a800fe495962ac3df5ed645457b9cd97df02e`; JSON `291a8855eb8b485e290a571707ef5ed4c80cde2b66e163b33d73e7e9267c9f70` |
| SOL top-100 TPD100 CSV/JSON (58 rows) | CSV `d65276769c56838d8c706274b009757c7df1169d46f2ca152748f4875f56e1c6`; JSON `3267e498609374e178fb1c8c17c215eb5fb2f4b0a390d3dfbab3694dc45f2e39` |
| SOL top-100 30d source JSONL | `58b4af856ae67bc2c35006c6e8029b4e0221133c6cadf40fcaf12fcd5e74d3a7` |
| SOL K3 candidates CSV (2,972 rows) | `6632c7913e14947c9b2043798deda843554b4dda01e4c522dc6bd4db23eaf39a` |
| SOL K3 human-like wallet list (2,274 lines) | `2fc4c3d6f625454cf9787fa5b048c3b79d536cc3274ad94c4e8da99ee2c5ba4b` |
| SOL K3 human 30d PnL CSV/JSON (2,274 rows) | CSV `3bc1ba440e1aff2128fd87c211a53265bf5d232219a2991e88f6cd922c3c346c`; JSON `5bdc1b5acb02a4025acaaea35a479a8e6fed4880d7749a0f2b6f20309e597894` |
| SOL K3 30d source JSONL | `f5ea54d8e9f6763ebd9387941626616c69d6319c37c969d26f6309a4423115bf` |
| BSC full K2 discovery CSV (732 rows) | `02aa70337ef097d760045b04aa36130a613af17627c46dc510466680cd59829c` |
| BSC human K2 wallet list / PnL CSV+JSON / manifest (355 rows) | wallet list `9afbe511ee4148512cfd4f78bbcb0acd44dbe77a515ce183eb9b36c351441ed6`; CSV `aaf6b4c0c4fe2fac09c06779c53a188d779edcbc2538cce9ec44465bae029caf`; JSON `6aa4d0d59bf1e190588bded3245c9ba64d325533cc2835e4c40c65d4148e41f8`; manifest `dcd42707970f92f3b9edfa137d984cbb86a03c77675ddaf8b9464e1bd068f5da` |
| BSC early top-100 input CSV | `1d5e6adaac1ff66e2db6498e526eb9a98058cc8b2a0709e10e22a0c90c1c3c82` |
| BSC top-100 PnL CSV / manifest (quality gates false) | CSV `865ec3136de379263705851bb90a72df178a2ae2dc8a1c7014578cbd2a217e84`; manifest `d0e43bc57784f24e151ce30809166af8891b71c1f975f0c64f0adf35fbb3636b` |
| BSC TPD100 CSV (92 rows) | `40fd239ca1e1097db39a0e29f03020789838ef0b24f7a05aa1f289b3e44473d5` |
| BSC source PNL JSONL files (first, retry, final correction) | `fb6789e18640393ba21ea7bd817f783fe6a0aefe61ef97767f3221c0c695b939`; `718d9c3ca9934f566b456d4d3e852bddd42553e6010cf83cb8313e5c1b70a3f3`; `37cc1cbd525611a2b5e60c914956ccdbe003f3b14f8c39d29fe20df0c5149122` |
