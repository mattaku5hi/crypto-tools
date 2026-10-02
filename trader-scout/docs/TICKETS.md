# Task graph (P0-P8)

Status: replaces GitHub Issues for this workspace — `gh` CLI is not installed in this environment, so
tasks are tracked here as a task graph with blocking relationships, per ROADMAP.md's phase structure.
Update statuses as work lands; do not let this drift from actual commits.

Legend: `done` | `in-progress` | `blocked` | `todo`

## P0 — Feasibility & corpus

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P0.1 | Source capability matrix | in-progress | — | `docs/p0/source-capability-matrix.md`: added provider category taxonomy (raw RPC / indexed history / aggregated market data — only first two feed the ledger) and per-network measurement candidates. Still no live credentials; nothing may become `live_verified` until measured |
| P0.2 | Deployment registry | in-progress | — | `docs/p0/deployment-registry.md`: entry table (4-condition schema) still has zero qualifying rows. A 2026-09-30 live census section now records observed program ids/discriminators from real pump.fun-mint transactions — none confirmed as a deployment yet, but no longer "empty pending research" |
| P0.3 | Ground-truth corpus (30+ hand-checked scenarios) | in-progress | — | Credentials now exist (Helius, Blockscout). 3 real fixtures captured (`docs/p0/measurements/fixtures/`: 2 pump.fun mint transaction sets + 1 wallet probe) but far short of 30 hand-checked scenarios; no longer credential-blocked, just not yet done |
| P0.4 | ADR-001..006 (+ ADR-009 decoder variant verification, 2026-10-02) | done | — | `docs/adr/`, committed in `07f066b` |
| P0.5 | ADR-008: Tier 1/Tier 2 extensibility (scout-api, TxDecoder, NormalizedActivitySource) | done | — | `56a0c2c`..`fc93928` (S0-S5). `docs/TIER-PLAN.md`, `docs/PROVIDERS.md` |
| P0.6 | `scout-probe`: CLI to run the P0.1 measurement checklist against a live provider and record dated results | todo | credentials | Scope: hit each candidate vendor's actual endpoints, record retention/coverage/CU-cost per `docs/p0/source-capability-matrix.md`'s "What to measure" section. Do not build before credentials exist — nothing to measure yet |

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

## Cohort pipeline gaps surfaced 2026-09-27 (insider redefinition)

Restricting "insider" to buyers entering before a pump on an *already* high-cap/graduated token
(not the launch segment) is a real improvement — it removes the bot/sniper noise floor. It also
surfaces two structural gaps that must be closed before this cohort can produce output, not just
configured around:

| ID | Task | Status | Blocked by | Notes |
|---|---|---|---|---|
| P4.1 | Solana AMM swap decoder (Raydium/Meteora/Orca) | todo | credentials, P0.2 | The only existing Solana decoder is a synthetic bonding-curve *buy*. A pump on an already-migrated token happens on an AMM pool, not the bonding curve — this decoder does not exist yet and nothing currently reads AMM swaps |
| P4.2 | Pump-leg detection from own decoded swap reserves | todo | P4.1 | "Before the pump" requires a price/liquidity time series derived from our own reserve deltas (invariant #16: never from an aggregator). No such series exists; depends on P4.1's output |
| P4.3 | Per-cohort quality gate overrides | todo | scout-analytics | `config/scout.example.toml`'s global `min_closed_episodes = 20` would reject most real insiders (5-10 episodes is typical for a concentrated inserter). Quality gates must become per-cohort-profile, not one global threshold, or the pipeline structurally cannot surface this cohort |
| P4.4 | CEX/bridge hot-wallet exclusion list (Solana) | todo | credentials | Funder-clustering for Sybil linkage is not usable without first excluding known exchange/bridge funding addresses — without it, most wallets cluster into one blob via a shared CEX hot wallet, producing false positive linkage, not true Sybil detection |
| P4.5 | Live polling `HistoryProvider`-adjacent port + watermark resume | todo | P2.3 (done) | `HistoryProvider::scan()` is a bounded, terminating scan, not a subscription with reconnect/resume. A distinct trait is needed for continuous watchlist polling, built on the durable watermark primitive `scout-storage` already provides |
| P4.6 | Honor `ProviderError::RateLimited { retry_after }` | todo | — | The type exists (`scout-api::error::ProviderError`) but nothing in `scout-engine`/CLI callers reads it yet — no backoff currently happens on 429 |
| P4.7 | Per-provider budget guard (`ProviderError::BudgetExhausted`) | todo | P0.6 (measured costs) | Free-tier credit ceilings must be enforced before a call, not discovered via a failed request mid-run; new typed error mapping to exit 4, same honest-refusal pattern as `ConfigurationRequired` |
| P0.7 | Overlap experiment: leaderboard candidates vs. `buyer-intersect` runner-scan | todo | credentials (have Codex/Helius) | Decides whether this workspace is a "strict verifier of public leaderboards" or an "independent smart-money detector" — a real product-shape decision, not an implementation detail. Pull ~N candidate wallets from a public leaderboard (GMGN, etc.) and ~N candidates independently via `buyer-intersect` against the same runner set over the same window; measure set overlap. Large overlap → leaderboards suffice, skip building independent discovery further. Small overlap → the independent path finds real alpha, worth the Helius budget. One day of data closes this, not more architecture |
| P0.8 | Measure actual runners/day per network (replace the 200-vs-20 guess) | todo | credentials (have Codex) | `docs/p0/measurements/2026-09-27-helius-blockscout.md` got partial numbers via Codex `filterTokens` (Solana, MC>=$1M + liquidity>=$10k, 24h window) but the count fluctuated between single-digit and low-tens across runs in the same session — needs a proper daily snapshot (same time of day, several consecutive days), not a single point-in-time call. Repeat for BSC/Base/Robinhood once Codex coverage for those chains is exercised the same way. This number directly sizes the Helius credit budget for P0.6/P4.7 |

| P0.9 | Identify program `DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH` | todo | — | Top-level 112-byte instruction, discriminator `414b3f4ceb5b5b88`, observed in both census mints. Checked against `sha256` of 5 candidate Anchor names (`global:buy`, `global:swap_base_in`, `global:swap_base_out`, `global:route`, `shared_accounts_route`) — no match. Guessing more names is low-yield; resolve identity from an official/verifiable source (verified IDL, on-chain program metadata, official docs) first, then derive discriminators from that IDL |
| P0.10 | Capture a pre-migration pump.fun bonding-curve sample | todo | credentials (have Helius) | Both census mints (`docs/p0/deployment-registry.md`) had already migrated to PumpSwap AMM, so the bonding-curve program (`6EF8rr...`) was absent from the 10-tx sample. "Not observed in 10 transactions" is not evidence of absence. Needs discovery of a very-new, low-MC/pre-migration mint (Codex `filterTokens` with a low-MC/new-pair filter, or the pump.fun surface directly) followed by a Helius `full`-mode probe |
| P0.11 | Buyer attribution via token-balance deltas, not position | done | — | `2c80cda`: `scout_core::SolanaTokenBalanceChange` (integer-only, `uiTokenAmount.amount` never `uiAmountString`) carried on `RawSolanaTransaction`; `HeliusProvider` resolves `accountIndex` against the same ALT-extended key space already built for instructions. 3 tests against the already-committed `pump_mint1_full.json` fixture (data[2]: owner `EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv`, delta `181,673,284,237`). Positional shortcuts remain forbidden by the type's doc comment. Follow-up: P0.13 turns this into something `classify_buy` can consume |
| P0.12 | Replace synthetic bonding-curve decoder with a confirmed deployment | in-progress | P0.2 conditions #2/#4 | `cece920`: `scout-dex-solana::bonding_curve_buy` rewritten against the official pump.fun IDL (`pump-fun/pump-public-docs` `idl/pump.json` @ `e0687ae9`), real `buy` (16 accounts, 25 bytes) and `sell` (14 accounts, 24 bytes), `user` = `accounts[6]` verified by IDL name + live SOL delta; real-data fixture `pump_bonding_curve_buy_probe.json`; event-CPI false-positive regression test kept. Remaining: registry conditions #2 (activation slot) and #4 (schema-table row) in `docs/p0/deployment-registry.md`. P0.10 (pre-migration sample) is effectively covered by the probe fixture's live bonding-curve buy |
| P0.13 | Aggregate token balance changes into net deltas for `classify_buy` | done | — | `9823fab`: `scout-normalize::solana_balance::aggregate_solana_token_balance_changes` keys by `(mint, owner)` via `BTreeMap`, checked `i128` → `SignedAmount`, `owner: None` excluded, multiple gaining owners → `Ambiguous { MultipleCandidates }`, always `ActionKind::Unknown` (Swap only from decoder evidence — see P0.16) |
| P0.14 | Base-unit vs. `Money`/`SignedAmount` scale boundary for Solana token amounts | todo | P0.13 | Token balance changes are raw base units paired with per-mint `decimals`; `scout-core::Money`/`SignedAmount` are `MONEY_SCALE`-scaled. These must never be implicitly converted. Decide and document (ADR note or explicit rationale in the conversion function) whether raw base units flow all the way to the report/ledger boundary, or whether a decimals-aware rescale happens at a named, checked, integer-only function — not silently inside a map/aggregation closure |
| P0.15 | Known-program/vault/PDA exclusion set for buyer attribution | todo | — | Pubkey bytes alone cannot distinguish a wallet from a program-owned account (PDA) or vault — P0.13's `MultipleCandidates` ambiguity will fire constantly without this. Cross-reference P4.4 (CEX/bridge hot-wallet exclusion for Sybil clustering) — likely the same exclusion-set mechanism, not a separate one |
| P0.16 | Solana `buyer-intersect` vertical slice: Helius paginated mint scan → pump.fun decoder → ADR-003/ADR-009 buy qualification → CLI | in-progress | — | Bounded `paginationToken` follow-through in `HeliusProvider` (`with_max_pages`, default 10×100 tx, unmeasured; `truncated` only with an unconsumed cursor). Decoder covers all six IDL trade variants; strict buyers only from `FixtureVerified` (`buy`, `sell`, `sell_v2`), `IdlOnly` would-be buys are counted and force exit 3 (ADR-009); unknown discriminators under `6EF8rr…` are coverage gaps. Data-layer fixes in the same slice: `meta.err` carried (failed tx never qualifies), closed token accounts (pre-only balances) no longer dropped. Remaining: live smoke run; P0.17 fixtures for `IdlOnly` variants. Scope: bonding curve only — post-migration venues not decoded (lower bound). Prerequisite for P0.7 |
| P0.17 | Golden fixtures for `buy_exact_sol_in`, `buy_v2`, `buy_exact_quote_in_v2` | todo | live Helius access | Capture one successful real tx per variant (e.g. `getTransactionsForAddress` on `6EF8rr…`, `sortOrder: desc`, one page), commit under `docs/p0/measurements/fixtures/`, assert `user`/`mint` against owner deltas, then promote to `FixtureVerified` per ADR-009 and bump `SOLANA_BUY_QUALIFICATION_VERSION` |

## Current frontier (tasks ready to start right now, no credentials needed)

- `scout-storage`: SQLite WAL embedded store (P2.3), independent of credentials
- Config loader for `config/scout.example.toml`
- Output/JSONL envelope (P1.4), once shape is fixed
- CLI wiring for the three binaries over the existing SDK layer
- `scout-engine` orchestration skeleton (bounded channels design, no live transport yet)
- Solana decoder mechanism on synthetic fixture, mirroring `scout-dex-evm`
- `examples/embedded_scanner.rs` (P8.1)

## When credentials arrive (P0.1 measurement pass)

Per the updated `docs/p0/source-capability-matrix.md`, before selecting any vendor: register 2-3
free-tier candidates per network (Solana: Helius, Shyft; BSC: BscScan/Etherscan V2 for indexed
history, plus any of Ankr/Chainstack/dRPC/GetBlock for raw `eth_getLogs`) and run the measurement
checklist in that file's "What to measure" section against each. No row may be promoted to
`live_verified` without a dated, actual successful call recorded per ADR-006. Do not pick a single
vendor from marketing claims alone — this is exactly the failure mode P0.1 exists to prevent.
