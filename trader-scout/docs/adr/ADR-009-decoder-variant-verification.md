# ADR-009: Decoder variant verification status and lower-bound coverage

Status: Accepted
Date: 2026-10-02

## Context

ADR-003 defines a qualifying buy as a recognized exchange execution (decoded swap/bonding-curve
buy) plus a positive owner-keyed net delta in a successful transaction. It does not say what
"recognized" means when one confirmed program exposes several trade instructions with different
evidence behind each. The 2026-10-02 review of pump.fun `6EF8rr…` (`docs/p0/deployment-registry.md`)
found six trade instructions in the pinned IDL, of which only three have a real successful
transaction committed as a golden fixture. Invariant #16 forbids claiming support from an
IDL/topic match alone; invariant #18 forbids silently skipping a recognized-but-unsupported shape.

## Decision

1. Every decoded instruction variant carries a verification status:
   - `FixtureVerified` — a committed successful real transaction where the decoded `user` and
     `mint` positions are asserted against that transaction's owner-keyed token deltas.
   - `IdlOnly` — layout is known from a pinned IDL (exact discriminator, arg types, data length,
     minimum account count) but no qualifying real fixture exists.
2. Only `FixtureVerified` buy variants can put a wallet into the strict buyer set.
3. An `IdlOnly` buy that would otherwise qualify (successful tx, positive owner delta, declared
   input mint) is **not** added. It is counted per variant and marks the run `Partial`
   (exit 3, ADR-005) with a reason naming the variant: the reported buyer set is then an explicit
   lower bound, never a silently incomplete one.
4. For a confirmed program, every discriminator is classified as trade (decoded), known non-trade
   (counted, taken from the pinned IDL), or unknown (coverage gap, exit 3). `NotMine` is reserved
   for instructions of other programs.
5. Accounts: a decoder requires at least the IDL's account count (Anchor remaining accounts may
   trail) and reads only IDL-named positions; data length is exact.
6. Changing a variant's status or the qualification rule bumps
   `SOLANA_BUY_QUALIFICATION_VERSION`, which appears in every report's scope block.

## Consequences

- The buy *definition* from ADR-003 is unchanged; this ADR fixes which decoder evidence counts as
  "recognized" and how missing evidence is reported.
- Runs over tokens whose buyers use `buy_exact_sol_in`, `buy_v2` or `buy_exact_quote_in_v2` will
  exit 3 until those variants get fixtures. That is intended; the fix is capturing fixtures, not
  relaxing the gate.
- Post-migration venues (PumpSwap AMM, Raydium, …) are outside the declared protocol scope
  entirely; the scope block states that the buyer set is a lower bound for migrated tokens.

## Amendment 2026-10-02 (same day): live evidence, arg-length policy, promotions

Live capture (`bins/scout-capture`, `6EF8rr…`, `sortOrder: desc`, 3 pages, captured
2026-10-02T13:40:20Z; trimmed fixture `docs/p0/measurements/fixtures/pump_variants_live_2026-10-02.json`,
16 successful transactions) showed successful executions whose instruction data deviates from
the IDL lengths: `buy` 24 bytes (×9 of 24 in the window), `buy_exact_sol_in` 24 bytes (×4 of 9;
a 26-byte one was seen in the smoke run), `buy_exact_quote_in_v2` 25 bytes (×2 of 16).
Required arguments were always present; only the trailing optional `track_volume` / extra bytes
varied. Anchor Borsh deserialization does not require consuming the whole buffer, so the program
executes these.

**Arg-length policy (replaces "data length exact" in decision 5):** data must be at least
discriminator + required args (24 bytes for all six trade variants) and parse those exactly.
Exactly one trailing byte on a variant with `track_volume` → `track_volume = Some(raw byte)`;
no trailing bytes → `None`; any other trailing length up to 32 bytes → `track_volume = None` and
`trailing_arg_bytes` recorded, encoding not guessed; shorter than required or more than 32
trailing bytes → `Malformed`.

**Promotions:** `buy_v2` (2/2 fixture txs: user owner delta == `amount`),
`buy_exact_quote_in_v2` (5/5: delta ≥ `min_tokens_out`), `buy_exact_sol_in` (6/6 confirm layout;
5 with delta ≥ `min_tokens_out`, 1 router-forward case below). All six trade variants are now
`FixtureVerified`; the `IdlOnly` path stays in code (tested through an injected policy) for
future IDL variants. Qualification version `pump-bonding-curve-buy/idl-e0687ae/v4`.

**Router-forward case (negative golden):** tx `bUh87USD…` executes `buy_exact_sol_in` as an inner
instruction under router program `proVF4pM…`; tokens land on `associated_user` (owner = decoded
`user`) and are forwarded within the same transaction to a token account of a different owner.
The decoded `user` nets zero and the final recipient has no instruction evidence, so neither is a
buyer (ADR-003 router hop, invariant #2). Such buyers are a known source of undercount until
route/ownership normalization exists (P3.3/P4.3-equivalent for Solana, see TICKETS P0.18); they
surface in `positive_delta_without_instruction`.
