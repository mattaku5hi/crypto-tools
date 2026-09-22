# Library ergonomics: behaving inside someone else's application

**Status: design intent, partially enforced by code that exists today.**
The rules below are already true of every crate in this workspace (there is
no `tracing_subscriber::init()`, no `std::process::exit`, no hidden runtime
creation anywhere in `scout-core` through `scout-analytics`) — but
`examples/embedded_scanner.rs`, the concrete proof-by-demonstration
`ARCHITECTURE.md` §4 and ROADMAP.md P8.1 call for, does not exist yet.

## The core rule: a library does not own the process

`AGENTS.md` invariant #15 states this as a hard non-negotiable: "SDK не
создает скрытый runtime, не ставит global logging subscriber, не читает
stdin и не завершает процесс." Each clause maps to a real integration
failure mode if violated:

- **Creating its own Tokio runtime** inside a library function means a
  downstream application that already has its own runtime (the overwhelmingly
  common case for any async Rust application) either gets a confusing
  "already inside a runtime" panic, or ends up running two runtimes side by
  side, doubling thread-pool overhead and making cancellation/shutdown
  reasoning far harder. A library must accept an already-running runtime
  as ambient context (every `async fn` here just assumes one exists) and
  never call `#[tokio::main]` or `Runtime::new()` itself.
- **Installing a global `tracing_subscriber`** — a process has exactly one
  global logging subscriber; if a library installs its own on load, it
  either fights the host application's own subscriber setup or silently
  overrides it, producing confusing or missing logs in the *host's* chosen
  format. A library emits `tracing` events/spans and lets the *application*
  decide what subscriber (if any) processes them.
- **Reading stdin or calling `std::process::exit`** are both things only an
  application's `main` should ever do — a library function that reads
  stdin steals input meant for something else in the host process; a
  library calling `exit()` skips the host's own cleanup/shutdown logic
  entirely, including any `Drop` implementations further up the call stack
  that might flush a buffer or close a file.

This is why the composition root pattern from
`01-workspace-and-crates.md` matters concretely here: `scout-app` is the
*only* crate in this workspace allowed to do any of the above, because it
is the binary's own `main`, not a library someone else will embed.

## Typed errors as the entire public error surface

A library that panics on bad input, or that exposes `anyhow::Error`-style
type-erased errors, forces every downstream consumer into one of two bad
choices: catch a panic (impossible to do safely across an `.await` boundary
in general) or `match` on a string message (fragile — any refactor of the
error text silently breaks calling code that pattern-matched on wording).

`ScoutError` (`crates/scout-core/src/error.rs`) is a closed `enum` with
`thiserror`-derived `Display` — every variant is a stable, matchable
identity (`ConfigurationRequired { port, env_var }`,
`ArithmeticOverflow { context }`, etc.), and adding a new variant to this
enum is itself a breaking API change the compiler will flag at every
downstream `match` site that isn't wildcard-catching (a genuine, if
occasionally annoying, tradeoff — exhaustive matches force conscious
handling of every case, at the cost of every new variant needing every
caller's attention).

## `BoxStream` and object safety

`HistoryProvider::scan()` (`crates/scout-providers/src/port.rs`) returns
`BoxStream<'_, Result<ScanEnvelope, ScoutError>>` rather than an `impl
Stream` or a generic associated type. This is a deliberate ergonomics
tradeoff: a generic `impl Trait` return type or GAT-based streaming trait
cannot be used as `dyn HistoryProvider` (object safety requires the
trait's methods to not depend on the concrete `Self` type in their return
position in an unerased way) — and this codebase specifically wants a
`Vec<Box<dyn HistoryProvider>>`-style registry so `scout-engine` can hold
an arbitrary, runtime-chosen set of providers without a generic parameter
propagating through every function that touches the registry. `BoxStream`
(`Pin<Box<dyn Stream<Item = T> + Send>>`, from the `futures` crate) pays a
small, one-time heap allocation and dynamic-dispatch cost per stream to buy
that object-safety — the same tradeoff as choosing a C++ virtual-dispatch
interface (`std::unique_ptr<AbstractBase>`) over a template-based static-
dispatch design when you genuinely need runtime polymorphism (a collection
of heterogeneous concrete types behind one interface), not compile-time
monomorphization.

## What `examples/embedded_scanner.rs` will need to prove

Per `ROADMAP.md` P8.1 and `ACCEPTANCE` F07, once written this example must
demonstrate, as a standalone `cargo run --example embedded_scanner`
inside a *pre-existing* Tokio application context that this project's code
did not create:

1. Constructing a `HistoryProvider` (a `FixtureProvider` today, since no
   live provider exists) and calling `scan()` to get a stream, inside the
   caller's own already-running Tokio runtime — no `#[tokio::main]` inside
   the library-facing code path.
2. Consuming a few items from that stream, then explicitly cancelling via
   the `CancellationToken` passed into `scan()`, and continuing the host
   application afterward — proving cancellation is cooperative and clean,
   not a forced abort.
3. Doing all of this **without linking any CLI-only dependency** — no
   `clap`, no `csv` file-input parsing, no dependency on `scout-app` at
   all. A "scanner-only build" (per F07's exact wording, "Scanner-only
   build не подтягивает обязательную ledger/formatting инфраструктуру")
   should be able to depend on just `scout-scan` + `scout-evm`/
   `scout-solana` and nothing from `scout-ledger`, `scout-analytics`, or
   `scout-app` at all, matching the layered-crate design from
   `01-workspace-and-crates.md`.

Until this example exists as a file, this remains a specific, checkable
promise about what the crate boundaries in this workspace are *supposed* to
guarantee — not yet a running proof that they do.

## Semver discipline (forward-looking)

This workspace is pre-1.0 (`version = "0.1.0"` in every crate) and not
published to crates.io, so semver has not yet had to be enforced in
practice. The rule that will matter once any of these crates gets consumed
by a second, independent project: adding a new variant to a public `enum`
like `ScoutError`, `ActionKind`, or `CapabilityStatus` is a breaking change
under strict semver (any external `match` without a wildcard arm stops
compiling) — the usual mitigation, `#[non_exhaustive]` on public enums
intended to grow over time, is not yet applied anywhere in this codebase
and is worth deciding on deliberately (per-enum, not blanket) before the
first tagged release, rather than discovering the breakage after a
downstream consumer's build fails on an upgrade.
