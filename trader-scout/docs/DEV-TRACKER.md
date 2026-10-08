# Dev tracker — 24/7 service specification (owner decisions 2026-10-06)

Scope of the always-on service: **discover and keep current three lists of token creators ("devs")**.
Smart-money discovery stays in the batch CLIs (`buyer-intersect` → `wallet-rank` / `wallet-stats`); the
service does not monitor traders. Rationale (owner): dev lists change slowly, so after the first
backfill the service consumes little provider quota.

## 1. Definitions

- **Dev** = the creator address of a launch (the account the launchpad records as creator). Several
  wallets of one person are not linked.
- **Launch** = a token created by the dev on a supported launchpad.
- **Migration** = the launchpad's own graduation event: pump.fun bonding curve complete → PumpSwap
  (Solana); four.meme graduation → PancakeSwap (BSC); Pons V2 / Bags curve completion (Robinhood);
  other launchpads with a curve as they are verified. Launchpads without a curve (Zora, Clanker on
  Base: launched straight into a Uniswap v4 pool) have **no migration rate**; their devs can only
  qualify for `top-runners` (other Base launchpads are to be checked for a curve).
- **ATH** = all-time-high market cap of a token in USD (source decided in B0).
- **Pending launch** = not migrated and younger than `pending_window` (default 3 days): it neither
  counts as a failure nor breaks a streak until the window expires.
- **Last activity** = time of the dev's latest launch.

## 2. Categories (thresholds live in the config file, these are the defaults)

| Category | Rule | Size |
|---|---|---|
| `top-runners` | ≥ 1 launch with ATH ≥ $1M **and** ≥ 2 further launches with ATH ≥ $500k; migration rate ≥ 3 % (not applied where the launchpad has no migration); last activity ≤ 365 days | unlimited |
| `top-migr` | migration rate ≥ 80 % over ≥ `min_launches` = 3 resolved launches; last activity ≤ 365 days | unlimited |
| `win-streak` | not in `top-runners` nor `top-migr`; current streak of ≥ 3 consecutive migrated launches (most recent resolved launches, pending ones skipped); last activity ≤ 365 days | unlimited |

`top-runners` and `top-migr` may overlap. Migration rate = migrated / resolved launches (pending
excluded); both raw counts are stored, never only the ratio.

## 3. Architecture principles

- **Facts, not verdicts.** PostgreSQL stores launches, migrations, ATH observations, timestamps and
  provenance per chain. Categories are derived from facts with the current config, so changing a
  threshold recomputes lists without rescanning.
- **Incremental.** One expensive first backfill (migrations of the last year → creators → their launch
  history → ATH of their migrated / large tokens); afterwards cursors per source and chain, only new
  events are read.
- **Cadence (defaults, configurable):** new launches and migrations every 15–60 min (must not miss
  events); category recomputation every 1–2 h; ATH of young tokens hourly, of old tokens daily.
- **Config:** a TOML file (category thresholds, `pending_window`, `min_launches`, launchpads per chain,
  cadences, delivery), re-read every cycle — edits apply without a restart.
- **Storage:** PostgreSQL (schema migrations with `sqlx`). The batch CLIs stay stateless.
- **Packaging:** multi-stage container image; `docker-compose` with PostgreSQL; Kubernetes manifests
  (Deployment, Secret for keys); Helm optional. Secrets only from env / Secret, never in images or logs.

## 4. Delivery

- **Files:** one per category × chain × terminal format (`top-migr_solana_gmgn.json`, …), best-ranked devs
  first, at most `delivery.max_wallets_per_file` (default 500; GMGN tracks at most 2,000). Ranking:
  top-runners by big runners, runners, migration rate; top-migr by rate, migrations, streak; win-streak by
  streak, migrations, rate (then the newest last launch). Label: `TR m5/40 s1 r3` (category, migrated /
  resolved, current streak, runners); emoji 🚀 top-runners, 🎓 top-migr, 🔥 win-streak. Creators still behind
  an unresolved shared intermediary (`contract:…`) are not wallets and are never exported.
- **Formats (pinned 2026-10-07 from the owner's own exports):**
  - `gmgn` — JSON array of `{"address", "name", "emoji"}` (GMGN docs, "Wallets Import Export",
    https://docs.gmgn.ai/index/wallets-import-export).
  - `axiom` — JSON array shaped like an Axiom wallet-tracker export: `trackedWalletAddress`, `name`, `emoji`,
    `createdAt` (ISO), `alertsOnToast` false, `alertsOnBubble` / `alertsOnFeed` / `alertsOnTransfer` true,
    `toastOnTransfer` false, `groupNames` `["Devs", "<category>"]`, `sound` "", `transferAudio` "",
    `highlightColor` null (the third-party guides' `alertsOn` field does not exist).
  - `basedbot` — text, one wallet per line, tab-separated `address⇥emoji⇥name⇥group` (group `Devs`), as in a
    BasedBot wallet-tracker export; chains may be mixed in one list.
- **Only on change:** a list is sent when its SET of wallets differs from the last delivered one (sha256 of
  the sorted addresses per category, chain and format in `deliveries`, with the wallets themselves for the
  next diff); label changes alone do not trigger a send. A list never delivered is not sent while empty. A
  failed send is retried next cycle.
- **Message:** one album per changed list with every changed format, files named
  `<category>_<chain>_<UTC yyyy-mm-ddThh-mmZ>_<format>.<ext>`; the caption is the change report — category,
  chain, time, wallet count with `(+added / −dropped)`, then `+ wallet m5/6 83% · streak 3 · runners 1 (0 big)`
  for each addition and `− wallet <reason>` for each drop (the first rule it now fails: `rate 60% < 80%`,
  `streak broken (1 < 3)`, `moved to top-migr`, `inactive 400 d`, `ranked below the top 500`, `no launch in the
  last 365 days`). Overflow beyond the caption limit follows as messages. `dev-tracker export --test` sends the
  largest list once, marked as a test, without recording it.
- **Channel:** Telegram `sendDocument` to `SCOUT_TELEGRAM_CHAT_ID` with `SCOUT_TELEGRAM_BOT_TOKEN`
  (`delivery.telegram = true`); the current files are also written to `delivery.out_dir`.

## 5. Operations

- `dev-tracker run --config <file>` — the daemon: ingestion (launches, migrations, dev identities, ATH) every
  `schedule.ingest_every_minutes`, derivation + delivery every `schedule.derive_every_minutes`; the config is
  re-read every cycle (a broken edit is reported and the last good config kept); one chain's failure does not
  stop the others; SIGTERM / Ctrl-C stop it between steps.
- One-shot commands: `migrate`, `ingest --chains …`, `derive`, `export [--send] [--test]`, `backfill`.
- Solana resilience: a failed Helius pass falls back to standard RPC (`SCOUT_SOLANA_FALLBACK_RPC_URL`, else the
  public RPC), at most 6 h of window per pass; every source keeps its cursor, so any gap is read later.
- Packaging: `deploy/Dockerfile` (multi-stage, non-root), `deploy/docker-compose.yml` (PostgreSQL 16 + the
  daemon, keys from `.env`, config and exports mounted), `deploy/k8s/dev-tracker.yaml` (ConfigMap + one-replica
  `Recreate` Deployment, keys from the Secret of `secret.example.yaml`; `postgres.example.yaml` for a
  single-node database). Exactly one daemon per database: two would double provider requests and sends.

- **Moving the database:** `deploy/db-dump.sh <file>` (pg_dump custom format, compressed) and
  `deploy/db-restore.sh <file>` (into an empty database; stop the daemon first). Facts and cursors travel
  together, so a restored service resumes where the snapshot ended and reads the gap on its first passes
  (EVM: one `eth_getLogs` range per source; Solana: up to `solana_max_pages` × 1000 txs per source and pass).
  Round trip verified 2026-10-07 (56k launches → 5.8 MB dump, identical row counts and cursors).

### Server requirements (measured 2026-10-07)

| Resource | Measured | Recommendation |
|---|---|---|
| CPU | passes are I/O-bound | 2 vCPU |
| RAM | daemon ≤ 100 MB (Solana catch-up of 20k txs: 99 MB); derivation streams one dev at a time (19 MB over 56k launches) | 4 GB (PostgreSQL 1–2 GB) |
| Disk | ≈ 660 B per launch with indexes; one year ≈ 32M launches (Solana 19.4M, BSC 9.2M, Robinhood 2.1M, Base 1.1M) ≈ 20 GB, +≈ 1.7 GB/month; dump ≈ 100 B/launch (≈ 3–3.5 GB/year) | SSD 80–100 GB |
| Traffic | gzip on (Helius full tx 16.9 KB → 3.3 KB); Solana ≈ 55k txs/day ≈ 180 MB/day; EVM < 1 GB/month; a one-year Solana backfill ≈ 64 GB | any 100 Mbit/s link |

## 6. Order of work

B0 measurements (event volumes per chain and launchpad, ATH source: own swaps vs an external API such
as Codex) → B1 schema → B2 launch/migration ingestion (Solana pump.fun and Robinhood Pons first, then
BSC four.meme, then Base: Zora/Clanker for `top-runners`, other launchpads checked) → B3 ATH → B4
category engine from config → B5 daemon with incremental cycles → B6 container / compose / k8s → B7
JSON formats + Telegram delivery on change.

## 7. Status and plan (2026-10-07)

**Built (B0–B7):** facts in PostgreSQL from 12 EVM sources (BSC Flap + four.meme, Robinhood Pons + Flap, Base
Zora + Clanker) and Solana pump.fun; dev identity behind shared intermediaries; ATH from Codex; categories from the
TOML; daemon; exports (GMGN / Axiom / BasedBot) with Telegram albums and change reports; container, compose, k8s;
portable database snapshots. Running data: only since 2026-10-06/07, so the lists are nearly empty (9 devs).

**Why a backfill:** every category looks back up to 365 days (migration rate, streak, runners). Without history
the lists fill only as time passes; with it they are complete from the first day of the service.

**How (owner decisions 2026-10-07):**
- EVM launches / migrations: `eth_getLogs` over one year on Alchemy PAYG (a few thousand calls, cents).
- EVM dev identity (≈ 3M lookups: BSC ≈ 2M, Robinhood ≈ 1M; Base needs none): the free dRPC plan, Alchemy only
  for answers dRPC leaves empty. Alchemy PAYG bills from the first CU (no free allowance), so this saves ≈ $15–33.
- Solana: Helius Developer plan for the backfill month (owner bought it 2026-10-07; ≈ 2M of its 10M credits,
  ≈ 64 GB of gzip traffic), then back to the free plan for 24/7 (≈ 13 % of it).
- ATH: Codex free tier (≈ 4.6k of the monthly 10k requests).
- After the backfill the 24/7 service stays on Alchemy (EVM) and Helius free (Solana).
- Estimated wall time ≈ 1 day (dRPC lookups 8–20 h dominate; Solana 3–6 h in parallel; inserts ≈ 1 h).

**Steps before the launch:**
1. Backfill mode: reads history backwards from where each source's data starts, with its own cursors (stoppable
   and resumable); forward ingestion keeps running; signer lookups migrated-first; empty dRPC answers retried on
   Alchemy.
2. Batch inserts (≈ 32M rows), parallel Solana time slices, a budget per source; a 2-minute dRPC rate measurement
   to confirm the estimate.
3. Free disk right before the launch: `cargo clean` of `target/` (124 GB; 35 GB free, the year needs ≈ 20 GB).

**Launch:** Friday evening (owner's internet is unstable until then), monitored over the weekend. Then
`deploy/db-dump.sh` → server → `deploy/db-restore.sh` → `dev-tracker run` (section 5).

### Prep results (2026-10-07, evening)

- `dev-tracker backfill` implemented (EVM logs backwards with a cursor per source, identity batches with a retry
  client, Solana by whole UTC days in parallel with a done-marker per day, ATH until the queue is empty) and
  smoke-tested: Robinhood logs over 1 day = 44 requests; Solana 1 day = 54,042 create txs in 117 s + 1,812
  migrator txs in 3 s, 59 Helius requests, peak RSS 87 MB → one year ≈ 2–3 h at 6 days in parallel.
- Batch inserts (`INSERT … SELECT FROM UNNEST`, 5,000 rows/statement): ≈ 40k rows/s → 32M rows ≈ 15 min.
- **dRPC free cannot do the identity backfill:** `eth_getTransactionByHash` / receipts return `null` for launch
  transactions a few hours old (fresh ones resolve; Alchemy resolves all 5 of 5 sampled), and it answers 429 from
  10 concurrent requests (300 requests at concurrency 10: 122 null, 178 × 429, 0 found).
- Measured one day for the identity volume: BSC 1,693 creators (807 with ≥ 2 launches), 3,243 launches through
  shared intermediaries (8 migrated); Robinhood 2,932 creators (224 repeat), 270 shared launches (2 migrated).
  A full-year identity pass on Alchemy ≈ 6M calls ≈ $30–60.
- Cheaper path verified: a dev's launches through an intermediary are found in their own history with one
  `alchemy_getAssetTransfers(fromAddress = dev, toAddress = intermediary, category external, zero values
  included)` — 6/6 sampled launch transactions found. Option under the owner's decision: resolve signers of
  migrated shared launches only, then each such dev's other launches via that call (BSC); classify only creators
  with ≥ 2 launches, 4 owner samples instead of 8 → ≈ 0.5M calls ≈ $5. Devs with no migration in the year stay
  unattributed behind intermediaries; they cannot qualify (top-migr / win-streak need migrations; a curve token
  reaches a runner's ATH only after graduating).

### Launch runbook (Friday)

1. Free disk and build: `cargo clean` (≈ 124 GB back), `cargo build --release -p dev-tracker`.
2. Environment: `.env` with `SCOUT_HELIUS_API_KEY` (Developer plan), Alchemy URLs, Codex key, Telegram;
   `SCOUT_SOLANA_FALLBACK_RPC_URL` set to the public RPC or empty (the dRPC value answers HTTP 400).
   `export SCOUT_DEVTRACKER_DATABASE_URL=postgres://scout:scout@127.0.0.1:55439/scout`.
3. Forward ingestion keeps running during the backfill, with deliveries off (the lists change massively while
   history arrives): a copy of the config with `[delivery] telegram = false`, then
   `nohup target/release/dev-tracker run --config <copy> > logs/run.log 2>&1 &`.
4. Backfill (resumable — rerun the same command after any stop):
   `nohup target/release/dev-tracker backfill > logs/backfill.log 2>&1 &`
   Order per EVM chain: logs (minutes) → identity (option B, ≈ 0.5M Alchemy calls); Solana days in parallel
   (≈ 2–3 h); ATH last (≈ 3.7k Codex requests).
5. Watch: `tail -f logs/backfill.log`; progress
   `SELECT source, position FROM cursors WHERE source LIKE 'backfill:%' ORDER BY 1;`
   (EVM: lowest block reached; Solana: one row per finished day); `df -h /`; database size
   `SELECT pg_size_pretty(pg_database_size('scout'));`; Alchemy dashboard (CU), Helius dashboard (credits).
6. Done: `dev-tracker derive` (counts per category), `dev-tracker export --test` (look at the album), then turn
   deliveries on (restart `run` with the real config) — the first real delivery sends the complete lists.
7. Snapshot for the server: `deploy/db-dump.sh devtracker-<date>.dump`.

### Estimate update (2026-10-08, sources added: Robinhood Doppler, Base Bankr / Noice / Flaunch)

These launchpads name the creator only in the transaction, so the backfill reads one receipt per launch. One-day
backfill smoke: Base 1,156 requests (Bankr 803, Noice 272, Flaunch 52 launches), Robinhood Doppler 964 requests
(941 launches), 33 s wall. A year ≈ 0.77M Alchemy requests ≈ $8 and ≈ 3–4 h (runs alongside Solana); with the
option-B identity pass (≈ $5) the one-year EVM backfill costs ≈ $13 on Alchemy. Steady state adds ≈ 2.5k receipts/day
(≈ $0.8/month). ATH for these launchpads is observed only for creators with ≥ 3 launches (Codex budget).
