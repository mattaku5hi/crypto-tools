# Local daemon and annual coverage — 2026-10-13

The daemon is deployed on the owner's current workstation and delivers developer
lists to Telegram. Annual ATH verification is still incomplete. Local operation
and the first delivery are verified; neither VPS deployment nor full lifetime
price coverage is claimed.

## Running service and delivery

The previous transient daemon used `config/dev-tracker.backfill.toml` with
`delivery.telegram = false`; its database had no delivery records. Credentials
were present and the bot responded. After a graceful stop, the rebuilt release
binary was installed at `~/.local/share/trader-scout/runtime/bin/dev-tracker`.
Configuration and private credentials live in `config/dev-tracker.toml` and
`deploy/.env` within that runtime; exports use the relative path `exports`.
The existing PostgreSQL database and persistent Docker volume were reused.

`deploy/systemd/install-user-service.sh` installs an enabled, non-transient user
unit with restart on failure. It validates the runtime and private environment
file, refuses to overwrite an existing unit, and does not start a second daemon.
The actual local service is active with no restarts at the verification point;
user lingering is enabled. Logs are read through
`journalctl --user -u trader-scout-dev-tracker.service`.

An explicit Telegram message warned that the lists have a 365-day derivation
window but remaining historical ATH gaps. At 16:46 UTC, `export --send` sent
10 non-empty category albums, each containing GMGN, Axiom and BasedBot files:
30 documents and 30 persisted delivery rows. A second export sent zero albums;
membership hashes, stored members and delivery timestamps were unchanged.
The daemon then started with Telegram enabled, retaining that delivery state.
Membership changes trigger subsequent delivery. Initially empty categories
are skipped under the existing delivery contract.

| Network | top-runners | top-migr | win-streak |
|---|---:|---:|---:|
| Solana | 39 | 423 | 133 |
| BSC | 12 | 75 | 23 |
| Base | 12 | 0 | 0 |
| Robinhood | 12 | 12 | 4 |

These are first-delivery counts, with 732 distinct developers across categories.
Live counts can change. The original runner/migration thresholds remain intact.
At 16:55:44 UTC the daemon's own derivation/export sent two changed Solana
albums: top-migr became 424 and win-streak 137; top-runners stayed 39. Their
six format records received new delivery timestamps, while the other eight
categories retained the first-delivery timestamp. This verifies automatic
changed-list delivery on live data, beyond the unchanged-repeat check.

Facts run every 30 minutes, derivation every 60 minutes and ATH every 240
minutes, with one Codex attempt per chain per ATH pass. Four networks therefore
permit at most 24 scheduled calls/day, plus restart/manual work; the persisted
shared application budget still applies. This cadence does not promise fresh
ATH for every historical token. Current progress and incident state were checked;
the configured external heartbeat returned HTTP 200.

## What the annual check establishes

Saved backfill evidence and current cursors show that the configured event
sources reached their historical floors: Solana has consecutive create and
migration markers for 2025-10-09 through 2026-10-09; BSC has four completed
source cursors; Base nine; Robinhood seven reaching genesis. BSC deliberately
overreads history and the 365-day derivation filter separately limits lists.
This establishes the configured source scope, not every launchpad or DEX.
Indexed checks found no actionable BSC/Robinhood identity gaps under the current
eligible-candidate selectors; this is not an all-wallet identity guarantee.

The frozen 679-token diagnostic cohort currently has **432 numeric values and
247 unknown**, including three absent ATH rows. The per-network split is:

| Network | Numeric | Unknown |
|---|---:|---:|
| Base | 399 | 104 |
| BSC | 13 | 0 |
| Robinhood | 6 | 47 |
| Solana | 14 | 96 |

These are candidate-cohort counts, not coverage percentages for all tokens in
the annual database. A numeric observation may itself have limited provider,
pool or time scope. Unknown values are not zero and do not satisfy runner gates.

Two guarded Base updates closed previously unknown numeric values:

- `92d958…8b07`: exact price × fixed 100B supply, corroborated within the accepted
  2% tolerance using the saved eight-pool daily-window and same-day USDC/FX
  evidence. FDV is 100,542,474 cents. Source is
  `geckoterminal-window-supply-swap-fx-validated-v2`; `ath_at` remains unknown.
  No new provider requests were needed. Other pools/lifetime remain unproven.
  The selected execution has only 0.000045 USDC notional, an explicit evidence
  limitation; a one-sided zero-output event has no execution-price ratio.
- `949b72…9b07`: a fresh exact-token Codex result supplied 1,052,794,500 cents,
  timestamp 1791731747 and 4,830 holders. Two bounded queries tested the default
  and explicit scam-inclusive filter; responses were identical. No filter fix
  was warranted. The existing primary-source policy was retained.

Each database write checked the old unknown row and observation before updating;
known positive values were not replaced. The first update follows ADR-021
amendment 23 and its existing golden fixture; category semantics did not change.

Three separate Solana Tracker token-info calls returned current holder counts
4, 2 and 1 for the three frozen Solana records with unknown holders. These are
current metadata, not historical ATH or holders at the peak; no ATH/holder
database write was made from them. Tracked application usage became 107/250,
within the owner's actual 2,500-request monthly allowance. Exact-token inputs,
responses and budget reservations are private audit evidence.

The remaining cohort's admission audit does not show an additional developer
that these gaps alone would admit under the other current facts. That does not
prove global annual discovery complete. Extra big runners count toward the
additional runner requirement after reserving the required minimum big runner;
the existing implementation already applies that rule correctly.

Complementary sources and access failures are recorded in
[year-history options](2026-10-13-ath-year-history-options.md) and
[holder metadata probes](2026-10-13-holder-metadata-probe.md).
Short recent windows and current prices cannot close a year of missing history.

## Validation and evidence

On the current Linux workstation, the release daemon used approximately 70 MiB
of systemd-accounted memory, with a 72.5 MiB peak at the observation point;
PostgreSQL's container reported approximately 213 MiB. These are live service
observations, not peak annual-backfill or shared-VPS measurements, and the native
service does not impose the compact Compose memory ceilings.

`cargo fmt --check`, workspace/all-targets/all-features Clippy with warnings
denied, 1,214 workspace/all-features tests and the locked release build passed.
Database tests used the separate `scout_test` database. Seven portable runtime
and state-bundle tests, installer syntax and generated-unit path validation
also passed. The real installed service, first delivery and unchanged-repeat
checks supply the local integration proof.

Private manifests/logs are in `.scout-data/checks/2026-10-13/` and
`.scout-data/diagnostics/`; wallet lists and credentials are excluded from Git.
Next tasks remain **B8.4** (qualified historical ATH gaps), **B9.1** (actual
GitHub release workflow execution) and **B9** (VPS deployment after local
debugging). The local installation does not mark annual filling complete.
