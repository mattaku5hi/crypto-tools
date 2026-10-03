# 2026-10-03 — live verification of Helius `getTransactionsForAddress` filters

Tool: `scout-capture` at commit `3398961` (request built by `HeliusRequestOptions`, same as
`HeliusProvider`). Documented shape: https://www.helius.dev/docs/rpc/gettransactionsforaddress
(read 2026-10-03). API key absent from every written fixture (grep).

| Check | Address | Request | Result |
|---|---|---|---|
| baseline | mint `GAwhcph…` | `limit 100`, desc, no filters | 100 tx, **30 failed** |
| `filters.status = "succeeded"` | same | + status | 100 tx, **0 failed** |
| `filters.blockTime {gte, lt}` | same | asc, `[2026-10-02T12:00Z, 13:00Z)` | first tx blockTime = 1790942400 = `since` exactly |
| `limit = 1000` | same | + status succeeded | 1,000 tx in one page; 16,101,966 bytes compact JSON (16.1 KB/tx; p50 16.2 KB, p99 30.6 KB, max 34.0 KB) |
| `filters.tokenAccounts` | wallet `2tgUbS9U…` | 3 pages × 1000, 7-day window, `none` vs `balanceChanged` | over the common 6,990 s range: `none` 2,900 tx, `balanceChanged` 3,000 tx; 100 tx only in `balanceChanged`, 0 only in `none`. The 100 extra txs do not list the wallet in `accountKeys` but change the balance of a token account it owns |

## Decisions (defaults flipped)

- `wallet-stats` / `wallet-rank`: `tokenAccounts = balanceChanged` by default — with `none`, 3.3 % of
  the wallet's balance-changing transactions were invisible (silent completeness loss, invariant
  #18). Status filter never applied to wallet scans (ADR-004 failed-tx fee overhead needs failed txs).
- `buyer-intersect`: `status = succeeded` by default — failed transactions never qualify, and they
  were ~25–30 % of busy-token pages.
- All three: server-side `blockTime` window when `--since/--until/--period` is given; page size 500
  (≈8 MB at the measured p50, 2× under the 16 MiB body cap; 1000-tx pages are allowed via
  `--page-limit` with a derived cap ≤ 64 MiB).
- Metering per docs: full mode 10 credits per 100 returned txs — page size changes request count,
  not credits; the status filter reduces credits by the share of failed txs.
