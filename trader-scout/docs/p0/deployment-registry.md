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
| `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` | confirmed pump.fun bonding-curve program — see "2026-10-01 confirmation" below | **CONFIRMED** against official IDL, on-chain executable state, and balance-delta cross-validation |

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
identifiable by position in `accounts`/`accountKeys`. Two separate worked examples from this
census both confirm the same method but are NOT the same transaction — do not conflate their
numbers:

- An early exploratory probe (not committed as a test fixture) found `accountIndex=4` in
  `postTokenBalances` referring to the buyer's **associated token account**, with the actual
  buyer being that ATA's `owner` field, and a balance delta of `206,321` base units. That
  transaction's static `accountKeys[0]` (fee payer/signer) happened to equal the same owner —
  coincidental to that specific transaction, not a general rule.
- The implementation landed in `2c80cda` is tested against a different, committed real
  transaction: `docs/p0/measurements/fixtures/pump_mint1_full.json` data[2] (signature
  `5XpoGEhyuhQPcSMc8qJ6vw83LrGpLkKuEeXZcn48Q7tJsho1c92cgxqhMVNsuGiU51UT3yFGMT5SVKa9YoXjZfiA`),
  where owner `EvtwrQSszv1qqr8U4GKjfcvjN43Yyf1isnzXJzva3GRv`'s tracked balance for the mint
  moved from `41,636,451` to `181,714,920,688` (delta `181,673,284,237`) — see
  `crates/scout-providers/src/helius.rs`'s
  `token_balance_changes_identify_the_buyer_by_owner_not_position` test for the exact assertion.
  (An earlier draft of that test mistakenly asserted the `206,321` figure from the first probe
  above against this second transaction's data; `cargo test` caught the mismatch before it was
  committed.)

**The only reliable method demonstrated so far, confirmed independently in both examples: match
`postTokenBalances[].owner` for the entry whose token balance increased for the target mint.** No
positional shortcut has been verified in either case.

None of the above is sufficient to add a row to the Schema table above — no contract
address is confirmed from an official source, no activation slot is pinned, no
ABI/IDL commit hash exists, and no golden fixture exercises a real decoder against a
confirmed deployment. This section records what a live sample showed so the next
research pass does not repeat the same probes from zero, not a claim of decoder
readiness.

## 2026-10-01 confirmation — pump.fun bonding-curve program fully identified

Following the mint-centric query measurement (2026-10-01), probed
`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` directly as the `address` parameter to
`getTransactionsForAddress` (not through a mint, not through Codex) — `sortOrder: "desc"`,
`limit: 5`, via a direct `curl` call, not through production code (the provider's
`fetch_transactions` hardcodes `sortOrder: "asc"`, which would have returned the oldest
transactions since program deployment, not current activity).

**On-chain confirmation:**
- `getAccountInfo` on the program address: `executable: true`, owner
  `BPFLoaderUpgradeab1e11111111111111111111111` — a real deployed, currently-upgradeable
  program, not a placeholder or closed account.
- All 5 probed transactions invoke this program id, either top-level or via CPI.
- Of those 5, 3 carry an instruction with discriminator `66063d1201daebea`
  (`sha256("global:buy")[..8]`) or `33e685a4017f83ad` (`sha256("global:sell")[..8]`)
  directly on this program's own instruction (not nested under a different program the
  way the PumpSwap AMM collision was) — each is exactly 24 bytes (8-byte discriminator +
  two little-endian `u64` values), matching the historically-assumed bonding-curve shape.
- Cross-validated against real economics, not just discriminator shape: in the
  transaction with `v1=2979651581366, v2=699200000`, `v1` is **byte-for-byte equal** to
  the token-balance delta observed between two accounts in that same transaction's
  `postTokenBalances` (−2979651581366 on the bonding-curve vault, +2979651581366 on the
  buyer's ATA) — this is a real token transfer, not merely a number that happens to
  parse.
- `accounts[2]` in that instruction equals the mint address found independently via
  `preTokenBalances`/`postTokenBalances` in all 3 checked cases.
- The real buyer (by SOL balance delta, `-588595186` lamports — the dominant
  non-fee-sized debit) sits at instruction account position `[6]`, not `[0]` or `[2]`.

**Official-source confirmation (closes P0.2 condition #3, the ABI/IDL requirement):**
On-chain IDL account lookup was attempted first (Anchor's `create_with_seed(upgrade_authority,
"anchor:idl", program_id)` convention, after resolving the upgrade authority via the
program's `ProgramData` account) — the derived address does not exist on-chain, so this
program does not publish its IDL that way (common for production Anchor programs). Located
the official IDL instead at `github.com/pump-fun/pump-public-docs` (`idl/pump.json`,
commit `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`, dated 2026-09-12), the project's own
published-docs repository (name: "pump", address field matches
`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` exactly). This is treated as confirmed, not
merely "found on GitHub," because its contents independently reproduce every empirically-derived
fact above without having been consulted to derive them:

| Fact | Derived empirically (live tx data) | Official IDL (`idl/pump.json`) |
|---|---|---|
| `buy` discriminator | `66063d1201daebea` | `[102,6,61,18,1,218,235,234]` = `66063d1201daebea` |
| `sell` discriminator | `33e685a4017f83ad` | `[51,230,133,164,1,127,131,173]` = `33e685a4017f83ad` |
| `buy` args | two `u64`s | `amount: u64`, `max_sol_cost: u64` |
| `sell` args | two `u64`s | `amount: u64`, `min_sol_output: u64` |
| mint position | `accounts[2]` | `accounts[2] = "mint"` |
| buyer position | `accounts[6]` (by SOL delta) | `accounts[6] = "user"` |

Full `buy` account order per the official IDL: `[0] global, [1] fee_recipient, [2] mint,
[3] bonding_curve, [4] associated_bonding_curve, [5] associated_user, [6] user,
[7] system_program, [8] token_program, [9] creator_vault, [10] event_authority,
[11] program, [12] global_volume_accumulator, [13] user_volume_accumulator,
[14] fee_config, [15] fee_program` (16 accounts; `sell` omits `token_program` at a
different position and `global_volume_accumulator`/`user_volume_accumulator`, totaling
14). This is the real account layout — **not** the 4-account
`[buyer, bonding_curve, mint, buyer_token_account]` shape
`scout-dex-solana::bonding_curve_buy` currently decodes against a synthetic fixture.

**This satisfies all four P0.2 conditions for a schema row:**
1. Address confirmed via on-chain `executable` state + official IDL's own `address` field
   (not copied from a third-party list without corroboration).
2. Activation slot: not yet pinned — the 5-transaction sample's slots
   (`452380124` for all 5, single block) are far later than deployment; actual deployment
   slot needs `getSignaturesForAddress` with `before`/pagination toward genesis, not done
   here (out of scope for this confirmation pass).
3. IDL commit hash: `e0687ae9b7e064a0f54efc7297c65eecfbba3a8f`
   (`pump-fun/pump-public-docs`, `idl/pump.json`), 2026-09-12.
4. Golden fixture: not yet created — `docs/p0/measurements/fixtures/` has no committed
   fixture built from this probe yet; the transactions analyzed here live only in this
   session's scratch files (`/tmp/bonding_curve_probe.json`), not committed to the repo.
   A schema row requires condition 4 too, so this program is confirmed-pending-fixture,
   not yet eligible for the table until a fixture is committed and a decoder exercises it
   (tracked as the next step in `docs/TICKETS.md` P0.12).


## 2026-10-02 review — full trade-instruction surface of `6EF8rr…` (supersedes the "buy/sell only" view above)

The 2026-10-01 section compared only `buy`/`sell`. Re-reading the **same pinned IDL**
(`e0687ae9`, now committed byte-for-byte as
`docs/p0/measurements/fixtures/pump_idl_e0687ae9.json`, sha256
`ffe966c42f1af41652ee753fe2f1e3f7cd4077d7e6f49faf3138959c8b56064b`; its trade instructions are
identical to `pump-public-docs` `main` as fetched 2026-10-02) shows **47 instructions, six of them
trades**. A decoder that knew only `buy`/`sell` silently returned `NotMine` for the other four,
so their buyers vanished while coverage looked complete (invariant #18). Findings:

| Variant | Discriminator | IDL accounts | mint / user positions | Live evidence in `pump_bonding_curve_buy_probe.json` | Status |
|---|---|---|---|---|---|
| `buy` | `66063d1201daebea` | 16 | `[2]` / `[6]` | tx2 (top-level), tx3 (inner/CPI): owner delta of `user` > 0, tx2 delta == `amount` (2979651581366) | `FixtureVerified` |
| `sell` | `33e685a4017f83ad` | 14 | `[2]` / `[6]` | tx1: `user` loses exactly `amount` (175202561501) | `FixtureVerified` |
| `sell_v2` | `5df6823ce7e940b2` | 26 | `[1]` base_mint / `[13]` | tx0 (inner): `user` owner delta == −`amount` (14072072687607), quote mint wSOL | `FixtureVerified` |
| `buy_exact_sol_in` | `38fc74089edfcd5f` | 16 | `[2]` / `[6]` | none | `IdlOnly` |
| `buy_v2` | `b817ee6167c5d33d` | 27 | `[1]` base_mint / `[13]` | none | `IdlOnly` |
| `buy_exact_quote_in_v2` | `c2ab1c46684d5b2f` | 27 | `[1]` base_mint / `[13]` | tx4 only — a **failed** tx (`InstructionError [4, Custom 6042]`): layout decodes, economics unverifiable | `IdlOnly` |

Additional facts recorded so they are not re-derived:

- **Live account counts exceed the IDL by 2** (`buy` 18 vs 16, `sell` 16 vs 14). Positions 0..IDL-count
  match the IDL; the extra entries are trailing Anchor remaining accounts. The decoder therefore
  requires *at least* the IDL count; data length stays exact. The 2026-10-01 statement that the IDL
  "reproduces every fact" was true for discriminators/args/positions but not for the account count.
- The other 41 IDL instructions plus the Anchor event-CPI tag `e445a52e51cb9a1d` form an explicit
  known-non-trade table (test-asserted equal to the IDL). Any other discriminator under this program
  is a coverage gap (exit 3), never `NotMine`.
- Verification policy for `IdlOnly` variants: see `docs/adr/ADR-009-decoder-variant-verification.md`.
  Promotion to `FixtureVerified` requires a committed **successful** real transaction whose
  decoded `user`/`mint` match that transaction's owner-keyed token deltas.

**P0.2 conditions for a schema row, updated:** #1 address — satisfied; #3 pinned IDL — satisfied
(and now committed); #4 golden fixture + decoder exercising it — satisfied for `buy`, `sell`,
`sell_v2` (`pump_bonding_curve_buy_probe.json`, committed in `cece920`; the "scratch files only"
note above is stale). **#2 activation slot — still open**, so no schema-table row yet.
