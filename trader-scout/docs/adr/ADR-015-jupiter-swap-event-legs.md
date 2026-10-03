# ADR-015: Jupiter v6 `SwapEvent` as verified swap-leg evidence for route swaps

Status: Proposed (finalize after live-fixture verification, P4.9)
Date: 2026-10-03
Amends: ADR-013 §2b (which legs count as decoder evidence of a swap).

## Context

ADR-013 books a signer-owned route swap from the wallet's own deltas only when at least one decoded,
FixtureVerified leg (pump.fun curve or PumpSwap) trades the token T. Live data
(`2026-10-02-wallet-rank-live.md`, ADR-013 re-run) leaves 189 outbound / 66 inbound unexplained
token movements in transactions with no such leg — largely routes through Meteora DLMM, Raydium
and Orca Whirlpool, often via Jupiter v6 (`JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`).

Jupiter v6's published IDL (`jup-ag/jupiter-cpi` `idl.json` @ `12bc5f67b94a2c3edc74d6e721a19442124a0bad`,
sha256 `764ea6d71b77458fd33aeb308d6e6bb19e660fc5320c5359f3b9cac96eba5c50`, pinned in
`fixtures/jupiter_v6_idl_12bc5f67.json`) declares `SwapEvent { amm: Pubkey, input_mint: Pubkey,
input_amount: u64, output_mint: Pubkey, output_amount: u64 }` (Anchor event, discriminator
`sha256("event:SwapEvent")[:8]` = `40c6cde8260871e2`), emitted once per executed hop through the
Anchor event-CPI wrapper (`e445a52e51cb9a1d`) to Jupiter itself, and `FeeEvent`.

## Decision (proposed)

1. A Jupiter `SwapEvent` emitted by the Jupiter program (program-id gated, inner instruction to
   itself with the event-CPI tag) is a **swap leg** for ADR-013 §2b when the event layout decodes
   exactly (no trailing bytes beyond the IDL unless an amendment documents them) and the event
   variant is `FixtureVerified` per ADR-009: on live fixtures, every hop's `input_amount` /
   `output_amount` equals the net balance change of the `amm`'s vault accounts for those mints in
   that transaction (or the route's first/last hop equals the signer's net deltas).
2. The event proves a swap of `input_mint` → `output_mint`; it does **not** attribute ownership.
   Ownership and consideration still come only from ADR-013 §2 a, c, d (signer, opposite-signed
   T/Q deltas, zero-net pass-throughs). A leg "trades T" when T is its input or output mint.
3. Venue programs CPI'd by Jupiter (DLMM, Raydium, Whirlpool, …) are not thereby "supported" as
   decoders (invariant #16); only the Jupiter event is trusted, and only as leg evidence.
4. Routes not going through Jupiter (e.g. the unidentified `DF1ow4ts…`) remain without evidence
   until their own events/IDLs are verified.

## Consequences

- Jupiter-routed trades on undecoded venues become bookable by ADR-013 at the wallet's exact deltas.
- If live verification fails (layout drift since the 2024 IDL), this ADR stays Proposed and the
  evidence is recorded instead.
