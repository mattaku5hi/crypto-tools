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
