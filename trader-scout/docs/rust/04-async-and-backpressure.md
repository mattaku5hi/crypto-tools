# Async, backpressure, and why not lock-free

**Status: design intent from `ARCHITECTURE.md` §11, mostly NOT yet
implemented.** As of this writing `scout-engine` — the crate that would host
everything below — is an empty skeleton crate with no logic. There is no
`tokio::spawn` call anywhere in this codebase yet. Read this as "the plan
and the reasoning behind it," and check `crates/scout-engine/src/lib.rs`
yourself before assuming any of this is running.

## Task ≠ thread, and why that matters for an I/O-bound scanner

In C++, launching 10,000 concurrent network requests the naive way means
10,000 OS threads, each with its own stack (typically megabytes by default)
and its own kernel scheduling overhead — that alone can exhaust memory or
thread-table limits long before any actual bottleneck.

A Tokio task is not an OS thread. `tokio::spawn(async { ... })` schedules a
state machine onto a small pool of OS threads (`rt-multi-thread`, the
feature enabled in this workspace's `Cargo.toml`) that cooperatively
multiplexes thousands of tasks — each task yields control at every `.await`
point rather than blocking a thread, so 10,000 in-flight RPC calls can share
perhaps 8 OS threads without exhausting anything. This is why async is a
concurrency-model fit for "many things waiting on I/O simultaneously," not
a raw speed multiplier for CPU work — a `tokio::spawn`'d task doing pure
computation with no `.await` points just burns a worker thread exactly like
a synchronous call would, and worse, starves every other task queued on
that same worker.

## Bounded channels: the single most important habit here

`ARCHITECTURE.md` §11 states it directly: "Bound на число сообщений без
bound на bytes недостаточен" and cites Tokio's own channel tutorial (S17)
for backpressure. The plan calls for `tokio::sync::mpsc::channel` (bounded)
throughout the pipeline — provider → raw store → decode → ledger — never
`unbounded_channel`.

Why this matters, concretely: an unbounded channel between "fetch results
arriving from N concurrent RPC calls" and "a single-threaded ledger
reducer that processes them" is a classic producer/consumer mismatch. If
producers run even slightly faster than the consumer — entirely plausible
when the consumer does exact-precision ledger math and the producers are
just deserializing JSON — the channel's internal queue grows without limit.
This is a slow-motion OOM: memory grows steadily, nothing crashes
immediately, and the failure mode shows up hours into a long-running scan
as the process gets OOM-killed with no clear single culprit line in the
logs.

A **bounded** channel makes this failure impossible by construction: once
the queue is full, `send().await` on the producer side blocks (yields,
doesn't spin) until the consumer drains capacity. This is backpressure —
the slow consumer's pace becomes the *whole pipeline's* pace, automatically,
without anyone writing an explicit rate limiter. The queue depth you choose
is a real engineering decision (a rough intuition from queuing theory,
Little's Law: average items in the queue ≈ arrival rate × average time an
item waits there — a deep queue trades memory for smoothing burstiness, a
shallow one trades memory for tighter latency and more backpressure
stalls), not a number to guess and forget; `scout.example.toml`'s
`queue_max_batches`/`queue_max_bytes` fields exist precisely so this is a
configured, measured decision rather than a hardcoded guess.

The C/C++ analogy: this is the same discipline as a bounded ring buffer
between a producer and consumer thread, deliberately sized so that a fast
producer either blocks or drops rather than growing a `std::deque` without
limit. Rust's bounded `mpsc` gives you that ring buffer, safe `Send`
transfer of ownership across threads, and the blocking/backpressure
behavior, all checked by the type system rather than by manual discipline.

## CPU work does not belong on an async worker thread

`SOURCES.md` S18 (Tokio's own `spawn_blocking` docs) and `ARCHITECTURE.md`
§11 are explicit: CPU-heavy parsing, ABI decoding, sorting, or compression
must never run directly inside an `async fn` body on a Tokio worker thread,
because it blocks that thread's ability to poll *any other task* scheduled
on it — one slow decode can stall dozens of unrelated in-flight RPC
continuations that happen to share that worker.

The fix is `tokio::task::spawn_blocking`, which moves the closure onto a
separate blocking-thread pool sized for exactly this purpose, or (per
`ARCHITECTURE.md`) a bounded Rayon pool for CPU-bound work specifically.
Either way, the rule is: **await points belong on the async side, CPU-bound
loops belong on the blocking side**, and the two must never be conflated.

**A specific footgun, directly from S18**: once a `spawn_blocking` task has
actually started running, cancelling its `JoinHandle` (dropping it, or a
surrounding `select!` losing the race) does **not** stop that blocking
closure from running to completion — there is no way to forcibly interrupt
a thread mid-execution safely. This means cancellation-aware blocking work
has to check a cancellation flag/token *itself*, periodically, inside the
loop — `abort()`-and-forget is not a real cancellation mechanism for
already-started blocking work, only for work still queued.

## `CancellationToken` and graceful shutdown

`tokio_util::sync::CancellationToken` (already a workspace dependency,
`Cargo.toml`) is the cooperative-cancellation primitive used throughout the
`HistoryProvider` trait's `scan()` signature
(`crates/scout-providers/src/port.rs`): every long-running stream takes one,
and is expected to check it periodically (via `select!` against
`cancelled()`, or between loop iterations) and unwind cleanly rather than
being killed mid-flight. This is cooperative, not preemptive — a task that
never checks the token never stops, same caveat as the `spawn_blocking`
point above.

The exit-code-141 case (`ADR-005`: closed stdout pipe, e.g. `... | head -1`)
is the sharpest test of this design: the pipeline's producers must notice
the broken pipe and stop cleanly — no panic, no backtrace dumped to a
half-closed terminal — which in practice means every writer to stdout
checks for `BrokenPipe` and treats it as a cancellation signal, not an
unexpected error to propagate as a crash.

## `Arc<Mutex<T>>` is fine — the question is *where*

A natural worry coming from systems programming: "isn't a mutex slow, isn't
this exactly what lock-free structures exist to avoid?" The honest answer
for this codebase's actual profile: **a short-held mutex around a small,
infrequently-contended piece of state is invisible** next to network I/O
latency measured in tens of milliseconds. `Arc<Mutex<T>>` — shared
ownership plus exclusive access — is the correct, boring, obviously-correct
default here, exactly like it would be in C++ with
`std::shared_ptr<std::mutex-protected T>`. The rule that actually matters,
called out explicitly in `AGENTS.md` invariant #14, is narrower: **no
global hot-path `Mutex<HashMap>`**, and **no lock guard held across an
`.await` point** (holding a lock while yielding to the scheduler can starve
every other task waiting on that same lock, for however long the await
takes — potentially far longer than the lock's actual critical section
needs).

`ARCHITECTURE.md` §11's sharding design — `hash(WalletKey) % shards`, each
shard owning its own state with no shared hot lock — sidesteps the question
entirely for the ledger's actual hot path: message passing (each wallet's
events routed to its owning shard via a channel) replaces shared mutable
state, which is the same "share nothing, communicate via channels"
philosophy Go and Erlang built entire runtimes around, and which Rust's
ownership model happens to make provably safe to implement (a channel send
transfers ownership; there is no way to accidentally keep a reference into
data you just handed off).

## Why not lock-free — stated honestly, not defensively

This section exists because it is tempting, coming from a systems
background, to reach for lock-free structures as a default "better"
choice. The spec is explicit and this doc agrees with it fully:

- `ARCHITECTURE.md` §14: "Нет обещания 'lock-free везде'... цель —
  отсутствие конкурентной записи в общую горячую структуру," not "no locks
  anywhere in the codebase."
- `SOURCES.md` S22, about `DashMap` specifically: "concurrent map с
  locking-поведением; не называть её lock-free." Even a widely-used
  "concurrent" collection in the Rust ecosystem is sharded-locking under
  the hood, not lock-free in the strict sense — a useful reminder that
  "concurrent" and "lock-free" are not synonyms, in any language.
- `AGENTS.md` invariant #14: "Не внедрять custom lock-free/unsafe без
  профиля, ADR и измеренной необходимости." A hand-rolled lock-free queue
  or hash map is real, hard, `unsafe`-heavy engineering — atomics with
  carefully chosen memory ordering (`Acquire`/`Release`/`SeqCst`), the ABA
  problem when nodes get reused, false sharing from cache-line contention
  between unrelated atomics living too close together in memory. These are
  the same hazards C++'s `<atomic>` and lock-free literature grapple with;
  Rust does not make them safe automatically — that is exactly why
  `#![forbid(unsafe_code)]` in every crate here (see
  `02-type-driven-correctness.md`) makes writing one **impossible** in
  this codebase's own crates without first removing that forbid attribute,
  which itself would need to survive an ADR and a measured profile showing
  a genuine mutex bottleneck.

The actual bottleneck this system expects, per its own architecture
documents, is **network RPC latency measured in tens of milliseconds**, not
microsecond-scale contention on an in-memory counter. A mutex acquired and
released in under a microsecond, contended occasionally, is not
distinguishable from free in that profile. Reaching for lock-free machinery
here would be solving a problem this system doesn't have, at the cost of a
much harder-to-verify-correct codebase — the definition of a premature and
misplaced optimization.

## `Send`, `Sync`, `'static` — the compiler as your thread-safety reviewer

In C++, whether a type is safe to share across threads is a fact about its
implementation that the compiler does not check — it lives in documentation
and code review discipline, and a subtle mistake (a type with a raw pointer
member that looks safe but has a shared mutable field) compiles fine and
fails only at runtime, if you're lucky enough to catch it under a race
detector.

`Send` (safe to transfer ownership across a thread boundary) and `Sync`
(safe to share a reference across threads) are compiler-verified marker
traits in Rust — a type is `Send`/`Sync` automatically if every field it
contains is, and the compiler refuses to compile code that would move a
non-`Send` type into a spawned task. `HistoryProvider: Send + Sync`
(`crates/scout-providers/src/port.rs`) is a real, enforced promise about
every implementation of that trait, checked at every call site, not a
comment saying "this should be thread-safe." `BoxStream` (a
`Pin<Box<dyn Stream + Send>>` alias from `futures`) exists specifically so
this trait can be object-safe (usable as `dyn HistoryProvider`) while still
carrying that `Send` guarantee through a type-erased boundary.
