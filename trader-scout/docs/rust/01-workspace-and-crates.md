# Workspace & crate boundaries

**Status: implemented.** The 16-crate + 3-binary layout below is the actual
`Cargo.toml` `[workspace.members]` list, not a plan.

## Why 16 crates instead of one

In C++ terms, this is the difference between one giant static lib with
everything `friend`-accessible, versus separate static libs with an explicit
public header per concern. The workspace splits along **testability and
reuse boundaries**, not file-size convenience:

- `scout-core` has zero knowledge of RPC, DEX protocols, or storage — it is
  pure domain types (`crates/scout-core/src/{identity,amount,error}.rs`).
  Anything importing it cannot accidentally reach into network code, because
  there is no network code to reach into.
- `scout-providers` depends on `scout-core` but knows nothing about ledger
  math or CLI formatting. A downstream project that only wants raw chain
  data (`ARCHITECTURE.md` §4: "Downstream-проект должен уметь: импортировать
  только `scout-evm`/`scout-solana` + `scout-scan` для raw data") can depend
  on exactly that subset and nothing else compiles into their binary.
- `scout-app` is the **composition root** — the only crate that knows about
  `clap`, file I/O, and wires concrete provider implementations to the
  `HistoryProvider` trait. No library crate (`scout-core` through
  `scout-analytics`) imports `clap`, `std::io::stdin`, or calls
  `std::process::exit`. This is the direct implementation of
  `AGENTS.md` invariant #15 ("SDK не создает скрытый runtime, не ставит
  global logging subscriber, не читает stdin и не завершает процесс").

The C++ analogy: if `libcore.a` linked against a `libcurl`-based transport
directly, every consumer of `libcore` would pay for `libcurl`'s symbols and
its failure modes, whether or not they ever make a network call. Splitting
the port (`scout-providers::HistoryProvider`, in
`crates/scout-providers/src/port.rs`) from any concrete implementation buys
the same thing Rust's trait objects buy over a C++ abstract base class: the
implementation is chosen at the edge (`scout-app`), and everything below it
compiles and tests without ever linking a transport.

## Ports & adapters, concretely

```
HistoryProvider (trait, scout-providers::port)
  ├── UnconfiguredProvider  — scout-providers::unconfigured (implemented, tested)
  ├── FixtureProvider       — scout-providers::fixture (implemented, tested)
  └── <real transport>      — not yet implemented, needs API credentials
```

Both existing implementations satisfy the same trait. `scout-engine` (once
implemented) will hold a `Vec<Box<dyn HistoryProvider>>` or similar registry
and never care which concrete type backs a given slot. This is why adding a
real Helius/Alchemy-backed provider later is additive: it implements the
existing trait, it does not require touching `scout-ledger`, `scout-
normalize`, or `scout-analytics` at all (see `ADR-006`).

## Why `Cargo.lock` is committed

This is a binary-producing workspace (three CLI tools), not a library
published to crates.io. Per Rust's own guidance, binary crates commit their
lockfile so every build — CI, a teammate's machine, a release — resolves to
the exact same dependency graph. `docs/p0/dependency-matrix.md` records
which versions were actually resolved on 2026-09-22; the committed
`Cargo.lock` is the enforcement mechanism, not just documentation of that.

## Why workspace-level `[lints]`

`Cargo.toml`'s `[workspace.lints.clippy]` block (`unwrap_used`,
`expect_used`, `panic`, `float_arithmetic`, `todo`, `unimplemented` = deny;
`indexing_slicing`, `as_conversions`, `integer_division` = warn) applies to
every crate via `[lints] workspace = true` in each crate's own `Cargo.toml`.
The alternative — copying a lint list into 19 separate `Cargo.toml` files —
is exactly the kind of drift that lets one crate quietly regress. A single
source of truth means changing the policy is a one-line diff, and CI's
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
(`.github/workflows/ci.yml`) enforces it identically everywhere.

The `indexing_slicing`/`as_conversions`/`integer_division` triad is `warn`,
not `deny`, deliberately — U256/decimal-handling code in `scout-evm` and
`scout-core::amount` needs narrow, deliberate integer conversions and
explicit `div_euclid`/`rem_euclid` splits (see `03-numeric-and-determinism.md`);
making those `deny` would force blanket `#[allow]` scattered everywhere,
defeating the lint's purpose. `unwrap_used`/`expect_used`/`panic` stay
`deny` even there — a checked-arithmetic bug should be a typed
`ScoutError`, never a panic, no matter how "obviously safe" the unwrap looks
today.

`#[cfg_attr(test, allow(...))]` in every crate's `lib.rs` relaxes
`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` **only inside
`#[cfg(test)]` modules** — test code asserting on a fixed, hand-constructed
input is a different risk profile than production code processing untrusted
chain data, and forcing `Result`-returning helpers throughout every test
would obscure what the test is actually checking.

## Feature flags (design intent, not yet exercised)

No crate here uses Cargo features today because there is nothing yet that
needs conditional compilation (e.g. an optional storage backend). When that
need arrives, the rule from `ARCHITECTURE.md` §4 applies: features must be
**additive** (enabling a feature adds capability, never changes existing
default behavior) so that two crates in the same dependency graph enabling
different feature subsets of a shared dependency don't produce a build that
silently differs from what either one asked for alone. This is a Cargo-wide
gotcha (feature unification across a workspace), not specific to this
project, but worth knowing before reaching for `--features`.
