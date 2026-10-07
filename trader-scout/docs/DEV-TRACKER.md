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
- **Formats (pinned 2026-10-07):**
  - `gmgn` — JSON array of `{"address", "name", "emoji"}` (GMGN docs, "Wallets Import Export",
    https://docs.gmgn.ai/index/wallets-import-export).
  - `axiom` — JSON array of `{"trackedWalletAddress", "name", "emoji", "alertsOn"}`, all four required
    (Axiom's docs only say "import"; field names from third-party guides and shared exports — to confirm with
    a real Axiom export).
  - `basedbot` — text, one wallet per line `address  emoji  name` (BasedBot plain-text import, per the
    open-source converter `ariefzzz5421/wallet-tracker-transfer`; BasedBot's docs are not publicly
    readable). BasedBot also reads GMGN JSON.
- **Only on change:** a list is sent when its SET of wallets differs from the last delivered one (sha256 of
  the sorted addresses per category, chain and format in `deliveries`); label changes alone do not trigger
  a send. A list never delivered is not sent while empty. A failed send is retried next cycle.
- **Channel:** Telegram `sendDocument` to `SCOUT_TELEGRAM_CHAT_ID` with `SCOUT_TELEGRAM_BOT_TOKEN`
  (`delivery.telegram = true`); the current files are also written to `delivery.out_dir`.

## 5. Operations

- `dev-tracker run --config <file>` — the daemon: ingestion (launches, migrations, dev identities, ATH) every
  `schedule.ingest_every_minutes`, derivation + delivery every `schedule.derive_every_minutes`; the config is
  re-read every cycle (a broken edit is reported and the last good config kept); one chain's failure does not
  stop the others; SIGTERM / Ctrl-C stop it between steps.
- One-shot commands: `migrate`, `ingest --chains …`, `derive`, `export [--send]`.
- Packaging: `deploy/Dockerfile` (multi-stage, non-root), `deploy/docker-compose.yml` (PostgreSQL 16 + the
  daemon, keys from `.env`, config and exports mounted), `deploy/k8s/dev-tracker.yaml` (ConfigMap + one-replica
  `Recreate` Deployment, keys from the Secret of `secret.example.yaml`; `postgres.example.yaml` for a
  single-node database). Exactly one daemon per database: two would double provider requests and sends.

## 6. Order of work

B0 measurements (event volumes per chain and launchpad, ATH source: own swaps vs an external API such
as Codex) → B1 schema → B2 launch/migration ingestion (Solana pump.fun and Robinhood Pons first, then
BSC four.meme, then Base: Zora/Clanker for `top-runners`, other launchpads checked) → B3 ATH → B4
category engine from config → B5 daemon with incremental cycles → B6 container / compose / k8s → B7
JSON formats + Telegram delivery on change.
