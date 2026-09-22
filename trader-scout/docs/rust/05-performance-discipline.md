# Performance discipline

**Status: not measured.** Nothing in this file is a benchmark result. Per
`AGENTS.md`'s own rule ("Не подставлять целевые числа вместо измеренных"),
every number quoted from the spec below is explicitly labeled a *candidate
target*, never a claim about this codebase's actual speed — because this
codebase has not been benchmarked yet.

## The rule: correctness first, then a measured baseline, then optimize

`ARCHITECTURE.md` §14 states this order directly: "Сначала корректность,
затем измеренная оптимизация." The temptation — especially coming from a
performance-sensitive C/C++ background — is to optimize while writing the
first version. This codebase deliberately resists that: `scout-ledger`'s
FIFO consumption (`crates/scout-ledger/src/fifo.rs`) is a straightforward
`BTreeMap` walk with checked arithmetic, not a hand-tuned data structure,
because no measurement yet justifies anything more complex, and a
"faster" but subtly wrong ledger is worse than a correct, boring one.

## What "bounded RSS" actually requires

The concrete promise (`ARCHITECTURE.md` §14): resident memory should stay
bounded even as input size grows tenfold, via disk spill rather than
unbounded in-memory growth. This is not automatic — it requires every
buffer in the pipeline (fetch queue, decode queue, reorder buffer, sort
buffer) to have an explicit capacity and an explicit "what happens when
full" policy (spill to disk, or backpressure the upstream producer — see
`04-async-and-backpressure.md`). A single unbounded `Vec` collecting "just
this one intermediate step's" results defeats the whole design, no matter
how well-bounded every other buffer is — which is exactly why
`ARCHITECTURE.md` calls this out as a property to verify with an actual
test (feed 10x the data, watch peak RSS via a system tool, confirm it
doesn't grow 10x too), not something to reason about statically and trust.

## Reordering, random completion order, and why it matters for reproducibility

`ACCEPTANCE` F01 requires identical checksums at concurrency 1, 8, and 32 —
i.e., however many workers fetch data and in whatever order their network
responses happen to arrive, the *final normalized output* must be
byte-identical. Network completion order is inherently nondeterministic
(this response might arrive before that one depending on server load, not
program logic), so any code path that processes events in arrival order
instead of canonical chain order (`ADR-002`'s `CanonicalLocation`: block
number + tx index for EVM, slot + block-tx-index for Solana) will produce
a result that depends on race conditions between network calls — a
Heisenbug class that is nearly impossible to reproduce and debug after the
fact.

The `BTreeMap`-keyed-by-canonical-order pattern from
`03-numeric-and-determinism.md` is the concrete mechanism that prevents
this: sort by canonical position before it enters the ledger, never by
"whichever network response happened to land first."

## How to benchmark honestly, when the time comes

`ARCHITECTURE.md` §14 fixes the required reporting fields for any speed
claim: compiler/toolchain version, CPU/core count/RAM/disk, release build
flags, a fixed fixture checksum (so "the same test data" is a verifiable
fact, not an assumption), event/wallet/token counts, processing order,
cache state (cold vs warm), network latency and provider rate limits at
measurement time. Any number reported without this context is not
reproducible and should not be trusted, including numbers eventually
produced by this project's own future benchmark suite if it omits this
context.

Two disciplines worth calling out for someone used to `perf`/`gprof`-style
profiling in C++:

- **Report p95/p99, not just mean latency.** A mean can look excellent while
  a meaningful tail of requests stalls badly — exactly the kind of thing
  that matters for "does this scanner occasionally hang for 30 seconds on
  one RPC call," which an average completely hides.
- **Never compare absolute numbers across a shared CI runner and a
  dedicated workstation.** `ARCHITECTURE.md` §14 says this explicitly — a
  shared CI runner's CPU allocation is noisy and non-exclusive; only
  relative regression on one controlled, dedicated machine is a meaningful
  signal for "did this change make things slower."

## The one number in this codebase's docs that looks like a target

`ARCHITECTURE.md` §14 names ">=100k already-normalized qualifying events/s"
as a candidate benchmark target for the in-memory intersection reducer
specifically — not for RPC fetch, JSON decoding, PnL computation, disk I/O,
or end-to-end pipeline throughput, all of which have entirely different
bottlenecks and have not been estimated at all. This number is explicitly
"subject to confirmation/revision in an ADR after a baseline exists" — it
is a hypothesis to test, not a specification to hit. As of this writing, no
baseline measurement exists for any part of this system's performance.

## What matters more than any single throughput number

`ARCHITECTURE.md` §14's own closing statement, which this documentation set
takes as more important than the throughput target above: bounded RSS under
10x data growth, stability under a slow downstream consumer, exact output
regardless of completion order, successful replay after a crash,
reasonable behavior under provider rate-limiting (429s), a correct offline
rerun with no network calls, and correct handling of one abnormally active
("hot") wallet without breaking sharding assumptions. A system that hits a
big throughput number but corrupts its ledger under any of those conditions
has not actually succeeded at anything this project cares about.
