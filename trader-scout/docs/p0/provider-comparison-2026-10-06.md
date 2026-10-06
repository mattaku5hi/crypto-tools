# 2026-10-06 — provider comparison (Solana, BSC, Base, Robinhood)

Owner constraint: no subscription; pay-as-you-go above a free allowance, at most a few dollars a month;
decent quality. Figures below are from the providers' pricing/doc pages fetched 2026-10-06 unless marked
**(3rd-party)** — those come from comparison blogs and must be confirmed on the provider's own page or by
a live test before a decision.

## Terms

| Provider | Free allowance | Free rate | Pay-as-you-go without subscription | Unit price | Chains we need |
|---|---|---|---|---|---|
| Helius (Solana only) | 1M credits/month | 10 req/s | **no** — extra credits ($5/M) only on paid plans from $49/month | `getTransactionsForAddress`: full = 10 credits per 100 tx (min 10), signatures-only = 10 flat; other RPC 1 credit, archival 10 | Solana |
| Alchemy | 30M CU/month | 500 CU/s (≈ 25 req/s) | **yes**, "no minimum subscription" | $0.525 / 1M CU; PAYG throughput from 10,000 CU/s; PAYG `eth_getLogs` unlimited block range (free: 10 blocks; responses ≤ 150 MB); debug/trace not on PAYG | Solana, BSC, Base, Robinhood (in use) |
| dRPC | 210M CU/month ≈ 10M requests **(3rd-party)**, public nodes | 100 req/s **(3rd-party)** | yes **(3rd-party)** | flat 20 CU per request, $6 / 1M requests **(3rd-party)** | BSC, Base, Robinhood (listed in Robinhood docs); Solana to verify |
| Chainstack | 3M request units/month | 25 req/s | "PAYG from $2.5 per 1M RU", subscription not required (page) — to confirm | 1 RU per request, archive 2 RU | Robinhood **(3rd-party)**; Solana/BSC/Base to verify on their chain list |
| Ankr | 200M API credits/month **(3rd-party)** | — | yes, $0.10 = 1M credits **(3rd-party)** | Solana: 500 credits per request ($50 / 1M requests) **(3rd-party)** | Solana, BSC, Base; Robinhood unknown |
| QuickNode | 10M API credits/month | 15 req/s | **no** — overage only on monthly plans | $0.50–0.62 / 1M credits (plan overage) | all four |

Alchemy CU (billing / throughput): Solana `getTransaction` 40, `getSignaturesForAddress` 40;
EVM `eth_getLogs` 60, `eth_getTransactionReceipt` 20, `eth_getBlockReceipts` **20 billed, 500 throughput**,
`alchemy_getAssetTransfers` 120, `eth_call` 26, `eth_getBalance` 20. Correction to P3.20: the 500-CU weight
of block receipts is the throughput weight; billed it costs 20 CU. On the free tier throughput (500 CU/s)
is the binding limit, which is why the BSC full-lifetime scan took hours while using little quota.

## Owner corrections (2026-10-06)

- Chainstack pay-as-you-go is offered only to "authorized teams"; no free tier for our use → dropped.
- QuickNode's free plan is a one-month trial, then a subscription → dropped.
- Remaining no-subscription options: Alchemy (free + PAYG) and dRPC (free). Decision: dRPC free plan as an
  automatic fallback for every chain (`SCOUT_<CHAIN>_FALLBACK_RPC_URL`, task A7).

## Our measured workloads, priced

- **Solana, one full transaction read.** Helius batch: 0.1 credit → the free 1M credits ≈ 10M tx/month
  (the owner's list burned it in about a day: ~4M full tx over several passes plus earlier runs).
  Per-transaction RPC elsewhere (`getSignaturesForAddress` + `getTransaction`): Alchemy 40 CU ≈ $21 / 1M tx;
  dRPC ≈ $6 / 1M tx beyond ≈ 10M free requests (3rd-party); Chainstack archive 2 RU ≈ $5 / 1M tx (to
  confirm); Ankr ≈ $50 / 1M tx (3rd-party). Per-transaction calls are also ~100x more requests than the
  Helius batch, so they are slow at free rate limits (100 req/s → 1M tx ≈ 2.8 h).
- **BSC full-lifetime token scan (4 four.meme tokens).** ~211k+ requests, mostly receipts at 20 CU billed:
  ≈ 4–5M CU ≈ $2–3 on Alchemy PAYG, and PAYG throughput (10,000 vs 500 CU/s) would cut hours to minutes;
  PAYG `eth_getLogs` has no 10-block cap, so the transfers-listing workaround becomes optional.
- **Dev tracker steady state:** launch/migration events only; not measured yet (B0).

## Working hypothesis (to confirm by live tests)

1. The cost driver is the approach, not only the provider: discovery should not read every full
   transaction of launch tokens with 0.5–2M txs. Two-tier design (ADR to write): cheap discovery
   (transfers / indexed trades / signatures), exact ledger verification only for the top candidates.
2. EVM (BSC, Base, Robinhood): Alchemy PAYG fits the owner's constraint (no subscription, per-CU billing,
   unlimited `getLogs`, 20x throughput). dRPC is the alternative to test (flat price, larger free tier).
3. Solana: Helius free stays the cheapest per transaction (batch reads); overflow without a subscription
   needs a second provider — dRPC or Chainstack are the candidates to test; Alchemy and Ankr are
   expensive per Solana transaction.

## Live test plan (same sample per provider)

- Solana: one listed token, 5,000 transactions from launch — wall time, errors/429, completeness vs our
  stored reference, units charged.
- BSC: one four.meme token — historical `eth_getLogs` over 100k blocks, 5,000 receipts, units charged.
- Base, Robinhood: same as BSC on one token each.
- Needs free keys for dRPC and Chainstack (owner registers); Alchemy PAYG is an account switch (card).
