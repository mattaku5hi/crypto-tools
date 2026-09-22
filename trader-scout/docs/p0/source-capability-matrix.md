# P0.1 — Source capability matrix

Status: living document, re-verify whenever a dependency/endpoint/claim changes (per SOURCES.md's
closing note and ADR-006).

Verified on 2026-09-22. **This workspace currently has zero provider API credentials configured.**
Per ADR-006, no row below may claim `live_verified` until an actual successful call against a live
endpoint is made and dated. Everything is `documented` (read from official docs) or `unknown`
(not yet investigated).

Legend: `documented` | `fixture_verified` | `live_verified` | `unsupported` | `unknown`

## Solana (mainnet)

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity (token → historical buyers) | unknown | — | Requires pool/launch discovery across bonding-curve + AMM migrations; no provider chosen yet |
| WalletActivity (wallet → trading history) | documented | S08 Helius enhanced history | Candidate only; historical retention limits and closed-account coverage not yet verified |
| Raw transactions/receipts | documented | S06 getSignaturesForAddress, S07 getTransaction | Standard RPC; `getSignaturesForAddress` is account-key mention search, not a full mint index |
| Historical state | unknown | — | Not needed for v1 spot-swap scope beyond tx-level data |
| Native/internal flows | documented | S07 getTransaction | pre/post native balances in tx meta |
| Historical token-account ownership | unknown | — | Current `getTokenAccountsByOwner` does not recover closed/reassigned accounts; needs indexed source |
| Prices (historical) | unknown | — | No price source selected yet |
| Finality | documented | Solana docs (commitment levels) | finalized vs confirmed; slot timestamp nullable |
| Earliest retained history | unknown | — | Depends on chosen RPC/indexer provider's retention |
| Paging | documented | S06/S07 | signature-based pagination |
| Rate limits / billing | unknown | — | No provider account exists yet |
| Provenance | documented | S06-S10 | citations dated 2026-09-21 in SOURCES.md |

## BSC mainnet (chain_id 56)

| Capability | Status | Source | Notes |
|---|---|---|---|
| TokenMarketActivity | unknown | — | No confirmed DEX deployment registry entry yet (P0.2) |
| WalletActivity | unsupported (public RPC) | S03 | `eth_getLogs` explicitly disabled on documented public mainnet endpoints |
| Raw transactions/receipts | documented | S03, S05 | Standard EVM JSON-RPC, but public endpoint log access unsupported |
| Historical state (archive) | unknown | — | No archive-capable endpoint configured |
| Native/internal flows | documented | S05 | Standard EVM tx/receipt fields |
| Prices | unknown | — | No source selected |
| Finality | documented | BSC docs (referenced via S03) | BSC-specific finality characteristics, not yet detailed here |
| Earliest retained history | unknown | — | Depends on provider |
| Rate limits / billing | unknown | — | No provider account; public endpoint has no logs regardless of budget |
| Provenance | documented | S03 | dated 2026-09-21 |

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
