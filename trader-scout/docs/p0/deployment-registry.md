# P0.2 — Deployment registry

Status: living document; **empty by design as of 2026-09-22**.

Per ROADMAP.md P0.2 and AGENTS.md invariant #16 ("Нельзя объявлять поддержку DEX по одному совпадению
event topic. Проверяются deployment, диапазон блоков, ABI/IDL, protocol semantics и golden
fixtures."), a deployment is only listed here once:

1. Its program/factory/manager address is confirmed from an official source (docs, verified
   contract, or an on-chain-confirmed deployment transaction), not copied from a third-party list.
2. Its activation block/slot (and upgrade/deactivation boundary, if any) is recorded.
3. A source/commit hash for the ABI/IDL used to decode it is pinned.
4. At least one golden fixture exists that exercises the decoder against this specific deployment.

**No entries currently satisfy all four conditions.** This workspace has not yet done the research
pass to confirm even the first vertical-slice DEX deployment (ARCHITECTURE.md §1: "Первым рабочим
slice делаем один подтвержденный EVM DEX на Base"). Populating this file with plausible-looking
addresses without that verification is explicitly the failure mode AGENTS.md forbids — better an
honest empty table than a fabricated one.

## Schema (for when entries are added)

| Field | Meaning |
|---|---|
| `network` | ChainKey network identifier |
| `protocol_family` | e.g. "uniswap-v2-style", "raydium-amm" |
| `version` | Specific protocol version this entry decodes |
| `contract_address` | Program/factory/manager address, confirmed source |
| `activation_block_or_slot` | First block/slot this deployment is active |
| `deactivation_or_upgrade_boundary` | If applicable; null if still active as of last check |
| `source_commit_hash` | Pinned commit of the ABI/IDL source used |
| `fixture_ids` | List of `tests/fixtures/` entries exercising this deployment |
| `coverage_status` | `documented` \| `fixture_verified` \| `live_verified` |
| `verified_at` | Date this row was last confirmed against its source |

## Candidate research targets (not yet confirmed — do not treat as supported)

- Base: a Uniswap-v3-style deployment (candidate per ARCHITECTURE.md §5's "Uniswap-style v2/v3"),
  or an Aerodrome/Slipstream deployment specific to Base. Neither address is recorded here until
  confirmed against official docs/deployment records.
- Solana: a Pump.fun bonding-curve program (S13 candidate) plus its PumpSwap AMM migration path, or
  a Raydium AMM/CPMM/CLMM program. Addresses intentionally omitted pending confirmation.
- BSC: blocked at the transport layer (public `eth_getLogs` disabled per S03) before deployment
  research is even actionable for live verification; documented-only entries could still be recorded
  once a specific Pancake-fork deployment is confirmed from official sources.
- Robinhood Chain: per ARCHITECTURE.md §5, "для Robinhood проверяется реально развернутый набор
  протоколов; наличие EVM профиля не заменяет этот этап" — expect this to stay empty through P0/P1
  unless a specific deployment is independently confirmed.
