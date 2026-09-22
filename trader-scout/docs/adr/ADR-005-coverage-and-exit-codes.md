# ADR-005: Coverage classification & exit codes

Status: Accepted
Date: 2026-09-22

## Context

CLI.md §8 fixes exit codes 0/2/3/4/130/141 with precise semantics, and ARCHITECTURE.md §6's
QualityReport section requires distinguishing declared-scope completeness from per-metric
eligibility. ACCEPTANCE D07/B08/E09 test that a small eligible count is not an error, that a partial
scan of one input token does not shrink `N`, and that provider/budget exhaustion is distinguishable
from small-sample ineligibility.

## Decision

### Two independent axes, never conflated

1. **Operational completion status** — did the run finish its declared scan/coverage contract?
   `Complete | Partial { reasons } | Failed { reasons }`. Drives the process exit code.
2. **Per-record eligibility** — did *this* wallet/token pass quality gates or have qualifying data?
   `Eligible | NotEligible { reasons } | NotEvaluated { reasons }`. Never affects the exit code by
   itself.

`NotEligible` (e.g. fewer than `min_closed_episodes`) with `Complete` operational status is a normal
successful run (exit 0) that happens to have a short or empty shortlist (D07). `NotEvaluated` (e.g. a
provider error prevented scanning a specific wallet) is recorded in the exclusions table but does
**not** reduce the denominator `N` used elsewhere (B08) and, if it affects a *required* input, pushes
the run's operational status to `Partial` (exit 3), never silently to `Complete`.

### Exit code mapping

```rust
pub enum RunOutcome {
    Complete,                    // exit 0 — includes legitimately empty/short shortlists
    ArgumentOrConfigError,        // exit 2 — bad args, format, AMBIGUOUS_CHAIN, incompatible flags
    IncompleteCoverage { partial_output: bool }, // exit 3
    InfrastructureUnavailable,   // exit 4 — CONFIGURATION_REQUIRED, credentials, storage, capability gap
    CancelledByUser,             // exit 130
    OutputPipeClosed,            // exit 141 — no panic/backtrace, producers stop cleanly
}
```

`--allow-partial` changes *what gets printed* (a marked-partial shortlist may be emitted instead of
being withheld) but never changes `IncompleteCoverage` into `Complete` — the exit code stays 3
(CLI.md §8 explicit note, re-affirmed here as a hard rule for `scout-app`).

### CONFIGURATION_REQUIRED is a first-class outcome, not an empty result

Per `scout.example.toml`'s `evm_history` placeholder comment ("ДОЛЖЕН давать CONFIGURATION_REQUIRED, а
не fake empty history"), any provider port that has no configured backend returns a typed error
variant `ScoutError::ConfigurationRequired { port, reason }` from its very first call, which the
engine maps to `InfrastructureUnavailable` (exit 4). It is a programming error for a scan to proceed
past an unconfigured required provider and report zero history as if it were a complete scan.

## Consequences

- `scout-app`'s report formatter always renders both axes: an exclusions table with reason-grouped
  counts, and a top-level operational status separate from "how many wallets made the top N".
- `scout-engine` propagates `NotEvaluated` reasons without collapsing them into `NotEligible` — mixing
  these would hide "we didn't check" behind "we checked and it failed", which AGENTS.md invariant #10
  forbids ("Unknown не сериализуется как zero" generalizes to "not-evaluated is not not-eligible").
- Every CLI integration test asserts both the exit code *and* the JSON/manifest's operational-status
  field, not one or the other.

## Alternatives considered

- Single `success: bool` flag: rejected — collapses "ran fine, few eligible wallets" and "provider
  quota exhausted mid-run" into the same signal, which is exactly the distinction CLI.md §8 and
  ACCEPTANCE B08/D07/E09 require callers to be able to make.
- Making `--allow-partial` return exit 0: rejected — CLI.md §8 explicitly states "не переписывает exit
  3 в success"; honoring that literally here.
