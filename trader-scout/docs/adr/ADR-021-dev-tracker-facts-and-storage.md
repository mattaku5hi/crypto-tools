# ADR-021: Dev tracker — facts, storage and derivation

Status: Accepted (2026-10-07)
Date: 2026-10-07
Builds on: `docs/DEV-TRACKER.md` (owner decisions 2026-10-06), B0 measurements
(`docs/p0/measurements/2026-10-06-dev-tracker-volumes.md` and its addenda), ADR-020 (EVM events),
ADR-012/015 (pump.fun programs).

## Decision

1. **Facts, not verdicts.** PostgreSQL stores immutable chain facts and refreshable observations;
   the three categories (`top-runners`, `top-migr`, `win-streak`) are derived on every cycle from
   those facts and the current TOML config. Changing a threshold never requires a rescan.
2. **Fact tables** (crate `scout-devdb`, schema migrations embedded and applied at start):
   - `launches(chain, token, launchpad, creator, created_block, created_at, tx_hash, source)` —
     one row per token; `creator` is the launchpad's own creator field (Flap `creator`, Pons
     `deployer`, four.meme `creator`, Zora `payoutRecipient`, Clanker `tokenAdmin`, pump.fun
     creator account).
   - `migrations(chain, token, launchpad, migrated_block, migrated_at, tx_hash, pool, source)` —
     the launchpad's own graduation event (Flap `LaunchedToDEX`, Pons `PoolGraduated`, pump.fun
     `Migrate`, four.meme graduation once identified). Launchpads without a curve (Zora, Clanker)
     have no rows here.
   - `ath(chain, token, ath_fdv_usd, ath_at, source, observed_at)` — refreshable observation
     (Codex `extrema.athFdv`, ADR B0.3); the latest observation per token wins.
   - `cursors(source, position, updated_at)` — per ingestion source (chain + launchpad + event),
     the last fully ingested block / slot / signature; ingestion is idempotent (upserts keyed by
     the natural key), so a replay after a crash is harmless.
   - `deliveries(category, format, content_hash, delivered_at)` — the last delivered export per
     category and terminal format (B7: send only on change).
3. **Derivation (B4)** is pure Rust over loaded facts (`DevFacts` per (chain, creator)):
   resolved launches = migrated + not migrated and older than `pending_window`; migration rate =
   migrated / resolved with both raw counts kept; current streak = consecutive migrated launches
   from the newest resolved launch backwards (pending launches skipped); runners = launches with
   `ath_fdv_usd` ≥ threshold. Launchpads without migration skip the migration-rate rules
   (owner: Zora/Clanker → `top-runners` only).
4. **Sources per chain** (B0): EVM launch/migration events by `eth_getLogs` polling on the keyed
   provider (Alchemy PAYG, fallback dRPC — ADR-020 amendments, A7), Solana pump.fun creates and
   migrations from on-chain data (standard RPC methods so Ankr / the public RPC can serve as
   fallback, A8), ATH from Codex in batches of 200. Codex counts are hints only (≈ 70 % migration
   coverage, no Flap/Pons).
5. **Tests** run against a real PostgreSQL when `SCOUT_TEST_DATABASE_URL` is set (local container);
   without it the database tests skip (CI stays offline). The derivation is tested without a
   database.

## Consequences

- Category membership is reproducible from the database at any time; history of membership is
  not stored as a fact (deliveries keep the content hash of what was sent).
- A launchpad whose creator field cannot be trusted (e.g. a factory that is the `msg.sender` for
  everyone) must be mapped to the real creator before its launches count; each launchpad's
  creator rule is documented next to its decoder.

## Amendment 1 (2026-10-07) — the dev behind a launch

Live (one day of EVM launches, 33k rows): most of the busiest "creators" on BSC and Robinhood are
contracts. Sampling 8 launch signers per contract shows two kinds: shared intermediaries (a different
`tx.from` per launch: Flap VaultPortal `0x90497450…`, launcher services `0xf2e2f0ae…` (BSC),
`0x0e1651ae…` (Robinhood, filling Pons' `originalDeployer`)) and single-operator bots (one signer for
every launch, e.g. `0x563b2841…` signed by `0x7485fbfd…` on both chains). On Base almost every Zora
creator is a smart wallet whose transaction signer is a shared ERC-4337 bundler.

Rule (`scout_devdb::SIGNER_RESOLVED_LAUNCHPADS` = flap, fourmeme, pons — launchpads whose creator field
is or can be the caller): a contract creator with several signers is a shared intermediary and each
launch's signer is the dev (`contract:<addr>` until resolved, never merged with real users). A
single-owner contract stays its own dev (its signer may be a relayer/bundler shared by many users, so it
is never merged into it). EOAs and every other launchpad (Zora `payoutRecipient`, Clanker `tokenAdmin`)
keep the creator field. Residual risk: a Pons/Flap dev on a smart wallet is resolved to its bundler.
Cost: one `eth_getCode` per new creator on those launchpads, ≤ 8 lookups per new contract creator, one
lookup per launch through a shared intermediary (live first pass: ≈ 13k requests for one day, all chains).

## Amendment 2 (2026-10-07) — pump.fun facts

- **Launch** = the self-CPI `CreateEvent` (IDL `pump_idl_e0687ae9.json`) in a successful transaction touching the
  `mint_authority` PDA `TSLvdd1p…` (`create` / `create_v2` only); the dev is `CreateEvent.creator` (normally the
  signer `user`; no signer resolution on Solana). `created_block` = slot, `tx_hash` = signature.
- **Migration** = `CompletePumpAmmMigrationEvent` (mint, pool) in a successful transaction signed by the migrator
  `39azUYFW…`. Its other transactions are not migrations: "Bonding curve already migrated" no-ops (no event) and
  buys. Live page (60 txs): every `migrate`/`migrate_v2` with `CreatePool` carried the event (44/44).
- **Cursor** = the next window's inclusive start in unix seconds (Helius `filters.blockTime.gte`); the window ends
  60 s before now. A pass cut by the page budget restarts at the newest block time it saw (re-read, idempotent).
- **Cost** (Helius full mode, 10 credits / 100 txs): ≈ 1.8k txs/hour → ≈ 4.5k credits/day steady state.
- **Backfill** is not done by default: one year of creates is ≈ 19M txs (≈ 1.9M credits), more than the free
  plan's month. Forward ingestion fills the window as time passes; a deeper first pass (90 days of migrations
  ≈ 20k credits, creates of qualifying creators only) waits for the owner's decision on credits.

## Amendment 3 (2026-10-07) — which launches get a signer lookup (owner decision: option B)

Measured on one day: BSC 3,243 launches through shared intermediaries (8 migrated), Robinhood 270 (2 migrated);
BSC 1,693 creators (807 with ≥ 2 launches), Robinhood 2,932 (224). Resolving every signer and classifying every
creator for a year would cost ≈ 6M Alchemy calls (≈ $30–60); dRPC free cannot serve it (it answers `null` for
launch transactions older than a few hours).

- A signer is looked up for every migrated launch through an intermediary (any age) and every launch younger than
  7 days (`identity::RECENT_SIGNER_WINDOW`), so new launches are always attributed; on Robinhood
  (`resolves_every_signer`) for all of them.
- A dev with a migrated launch through an intermediary is a candidate; its transactions to every shared
  intermediary of the chain are listed once (`alchemy_getAssetTransfers`, top-level calls, zero values included;
  6/6 sampled launches found) and matched to launches by hash (`launches_by_tx` index, table `dev_histories`).
  This runs in every cycle, so a dev who becomes a candidate later (an old launch migrating) gets its history then.
- A launch through an intermediary whose signer is still unknown belongs to no dev (excluded from the derivation,
  never one pseudo-dev per contract). Such a dev has no migration in the window, so it cannot qualify (top-migr and
  win-streak need migrations; a curve token reaches a runner's ATH only after graduating).
- Only creators with ≥ 2 launches are classified (a single launch is its creator's own); 4 owner samples, not 8.
- Expected one-year identity cost ≈ 0.5M calls ≈ $5; steady state cheaper than before.
