# Provider registration guide

Practical, copy-pasteable: where to register, what to set. Everything
here is Tier 1 (raw history we decode ourselves) unless marked Tier 2.
See `docs/p0/source-capability-matrix.md` for the taxonomy and
measurement methodology, and ADR-006/ADR-008 for why an unconfigured
provider must fail loudly (exit 4) rather than return empty history.

**No numbers in this file are promises.** Free-tier limits change
without notice; anything quoted here is what the vendor's docs said at
write time, not a guarantee this workspace has measured.

## Order to register in (cheapest -> most useful first)

### 1. Etherscan V2 — one key, all EVM chains including BSC

- Register: https://etherscan.io/register, then https://etherscan.io/myapikey
- One API key works across every EVM chain Etherscan V2 supports (BSC,
  Base, mainnet, ...) via a `chainid` query param — this is what closes
  the `eth_getLogs`-retention gap that no RPC-only vendor solves cheaply.
- Env var: `SCOUT_ETHERSCAN_API_KEY` (not yet wired into
  `config/scout.example.toml` — P0.1 follow-up; today the EVM history
  port only has the RPC-side env var below).
- What it unlocks: `txlist`/`tokentx`-style indexed per-address history
  for BSC/Base, which `docs/p0/source-capability-matrix.md` identifies
  as the actual bottleneck (not raw RPC access).

### 2. One EVM RPC provider — dRPC or Ankr (raw `eth_getLogs`)

- dRPC: https://drpc.org (sign up, create a project, copy the HTTPS
  endpoint for the chain you need).
- Ankr: https://www.ankr.com/rpc/ (public endpoints need no signup;
  a free account raises rate limits).
- Config: `config/scout.example.toml` → `[chains.bsc]` /
  `[chains.base]` → `rpc_url_env`. Already wired to:
  - `SCOUT_BSC_RPC_URL`
  - `SCOUT_BASE_RPC_URL`
  - `SCOUT_ROBINHOOD_RPC_URL`
- What it's for: raw `eth_getLogs`/`eth_getTransactionReceipt` —
  transport, not indexed history. Etherscan V2 above is what actually
  gives you per-address history; this is the decode-time RPC fallback.

### 3. One Solana provider — Helius (free tier)

- Register: https://www.helius.dev (free tier, dashboard gives an API
  key immediately).
- Config: `config/scout.example.toml` → `[chains.solana]` →
  `rpc_url_env = "SCOUT_SOLANA_RPC_URL"`.
- `[providers.solana_history]` → `api_key_env = "SCOUT_HELIUS_API_KEY"`
  is already the wired name in `config/scout.example.toml` — set that
  exact env var.
- What it's for: `getSignaturesForAddress` + retention/closed-ATA
  coverage (the actual decision axis per the capability matrix), never
  Helius's own parsed-tx decoders — this workspace decodes bonding-curve
  buys itself (`scout-dex-solana`).

## Exact env var -> config path map

| Env var | Set by you | Config file field | What breaks without it |
|---|---|---|---|
| `SCOUT_EVM_HISTORY_API_KEY` | after choosing a Tier-1 EVM history vendor | `[providers.evm_history].api_key_env` | `buyer-intersect`/`wallet-rank`/`wallet-stats` exit 4 (`ConfigurationRequired`) — this is the port `UnconfiguredProvider` is wired to today in all three binaries |
| `SCOUT_HELIUS_API_KEY` | Helius dashboard | `[providers.solana_history].api_key_env` | Solana history port stays unconfigured, same exit 4 |
| `SCOUT_SOLANA_RPC_URL` | Helius (or any Solana RPC) | `[chains.solana].rpc_url_env` | genesis-identity preflight (ADR-002) cannot run |
| `SCOUT_BSC_RPC_URL` | dRPC/Ankr/Chainstack/GetBlock | `[chains.bsc].rpc_url_env` | same, for BSC |
| `SCOUT_BASE_RPC_URL` | dRPC/Ankr/Chainstack/GetBlock | `[chains.base].rpc_url_env` | same, for Base |
| `SCOUT_ROBINHOOD_RPC_URL` | vendor supporting chain 4663 | `[chains.robinhood].rpc_url_env` | same, for Robinhood chain |

`SCOUT_EVM_HISTORY_API_KEY` is the literal string `UnconfiguredProvider`
constructs with today (`bins/*/src/main.rs`,
`crates/scout-providers/src/unconfigured.rs`'s own tests) — verified
against the actual source, not assumed from the config file alone.

## What happens if you set nothing

Every one of the three CLIs (`buyer-intersect`, `wallet-rank`,
`wallet-stats`) exits 4 (`InfrastructureUnavailable`, CLI.md §8) with a
message naming which env var to set. This is intentional per ADR-006 —
never a silent empty result.

## Tier 2 (never the ledger's source of truth)

Dune, Birdeye, DEX Screener, GeckoTerminal, Chainbase — useful for
discovery/cross-check, never wired as a `HistoryProvider`. If you want
to feed one of these into analysis anyway, that requires:
1. `scout-sdk`'s `external-data` feature enabled at build time (off by
   default, ADR-008).
2. An explicit `ExternalDataOptIn::acknowledge(...)` token at your
   composition root, with a stated reason.
3. `Ledger::acquire_unverified(...)` instead of `Ledger::acquire(...)`
   — the resulting lot's basis is forced `Unknown`, so any disposal
   touching it reports `realized_trade_pnl: None`, never a number
   computed from unverified data.

There is currently no `NormalizedActivitySource` implementation for any
of the vendors above — `scout-api::NormalizedActivitySource` is the
trait you'd implement to add one.
