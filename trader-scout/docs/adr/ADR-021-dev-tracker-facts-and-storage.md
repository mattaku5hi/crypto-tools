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
