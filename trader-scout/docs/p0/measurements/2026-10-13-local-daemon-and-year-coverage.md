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
changed-list delivery on live data, beyond the unchanged-repeat check. A further
changed win-streak Solana album was delivered at 18:16:30 UTC. The 18:43 audit
found current fact-cursor ages of 16–24 minutes and a 27-minute-old export,
within their configured cadences. Two isolated external heartbeat send errors
were logged, while internal beats and subsequent progress continued; zero
service restarts were observed.

After connectivity returned, the 19:45 UTC check found an internal beat less
than one minute old and Base/Robinhood source updates within nine minutes.
The daemon had automatically delivered changed Solana top-migr and win-streak
albums at 19:19:44 UTC; the current exports contain 39/424/140 Solana developers
in top-runners/top-migr/win-streak. The other network counts above are unchanged.

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

The frozen 679-token diagnostic cohort currently has **434 numeric values and
245 unknown**, including three absent ATH rows. The per-network split is:

| Network | Numeric | Unknown |
|---|---:|---:|
| Base | 401 | 102 |
| BSC | 13 | 0 |
| Robinhood | 6 | 47 |
| Solana | 14 | 96 |

These are candidate-cohort counts, not coverage percentages for all tokens in
the annual database. A numeric observation may itself have limited provider,
pool or time scope. Unknown values are not zero and do not satisfy runner gates.

Four guarded Base updates closed previously unknown numeric values:

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

- `fec3a8…cb07`: the exact May 3 GeckoTerminal high also appears as May 4
  open/high; choosing the later tied candle had falsely suggested a date gap.
  The May 3 candle itself supplies a valid same-day comparison. Receipt-backed
  USDC execution with dated FX differs by less than 0.084% at both endpoints.
  The scoped four-pool value is 111,664,197 cents; `ath_at` stays unknown. Six
  returned daily bars and the complete zero-Swap empty-pool exception are
  preserved. Its 0.011818 USDC notional is small, and the discordant Birdeye
  observation remains separate. No new provider calls were needed.
- `058e72…c738`: anonymous Zora history returned 92 indexed execution edges.
  The selected maximum matches a saved successful receipt and a two-pool WETH
  pool-core route in the same transaction. Coinbase's corresponding one-minute ETH/USD
  range corroborates it within 0.89% at both endpoints. Fixed one-billion supply
  gives 150,658,261 cents under source
  `zora-window-supply-route-fx-validated-v1`; `ath_at` remains unknown. The price
  is conservatively floored to 24 decimal places without changing the FDV-cent
  floor. The cached chain scan found 326 swaps through its later anchor, so the
  92-edge index is explicitly partial. General `priceUsdc` timestamp semantics
  remain undocumented; only the selected edge has the independent route/FX
  corroboration. Birdeye's disagreement with the route stays visible. The seven
  anonymous probe requests used no paid credentials; the guarded write made
  zero additional provider calls. Trades were not inserted as OHLC candles.
  A later offline receipt audit found ten core Swaps across four pools and
  hook WETH payouts totaling 3.312% of the external input. The route comparison
  is an execution average from core events; final wallet settlement and fee
  allocation remain unresolved. It is not a wallet-net cost or PnL input.

Each database write checked the old unknown row and observation before updating;
known positive values were not replaced. The GeckoTerminal updates follow ADR-021 amendment 23; the selected Zora
source is specified in amendment 24. Both new cases have exact-arithmetic golden
fixtures. Category thresholds and membership semantics did not change.

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
After the two new golden cases, the full fmt/Clippy/test sequence passed again
with 1,216 tests, using `scout_test` (483.42 seconds on the loaded workstation).
Database tests used the separate `scout_test` database. Seven portable runtime
and state-bundle tests, installer syntax and generated-unit path validation
also passed. The real installed service, first delivery and unchanged-repeat
checks supply the local integration proof.

Private manifests/logs are in `.scout-data/checks/2026-10-13/` and
`.scout-data/diagnostics/`; wallet lists and credentials are excluded from Git.
Next tasks remain **B8.4** (qualified historical ATH gaps), **B9.1** (actual
GitHub release workflow execution) and **B9** (VPS deployment after local
debugging). The local installation does not mark annual filling complete.

## Fresh private state package

The PostgreSQL custom dump taken after all four guarded updates passed archive
listing. Its command ran from 18:41:03 to 18:53:47 UTC without pausing the daemon;
these times bracket pg_dump's consistent internal snapshot, rather than naming
the exact snapshot transaction timestamp. Level-1 compression produced
3,011,427,325 bytes in 763.64 seconds, SHA-256
`e725df24825141f4fda94d04bd07b77ac0556bcbb2cfb2aac0fd24c29c7ab117`.

The protected bundle is
`.scout-data/deployment/2026-10-13-local-daemon-final/trader-scout-state-schema10-local-20261013.tar`.
It contains 39 payload files: schema-10 database dump, relative-path config,
36 separately dated exports and provenance including the guarded price
certificates. Size is 3,011,911,680 bytes; SHA-256
`ffed4fa6ddc1c0ed3cab15a3fa223513af7f81e0f7c7325507de8fd5c8256d4a`.
Packing, verification, extraction and independent hashes of every extracted
file passed in 114.54 seconds. Archive/files are 0600, extraction root 0700;
credentials are excluded and remain supplied separately in `deploy/.env`.
The source binary is the unchanged release build from `7c1ca5c`; deployment
assets were committed as `cc46164`, and the additional golden cases as
`ae6c055`.

The reported workstation exposes 16 CPUs (i7-12700K model) and 22.9 GiB RAM;
PostgreSQL occupied about 18.5 GiB including indexes at the final observation.
Cache state was uncontrolled and the daemon, tests and other workstation jobs
were active during snapshot work. These timings are operational observations,
not isolated throughput benchmarks. A fresh full restore was not performed for
this dump; the earlier schema-10 full restore proofs remain separately dated
in [portable deployment validation](2026-10-11-portable-deployment-validation.md).

## B8.4 state package after 26 guarded closures (2026-10-14 Moscow)

A new consistent PostgreSQL custom dump completed in 78.99 seconds, bracketed
by `2026-10-13T23:55:55Z–23:57:14Z`, while the daemon remained active. Its
3,013,515,453 bytes have SHA-256
`e71fae6bc2fe4f2f3dd4ce2d26cd81e60db51e3c902f079e37ea3e0d92b7232e`.
The archive table of contents passed validation. This snapshot includes the
26 committed guarded closures: 460 numeric / 219 unknown in the frozen
679-token candidate cohort. It precedes any batch-6 promotion.

The protected schema-10 package is
`.scout-data/deployment/2026-10-14-b84-26-closures/trader-scout-state-schema10-b84-26-closures-20261014.tar`,
3,013,990,400 bytes, SHA-256
`67650d1b3540f3d582ee1026ce63176db4331c6eb90518594cc014ac74adb6b3`.
All 39 payload files were verified and extracted; the 36 export files are
individually dated in provenance. The package contains the database dump,
configuration, exports and provenance; `deploy/.env` is supplied separately.
Packing, verification and extraction took 38.39 seconds. A fresh full database
restore was not performed for this package; the prior isolated restore and
schema-10 round trip remain separate evidence.

The live binary is unchanged from source `7c1ca5c…`; verified test/documentation
changes are at `7da9765…`. The service has zero restarts. This package is a
new partial-ATH checkpoint, not a claim that B8.4 or annual market-wide ATH
coverage is complete.

## Sixty-eight-closure state checkpoint (2026-10-14)

After the twelve conservative lower estimates (ADR-021 amendment40), a new
private schema10 bundle freezes 502 numeric /177 unknown in the fixed679
diagnostic cohort. Runtime binary remains source7c1ca5c, SHA-256
`b762ee94fc36d0f5f9a346f87e5c646edf01bea0c18a4e389c53c03c80f3700d`;
configuration checkout396cf90. The consistent pg_dump ran from
02:21:56 to02:23:07UTC without pausing the daemon. All68 closures match its
ATH table exactly, including holder/liquidity metadata and all12 explicit
lower-bound sources. This supersedes the separately preserved26-closure
bundle as the latest state checkpoint.

Private archive:
`.scout-data/deployment/2026-10-14-b84-68-closures/trader-scout-state-schema10-b84-68-closures-20261014.tar`,
3,014,778,880bytes, SHA-256
`8562eece3e9dfc10072b7891caa23c3154e5df3473c1a6f872573a1265ee2820`.
Its39 allowlisted payloads include36 separately dated import exports; no
secrets. Dump TOC, packing, extraction, all payload hashes and0600/0700 modes
passed; workspace fmt/Clippy/1232tests are hash-linked. A separate
isolated full restore subsequently passed: schema1–10, fixed679cohort, all68
closure rows, all100 pinned observations and quota7+100 were verified. The
1CPU/1GiB/no-network container and its private volume were removed. Earlier
path/log-name verifier failures are retained; the final independent query
proof is `restore-v3-verification.json`. The orchestration exit code was not
captured in that final attempt, so successful completion is established by
restored schema/data checks rather than an asserted measured process exit.


## Seventy-closure archive checkpoint (2026-10-14)

A newer schema10 consistent dump runs02:48:13–02:49:34UTC, after the two
transaction-local vault closures. It contains all70 expected ATH rows exactly
including sources, observation times, lower FDV floors and holder/liquidity.
Fixedcohort504numeric/175unknown. The39-payload/36-export archive is verified
and extracted with0600/0700 modes; secrets are excluded and dates separated.

Private archive:
`.scout-data/deployment/2026-10-14-b84-70-closures/trader-scout-state-schema10-b84-70-closures-20261014.tar`,
3,014,789,120bytes, SHA-256
`739a58b3f8b301de82672520069a6774743fb1e5d4e61f54499b2cde34257d52`.
Source binary remains7c1ca5c; checkoutcc49996; workspace1233tests passed.
This70-closure dump has archive/row verification, while the separate full
restore above validates the68-closure dump. A full70-dump restore is not
claimed. Both immutable checkpoint packages remain preserved.

## Seventy-four-closure archive checkpoint (2026-10-14)

The next consistent schema10 dump runs03:47:51–03:49:03UTC and includes all74
guarded closure rows, including the four first-buy recoveries. Its fixed
cohort is508numeric/171unknown. Streamed ATH-table reconciliation matches all
74 rows exactly across source, observation time, FDV, ATH date, holders and
liquidity before packing. All39 payloads, including36 exports, pass hash and
0600/0700 mode verification; secrets are excluded.

Private archive:
`.scout-data/deployment/2026-10-14-b84-74-closures/trader-scout-state-schema10-b84-74-closures-20261014.tar`,
3,015,116,800bytes, SHA-256
`7893ea60a0b463baaac7c4d9b89bc372f361e80c04ab2e7982b06d7e5d360a15`.
Dump SHA-256:
`95a02e34540efeec71af8474fb15bb86c60190c76b9c4e75cdc6b081ec74f4b4`.
Binary remains7c1ca5c; checkoutfb167df; linked round14b checks include1233
passing tests. This archive was not full-restored; the separately verified
68-closure restore above retains its own scope and exit-code caveat. This
package is a partial ATH checkpoint, not annual/all-market completion.

## Seventy-eight-closure archive checkpoint (2026-10-14)

The latest consistent schema10 dump runs04:40:47–04:42:02UTC after the
chronological three-case and Fht guarded writes. Fixedcohort512numeric/
167unknown. Before packing, streamed ATH-table reconciliation matches all78
closure rows exactly across all six recorded fields. All39 payloads and36
exports pass hashes and0600/0700 modes; secrets are excluded. Exports/config
are separately captured from04:41:53UTC while the daemon continues running.

Private archive:
`.scout-data/deployment/2026-10-14-b84-78-closures/trader-scout-state-schema10-b84-78-closures-20261014.tar`,
3,015,464,960bytes, SHA-256
`600c01914a8ce61b6a06c2ea77040d43dd6d0b370103920a44424572120e826f`.
Dump SHA-256:
`93d9d1f31b4a4efc8d9be06c52d00159fde49ab5a4d4350dd53ce58d6b87bd67`.
Checkout8ffc1f2; binary remains7c1ca5c; full round16 checks include1234tests.
Supplemental provenance pins corrected certificate-v4, Fht evidence/goldens
and guarded-write manifests. No full restore of this78-closure dump is
claimed: the separate68-closure restore retains its own validated scope and
exit-code caveat. All immutable earlier archives remain preserved. This
package includes the latest78 closures and still does not establish annual
or all-market ATH completeness.

## Fresh79-closure state checkpoint (2026-10-14)

The schema10 private package now includes the guarded vC95 reference: fixed
cohort513numeric/166unknown and79new values (SOL65/Base2/RH12). The consistent
dump ran05:24:23–05:25:38UTC; its ATH table has1,365,812rows. All79closures
match six exact dump fields, including source and observed version. Export
capture is separately dated. The39payloads (36exports, config, dump and
provenance) passed archive verification, extraction, every payload hash and
0600/0700mode checks. Secrets are supplied separately and excluded.

Archive3,015,772,160bytes, SHA-256:
`823dcd10d864344c5e3b26e32325330012c5d33121a7d6c42c3d58280a4c6301`.
Dump3,015,294,453bytes, SHA-256:
`4c1aa992f0a5f01547394a2155355fbdd0adc2e79914f9af2db4968b389bc879`.
Configuration checkoutfd689f7; unchanged live binary7c1ca5c. Full workspace
fmt/Clippy/1234tests passed against scout_test in48.16seconds. Source proof,
corrected dated curve-wire audit, raw-flow gaps, golden and write manifests
are pinned in provenance. Full restore of this79-closure dump was not run;
the independently verified68-closure restore remains a separate checkpoint.
Annual/all-market ATH is incomplete and B8.4 remains active.

## Fresh80-closure state checkpoint (2026-10-14)

The schema10 private package includes4ev and all previous guarded references:
fixedcohort514numeric/165unknown,80new values (SOL66/Base2/RH12). The
consistent dump ran06:10:57–06:12:09UTC; its ATH table has1,365,812rows.
All80closures match source, observed version, FDV, date, holders and liquidity
exactly.36separately dated exports, config, dump and provenance form39payloads;
archive verification, extraction, every hash and0600/0700mode checks passed.
Secrets remain separate.

Archive3,015,915,520bytes, SHA-256:
`da1845d77e947508b3f3fe1703402342a912654597162280cf171a823687bba2`.
Dump3,015,440,589bytes, SHA-256:
`4b0bd5d31d4454c883df20fa9118accdcf36249f5be91bf3f1182785158e9a70`.
Configuration checkout26509f6; unchanged live binary7c1ca5c. Full workspace
fmt/Clippy/1235tests passed against scout_test in48.12seconds. Observed4ev
source,96 unproven PRE diagnostics and the bundled-sale gap are pinned in
provenance. Full restore of this80-closure dump was not run; the68-closure
restore remains separately verified. B8.4/annual all-market ATH is incomplete.

Reproducible old extraction scratch was rehashed against retained archives
before cleanup; all source archives, dumps and proofs remain. An additional
17.1GB of apparent Rust incremental compiler-cache data was removed after
checking there were no active workspace compilers. Tested binaries and
dependencies remained; this build-cache footprint is not daemon memory.
