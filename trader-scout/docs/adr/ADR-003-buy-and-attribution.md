# ADR-003: Buy definition & owner attribution

Status: Accepted
Date: 2026-09-22

## Context

AGENTS.md invariants #1, #2, #6 and ARCHITECTURE.md §7 define what counts as a "buy" for
`buyer-intersect` and forbid treating routers/relayers/first-signers as the economic owner without
evidence. ACCEPTANCE B01-B10 and E05 give concrete test cases (router hops, atomic roundtrips, fee
payer ≠ owner, relayer sender).

## Decision

### Buy definition (v1)

A qualifying buy is **net acquisition by wallet/token within a single successful transaction**:

1. The token actually reached the economic owner's controlled accounts (not merely passed through as
   a router hop).
2. A recognized exchange execution occurred (decoded swap/bonding-curve buy), with the owner paying
   real consideration — not a transfer, airdrop, reward, bridge receipt, wrap/unwrap, or LP withdrawal.
3. The **net delta** of that token across the owner's economic accounts in the transaction is
   positive.

This is computed as a per-transaction net, not a per-instruction/per-leg count — B02 ("A bought T1 a
hundred times across different pools" still yields `hit_count=2`, not per-buy-event counting) and B04
(SOL→USDC→T1 routing is one economic buy of T1; USDC intermediate nets to zero and is not itself a
hit) both fall out of "net delta per (wallet, token, tx)", not "count every decoded swap leg".

### Owner attribution

```rust
pub enum AttributionStatus {
    Confident { owner: WalletKey, evidence: AttributionEvidence },
    Ambiguous { candidates: Vec<WalletKey>, reason: AmbiguityReason },
}
```

`tx.from`, the first transaction signer, and `Swap.sender`/`recipient` fields are **candidate**
signals only — never auto-promoted to owner. `AttributionEvidence` records which signal(s) actually
justified the confident classification (e.g. "recipient token account's tracked owner matches, and no
router/relayer pattern detected for this protocol/version"). A wallet resolved only via an
unconfident signal (B05: relayer sender with no independent evidence) is `Ambiguous` and is excluded
from the *strict* buyer list — it does not silently become a confident hit.

### Roundtrips and self-transfers

Atomic roundtrips with zero net end-of-transaction acquisition (B06) never qualify as a buy for
`buyer-intersect`, but the underlying decoded actions and their fees are still recorded in the ledger
(AGENTS.md invariant #1: individual actions are preserved, not netted away at the raw-event level —
only the *buy qualification* logic nets within the transaction).

Self-transfers between token accounts of the same confidently-attributed owner net to zero and do not
create spurious buy/sell pairs.

### Hit counting for buyer-intersect

Each distinct input `AssetKey` maps to one `TokenId` slot. Each qualifying `(WalletKey, TokenId)` pair
contributes exactly one hit regardless of how many buy events or pools produced it (B02). A wallet's
`hit_count` is the cardinality of qualifying distinct `TokenId`s, not a sum of buy events.

## Consequences

- `scout-normalize` must expose per-transaction net-delta computation as a first-class operation, not
  bury it inside a swap decoder — both EVM and Solana decoders feed a shared normalization step.
- Any decoder that reports "buy" per matched DEX event (rather than per net-delta-positive
  wallet/token/tx) is a bug relative to this ADR and must be fixed before its DEX is declared
  supported (AGENTS.md invariant #16/#20: no stub support).
- `Ambiguous` attributions still flow into `wallet-stats` (full transparency) but are excluded from
  strict `buyer-intersect` hits and from strict `wallet-rank` quality-gated eligibility.

## Alternatives considered

- Counting every decoded swap execution as a hit-contributing event: rejected — directly contradicts
  B02 and would make `hit_count` a volume metric instead of a distinct-token-coverage metric.
- Treating `tx.from` as owner by default with an opt-out flag: rejected — AGENTS.md invariant #2 is
  phrased as a hard "is not" (не являются универсальным идентификатором), not a default-with-override;
  ambiguity must be the explicit fallback, not confident attribution the default.
