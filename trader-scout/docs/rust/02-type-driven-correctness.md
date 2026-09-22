# Type-driven correctness

**Status: implemented.** Every example below is real code with passing
tests, not illustrative pseudocode.

## Newtypes: making a category of bug uncompilable

In C++, `WalletKey` might be a `struct { int family; uint64_t network_id;
std::vector<uint8_t> address; }`, and nothing stops you from comparing two
instances field-by-field incorrectly, or passing a `Base` address where a
`BSC` one was expected — both are "just bytes."

`crates/scout-core/src/identity.rs` makes the network part of the *type*,
not a field you might forget to check:

```rust
pub struct ChainKey {
    pub family: ChainFamily,
    pub network_id: NetworkId,
    pub genesis_identity: GenesisIdentity,
}
pub struct WalletKey { pub chain: ChainKey, pub address: AddressBytes }
```

`#[derive(PartialEq, Eq, Hash, Ord)]` on `WalletKey` compares the *whole*
struct, including `chain`. There is no code path where someone accidentally
writes `wallet_a.address == wallet_b.address` and calls it a day — comparing
just the address field requires deliberately reaching past the type's public
API. The test
`same_address_bytes_on_different_solana_clusters_are_different_wallet_keys`
(`crates/scout-core/src/identity.rs`) is the concrete proof: identical
32-byte address on mainnet vs devnet produces two `WalletKey`s that are
`!=`, because the compiler-derived equality includes `chain`.

`Money`, `RawAmount`, `SignedAmount` (`crates/scout-core/src/amount.rs`) are
the same idea applied to numbers: each is a distinct type wrapping a
primitive (`i128`, `U256`, `I256`), so `Money` and `RawAmount` cannot be
added to each other by accident — the compiler rejects it, where in C++
both might silently decay to compatible arithmetic types.

## Enums instead of bool + sentinel value

A very common C-style pattern is `bool success; double value; // -1.0 if
invalid`. This repo replaces every instance of that pattern with a
tagged union (Rust `enum`) that makes the invalid states unrepresentable in
the return type itself, not just documented in a comment:

```rust
// crates/scout-normalize/src/attribution.rs
pub enum AttributionStatus {
    Confident { owner: WalletKey, evidence: AttributionEvidence },
    Ambiguous { candidates: Vec<WalletKey>, reason: AmbiguityReason },
}
```

There is no `AttributionStatus::owner: Option<WalletKey>` with a boolean
`confident` flag that could get out of sync. `confident_owner()` pattern-
matches and returns `None` for `Ambiguous` — a caller literally cannot
extract an owner from an ambiguous attribution without the type system
routing them through the `None` case (test:
`ambiguous_status_never_exposes_a_confident_owner`).

`RatioStatus<T>` (`crates/scout-analytics/src/ratio.rs`) does the same for
profit factor, which mathematically can be undefined or unbounded:

```rust
pub enum RatioStatus<T> {
    Value { value: T },
    NoObservedLosses,
    Undefined,
}
```

No `f64::INFINITY`, no `-1.0` sentinel, no `NaN`. `serde` tags the variant
as a `"status"` string field, so the JSON output is
`{"status":"no_observed_losses"}` with no numeric field at all — a consumer
parsing this cannot mistake "unbounded" for a huge finite number, because
there is no number to misread (test:
`ratio_status_serializes_without_a_numeric_value_field_for_non_value_variants`).

`BasisStatus` (`crates/scout-ledger/src/lot.rs`) is the same pattern for
"do we actually know this lot's cost basis":

```rust
pub enum BasisStatus { Known, Unknown { reason: String } }
```

This is the type-level enforcement of `AGENTS.md` invariant #6 ("Не считать
неизвестную себестоимость нулевой") — `Ledger::dispose`
(`crates/scout-ledger/src/fifo.rs`) checks this enum and returns
`realized_trade_pnl: None` rather than a computed number, whenever any
consumed lot's status is `Unknown`.

## `Result` instead of exceptions

C++ exceptions are invisible in a function's signature — you find out a
function can throw by reading its implementation or documentation, and
nothing forces every caller to acknowledge the possibility. Rust's
`Result<T, E>` puts the failure mode in the type signature itself:

```rust
pub fn dispose(&mut self, ...) -> Result<DisposalResult, ScoutError>
```

The caller cannot get `DisposalResult` without handling (or explicitly
propagating via `?`) the `ScoutError` case first. `ScoutError`
(`crates/scout-core/src/error.rs`) is a closed enum
(`ConfigurationRequired`, `InvalidAddress`, `AmbiguousChain`,
`UnsupportedDecimals`, `ArithmeticOverflow`) — a `match` on it that omits a
variant fails to compile unless you add a wildcard arm, which forces a
conscious decision rather than an accidental fallthrough.

## `#[must_use]`

`Money::checked_add`/`checked_sub` are marked
`#[must_use = "this returns the result of the checked addition without
modifying self"]`. Without this, `balance.checked_add(&fee);` — computing a
new value and discarding it, perhaps meaning to write
`balance = balance.checked_add(&fee)?` — compiles silently in a mutation-
free type system, exactly the class of bug `#[must_use]` exists to catch at
compile time instead of in production.

## `#![forbid(unsafe_code)]` — and its actual scope

Every crate's `lib.rs` starts with `#![forbid(unsafe_code)]`. This is a
**hard compiler error**, not a lint warning, if any `unsafe` block appears
in that crate's own source. It does **not** mean the dependency graph
contains no `unsafe` — `alloy-primitives`, `rusqlite`'s bundled SQLite (C
code via FFI), and effectively every crate touching raw bytes or a C
library uses `unsafe` internally, and that is expected and fine. The
guarantee is narrower and still valuable: *this project's own logic* cannot
introduce a use-after-free, data race, or undefined-behavior bug through a
raw pointer or unchecked cast, because there is no unsafe code in this
project's logic to introduce it. `AGENTS.md`'s own phrasing is exact here:
"Это не утверждение, что все зависимости не содержат unsafe."
