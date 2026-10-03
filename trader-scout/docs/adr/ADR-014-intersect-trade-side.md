# ADR-014: Trade-side matching in buyer-intersect (`--side buy|sell|any`, default `any`)

Status: Accepted (owner decision, 2026-10-03)
Date: 2026-10-03
Amends: CLI.md §3 (buyer-intersect matched only qualified buys), ADR-003 (buy qualification —
unchanged, extended with a sell counterpart and more venues).

## Context

The owner needs the intersect utility to find wallets that **traded** (bought or sold) several of the
input tokens, not only buyers, and needs it soon. At the same time the Solana slice only recognized
pump.fun bonding-curve buys, while live data (`2026-10-02-wallet-rank-live.md`) shows most activity
on migrated tokens happens on PumpSwap AMM and through router/aggregator routes. Both venues and the
ADR-013 route rule are already implemented and verified for the ledger.

## Decision

1. `--side buy|sell|any`, **default `any`** (owner decision). A wallet hits token T when it has at
   least one qualifying operation of the selected side(s) on T within scope. `--min-token-hits K`
   counts distinct input tokens hit (unchanged meaning, side-filtered). `any` = a buy **or** a sell
   on each of the K tokens (owner-confirmed).
2. **Qualifying operation** (never a transfer, airdrop or other receipt — invariant #1; never
   attributed to a router, relayer or fee payer — invariant #2):
   - pump.fun bonding-curve buy: ADR-003/ADR-009 qualification, unchanged; bonding-curve sell: the
     symmetric rule (FixtureVerified `sell`/`sell_v2` variant, decoded `user` = wallet, negative
     owner-keyed net delta of T in the same transaction).
   - PumpSwap trade (ADR-012): decoded `user` = wallet, FixtureVerified variant, wallet's base leg on
     T reconciles exactly; side in token terms (reversed pools inverted per ADR-012 §2). Quote
     funding does not matter for the side (the wallet owns the tokens it received or gave).
     Router-forwards are not attributed.
   - Route swap (ADR-013 §2 a–d): side = sign of the wallet's net delta of T.
   - IdlOnly variants never qualify; they are counted and force incomplete coverage (ADR-009).
3. **Output** keeps the `buyer_match` record kind (downstream JSONL compatibility) and adds, per
   matched token, which sides were observed (`buy`, `sell`) with first-qualifying evidence
   (signature, slot) and per-side counts; run_meta records `side` and the qualification version
   (bumped).
4. **Window.** `--since/--until/--period` as ADR-011 (newest-first walk to the boundary = complete for
   the window). Without a window the mint scan stays oldest-first from token creation (early
   activity first), with today's truncation semantics.
5. Unknown ≠ zero and partial scans keep N and exit 3 exactly as before.

## Consequences

- Results are a superset of the old buy-only set at default settings; `--side buy` reproduces the
  buy-only semantics (now over more venues).
- Undecoded venues (DLMM, Raydium, Whirlpool, Jupiter-only routes without a verified leg) remain a
  lower-bound gap, named in the scope text (P4.9).
