# Solana runner-potential price × supply window audit — 2026-10-11

## Result

The cached Solana Tracker chart for JC supports a **maximum among its 43 returned high-price rows** of $227310.99424807910909633689232512 (floor-cent value $227310.99), at 2026-10-10T22:44:14+00:00. The lower and upper supply-bound maxima occur on that same row and have the same floor-cent value (22731099 cents). This is a row-scoped result, not a provider-window or lifetime ATH determination.

JC’s source observation reports $0.00023536979981208752 per token at 2026-10-10T22:44:14.212000+00:00 (pool `4QnxqajHAqiKuTZjfsfkBA8qSxb7AY4p8axVZeNmVNuq`; response SHA-256 `f8eca68d1dd8719417ba4eaab99aae2ce3d2c1ab526d9cff2d11d0c7855e58da`). The returned chart high at 1791672254 is the same value, and the provider timestamp is within that second. The exact-second supply is 965,761,089.271256 tokens; there were no burns in that bucket. The mint-history ledger has 87 successful transactions, 30 burns, zero failed rows, and reconciles birth supply 1,000,000,000 to the finalized anchor.

## Method and coverage

For every chart row at Unix second `t`, the conservative supply upper bound subtracts only burns with `block_time < t`; the lower bound also subtracts burns at `t`. The calculation uses exact decimal inputs with 128-digit Decimal context and floors each FDV bound to cents. Across the 43 returned rows, both maximum endpoints occur at `t=1791672254` and produce the same floor-cent result. The complete row-by-row values and source hashes are in [the private diagnostic sidecar](../../../.scout-data/diagnostics/solana-runner-potential-supply-window-audit-2026-10-11/jc-chart-observed-row-fdv-maximum.json).

The chart request was `type=1s`, USD, with timezone omitted. The verified raw response covers returned timestamps 1791671912 through 1791672254: 43 rows across 342 seconds, with a maximum gap of 75 seconds. One row has positive high but zero low, close, and volume; raw values remain unchanged. The provider response contract does not establish that missing timestamps mean no trades or unchanged prices. Thus an unreturned second could contain a higher price, and the returned-row maximum cannot be promoted to a maximum over the whole recording window.

The sidecar retains raw chart SHA-256 `e0d4399401ee02baacab63fe217b42c6598b26564492fdb114ee9fafa8865fc0`, request-manifest SHA-256 `2b5806c5527fce11bff2a9d70ea2b47b1ed8eb59d0970d23478143225f8589b4`, supply-ledger SHA-256 `8fb5cb4815b58261591f335406be33e7586b1352b1fe5516bb73ea2e6fcb1e01`, and the source-observation SHA-256 `f8eca68d1dd8719417ba4eaab99aae2ce3d2c1ab526d9cff2d11d0c7855e58da`. No API requests or database writes were made for this audit.

## Remaining proof

To claim a provider-window FDV maximum, the price source must provide a coverage-complete high series for the full relevant recording interval (or document that omitted buckets are complete no-trade buckets), and those highs must be joined against supply at each bucket with ordering uncertainty bounded. Otherwise a verified on-chain all-pool price scan with supported quote-to-USD conversion and complete pool scope is needed. This audit does not resolve that gap for 48vcic or 81kia, and does not assert a lifetime ATH for any candidate.
