# ADR-007: `scout-sdk` feature gates to avoid a god-crate

Status: Accepted
Date: 2026-09-22

## Context

`scout-sdk` currently depends on every library crate in the workspace (core,
scan, normalize, ledger, analytics, storage, engine, providers, evm, solana).
ROADMAP.md P8.1 and ACCEPTANCE F07 require `examples/embedded_scanner.rs` to
prove a "scanner-only build" does **not** pull in ledger/analytics/formatting
infrastructure — a third party embedding just the raw-scanning capability
must not pay for compiling (or trusting) code paths it never calls.

As written, `scout-sdk` cannot satisfy that: any consumer depending on it —
including an example meant to prove the opposite — pulls the full graph.
This directly contradicts the layered-crate story in
`docs/rust/01-workspace-and-crates.md`, which describes `scout-providers` +
`scout-evm`/`scout-solana` as independently importable for raw data. That
story was true of the lower crates; `scout-sdk` as a single unconditional
facade broke it once it existed.

## Decision

`scout-sdk`'s `Cargo.toml` gets Cargo features, additive per Cargo's own
convention (enabling a feature only adds capability, never changes default
behavior for anyone already depending on a narrower feature set — see
`docs/rust/01-workspace-and-crates.md`'s closing note on feature
unification):

```toml
[features]
default = ["scan"]
scan = ["dep:scout-scan", "dep:scout-evm", "dep:scout-solana", "dep:scout-providers"]
ledger = ["scan", "dep:scout-normalize", "dep:scout-ledger"]
analytics = ["ledger", "dep:scout-analytics"]
full = ["analytics", "dep:scout-storage", "dep:scout-engine"]

[dependencies]
scout-core = { workspace = true }
scout-scan = { workspace = true, optional = true }
scout-evm = { workspace = true, optional = true }
scout-solana = { workspace = true, optional = true }
scout-providers = { workspace = true, optional = true }
scout-normalize = { workspace = true, optional = true }
scout-ledger = { workspace = true, optional = true }
scout-analytics = { workspace = true, optional = true }
scout-storage = { workspace = true, optional = true }
scout-engine = { workspace = true, optional = true }
```

`default = ["scan"]` matches ARCHITECTURE.md §4's own stated minimum
("импортировать только `scout-evm`/`scout-solana` + `scout-scan` для raw
data"). A consumer wanting ledger math opts in with `features = ["ledger"]`;
the three CLI binaries (which need everything) depend on `scout-sdk` with
`features = ["full"]` explicitly, so their requirement is visible in their
own `Cargo.toml` rather than implied by an unconditional dependency graph.

`examples/embedded_scanner.rs` (ROADMAP.md P8.1) is built against
`scout-sdk` with **default features only** — this is the concrete mechanism
that makes ACCEPTANCE F07 checkable: `cargo build --example embedded_scanner
--no-default-features --features scan` must succeed and must not compile
`scout-ledger`/`scout-analytics`/`scout-storage` into the resulting binary.
A CI job asserting this (e.g. via `cargo tree -e no-dev --features scan`
containing no `scout-ledger` entry) is the honest way to keep this true
over time, not a one-time manual check.

## Consequences

- `scout-sdk`'s public API (`pub use` re-exports) must itself be
  `#[cfg(feature = "...")]`-gated per item, matching each dependency's
  feature — re-exporting a type from an optional dependency unconditionally
  would defeat the point by forcing that dependency to compile regardless
  of which features are enabled.
- This is a breaking change for any code currently doing
  `scout-sdk = { workspace = true }` without a `features` list, since
  `default = ["scan"]` is narrower than "everything" — the three CLI
  binaries must be updated to request `features = ["full"]` explicitly at
  the same time this ADR's Cargo.toml changes land, not in a follow-up.
- Feature interactions are additive only: no cfg'd-out code path may change
  the *meaning* of code compiled under a broader feature set (e.g. `ledger`
  feature enabled must never alter `scan`-only behavior) — this is the same
  rule `docs/rust/01-workspace-and-crates.md` already documents for the
  workspace generally, restated here because `scout-sdk` is where it first
  becomes load-bearing rather than theoretical.

## Alternatives considered

- Leaving `scout-sdk` as an unconditional full-dependency facade and putting
  `examples/embedded_scanner.rs` directly against `scout-providers` instead:
  rejected as the *short-term* fix (it does prove F07 today) but it doesn't
  resolve the god-crate problem for any other consumer of `scout-sdk`
  itself, and ROADMAP.md P8.1 specifically frames the embedding example as
  exercising the SDK crate, not a lower-level crate directly.
- Splitting `scout-sdk` into multiple crates (`scout-sdk-scan`,
  `scout-sdk-ledger`, ...) instead of one crate with features: rejected —
  Cargo features are the idiomatic mechanism for optional capability within
  one public-facing facade crate, and a single crate keeps the "convenient
  public facade" framing from ARCHITECTURE.md §3 intact; multiple crates
  would just move the god-crate problem into "which of five near-identical
  crates do I depend on," a worse discoverability tradeoff for no added
  safety.
