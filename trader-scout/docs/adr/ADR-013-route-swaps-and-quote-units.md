# ADR-013: Route swaps, wallet-side consideration and per-quote-unit ledgers (Solana)

Status: Accepted
Date: 2026-10-02
Amends: ADR-010 §2 (consideration only from events), ADR-012 §3 (attribution). Closes the design
part of TICKETS P0.18 for signer-owned routes.

## Context

Investigation of two GMGN "top trader" wallets (`router_wallet_{9oC3,tAwv}_page_2026-10-02.json`,
200 txs) found:

- They trade **USDC ↔ token**; PumpSwap is one hop (or one split leg) of a route built by
  aggregator/router programs (`DF1ow4ts…`, Jupiter v6, `proVF4pM…`) across PumpSwap, Meteora DLMM,
  Raydium CLMM/CPMM, Orca Whirlpool. Atomic arbitrage: 0/200.
- In 100% of the 62 successful txs with a decoded PumpSwap leg the wallet is a signer, is **not**
  the fee payer (a shared relayer is), and its owner-keyed deltas are an opposite-signed pair: one
  traded token, one quote asset (USDC). Pass-through owners (`ARu4n5mF…`, used by `proVF4pM`) net
  exactly zero on every mint (16/16) and never sign.
- **Bug (ledger/3):** 7 Jupiter-routed trades were priced from the PumpSwap event (e.g. 24.47 SOL)
  while the wallet paid USDC and its only SOL movement was −550,840 lamports of ATA rent: the
  reconciler accepted a 99.998 % "quote residual". Event prices of one hop are not the wallet's
  consideration.

## Decision

1. **Event pricing guard (bug fix).** An AMM or curve trade may take its consideration from the
   paired event only if, in that transaction, the wallet's owner-keyed non-zero net deltas involve
   no asset other than the traded token and SOL/wSOL, and for a buy the wallet's SOL outflow
   (native + wSOL, fee added back if payer) is at least the event cost (rent, tips and platform fees
   only add to it). Otherwise the event is not the wallet's price; the trade is handled by §2 or
   recorded with Unknown consideration (`RouteLegNotWalletPrice`). Never a partial-hop price.
2. **Route swap (wallet-side consideration).** A successful transaction is a *route swap* of wallet
   W when all hold:
   a. W is a signer; relayer co-signers/fee payers are allowed and are never attributed.
   b. At least one decoded, FixtureVerified swap leg (pump curve or PumpSwap) in the transaction
      trades token T (invariant #1: decoder evidence that this is a swap, not a transfer).
   c. W's owner-keyed non-zero net deltas are exactly one traded token T and one quote asset Q from
      the allowed set, with opposite signs (SOL native movement that is only rent/tips/fees is
      ignored when Q ≠ SOL and is reported in the native-residual diagnostic).
   d. Every owner named as `user` of a decoded leg other than W is not a signer and nets exactly zero
      on every mint (pass-through). Otherwise not attributed (router-forward to another owner,
      P0.18 remains open for that case).
   Then the trade is booked from W's own deltas: token amount = |ΔT|, consideration = |ΔQ| in Q's
   unit (fee: if W is the fee payer, ADR-010 §4 applies in SOL and only for SOL-quoted trades; a
   SOL fee on a USDC trade is overhead in the SOL ledger's diagnostics, not mixed into USDC basis).
   This is exact (balances are integers) and is the wallet's true price; it is allowed **only**
   under a–d. Single-hop direct trades that satisfy §1 keep event pricing (the two agree).
3. **Quote units.** Allowed Q: SOL (lamports, native+wSOL), USDC
   (`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`, raw 6-dp units), USDT
   (`Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB`, raw 6-dp units). Each is its own `QuoteUnit`;
   stablecoins are **not** assumed to equal USD or each other (no FX, invariant #6). Any other Q →
   trade recorded, PnL `Unknown { UnsupportedQuoteAsset }`.
4. **Per-quote-unit accounting.** Each lot carries its quote unit. A disposal consuming lots of a
   different unit than its proceeds → that part's PnL is `Unknown { CrossQuoteUnit }`; the episode
   is `ClosedUnknown`. Realized PnL, consumed basis, ROI and profit factor are reported **per quote
   unit** and never summed across units. Win/loss/breakeven is the sign of a `ClosedKnown`
   episode's PnL in its own unit, so win rate is defined across units; it is reported overall and
   per unit.
5. **Ranking.** `wallet-rank --quote sol|usdc|usdt` (default `sol`) selects the unit for PnL/ROI/PF
   ranking and gates on that unit's closed episodes; other units' figures are shown, never mixed.
6. **Versions.** Ledger version bump; report scope lists allowed quote units and the route rule.

## Consequences

- Route traders that pay in USDC become measurable in USDC; SOL-quoted bot users keep event pricing.
- Rule 2c deliberately rejects multi-asset transactions (e.g. two tokens bought at once); they stay
  unknown until a decomposition rule is evidenced.
- Rule 2 trusts W's deltas only after decoder evidence of a swap; plain transfers can never become
  trades (invariant #1). Undecoded-venue-only transactions (no FixtureVerified leg) remain
  continuity breaks (P4.1 follow-ups: DLMM, Raydium, Whirlpool decoders).
