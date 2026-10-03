# ADR-017: OKX DEX Router order events as swap and ownership evidence (Solana)

Status: Accepted (2026-10-03)
Date: 2026-10-03
Amends: ADR-013 §2b (leg evidence) and §2d (ownership through routers); ADR-009 (verification).

## Context

`proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u` is "OKX: DEX Router" (its on-chain Anchor IDL, pinned as
`fixtures/okx_dex_router_onchain_idl_2026-10-03.json`, sha256 `c1f85197…54c9`). After ADR-015 it is
the router behind every remaining unbooked route-shaped transaction on the router fixtures (2 + 2)
and behind ADR-009's router-forward case `bUh87USD`. Its order events
(`SwapWithFeesCpiEvent2` and five sibling variants) are emitted as Anchor event-CPIs to itself
(authority `Ag3hiK9svNixH9Vu5sD2CmK5fyDWrx9a1iVSbZW22bUS`, 106/106) and carry `source_mint`,
`destination_mint`, **`source_token_account_owner`, `destination_token_account_owner`**,
`amount_in`, `source_token_change`, `destination_token_change` (+ fee tails). Per-hop `SwapEvent`
carries only a `Dex` enum and amounts (no mints).

## Decision

1. Runtime gate: program id + event-CPI tag + authority + exact Borsh length per the pinned IDL.
2. An order event proves a swap `source_mint → destination_mint` (ADR-013 §2b leg evidence). It
   supports attribution to signer W only when source owner = destination owner = W, or that owner is
   a non-signing zero-net pass-through (§2d). Different owners (swap with receiver) → attributed to
   nobody automatically; counted `okx_swap_with_receiver_not_attributed`.
3. Consideration always comes from W's own deltas, never from event amounts.
4. Verification criterion for order events (same standard as ADR-015 legs): the token side is exact
   against the owner's net delta on every live sample, and the quote side is never better for the
   owner than the event (fees routed to other owners only make it worse). Evidence
   (`crates/scout-engine/tests/okx_router_legs.rs`): `SwapWithFeesCpiEvent2` 24 samples — token side
   24/24 exact; quote side 2/22 exact, the other 20 short by 0.52–1.41 USDC in the owner's
   disfavour (e.g. exactly the `commission_account` owner's gain); hops 77/77 match their venue CPI
   on ≥1 side. → `SwapWithFeesCpiEvent2` FixtureVerified; the other five variants IdlOnly (no
   samples). Per-hop `SwapEvent` is never a leg (no mints).
5. Effect on the router fixtures: route swaps 49→51 (9oC3) and 77→79 (tAwv); unbooked route-shaped
   0 / 0. `bUh87USD` (receiver ≠ signer) stays unattributed.
