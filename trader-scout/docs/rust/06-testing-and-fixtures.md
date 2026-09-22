# Testing strategy & fixtures

**Status: partially implemented.** Unit tests exist and pass throughout
(62 tests as of the last commit — check `cargo test --workspace` yourself).
Property tests (`proptest`, already a workspace dependency) are not yet
wired into any crate. Golden on-disk fixtures with the provenance schema
described below do not yet exist as files in `tests/fixtures/` — the
provenance *contract* is implemented and tested
(`crates/scout-providers/src/fixture.rs`'s `Fixture::validate()`), but no
actual fixture files have been written to disk yet.

## The testing pyramid used here, bottom to top

**Unit tests**, colocated with the code they test in `#[cfg(test)] mod
tests` blocks — the Rust convention, versus C++'s usual separate test-file
layout. Every ACCEPTANCE scenario that is implementable offline is written
as a unit test *named after the acceptance criterion it proves*:
`acceptance_c01_full_lot_disposal_matches_worked_example`
(`crates/scout-ledger/src/fifo.rs`),
`rejects_log_with_wrong_topic_count` (mapping to AGENTS.md invariant #18,
`crates/scout-dex-evm/src/v2_swap.rs`). This is deliberate: a spec document
and a test suite drift apart the moment nobody enforces the link between
them; naming the test after the requirement makes that link visible in
`cargo test`'s own output, and a spec reviewer can literally search test
names against ACCEPTANCE.md's list to check coverage.

**Property tests** (not yet used, `proptest` sits unconsumed in
`Cargo.toml`) are the right tool for the *conservation* and *invariant*
properties `ACCEPTANCE.md`'s closing section lists explicitly:
`opening + acquisitions - disposals = closing` for known inventory flows,
`sum(fee allocations) + unallocated = actual fee`, `apply(E); apply(E) =
apply(E)` (idempotency), `canonical(snapshot+suffix) = canonical(full
history)` (replay correctness). These are universal statements — "for any
valid sequence of ledger operations, this equation holds" — which a
hand-picked set of example unit tests can never fully cover, no matter how
many examples you write by hand; property-based testing generates hundreds
of randomized inputs satisfying a type constraint and checks the invariant
holds for all of them, closer to fuzzing than to example-based testing.
This is genuinely underused in this codebase right now and is one of the
highest-value additions once `scout-ledger`'s API stabilizes further.

**Golden fixtures** are the mechanism for "this decoder correctly handles
this *exact* real-world-shaped transaction," and they are where the
provenance contract in `crates/scout-providers/src/fixture.rs` matters most:
a `Fixture` claiming `FixtureProvenance::Mainnet` must carry a non-empty
`chain`/`block_or_slot`/`tx`/`captured_at`/`source`, enforced by
`Fixture::validate()` and tested directly
(`mainnet_provenance_with_blank_tx_is_rejected`). This exists because
`AGENTS.md` explicitly forbids presenting a synthetic fixture as a real
mainnet transaction — a rule easy to violate by accident (copy a fixture,
forget to update its provenance block) if nothing enforces it mechanically.
The `decode_v2_style_swap` test suite
(`crates/scout-dex-evm/src/v2_swap.rs`) is the current concrete example: its
fixture is explicitly commented as synthetic and never claims to be a real
pool address or transaction.

**CLI integration tests** (planned, not yet written — would use
`assert_cmd`, not yet a dependency) exercise the actual compiled binary as a
subprocess: given a specific input file/stdin, does `buyer-intersect`
produce the right stdout, the right exit code, the right stderr warnings?
This is the layer that actually proves `ADR-005`'s exit-code contract (0/2/
3/4/130/141) end-to-end, because unit tests inside the library crates
cannot observe what `std::process::exit` actually does — only a real
subprocess invocation can.

## Why `cfg_attr(test)` relaxes lints only inside tests

Every crate's `lib.rs` carries:

```rust
#![cfg_attr(test, allow(
    clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing
))]
```

Production code processing untrusted on-chain data must never `unwrap()` —
a malformed or adversarial input should produce a typed error, not crash
the whole run. But a test asserting against a hand-constructed, known-valid
fixture is a different risk profile entirely: forcing every test assertion
through a `Result`-returning helper just to avoid `.unwrap()` would bury the
actual assertion under boilerplate error-handling that provides no real
safety benefit in a test context (the "input" is the test author's own
fixed literal, not attacker-controlled data). The `cfg_attr` scopes this
relaxation precisely to `#[cfg(test)]` code — production paths in the same
file remain under the full deny policy, checked by
`cargo clippy --workspace --all-targets -- -D warnings` (note `--all-
targets`, which includes test code in the lint pass — the relaxation is a
deliberate, visible exception, not a lint that silently never ran against
tests at all).

## The offline/live split in CI

`.github/workflows/ci.yml` runs `cargo test --workspace` against synthetic
fixtures only, with zero network dependency — `AGENTS.md`'s CI clause
("Offline CI не зависит от платных RPC или нестабильного live-chain
состояния") made literal. `.github/workflows/live-smoke.yml` exists as a
separate, `workflow_dispatch`-only placeholder for the day a real
credential-backed provider exists; it deliberately never triggers on
`push`/`pull_request`, so a future live-backed test cannot accidentally leak
provider API keys into a public PR's CI logs by inheriting the offline
workflow's broader trigger set.
