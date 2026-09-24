# ADR-008: External provider/decoder extensibility (Tier 1 / Tier 2)

Status: Accepted
Date: 2026-09-22

## Context

`trader-scout` ships as an embeddable library (`scout-sdk`, ADR-007), not only as
three CLI binaries. A downstream integrator must be able to supply their own
on-chain data source and/or their own protocol decoder — using their own RPC
budget and their own DEX coverage — without forking this workspace. The
`HistoryProvider` trait (`scout-providers::port`) already gives this shape for
data sources; nothing today gives it for decoders, and the data-source shape
itself has three defects that block a real third-party implementation:

1. `ScanEnvelope { raw_payload_description: String }` carries no actual raw
   data — a third party cannot build anything against a description string.
2. `ConfigurationRequired { port: &'static str, env_var: &'static str }`
   requires `&'static str`; a provider constructed from runtime config data
   cannot produce one.
3. `ScoutError` is a closed enum owned by `scout-core`; a third-party crate
   cannot add its own error variant to it.

Separately, AGENTS.md invariant #16 ("Нельзя объявлять поддержку DEX по
одному совпадению event topic") and invariant #6 ("Не считать неизвестную
себестоимость нулевой") both apply with extra force to externally-supplied
code: a third-party decoder claiming blanket protocol support, or a
third-party data source whose classification we did not verify, must not be
able to silently enter the strict ledger/ranking path.

## Decision

### Two tiers, two distinct traits — never one trait with a mode flag

- **Tier 1 — `HistoryProvider`**: returns raw, undecoded chain data (EVM
  logs/receipts, Solana instructions). Our own decoders (or a third party's
  registered decoder, see below) classify it. This is the **trusted path**:
  its output is eligible for strict `wallet-rank`/`buyer-intersect` by
  default, same as our own provider implementations today.
- **Tier 2 — `NormalizedActivitySource`**: returns already-classified
  economic actions from an external system (a caller's own indexer, a Dune
  export, etc.). This is the **unverified path** — see the trust-token
  design below for how it stays out of the strict ledger path by
  construction, not by convention.

A single trait with a `trusted: bool` field was considered and rejected: a
mode flag is data, and data can be set wrong by mistake at any call site.
Two traits mean a caller must structurally choose which contract to
implement, and the type system — not a runtime check someone forgot to
add — determines what happens to the output.

### Tier 2 trust is enforced by an unforgeable token, not a runtime check

Per AGENTS.md invariant #6's own enforcement pattern (`BasisStatus::Unknown`
→ `realized_trade_pnl: None`, a type-level fact the ledger cannot ignore),
Tier 2 data reaching the ledger is wrapped in a type whose only
`ExternalUnverified` constructor requires an opt-in token:

```rust
// scout-api
#[non_exhaustive]
pub struct ExternalDataOptIn {
    // Deliberately not Default, not constructible from a literal, not
    // Clone-from-nothing. Only the composition root (scout-app / a
    // library caller explicitly wiring Tier 2) can construct one, and
    // only when the corresponding config/CLI flag is explicitly set.
    _private: (),
}

impl ExternalDataOptIn {
    /// The only constructor. Callers must justify *why* they are
    /// opting into unverified data — the reason is threaded into the
    /// report/manifest, not discarded.
    pub fn acknowledge(reason: ExternalDataAcknowledgement) -> Self { .. }
}

pub enum TrustLevel {
    Verified,                                   // Tier 1, our/registered decoders
    ExternalUnverified(ExternalDataOptIn),       // Tier 2, explicit opt-in
}
```

`scout-ledger`'s strict-path entry points require `TrustLevel::Verified` (or
a `Vec` proven all-`Verified`) for anything contributing to strict
`wallet-rank`/`buyer-intersect` ranking; a mixed or `ExternalUnverified`
input is representable only in the non-strict `wallet-stats` path, mirroring
how unknown-basis lots already behave (ADR-004).

**Trust composition enters the report fingerprint** (ACCEPTANCE G03: "Любой
отчет содержит... versions" — a trust-level mix is exactly this kind of
policy fact). A cached result computed with Tier 2 data included must never
be silently reused for a request that expects Tier-1-only strictness.

Both barriers apply together, not either/or: the `ExternalUnverified`
constructor exists in `scout-api` unconditionally (so the type is always
representable and testable), but `scout-sdk`'s default feature set does not
expose the machinery that would let a caller reach it without explicitly
enabling `scout-sdk`'s `external-data` feature. A caller must both compile
with the feature on *and* construct the token at runtime — two independent,
deliberate steps, neither of which happens by accident.

### `RawPayload` replaces the string, and lives in `scout-core`

`ScanEnvelope`'s payload becomes a real enum:

```rust
// scout-core
pub enum RawPayload {
    EvmLog(RawEvmLog),
    EvmTransaction(RawEvmTransaction),
    SolanaInstruction(RawSolanaInstruction),
    SolanaTransaction(RawSolanaTransaction),
}
```

`RawEvmLog`/`RawEvmTransaction` (currently `scout-evm`) and
`RawSolanaInstruction`/`RawSolanaTransaction` (currently `scout-solana`) move
into `scout-core`. `scout-core` already depends on `alloy_primitives` for
`RawAmount`, so this adds no new dependency for the EVM types; the Solana
types are plain byte arrays with no dependency at all. `scout-evm`/
`scout-solana` keep `pub use scout_core::{...}` re-exports so this is a
mechanical move, not a behavior change — S1 below does exactly this and
nothing else.

The alternative — leaving raw types in `scout-evm`/`scout-solana` and having
`scout-api` depend on both — was rejected: it would make the API crate pull
full EVM/Solana primitive stacks, working against ACCEPTANCE F07's
scanner-only-build proof (ADR-007) for anyone depending on the API crate
alone without also wanting our EVM/Solana implementations.

### New crate: `scout-api`

A minimal crate holding every extension-point trait and its associated
types, depending only on `scout-core` + `async-trait` + `futures` +
`tokio-util` — explicitly no `tokio` runtime dependency, keeping AGENTS.md
invariant #15 (no hidden runtime) true of the API surface itself, not just
of our implementations.

```
scout-api/
  src/
    history_provider.rs   // Tier 1: HistoryProvider, ScanRequest/Plan/Task, SourceCapabilities
    normalized_source.rs  // Tier 2: NormalizedActivitySource, TrustLevel, ExternalDataOptIn
    decoder.rs             // TxDecoder, DeploymentScope, DecodeOutcome
    price_source.rs        // PriceSource (stub trait, S2 — cheap to declare now, keeps history coherent)
    storage_port.rs         // RawStore/MetadataStore (stub trait, S2 — extraction from SQLite deferred)
    error.rs                // ProviderError
```

`scout-providers` (existing crate) becomes **our implementations** of
`scout-api`'s traits — `UnconfiguredProvider`, `FixtureProvider`, and later
the Shyft/dRPC/Etherscan adapters — and re-exports the trait surface so
existing call sites (`use scout_providers::HistoryProvider`) keep compiling
during the transition.

### `ProviderError` replaces the `&'static str` constraint

```rust
// scout-api
#[non_exhaustive]
pub enum ProviderError {
    ConfigurationRequired { port: String, detail: String },
    RateLimited { retry_after: Option<std::time::Duration> },
    Unsupported { capability: String },
    Transport(Box<dyn std::error::Error + Send + Sync>),
    Other(Box<dyn std::error::Error + Send + Sync>),
}
```

`RateLimited` is not optional polish: ACCEPTANCE E09 requires correct
behavior under provider rate limits (429/Retry-After), and without a
dedicated variant the engine cannot distinguish "back off and retry" from
"this will never work" — both would otherwise collapse into an opaque
`Transport` error. `HistoryProvider::plan`/`scan` return `ProviderError`,
not `scout_core::ScoutError`; `scout-engine` maps `ProviderError` into the
workspace's own `ScoutError`/exit-code contract (ADR-005) at the boundary
where it consumes a provider, so `scout-core` itself never needs to know
about third-party error shapes. Every public enum newly introduced for this
extensibility surface (`ProviderError`, `TrustLevel`, `DecodeOutcome`,
`SourceCapabilities`'s status values where not already `#[non_exhaustive]`)
carries `#[non_exhaustive]` — adding a variant later must not be a breaking
change for implementers, addressing the open question already flagged in
`docs/rust/07-library-ergonomics.md`.

### Decoder trait: `Ok(None)` vs `Err(..)` is a real distinction, not detail

```rust
// scout-api
pub enum DecodeOutcome<T> {
    NotMine,           // this payload does not match my protocol shape — normal, not an error
    Decoded(T),
    Malformed(String), // looked like mine, but the shape is broken — invariant #18, never silently skipped
}

pub trait TxDecoder<Raw, Decoded>: Send + Sync {
    fn scope(&self) -> DeploymentScope;
    fn decode(&self, raw: &Raw) -> DecodeOutcome<Decoded>;
}

pub struct DeploymentScope {
    pub chain: scout_core::ChainKey,
    pub contract_addresses: Vec<scout_core::AddressBytes>,
    pub active_from: BlockOrSlot,
    pub active_until: Option<BlockOrSlot>,
}
```

This fixes a real bug found while writing this ADR: `scout-dex-evm`'s
existing `decode_v2_style_swap` returns `Err(SignatureMismatch)` for a log
that simply belongs to a different event — conflating "not mine" with "mine
but broken." Scanning a block's logs against several decoders would report
an "error" for every log that isn't the exact event each individual decoder
happens to look for. `DecodeOutcome::NotMine` fixes this: a decoder registry
tries each registered decoder in turn and only surfaces `Malformed` (a real
problem) to the caller, never `NotMine` (an expected non-match). Migrating
`decode_v2_style_swap` to this shape is in scope for S4.

`DeploymentScope` is mandatory at registration, not optional — a decoder
without a scope would implicitly claim global support for its protocol
shape across every chain and address, directly violating invariant #16
("Проверяются deployment, диапазон блоков..."). The registry dispatches a
raw payload only to decoders whose scope actually covers that payload's
chain/address/block-or-slot; it is a routing table keyed by verified
deployment facts, not a flat list of "try everything."

### Adapter resolution: distinguish "not configured" from "does not exist"

`scout.example.toml`'s `adapter = "helius-history"` string is not resolved
against anything today — any name, including a typo, currently falls
through to `UnconfiguredProvider` and reports `ConfigurationRequired` (exit
4, per ADR-005/ADR-006). That is the right outcome for "the correctly-named
adapter has no credentials," and the wrong outcome for "the adapter name is
misspelled or was never registered" — a user chasing the wrong problem
("add API keys") when the actual issue is a config typo is a real
usability defect, not just an edge case.

The registry (S6) must distinguish these explicitly:

```rust
pub enum AdapterResolution {
    Found(Box<dyn HistoryProvider>),
    UnknownAdapter { requested: String, registered: Vec<String> }, // exit 2, ArgumentOrConfigError (ADR-005)
    ConfigurationRequired { port: String, detail: String },        // exit 4, InfrastructureUnavailable
}
```

### Conformance kit

A separate `scout-api-conformance` crate (not a dev-dependency hidden inside
`scout-api`, so third parties can depend on it directly to test their own
implementations) provides a reusable test suite any `HistoryProvider` or
`TxDecoder` implementation can run against itself:

- never panics on malformed/adversarial input (invariant #17/#18);
- respects `CancellationToken` within a bounded time;
- `ConfigurationRequired` surfaces from both `plan()` and `scan()` — never
  an empty-but-`Ok` stream (the exact failure mode `scout.example.toml`
  already warns against, ADR-006);
- an unexercised capability reports `Unknown`, never `Unsupported`
  (ADR-006's existing rule, now enforced for third parties too);
- `RateLimited` propagates to the caller rather than being silently
  retried forever inside the implementation;
- identical input run twice produces identical output ordering
  (ACCEPTANCE F01's determinism requirement, now checkable for any
  implementation, not just ours).

## Consequences

- `scout-providers`, `scout-dex-evm`, `scout-dex-solana` all gain a new
  dependency on `scout-api` and become "reference implementations of
  scout-api's traits" rather than the sole implementation surface.
- `scout-sdk`'s feature graph (ADR-007) gains an `external-data` feature,
  off by default, gating the `NormalizedActivitySource`/`ExternalDataOptIn`
  machinery — so the default (`scan`) and `full` feature sets never expose
  a way to reach the unverified path without an explicit, separate opt-in.
- Every future first-party provider (Shyft/dRPC/Etherscan, once
  credentials exist, per the user's own P0.1 research) is implemented
  against exactly the same `scout-api::HistoryProvider` trait a third party
  would use — there is no privileged internal-only interface.
- `docs/PROVIDERS.md` (S8) becomes the actual integration guide; without it
  this ADR is necessary but not sufficient for a third party to succeed.

## Alternatives considered

- Single `HistoryProvider` trait with a `trusted: bool` or `TrustLevel`
  field directly on `ScanEnvelope`: rejected — see "two traits" above; a
  boolean/enum field is a fact that can be set incorrectly at any call
  site, where a type-level split makes the wrong state unrepresentable in
  the strict path.
- Runtime-only trust gating (a config flag checked by `scout-ledger` before
  accepting input, no special token type): rejected — a config flag can be
  flipped by anyone touching config, including accidentally during
  refactors; a token that must be explicitly constructed with a documented
  reason, per call, is harder to trip over and self-documents *why* a
  particular run accepted unverified data.
- Keeping raw types in `scout-evm`/`scout-solana` and letting `scout-api`
  depend on both: rejected per the `RawPayload`/`scout-core` discussion
  above — it would make the API crate's own dependency footprint exactly
  the thing ADR-007 was written to avoid.
