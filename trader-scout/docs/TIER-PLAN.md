# Tier 1 / Tier 2 data source plan

Fixes the plan the user asked to lock in. See ADR-008 for the full
architectural decision this plan implements, `docs/PROVIDERS.md` for
the copy-pasteable registration steps, and
`docs/p0/source-capability-matrix.md` for the underlying taxonomy and
measurement methodology.

## The two tiers

**Tier 1 — source of truth for the ledger.** A `HistoryProvider`
returns raw payloads (`scout_core::RawPayload`); we decode them
ourselves via a scope-checked `TxDecoder` (ADR-008 S4). Every fact that
reaches `scout-ledger::Ledger::acquire` came from this path.
`TrustLevel::Verified` — only ever constructed by `scout-engine` for
data that actually passed through decode.

**Tier 2 — never the ledger's source of truth.** A
`NormalizedActivitySource` returns already-classified claims we did not
derive ourselves (`ExternalActivityClaim` — no trust field, so a
source cannot self-declare `Verified`). Reaching the ledger at all
requires an explicit `ExternalDataOptIn` token
(`Ledger::acquire_unverified`), and the resulting lot's basis is forced
`Unknown` — `realized_trade_pnl` is `None` for any disposal touching
it, never a number computed from unverified data (AGENTS.md invariant
#6's existing pattern, extended here).

## Tier 1: what to register, in order

1. **Etherscan V2** — one key, all EVM chains including BSC. Closes the
   actual bottleneck this workspace identified: `eth_getLogs`-based
   retention is thin on every free RPC vendor, but Etherscan V2's
   `txlist`/`tokentx` gives indexed per-address history directly.
2. **One EVM RPC provider** (dRPC or Ankr) — raw transport for decode-
   time `eth_getLogs`/`eth_getTransactionReceipt` calls. Not a
   substitute for #1; a complement.
3. **Helius** (Solana, free tier) — `getSignaturesForAddress` +
   retention/closed-ATA coverage is the decision axis, not Helius's own
   parsed-tx decoders (which this workspace does not use, per invariant
   #16 — we decode bonding-curve buys ourselves in `scout-dex-solana`).

Full env-var-to-config mapping and registration URLs: `docs/PROVIDERS.md`.

## Tier 2: what's available, not yet wired

Dune, Birdeye, DEX Screener API, GeckoTerminal API, Chainbase — useful
for discovery/cross-check only (AGENTS.md invariant #16: aggregated
market data is never the ledger's source of truth). No
`NormalizedActivitySource` implementation exists yet for any of these;
`scout-api::NormalizedActivitySource` is the trait to implement when
one is needed.

## Numbers are unmeasured

Every free-tier limit the user's own research table lists (Ankr 200M
credits/mo, dRPC 210M CU/30 days, Shyft $0 unlimited, etc.) is that
table's research, not a verified fact of this workspace. Before any
vendor is marked `live_verified` in `docs/p0/deployment-registry.md`,
the measurement checklist in `docs/p0/source-capability-matrix.md`
("What to measure") must be run against it with dated, actual
successful calls (ADR-006). This is tracked as ticket P0.1 in
`docs/TICKETS.md`.

## Status as of this plan

- Architecture (ADR-008, `scout-api`, `TxDecoder`, `NormalizedActivitySource`,
  `ExternalDataOptIn`, ledger gating): **done**, commits `56a0c2c`..`fc93928`.
- Registration (this document): **written, not yet executed** — no
  credentials exist in this environment (verified: grepped `.env`/rc
  files/environment for provider key names, found none).
- Measurement pass (P0.1): **blocked on credentials**, tracked in
  `docs/TICKETS.md`.
