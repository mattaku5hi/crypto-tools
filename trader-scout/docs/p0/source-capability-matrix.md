# P0.1 — Source capability matrix

Status: living document, re-verify whenever a dependency/endpoint/claim changes (per SOURCES.md's
closing note and ADR-006).

Verified on 2026-09-22. **This workspace currently has zero provider API credentials configured.**
Per ADR-006, no row below may claim `live_verified` until an actual successful call against a live
endpoint is made and dated. Everything is `documented` (read from official docs) or `unknown`
(not yet investigated).

Legend: `documented` | `fixture_verified` | `live_verified` | `unsupported` | `unknown`

## Provider category taxonomy (read before picking a vendor)

Candidates fall into three categories with very different roles in this architecture. Conflating
them is the single easiest way to end up trusting an untrusted classification of "what counts as a
buy" — exactly what AGENTS.md invariants #16/#18 forbid.

1. **Raw RPC** (Ankr, Chainstack, dRPC, GetBlock, Alchemy, QuickNode, public endpoints) — exposes
   `eth_getLogs`/`getTransaction`/`getSignaturesForAddress` directly. This is transport, not history;
   it requires our own decoders on top (scout-dex-evm/scout-dex-solana) regardless of vendor.
2. **Indexed per-address history** (Helius enhanced history, Shyft, BscScan/Etherscan V2
   `account/txlist`+`tokentx`, Chainbase) — answers "all transactions for this address" in one call.
   Candidate for `WalletActivity`/discovery; still requires our own decode+normalize pass, never
   accepted as pre-classified "this was a buy."
3. **Aggregated market data** (Birdeye, DEX Screener, GeckoTerminal, Dune) — pre-computed trades/
   volume/PnL by a third party. **Never a ledger source of truth** — this project builds its own
   FIFO/attribution specifically because third-party buy/sell classification is not trusted
   (AGENTS.md invariant #16). Legitimate uses: discovery-candidate generation and cross-checking our
   own computed numbers (a divergence from DEX Screener is a signal to investigate our decoder, not
   evidence either number is right). Dune is particularly useful for the P0 research pass itself
   (SQL over historical pool/wallet data to find deployment candidates for P0.2) but its output is
   not reproducible from our own fixtures, so it never enters the ledger path.

## Solana (mainnet)

Key fact: `getSignaturesForAddress` (S06) is a **base RPC method**, not an indexer-only feature — Solana
has a built-in per-address signature index, unlike EVM. The decisive axis for any Solana RPC/indexer
candidate is therefore **retention depth** (how far back history is actually kept — most public nodes
prune it; only archival tiers keep full history) and **closed/reassigned token-account coverage**
(`getTokenAccountsByOwner` only reflects current ownership; a wallet's full trading history requires
resolving accounts it no longer owns, which base RPC cannot do — this is where an indexer earns its
keep). Helius's headline "enhanced/parsed transactions" feature is largely moot for this project's
ledger path specifically, since we require our own pinned-ABI/IDL decoders per invariant #16 — so a
provider's raw-history completeness and retention matter far more here than its parsing convenience.

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity (token → historical buyers) | unknown | — | Requires pool/launch discovery across bonding-curve + AMM migrations; no provider chosen yet |
| WalletActivity (wallet → trading history) | unknown | — | Candidates to measure: Helius (free tier), Shyft ($0/unlimited credits per user's research, 10 RPS). Decisive factor is retention depth + closed-ATA coverage, not parsed-tx convenience (see above) |
| Raw transactions/receipts | documented | S06 getSignaturesForAddress, S07 getTransaction | Standard RPC; `getSignaturesForAddress` is account-key mention search, not a full mint index |
| Historical state | unknown | — | Not needed for v1 spot-swap scope beyond tx-level data |
| Native/internal flows | documented | S07 getTransaction | pre/post native balances in tx meta |
| Historical token-account ownership | unknown | — | Current `getTokenAccountsByOwner` does not recover closed/reassigned accounts; needs token/pool index or historical owner-aware indexer |
| Prices (historical) | unknown | — | No price source selected yet; aggregators (Birdeye/GeckoTerminal) are cross-check candidates only, never a basis-price source of truth |
| Finality | documented | Solana docs (commitment levels) | finalized vs confirmed; slot timestamp nullable |
| Earliest retained history | unknown | — | **This is the deciding measurement for provider choice** — must be measured per candidate, not assumed from a pricing page |
| Paging | documented | S06/S07 | signature-based pagination |
| Rate limits / billing | unknown | — | No provider account exists yet; free-tier RPS/credit numbers from third-party comparisons are unverified until measured directly |
| Provenance | documented | S06-S10 | citations dated 2026-09-21 in SOURCES.md |

**Candidates to measure in P0.1** (unverified, from user research, not yet confirmed against live
endpoints): Helius free tier, Shyft free tier. Both must be measured for actual retention depth and
closed-ATA coverage before either is selected — neither is currently `live_verified` or even
`documented` beyond their own marketing claims.

## BSC mainnet (chain_id 56)

Key fact: base EVM JSON-RPC has **no per-address history method at all** (S05) — the only mechanism is
`eth_getLogs` over a block range, filtered by topics. Public BSC endpoints disable this method entirely
(S03), which blocks *transport*, not a specific vendor's feature — **any** private RPC endpoint
(Ankr, Chainstack, dRPC, GetBlock, Alchemy, QuickNode) removes this specific blocker equally; no
previously-assumed vendor preference here is justified without a measured reason. Providers also cap
the maximum block range per `eth_getLogs` call (typically 2k-10k blocks), so a full wallet history over
a long period costs many sequential calls — actual credit/CU cost must be measured per provider per
method, not read off a "N million credits/month" headline, since per-method CU cost varies
substantially (e.g. `getTransaction` with full receipt bodies typically costs far more than a simple
call).

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity | unknown | — | No confirmed DEX deployment registry entry yet (P0.2) |
| WalletActivity | unsupported (public RPC) / unknown (indexed) | S03 | `eth_getLogs` disabled on public mainnet endpoints (S03) — a hard blocker for the raw-RPC path specifically, removed by any private RPC. Indexed-history candidate: BscScan/Etherscan V2 `account/txlist`+`tokentx` (per-address transaction listing, free tier exists) — not yet measured |
| Raw transactions/receipts | documented | S03, S05 | Standard EVM JSON-RPC, but public endpoint log access unsupported |
| Historical state (archive) | unknown | — | No archive-capable endpoint configured |
| Native/internal flows | documented | S05 | Standard EVM tx/receipt fields |
| Prices | unknown | — | No source selected; aggregators are cross-check candidates only |
| Finality | documented | BSC docs (referenced via S03) | BSC-specific finality characteristics, not yet detailed here |
| Earliest retained history | unknown | — | Depends on provider; must be measured, not assumed |
| Rate limits / billing | unknown | — | No provider account; public endpoint has no logs regardless of budget. Per-method CU cost (not just monthly credit total) must be measured before any vendor comparison is meaningful |
| Provenance | documented | S03 | dated 2026-09-21 |

**Candidates to measure in P0.1** (unverified): BscScan/Etherscan V2 API for `WalletActivity` via
`account/txlist`+`tokentx` (free tier, per user's research); any of Ankr/Chainstack/dRPC/GetBlock for
raw `eth_getLogs`-based `TokenMarketActivity` and verification of the indexed history's completeness.
Alchemy's actual BSC support on its free tier is itself unverified and should not be assumed — its
distinguishing EVM feature (`alchemy_getAssetTransfers`) has a free BscScan/Etherscan-equivalent, so it
carries no assumed advantage over the other raw-RPC candidates here pending measurement.

## Base mainnet (chain_id 8453)

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity | unknown | — | No confirmed DEX deployment registry entry yet (P0.2) |
| WalletActivity | unknown | — | No indexed wallet-history provider configured |
| Raw transactions/receipts/logs | documented | S04, S05 | Public `mainnet.base.org` documented as connectable; log-scan capability at scale unverified |
| Historical state (archive) | unknown | — | Not verified against any specific endpoint |
| Fees | documented | S15 | L2 execution + L1 fee components documented; adapter not yet implemented |
| Prices | unknown | — | No source selected |
| Finality | documented | Base docs (via S04) | OP-stack-style finality; not yet detailed here |
| Rate limits / billing | unknown | — | Public endpoint only; no paid provider configured |
| Provenance | documented | S04, S15 | dated 2026-09-21 |

## Robinhood Chain mainnet (chain_id 4663)

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity | unknown | — | No confirmed DEX deployment on this chain as of this writing; likely remains `unknown`/`unsupported` through P0 given the chain's youth (ARCHITECTURE.md §5 anticipates this) |
| WalletActivity | unknown | — | No indexed provider identified |
| Raw transactions/receipts | documented | S01, S02 | Arbitrum-compatible L2 infra documented; no independent capability verification done |
| Fees | documented | S16 | Separate fee model documented; must not copy Base's formula without verification |
| Finality | unknown | — | Not yet detailed |
| Testnet identity (chain_id 46630) | documented | S01 | Must never be conflated with mainnet 4663 in any scan or report |
| Rate limits / billing | unknown | — | Public endpoint only |
| Provenance | documented | S01, S02, S16 | dated 2026-09-21 |

## Cross-cutting notes

- No row in this matrix may be promoted to `live_verified` by editing this file alone — it requires
  an actual dated, successful call recorded alongside the promotion (ADR-006).
- BSC's `eth_getLogs` restriction on public endpoints is a hard blocker for TokenMarketActivity/
  WalletActivity on that chain until a paid/private RPC endpoint is configured — this is not a code
  bug to work around by "trying harder" against the public endpoint.
- Robinhood Chain DEX support is expected to remain thin through P0-P1; ROADMAP.md P6.2 explicitly
  plans for "нет нужного wallet index → честный собственный indexing path либо явный unsupported" —
  this matrix documents that expectation rather than inventing deployment addresses to fill cells.
- **Do not select a single vendor before measuring.** The `HistoryProvider` trait
  (`crates/scout-providers/src/port.rs`) makes vendor choice a reversible configuration decision, not
  an architectural one — swapping providers later is a new trait implementation plus config, not a
  rewrite of scout-ledger/scout-normalize/scout-analytics. Register 2-3 free-tier candidates per
  network and let the measurements below decide, per ARCHITECTURE.md §5's explicit rejection of "one
  universal provider."
- **What to measure per (provider, network) pair before any row here may become `live_verified`:**
  earliest retained block/slot (binary-search against "give me block N"); maximum block range per
  `eth_getLogs` call (EVM) or page size/depth of `getSignaturesForAddress` (Solana); whether archive
  methods are available on the free tier at all; actual credit/CU cost of a representative scan of one
  known wallet over a fixed period; behavior under rate limits (429s, burst vs. sustained, whether
  `Retry-After` is honored); **provider divergence** — does the same request against two different
  providers return identical results (this also catches silently-incomplete responses, and is a
  distinct ACCEPTANCE-level requirement, not just a nice-to-have); ToS terms for commercial/analytical
  use on the free tier.
