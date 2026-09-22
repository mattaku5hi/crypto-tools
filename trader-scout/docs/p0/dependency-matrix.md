# Dependency compatibility matrix (P0 artifact)

Checked 2026-09-22 on this machine: `rustc 1.98.1`, `cargo 1.98.1` (stable-x86_64-unknown-linux-gnu).
Verified via disposable `cargo new` + `cargo add` smoke tests in `/tmp` (not committed), resolving
against live crates.io. This is *dependency resolution compatibility*, not capability/live-API
verification — that is tracked separately in `docs/p0/source-capability-matrix.md`.

| Crate | Resolved version | Notes |
|---|---|---|
| serde (derive) | 1.0.229 | — |
| serde_json | 1.0.151 | — |
| thiserror | 2.0.20 (impl) | v2 |
| tracing | 0.1.44 | — |
| tracing-subscriber | 0.3.23 | env-filter feature |
| clap (derive) | 4.6.7 | — |
| tokio | 1.53.1 | needs explicit features; bare `tokio = "1"` resolves but most crates need `rt-multi-thread,macros,time,sync,fs` |
| tokio-util | 0.7.19 | — |
| futures | 0.3.34 | — |
| bs58 | 0.5.1 | chosen over solana-sdk/solana-client to avoid transitive version churn/conflicts with alloy |
| rusqlite (bundled) | 0.40.2 | bundled = compiles C sqlite, slow first build |
| proptest | 1.x (dev-dependency only) | must be added under `[dev-dependencies]`, not `[dependencies]` |
| alloy-primitives | 1.7.3 | — |
| alloy-json-rpc | 2.4.2 | — |
| alloy-rpc-types-eth | 2.4.2 | serde+std features |
| csv | 1.4.0 | — |
| reqwest | 0.13.5 | **`rustls-tls` feature name does not exist in 0.13.x** — rustls is default (`__rustls`/`default-tls`); request `features = ["json", "gzip"]` only, do not pass a tls feature name |

## Rejected / deferred

- `solana-sdk`, `solana-client`: not smoke-tested. AGENTS.md forbids blocking I/O on Tokio workers and
  favors minimal RPC types; these crates pull large, fast-moving transitive graphs and historically
  churn API every few months. Decision: hand-roll minimal Solana JSON-RPC request/response types over
  `bs58` + `serde`, add narrowly-scoped crates later only if a specific capability is missing and the
  cost is justified in an ADR.
- `alloy` (the "full" meta-crate): resolves, but pulls the entire feature surface (signers, providers,
  node bindings, wasm targets) we do not need for a read-only history scanner. Using narrow
  `alloy-primitives` + `alloy-json-rpc` + `alloy-rpc-types-eth` instead; revisit if a provider trait
  needs `alloy-provider`.

## Re-verification note

Per AGENTS.md: "не копировать номера зависимостей без проверки" — the above were resolved live against
crates.io on the stated date. Anyone bumping a dependency later must re-run resolution (`cargo update -p
<crate> --dry-run` or a fresh smoke test) rather than copy a number from this file into a different
context.
