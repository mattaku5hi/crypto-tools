# ADR-015: Jupiter v6 `SwapEvent` as verified swap-leg evidence for route swaps

Status: Accepted with amendment (2026-10-03, live-fixture verification below)
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

## Amendment — live verification (2026-10-03)

Evidence: `crates/scout-engine/tests/jupiter_swap_legs.rs` over 6 committed fixtures, 37 successful
Jupiter transactions, 111 hops.

- The live program emits **`SwapsEvent`** (disc `982f4eebc0606e6a` = `sha256("event:SwapsEvent")[:8]`)
  for 110/111 hops; it is **not** in the pinned 2024 IDL. Layout (derived from live data, exact length
  in every sample, no trailing bytes): `u32 count` + `count × 112` bytes, item =
  `input_mint, input_amount u64, output_mint, output_amount u64, amm` (amm last). The IDL `SwapEvent`
  occurs once (`pump_mint2_full.json`, `5twkEEg4…`). `FeeEvent` has no live sample → IdlOnly, never a
  leg. Event authority is always `D8cy77BBepLMngZx6ZukaTff5hCt1HrWyKk3Hnd9oitf` (gated).
- `amm` holds the **venue program id** (PumpSwap, DLMM, Raydium CPMM/CLMM…), not a pool. The vault
  check in §1 is replaced by: the hop's venue CPI (program = `amm`, same stack height, before the
  event, matched in order) moves a CPI token account by exactly ±input or ±output amount — 111/111
  hops pass at least one side (both 71, input 83, output 99); intermediate mints conserved 38/38;
  signer edge on the token side exact 31/31. The quote (USDC) edge differs from first/last hop by
  ~4.5 bps routed to other owners (fees), so wallet consideration stays the wallet's own delta
  (ADR-013 §2), never the hop amount.
- Verdict: `SwapsEvent` and `SwapEvent` are FixtureVerified **as leg evidence only**. Re-verify on
  any new IDL pin or layout change. Runtime trust = program id + event-CPI tag + authority + exact
  length; account-level reconciliation is a test-time verification.
- Measured effect (router fixtures): route swaps 36→38 (9oC3) and 62→68 (tAwv); remaining unbooked
  route-shaped txs 13 / 11, of which 11 / 9 go through `DF1ow4ts…` (no verified events).

## Amendment — DFlow Aggregator v4 (2026-10-03)

- `DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH` = DFlow Aggregator v4 (solanacompass program
  analytics; Carbon indexer decoder). No official IDL found; schema pinned from the third-party,
  Codama-generated Carbon decoder (`sevenlabs-hq/carbon` @ `1e6e16b46e0efb0fc9cd6c8684ed9721e7da716a`,
  `fixtures/dflow_aggregator_v4_carbon_1e6e16b_events.rs.txt`, sha256 `7322e999…1252`).
- Events via Anchor event-CPI to itself, authority `8xeaWCsJYxRoudEZGJWURdfrtFhLYZz9b4iHJnW5tb3d`
  (gated; derived from fixtures, 281/281). `SwapEvent` (`40c6cde8260871e2`, 112-byte payload, same
  field order as Jupiter's IDL `SwapEvent`) is leg evidence under §§1–3; `FeeEvent`
  (`494f4e7fb8d50ddc`) is decoded, never a leg; no batched variant observed.
- Live verification (`crates/scout-engine/tests/dflow_swap_legs.rs`): 6 fixtures, 86 successful
  txs, 248 hops — every hop exact on ≥1 side via its venue CPI (in 195, out 223, both 170);
  intermediates conserved 92/93 (one 1-lamport rounding on a curve hop); signer token edge 78/78;
  USDC edge never exact (−27…+200 bps; fees), so consideration stays the wallet's own delta.
- Effect: route swaps 38→49 (9oC3), 68→77 (tAwv); remaining unbooked route-shaped txs 2 / 2, all via
  `proVF4pM…` (no verified events). §4's example of an unevidenced router is now `proVF4pM…`.
  Ledger/7, trade qualification v8.

## Amendment — Jupiter on-chain IDL (2026-10-03)

The on-chain Anchor IDL of `JUP6…` (`fixtures/jupiter_v6_onchain_idl_2026-10-03.json`, sha256
`12a08561…cdca8`) defines `SwapsEvent { swap_events: Vec<SwapEventV2> }`, `SwapEventV2 { input_mint,
input_amount u64, output_mint, output_amount u64, amm }` — exactly the layout derived from live data
above. `SwapsEvent` is now IDL-confirmed and fixture-verified; IDL-equality tests pin it.
`CandidateSwapResults`, `CandidateSwapQuoteError`, `BestSwapOutAmountViolation` are known non-leg
events (gated, named, never legs).
