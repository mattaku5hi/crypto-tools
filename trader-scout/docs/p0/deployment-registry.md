# P0.2 — Deployment registry

Status: living document. **The entry table below (Schema section) remains empty by
design** — no deployment has satisfied all four conditions. A dated census-findings
section further down records live observations that do not yet satisfy those
conditions either; observing a program id in a transaction is not the same as
confirming a deployment.

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

## Solana census findings (2026-09-30) — program ids observed, semantics unverified

**Sample size: 10 transactions** (5 per mint, 2 pump.fun-minted tokens), captured live via
`HeliusProvider`/`getTransactionsForAddress` and saved to
`docs/p0/measurements/fixtures/pump_mint{1,2}_full.json`. This is a small, arbitrary sample —
findings below describe what was observed in it, not the full population of pump.fun trading
activity. "Not observed" in a 10-transaction sample is not evidence of absence.

**Program ids observed** (selection; full list in the fixtures — none confirmed as a
decodable deployment per this file's four conditions — presence in a live
transaction is not `live_verified`):

| Program id | Frequency in sample | Identity |
|---|---|---|
| `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` | most-common non-system program across both mints | Believed to be PumpSwap AMM (unconfirmed against an official source) |
| `DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH` | present as a top-level program in both mints' samples | **Identity unknown** — likely a router/aggregator (112-byte instruction data, 54-account instruction observed once), program name not confirmed |
| `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` (bonding-curve candidate) | **not observed in this 10-tx sample** | Both sampled mints had already migrated to AMM trading; a pre-migration sample would be needed to observe bonding-curve activity at all |

**Discriminator findings — none establish an actual instruction entry point:**

- `66063d1201daebea` — this is mathematically `sha256("global:buy")[..8]` (verified locally).
  It was found on an **inner instruction** under `pAMMBay6...` carrying only 24 bytes of data
  (8-byte discriminator + two little-endian `u64` values: `206321` and `10`, where `206321`
  exactly matches the buyer's token-balance delta in that same transaction's `postTokenBalances`).
  This shape — short, matching a balance delta, nested under an Anchor program as an inner call —
  is consistent with an **Anchor `#[event_cpi]` self-invoked event log**, not a real `buy` entry
  point. **Collision with an Anchor event-CPI log; NOT established as a buy entry point.**
- `e445a52e51cb9a1d` — observed twice as a discriminator on different instructions under
  `pAMMBay6...`. This is the fixed Anchor `#[event_cpi]` wrapper tag, the same 8 bytes on *any*
  Anchor program using that feature — it identifies the event-log dispatch mechanism itself, not
  a specific instruction or event name.
- `414b3f4ceb5b5b88` — the discriminator on the actual top-level 112-byte instruction under
  `DF1ow4t...` in one sampled transaction. Checked against `sha256("global:buy")`,
  `sha256("global:swap_base_in")`, `sha256("global:swap_base_out")`, `sha256("global:route")`, and
  `sha256("shared_accounts_route")` — **no match against any tested name**. Recorded as
  observed/unidentified; `DF1ow4t...`'s program identity itself is unconfirmed, so no further
  discriminator guessing was attempted against it.

**Buyer identification method (correcting an earlier same-session claim that a buyer's account
position was index 4 in a static account list — that was wrong):** the buyer is not reliably
identifiable by position in `accounts`/`accountKeys`. In the one worked example checked closely,
`accountIndex=4` in `postTokenBalances` referred to the buyer's **associated token account**
(a distinct address from the buyer's wallet), and the actual buyer is that ATA's `owner` field.
Separately, the transaction's static `accountKeys[0]` (fee payer/signer) happened to equal that
same owner in this specific transaction — coincidental to this transaction, not a general rule.
**The only reliable method demonstrated so far: match `postTokenBalances[].owner` for the entry
whose token balance increased for the target mint.** No positional shortcut has been verified.

None of the above is sufficient to add a row to the Schema table above — no contract
address is confirmed from an official source, no activation slot is pinned, no
ABI/IDL commit hash exists, and no golden fixture exercises a real decoder against a
confirmed deployment. This section records what a live sample showed so the next
research pass does not repeat the same probes from zero, not a claim of decoder
readiness.
