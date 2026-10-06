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

- **Format:** JSON importable into the wallet trackers of BasedBot, Axiom and GMGN (one file per
  category and terminal format as needed). The exact import formats are to be researched and pinned
  with samples before implementation (B7).
- **Only on change:** a file is sent only when its content differs from the last sent version (content
  hash per category and format stored in PostgreSQL).
- **Channel:** a Telegram bot (owner proposal); token from env / Secret.

## 5. Order of work

B0 measurements (event volumes per chain and launchpad, ATH source: own swaps vs an external API such
as Codex) → B1 schema → B2 launch/migration ingestion (Solana pump.fun and Robinhood Pons first, then
BSC four.meme, then Base: Zora/Clanker for `top-runners`, other launchpads checked) → B3 ATH → B4
category engine from config → B5 daemon with incremental cycles → B6 container / compose / k8s → B7
JSON formats + Telegram delivery on change.
