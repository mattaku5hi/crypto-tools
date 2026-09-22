# ADR-001: Money & RawAmount representation

Status: Accepted
Date: 2026-09-22

## Context

AGENTS.md invariant #7 forbids `f32/f64` for raw amounts, monetary ledger, or quality-threshold
comparisons. CLI.md §7 requires large integers and money to serialize as JSON strings. We need one
consistent representation across EVM (up to 256-bit raw token amounts), Solana (u64 lamports/token
amounts), signed deltas (net wallet flow can be negative), and USD-denominated ledger money (needs
fixed decimal places, not the token's native decimals).

## Decision

Three distinct types, never conflated:

1. **`RawAmount`** — unsigned on-chain native-precision amount. Backed by `alloy_primitives::U256`
   (256 bits covers both EVM `uint256` and Solana `u64` without truncation risk). Carries no implicit
   decimals; decimals come from the asset's `AssetKey` metadata and are applied only at
   presentation/pricing time, never mutated in place.
2. **`SignedAmount`** — signed native-precision delta (net wallet balance change in a tx). Backed by
   `alloy_primitives::I256`. Used for reconciliation, never for display without a sign-aware
   formatter.
3. **`Money`** — fixed-point ledger currency value (USD by default, `quote_asset_policy` may allow
   others). Newtype over `i128` with a fixed scale constant `MONEY_SCALE: u32 = 8` (matches common
   stablecoin decimals, avoids double-rounding when translating token amounts priced in USDC/USDT).
   All arithmetic is `checked_add`/`checked_sub`/`checked_mul` returning `Result<Money, ScoutError>`
   on overflow — no silent wraparound (ACCEPTANCE C13).

`Money` division (e.g. `realized_cost_roi`) is the **only** place floats may appear, and only for the
*final* ratio value handed to the JSON/table formatter — never for intermediate ledger state. That
formatter module carries a narrowly scoped `#[allow(clippy::float_arithmetic)]`; nothing else in the
workspace may use it.

Serialization: `RawAmount`, `SignedAmount`, and `Money` all serialize as decimal strings (custom
`serde::Serialize`/`Deserialize`), never as native JSON numbers — this satisfies CLI.md §7's "large raw
integers/money are strings" and sidesteps JS `Number` precision loss for any downstream consumer.

Unknown/unrepresentable decimals (invariant #7: "Decimals неизвестны или превышают поддержанную
арифметику → typed error/unknown") surface as `ScoutError::UnsupportedDecimals { asset, decimals }`,
never a guessed default.

## Consequences

- No `f32`/`f64` field anywhere in `scout-core`, `scout-ledger`, or `scout-analytics` public types.
- `Money` cannot represent Infinity/NaN by construction (it is a bounded `i128`); PF's
  `no_observed_losses` / `undefined` statuses are modeled as a `Statused<Money>` enum (see ADR-004),
  not a numeric sentinel.
- Converting `RawAmount` (token-native decimals) to `Money` (fixed 8dp) is a checked, explicit,
  asset-decimals-aware operation living in `scout-pricing`, not something `scout-core` does implicitly.
- Overflow on `U256`/`I256` during summation (e.g. absurd raw amounts) must be checked and produce a
  typed error, not panic — enforced by the workspace `panic`/`unwrap_used`/`expect_used` lint deny.

## Alternatives considered

- `rust_decimal` / `bigdecimal` for `Money`: rejected for v1 — `i128` fixed-point at scale 8 covers the
  USD ranges we need with simpler, faster checked arithmetic and no external decimal-parsing surface;
  revisit only if a currency needs a different scale per-instance (not expected for v1's
  `quote_asset_policy`).
- `u128` for `RawAmount`: rejected — EVM `uint256` genuinely needs the full 256 bits (e.g. some
  fee-on-transfer or rebasing token accounting can produce large intermediate values); `U256` from
  `alloy-primitives` is already a workspace dependency and battle-tested for this exact purpose.
