# ADR-006: Source strategy & capability taxonomy

Status: Accepted
Date: 2026-09-22

## Context

ROADMAP.md P0.1 requires a capability matrix per network across five maturity levels; AGENTS.md
invariant #18 forbids silently skipping unfamiliar formats; SOURCES.md documents concrete provider
limitations (BSC public `eth_getLogs` disabled, no universal wallet-history RPC method, Solana
`getSignaturesForAddress` is not a full mint index). This ADR fixes the taxonomy and the port contract
so "capability" claims stay honest as credentials arrive incrementally.

## Decision

### Capability maturity levels (ROADMAP.md P0.1, fixed vocabulary)

```rust
pub enum CapabilityStatus {
    Documented,       // read from official docs/source, not exercised
    FixtureVerified,   // exercised against a stored fixture, no live network call
    LiveVerified,      // exercised against a live endpoint on a stated date
    Unsupported,       // known to not work (e.g. BSC public eth_getLogs disabled)
    Unknown,           // not yet investigated; NEVER treated as Unsupported or as a pass
}
```

`docs/p0/source-capability-matrix.md` records one row per (network, capability) pair with this status,
a source citation, and a date. A capability may only be claimed `LiveVerified` after an actual
successful call against a live endpoint on that date — this workspace has zero credentials as of this
ADR, so **no row may currently claim `LiveVerified`**; everything is `Documented` or `Unknown` until
API keys arrive (per SOURCES.md's explicit "П0 и последующих benchmark'ов" gate).

### Provider port contract

```rust
#[async_trait::async_trait]
pub trait HistoryProvider: Send + Sync {
    fn capabilities(&self) -> SourceCapabilities;
    async fn plan(&self, request: &ScanRequest) -> Result<ScanPlan, ScoutError>;
    fn scan(&self, task: ScanTask, cancel: CancellationToken) -> BoxStream<'_, Result<ScanEnvelope, ScoutError>>;
}
```

(Mirrors ARCHITECTURE.md §4's `HistorySource`/`TxDecoder` shape; named `HistoryProvider` here to match
this workspace's `scout-providers` crate.)

Two concrete implementations exist before any credentials arrive:

1. **`UnconfiguredProvider`** — the default for any port whose `*_env` variable (per
   `scout.example.toml`) is unset. `capabilities()` returns an all-`Unknown`/`Unsupported`
   `SourceCapabilities`; `plan()`/`scan()` immediately return
   `ScoutError::ConfigurationRequired { port, env_var }`. This is what every provider slot resolves to
   right now — it is wired into `scout-engine` and exercised by tests, not a placeholder comment.
2. **`FixtureProvider`** — reads from `tests/fixtures/` (P0.3's ground-truth corpus) and reports
   `CapabilityStatus::FixtureVerified` for exactly the (network, capability) pairs its fixtures cover.
   Used for all offline acceptance tests (A01-A05, most of B/C/D) so those gates do not require live
   credentials.

When a real network-backed provider (Helius, Alchemy, etc.) is added later, it implements the same
`HistoryProvider` trait and is swapped in via config — no call site in `scout-engine`, `scout-scan`, or
any CLI binary changes. This is the concrete mechanism behind "when keys arrive we wire transport, not
rewrite logic."

### Fixture provenance requirement

Every fixture under `tests/fixtures/` carries a mandatory provenance block:

```json
{"provenance": {"kind": "synthetic", "chain": "base", "captured_at": null, "source": "hand-constructed for C02"}}
```

`kind: "mainnet"` is only permitted with real `chain`/`block`/`tx`/`captured_at`/`source` fields
populated from an actual observed transaction — AGENTS.md's ban on presenting synthetic fixtures as
real mainnet transactions is enforced by a test that rejects any fixture claiming `kind: "mainnet"`
without a non-null `tx` field.

## Consequences

- `docs/p0/source-capability-matrix.md` and `docs/p0/deployment-registry.md` are living documents,
  re-verified (per SOURCES.md's closing note) whenever a dependency, endpoint, or claimed capability
  changes — not written once and trusted forever.
- No CLI path can silently return an empty successful history for a provider that was never
  configured; the failure is loud and typed (ties directly into ADR-005's `InfrastructureUnavailable`
  exit code).
- P0.3's 30+ hand-checked corpus entries are tracked as `synthetic` fixtures until real API keys allow
  capturing genuine mainnet samples; the corpus is explicitly incomplete at that stage and the ROADMAP
  gate for P0 says so rather than claiming false completeness.

## Alternatives considered

- A single boolean `supports_network: bool` per provider: rejected — SOURCES.md's own citations show
  capability is per (provider, network, method, time-range), e.g. BSC logs specifically disabled on
  public RPC while other methods work; a single boolean cannot represent that and would inevitably get
  rounded up to "supported."
- Returning `Ok(empty_stream)` for an unconfigured provider instead of a typed error: rejected — this
  is exactly the "fake empty history" `scout.example.toml` warns against and would make a
  misconfigured run indistinguishable from "wallet genuinely has no history."
