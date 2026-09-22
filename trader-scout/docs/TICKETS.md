# Task graph (P0-P8)

Status: replaces GitHub Issues for this workspace — `gh` CLI is not installed in this environment, so
tasks are tracked here as a task graph with blocking relationships, per ROADMAP.md's phase structure.
Update statuses as work lands; do not let this drift from actual commits.

Legend: `done` | `in-progress` | `blocked` | `todo`

## P0 — Feasibility & corpus

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P0.1 | Source capability matrix | done | — | `docs/p0/source-capability-matrix.md`, all `documented`/`unknown` (no credentials) |
| P0.2 | Deployment registry | done | — | `docs/p0/deployment-registry.md`, intentionally empty pending research |
| P0.3 | Ground-truth corpus (30+ hand-checked scenarios) | blocked | credentials | Cannot capture real mainnet samples without provider keys; synthetic-only fixtures can proceed independently |
| P0.4 | ADR-001..006 | done | — | `docs/adr/`, committed in `07f066b` |

## P1 — Workspace, domain contracts, input/output

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P1.1 | Cargo workspace skeleton | done | — | `c89ded1` |
| P1.2 | scout-core domain types | done | — | `08d25a7`: identity, amount, error |
| P1.3 | Input adapters (lines/csv/jsonl) | in-progress | — | `fca33e2`: lines+csv done, jsonl deferred to output-schema work |
| P1.4 | JSON Schema for input/output | todo | P1.3 (jsonl) | |

## P2 — Transport, scheduler, durable raw storage

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P2.1 | HTTP/RPC clients, chain identity preflight | todo | credentials | Cannot preflight genesis identity without a live endpoint |
| P2.2 | Bounded admission, retries, circuit breaker | todo | P2.1 | |
| P2.3 | SQLite WAL embedded store | todo | P1.2 | Can start independent of credentials |
| P2.4 | Mock RPC server | todo | P2.1 (interface) | Can build against the HistoryProvider trait before real transport exists |

## P3 — First EVM vertical slice (Base)

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P3.1 | EVM adapter (logs/tx/receipts) | blocked | credentials, P0.2 | |
| P3.2 | One confirmed Base DEX decoder | blocked | P0.2 (deployment confirmed) | |
| P3.3 | Ownership + route normalization | todo | P3.1, P3.2 | |
| P3.4 | buyer-intersect + ledger on fixtures | in-progress (design) | scout-ledger | Can build against synthetic fixtures before P3.1/P3.2 land |

## P4-P8

Deferred until P0-P3 gates are met; not yet broken into sub-tasks. See ROADMAP.md for the phase
descriptions this graph will expand into.

## Current frontier (tasks ready to start right now, no credentials needed)

- `scout-providers`: `HistoryProvider` trait + `UnconfiguredProvider` + `FixtureProvider` (ADR-006)
- `scout-ledger`: FIFO lots, fee allocation, property tests (ADR-004)
- `scout-normalize`: net-delta buy classification (ADR-003), against synthetic fixtures only
- P1.4 JSON Schema, once jsonl input/output shape is fixed together
