# Rust decisions in trader-scout — reading guide

This directory explains **why** the codebase is built the way it is: crate
boundaries, type-driven correctness, numeric/determinism choices, async and
backpressure design, performance discipline, testing strategy, and library
ergonomics. It is written for someone comfortable in C/C++ who wants the Rust
idioms explained by contrast, not from zero.

## Honesty markers — read this first

Three different confidence levels appear throughout these documents, always
labeled explicitly:

- **Implemented & tested** — code exists in this repo, has unit/property
  tests, and the claim is checked by `cargo test`. Cited with a file path.
- **Designed, not yet implemented** — the architecture/ADR fixes the
  approach, but no code exists yet (e.g. most of `scout-engine`'s
  concurrency is currently an empty crate). Never presented as working.
- **Not measured** — anything about performance, throughput, or latency
  that has not been benchmarked on real hardware with real data. Per
  `AGENTS.md`'s explicit rule ("Не подставлять целевые числа вместо
  измеренных"), no number here is a promise.

As of this writing, `scout-engine` (the orchestration crate that would host
bounded channels, `spawn_blocking`, cancellation) is an **empty skeleton
crate** — `crates/scout-engine/src/lib.rs` has no logic yet. Everything in
`04-async-and-backpressure.md` about concurrency is design intent from
`ARCHITECTURE.md` §11, not a description of running code. The type-driven
correctness and numeric-determinism material (`02`, `03`), by contrast, *is*
implemented and tested today in `scout-core`/`scout-ledger`/`scout-analytics`.

## Reading order

1. [`01-workspace-and-crates.md`](01-workspace-and-crates.md) — why 16 crates, ports & adapters, composition root
2. [`02-type-driven-correctness.md`](02-type-driven-correctness.md) — newtypes, enums over sentinels, `Result`, `forbid(unsafe_code)`
3. [`03-numeric-and-determinism.md`](03-numeric-and-determinism.md) — no floats in the ledger, `BTreeMap` vs `HashMap`, checked arithmetic
4. [`04-async-and-backpressure.md`](04-async-and-backpressure.md) — Tokio model, bounded channels, `spawn_blocking`, cancellation, why not lock-free
5. [`05-performance-discipline.md`](05-performance-discipline.md) — how to benchmark honestly, bounded RSS, what a "target" is vs a measurement
6. [`06-testing-and-fixtures.md`](06-testing-and-fixtures.md) — unit/property/golden/CLI-integration layering, fixture provenance
7. [`07-library-ergonomics.md`](07-library-ergonomics.md) — how a library crate must behave inside someone else's application

Every section cites concrete files and line-level context from this repo —
read the doc and the code side by side.
