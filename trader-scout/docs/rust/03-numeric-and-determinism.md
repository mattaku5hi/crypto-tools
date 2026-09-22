# Numeric types & determinism

**Status: implemented.** Every claim below is backed by code in
`crates/scout-core/src/amount.rs`, `crates/scout-ledger/src/fifo.rs`, and
tests that pass today.

## Why there is no `f64` in the ledger

A C++ trading tool built on `double` for money will eventually hit the
classic bug: `0.1 + 0.2 != 0.3` in IEEE-754 binary floating point, and
outputs like `999.9999999999999` where a human expects `1000`. Over enough
FIFO lot consumptions, rounding error accumulates and the books stop
balancing exactly — which is fatal for a ledger whose entire purpose is
exact reconciliation (`ACCEPTANCE` §C's worked examples are exact integers,
not "close enough").

This repo's answer: `Money` (`crates/scout-core/src/amount.rs`) is a newtype
over `i128`, storing a value already multiplied by `10^MONEY_SCALE` (8).
"$1000.00000000" is stored as the integer `100000000000000`. Every
operation — `checked_add`, `checked_sub` — is integer arithmetic with an
explicit overflow check, never a float operation that could silently lose a
few ULPs.

`RawAmount` (`U256`) and `SignedAmount` (`I256`) exist because on-chain
token amounts routinely need more than 64 bits (a token with 18 decimals and
a large supply overflows `u64` easily) and EVM's native `uint256` type
demands full 256-bit precision to represent exactly, with no truncation.
Using `alloy_primitives::U256` here is the same idea as using a proper
128-bit or arbitrary-precision integer type in C++ instead of quietly
truncating into a `uint64_t` and hoping nothing ever exceeds it.

## Checked arithmetic, not saturating or wrapping

C's/C++'s unsigned integer overflow wraps silently by the standard (well-
defined but usually unwanted); signed overflow is undefined behavior. Both
are footguns a careful C++ codebase defends against with manual bounds
checks scattered through the code. Rust's `checked_add`/`checked_sub`
return `Option`/errors instead of a wrapped value:

```rust
// crates/scout-core/src/amount.rs
pub fn checked_add(&self, other: &Money) -> Result<Money, ScoutError> {
    self.0.checked_add(other.0).map(Money).ok_or(ScoutError::ArithmeticOverflow { .. })
}
```

The `?` operator then makes overflow propagate as a normal error through
any function calling this — `Ledger::dispose`
(`crates/scout-ledger/src/fifo.rs`) uses `checked_sub`/`checked_add`
throughout its FIFO consumption loop specifically so an adversarial or
malformed on-chain amount produces a typed `ScoutError`, not silent data
corruption or (in a debug build) a panic that takes down the whole run
mid-report.

## `div_euclid`/`rem_euclid`, and why `integer_division` is `warn`

The workspace's clippy policy denies `float_arithmetic` everywhere but only
*warns* on `integer_division` — because integer division is unavoidable and
correct here, the lint just wants deliberate attention paid to rounding
direction. `Money::Display`
(`crates/scout-core/src/amount.rs`) needs to split a scaled integer into
whole and fractional parts:

```rust
let magnitude = self.0.unsigned_abs();
let whole = magnitude.div_euclid(scale.unsigned_abs());
let frac = magnitude.rem_euclid(scale.unsigned_abs());
```

`div_euclid`/`rem_euclid` (rather than `/`/`%`) guarantee the remainder is
always non-negative for a given divisor, which matters once you split sign
from magnitude explicitly (a real bug caught during this project: mixing
signed `/`/`%` directly on a negative value produced `"-1.50000000"` for
`-0.5`, because Rust's default truncating division rounds toward zero, not
away from it, for the *whole* part when the dividend is negative — splitting
sign first and doing all division on the unsigned magnitude sidesteps the
whole class of off-by-one-on-negative-values bugs).

`proportional_money` in `crates/scout-ledger/src/fifo.rs` — computing
`total * numerator / denominator` for partial-lot fee allocation
(`ACCEPTANCE` C02's exact worked example, 100 units bought, sell 40 →
`consumed_basis=404` exactly) — special-cases full consumption
(`numerator == denominator`) to skip division entirely, because *any*
division introduces rounding risk and the full-lot case doesn't need it.

## Why `BTreeMap`, not `HashMap`

This is the determinism-under-concurrency question, and it is the single
most important habit carried over from systems programming into async Rust
in this codebase.

`std::collections::HashMap`'s iteration order is **not** specified and
depends on hash-seed randomization (a DoS mitigation, similar in spirit to
ASLR's role in security — both intentionally make one aspect of program
behavior non-reproducible run-to-run). Two runs of the same program, same
inputs, produce `HashMap` iteration in *different* orders. For a
concurrency-agnostic scanner, that is a serious problem: `ACCEPTANCE` F01
requires "concurrency 1/8/32 → одинаковые normalized ledger/report
checksums" — i.e., the *same output* regardless of how many workers ran and
in what order their results arrived. If any code path serializes a
`HashMap` directly, two runs with different concurrency could produce
byte-different JSON for logically identical data, breaking that invariant
outright.

`BTreeMap` iterates in sorted key order, always, deterministically, given
the same keys — regardless of insertion order. This repo uses it everywhere
a collection might be serialized or iterated in a way that affects output:

- `SourceCapabilities::by_capability: BTreeMap<String, CapabilityStatus>`
  (`crates/scout-providers/src/capability.rs`) — so two providers report
  their capabilities in the same key order.
- `Ledger::lots: BTreeMap<u64, Lot>` (`crates/scout-ledger/src/fifo.rs`) —
  keyed by `acquisition_sequence`, giving FIFO consumption order "for free"
  from ascending iteration, with zero separate sort step and zero ambiguity
  about which lot is "oldest."
- `NetDeltaInput::flows: BTreeMap<AssetKey, AssetFlow>`
  (`crates/scout-normalize/src/classify.rs`) — asset iteration order is
  stable across runs.

The C/C++ analogy: this is exactly the same discipline as choosing
`std::map` over `std::unordered_map` when you need reproducible iteration
for output/logging/checksums, and reserving `unordered_map` for pure lookup
tables whose iteration order is never observed. The Rust standard library
just makes the *unspecified-ness* of `HashMap`'s order an explicit,
documented fact you have to actively reason about, rather than an
implementation detail your compiler happens not to change today.

## Where floats *are* allowed

Nowhere in `scout-core`, `scout-ledger`, or the buy/attribution logic of
`scout-normalize`. The only sanctioned location (per ADR-001) is a
narrowly-scoped, explicitly `#[allow(clippy::float_arithmetic)]`-annotated
formatter that turns a *final* `Money` ratio into a human-readable
percentage for table output — never an intermediate computed value that
feeds back into further ledger math. That formatter does not exist yet in
this codebase; when it is written, the `#[allow]` must be scoped to that one
function, not the module or crate, so the workspace-wide `deny` keeps
catching accidental float creep everywhere else.
