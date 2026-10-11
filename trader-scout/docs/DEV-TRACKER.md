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

- `dev-tracker run --config <file>` — the daemon: facts ingestion (launches, migrations, dev identities) every
  `schedule.ingest_every_minutes`, derivation + delivery every `schedule.derive_every_minutes`; the config is
  re-read every cycle (a broken edit is reported and the last good config kept); one chain's failure does not
  stop the others; SIGTERM / Ctrl-C stop it between steps.
- One-shot commands: `migrate`, `ingest --chains …`, `derive`, `export [--send] [--test]`, `backfill`.
- Alerts (`[alerts]`, same bot and chat): 🟢 start, ⏹ stop (also when the stop arrives in the middle of a pass —
  the pass is abandoned safely), 🟠 restart after an unclean stop (crash, `kill -9`, OOM, power loss: a liveness
  beat is written every minute and the next start compares), 🔴 an incident — a chain's pass (`ingest:<chain>`),
  Helius while Solana runs on its fallback (`provider:helius`), the derivation or the export failing for
  `after_minutes` (30) — sent ONCE, and 🟢 its recovery once (tables `incidents`, `daemon_state`). A server that is
  dead and never restarts cannot report itself: `SCOUT_HEARTBEAT_URL` (a heartbeat URL of healthchecks.io — free
  plan: 20 checks —, Better Stack or Cronitor) gets a plain GET every minute while the process and its database are alive, and the monitor alerts
  when the pings stop; on healthchecks.io a clean stop is also written to the check's log (`<url>/log`). Progress watchdog:
  every finished step (a chain pass, the derivation, the export) is marked; no mark for `[alerts] stall_minutes`
  (90, never below the longest cadence + 15 min) = alive but stuck → one 🔴 from the bot and the external pings
  stop (the monitor alerts too); progress again → one 🟢 and pings resume.
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

### Bankr / Noice receipt reduction (2026-10-11)

The ingestion query now retrieves `Create` and companion `Lock` events together
from the same initializer. Locks are grouped by block and transaction index;
the first same-emitter Lock after each Create uses the existing largest
non-protocol beneficiary rule. A missing or unusable beneficiary still invokes
the original receipt attribution. Other creator rules remain as before.
Source reports count Create events only; the existing 200,000-log accumulation
cap includes both event types. The 16 MiB response bound and adaptive range
splitting remain enabled, and failed queries do not advance the cursor.
`--evm-step-blocks` is the maximum step. Bankr/Noice halve it after more than
5,000 Create events, down to 2,000 blocks or a smaller requested maximum;
below 2,500 events they double it back up to that maximum. Each source starts
with its own requested maximum, so sparse unrelated sources keep wide ranges.
Base backfill uses RPC concurrency 32 after a bounded Flaunch SDK measurement;
other chains use 8. Request weighting, pacing, retry limits and input ordering
remain enforced. The benchmark below is sampled evidence, not sustained quota
or year-wide throughput: [session measurements](p0/measurements/2026-10-10-backfill-and-bsc-pnl.md).

Eight historical Noice transactions passed exact query/receipt Lock comparison
and beneficiary comparison. A two-transaction mainnet fixture and synthetic
multi-launch/fallback tests retain that evidence. This verifies the sampled
attribution; it is not a year-wide throughput measurement. The earlier
one-receipt-per-launch cost estimates predate this change.

### Base ATH and shared Codex budget (2026-10-10)

The earlier request estimates above are historical estimates. The live year
backfill exposed 8.8 million eligible Base tokens without an ATH observation:
direct 200-address batches alone would require about 44,000 requests. The
dashboard showed roughly 5.5k of 10k requests used. See
[measured source coverage](p0/measurements/2026-10-10-base-ath-sources.md) and
[ADR-021 amendments 6–8](adr/ADR-021-dev-tracker-facts-and-storage.md).

Run facts and identities independently of ATH:

```sh
target/release/dev-tracker backfill --chains bsc,base,solana \
  --phases logs,identity,solana --evm-step-blocks 50000 \
  --identity-batch 2000 --solana-max-pages 2000
```

Base's optional provider-index pass requests positive ATH in pages of up to
200, including scam-labelled tokens, and intersects each page with tracked
launches. It enumerates Codex's `createdAt` filter over the last 365 closed
UTC days with resumable per-day/interval/offset cursors. Codex's documentation
conflicts about the date's meaning, and live results can return dates outside
the requested interval. Valid ATH for these rows is retained with source
`codex-index-date-mismatch`; cursors retain a mismatch-row count and up to four
examples per day. Completion proves that enumeration reached natural ends,
not returned-date membership or ATH for every contract created in that year. Observed ATH is cumulative at fetch time, not a
historical snapshot at the interval end. Tokens without a provider result remain
unknown. Run after launch facts are available, so completed days have the final
tracked-token intersection:

```sh
target/release/dev-tracker backfill --chains base --phases ath \
  --ath-from-index --days 365 --ath-requests 2000
```

`--ath-requests` counts HTTP attempts, including retries. A normal budget stop
reports `complete=false`; rerunning resumes. Authentication, provider quota,
malformed response and saturated one-second intervals stop with an explicit
error and preserve the unfinished cursor. Logs distinguish queried provider
rows, tracked matches and completed days. This is a mutable provider index,
without an immutable snapshot guarantee.

The shared Codex account counter is an opt-in row in `cursors`. Every daemon or
backfill HTTP attempt atomically spends one allowance; a zero counter prevents
another request, while chain ingestion continues. No counter means legacy
unlimited behavior. Set a conservative allowance from the dashboard before
running the new binary; do not reseed it on every restart or concurrent run.
When the billing period changes, verify the dashboard and explicitly replace
the allowance. Removing a cursor to reopen an index day requires a fresh pass
and consumes requests again. There is no automatic monthly reset or purchase.

For this session, the initial application allowance is 4,000 attempts, below
the roughly 4.5k remaining in the supplied dashboard. Initialize once:

```sql
INSERT INTO cursors (source, position, updated_at)
VALUES ('quota:codex:remaining', '4000', extract(epoch FROM now())::bigint)
ON CONFLICT (source) DO NOTHING;
SELECT position, updated_at FROM cursors WHERE source = 'quota:codex:remaining';
```

The counter controls this application's attempts; other clients using the
account can still consume the provider's quota. Provider quota errors therefore
remain terminal for the pass even if this counter is positive.

The current local processes use transient user services
`trader-scout-dev-tracker.service` and `trader-scout-year-backfill.service`.
Inspect them with `systemctl --user show <unit> -p ActiveState -p MainPID` and
read `logs/run.log` / `logs/backfill.log`. They run outside the agent's process
group and use the existing private `.env`, without embedding keys in command
arguments. They survive termination of the launching agent; they are not an
installed reboot deployment. B9 remains the server/deployment task.

A stopped transient unit can be garbage-collected; recreate it with `systemd-run --user` when it no longer exists, rather than starting a missing unit.

### Light Codex usage in the daemon (2026-10-11)

Facts and ATH have independent schedules (ADR-021 amendment 9). Default
facts cadence is 30 minutes, derivation 60 minutes, and ATH 240 minutes with
one request (up to 200 candidates) per chain per ATH pass. For four chains
that is at most 24 scheduled calls/day, 744 over 31 days, plus restarts and
manual jobs; empty queues use fewer calls. The direct observer has no
automatic HTTP retries. `ath_requests_per_chain = 0` disables ATH and does
not clear an existing Codex incident. The shared account counter still
bounds all jobs. Explicit legacy `ath_requests_per_chain = 20` settings are
not automatically rewritten: review them before a production start.

This is a request budget with limited coverage/freshness, not full monitoring
of millions of candidates. Helius (Solana) and Alchemy/Infura (BSC/Base)
continue chain facts independently. See the
[free ATH provider research](p0/measurements/2026-10-11-free-ath-providers.md)
for qualified historical/forward supplements to Codex.

### Targeted EVM runner ATH backfill

`dev-tracker backfill --chains <solana|base|bsc|robinhood> --phases ath
--ath-runners-from-index`
uses `config/dev-tracker.toml` (override with `--ath-index-config`) to read
the current `top_runners` minimum ATH, maximum plausible ATH and minimum
holders. It enumerates the selected Codex mainnet index with those filters,
then stores observations only for that chain's launch tokens already in
PostgreSQL. Chain and network keep identical address strings isolated.
Solana mints must decode from Base58 to 32 bytes and retain their case-sensitive
spelling through lookup and storage.
This mode has a separate cursor for each filter configuration and uses
`--ath-requests` as its request-attempt budget. It makes no automatic retries.
For a fixed token-birth floor, add
`--ath-index-token-created-since <unix-seconds>`; it is valid only with
`--ath-runners-from-index` and uses Codex `tokenCreatedAt` (token creation),
not the first-pair `createdAt` used for ranking. The query requests
`token.createdAt`; missing, malformed, negative, future, or below-floor
returned values are reported as quality gaps without discarding valid ATH.
The floor and runner thresholds are recorded in the report and cursor identity.

The mode removes the daily first-pair `createdAt` bound used by
`--ath-from-index`; it is still a filtered provider enumeration, not a complete
historical ATH index. `complete=true` means Codex returned a short page for
that filter. Index changes can affect offset pagination; saturation at 10,000
results is reported as incomplete. Unknown tokens remain unknown and do not
receive zero ATH. A token-birth floor narrows provider filter membership only;
it does not prove complete birth or launch coverage. Category derivation still
uses its configured `since_days` launch-history scope. See ADR-021 amendment 10
for cursor and coverage details.
