# GitHub CI and server transfer readiness — 2026-10-14

The deployment path is implemented and locally exercised, but the current
checkout has no verified GitHub release artifact or VPS deployment.

The public GitHub API and read-only remote refs show main atcf367e9 and the
remote feature branch at44b7a53. At the audit, local4e5bba9 was84commits ahead
of that feature branch. The release workflow is absent remotely. The latest
[CI run#88](https://github.com/mattaku5hi/crypto-tools/actions/runs/38063289874)
succeeded on old main on2026-10-10; it does not validate the current checkout.
The gh CLI is unavailable; authenticated workflow dispatch was not attempted. No
branch, tag, release or server deployment was published during this audit.

The local release workflow pins Rust1.98.1, builds four Linux x86_64 GNU CLIs
inside `rust:1.98.1-bookworm`, packages a versioned TAR.GZ with checksums and
version/source/lockfile manifests, and smoke-runs binaries on Debian Bookworm.
Offline Rust checks use local PostgreSQL scout_test; secrets and dependency
advisories have separate gates. Dependency license metadata is inventoried;
there is no enforced license allowlist. Public artifacts exclude private
state and credentials. Manual workflow runs create Actions artifacts;
`trader-scout/v*` tags publish releases. Actual first execution is B9.1 pending.

Added the existing seven portable-runtime/state-bundle Python tests to both
CI workflows and shell syntax validation to the release checks. All seven
tests passed locally, deployment shells passed syntax checks, and both
workflow YAML files parsed. Most recent unchanged Rust workspace verification
is1236passingtests plus fmt/all-target/all-feature Clippy in isolatedscout_test.
These local results are not a GitHub workflow run.

Portable Compose and compact overlays validate with dummy credentials.
Runtime setup handles rootless/rootful identity, relative config/export paths,
private0600 `deploy/.env`, explicit PostgreSQL volume, and a read-only daemon
image. The compact limits are1GiB PostgreSQL plus512MiB daemon; shared load
with the two XGBoost services is not measured. Native packages are x86_64;
ARM requires a separate build. Kubernetes files are templates: daemon image
is a placeholder and exports use emptyDir, so they are not a completed VPS
installation. Compose is the locally verified transfer route.

Two retained state TARs contain schema10 dumps, config,36exports and
provenance. Both pass the stdlib verifier and all39payload hashes, with tar
hashes matching sidecars. Standalone dumps and unpacked dump copies were
pruned under the approved two-package retention policy; extract the verified
archive to obtain `backups/database.dump`. Current live cohort is515numeric /
164unknown (81new references), whereas the latest sealed bundle contains80.
A fresh full restore of bundles79/80 was not performed. Prior68 restore data
verification passed; process exit capture was unavailable, and its old archive
is now pruned. This proves the procedure at that checkpoint, not a restore
of the latest data.

For actual cutover: stop the local daemon, snapshot and verify a final private
bundle, transfer it plus the verified public release and0600 env separately,
restore into a fresh uniquely named empty volume, verify schema/cursors/quota/
delivery state, then start one server daemon. Keep the source stopped to
avoid duplicate Telegram delivery and account quota use. The portable README
now records this order. No production restart or restore was performed here.

The current daemon is healthy: PID92417, zero restarts/incidents, heartbeat
age20seconds at the audit,28,479,478launches,197,958migrations and941cursors.
This is ongoing facts ingestion; targeted historical ATH gaps remain under
B8.4, with H4/9sEF evidence being checked separately. Annual/all-market ATH
completion is not claimed.

Changed: both CI workflow test steps, portable cutover README, ticket statuses
and this dated readiness record. Private verification logs:
`.scout-data/checks/2026-10-14/b9-readiness/`. Next tasks: B8.4 historical gaps,
B9.1 publish/exercise the release workflow, B9 server cutover when a host exists.

## Later checkpoint: 95 new references, 2026-10-14 14:14 UTC

The later consistent live dump contains all 95 new reference rows: each source,
observation time, stored FDV, optional ATH date, holder count and liquidity value
matches the frozen progress evidence. The 679-key cohort now has529 numeric /
150 unknown entries (SOL 15, Base 100, Robinhood 35, BSC 0 unknown). The four latest
SOL references cover captured selected-pool windows; they do not establish
annual/all-market ATH. All four are below the runner threshold and do not change
developer-list membership. B8.4 remains incomplete.

The new schema 10 private bundle contains 39 payloads, including 36 exports. Packing,
verification and extraction passed, with all payload hashes and permissions
checked. Archive SHA-256:
`701332da041bb8729eef8c90c4ba560f268c83ca43ed02fd56dd9e018e9417e0`.
The dump started at 14:12:57.985 UTC and finished at 14:14:15.874 UTC; exports have
their own recorded capture times. Credentials remain separate. A full database
restore of this 95-reference snapshot was not performed; the earlier 68-reference
restore evidence remains historical proof only.

Under the approved retention policy, complete bundles 80/95 are retained.
Bundle 79 and the new standalone/extracted dump duplicates and derived ATH COPY
were removed after hash checks and successful 95-row reconciliation, freeing
8.54 GiB. Small provenance, exports and raw diagnostics remain. To recover the
dump, extract the verified 95 bundle; the pack script's original dump input was
intentionally pruned.

Latest code validation passed 1247 tests, fmt and workspace/all-target/all-feature
Clippy with warnings denied, using isolated scout_test. No production restore,
daemon restart, GitHub publication or VPS deployment occurred. Final cutover
still requires a stopped-source snapshot and restore/state checks before one
server daemon starts. Private evidence:
`.scout-data/deployment/2026-10-14-b84-95-closures/bundle-manifest.json`,
`closure-dump-reconciliation.json`, and
`.scout-data/maintenance/snapshot-retention-95-applied-2026-10-14.json`.

## Subsequent full restore: 2026-10-14 14:41 UTC

The same 95-reference archive was subsequently restored into a fresh isolated
PostgreSQL 16 instance. `pg_restore --exit-on-error` exited 0 in 444 seconds.
Read-only checks found migrations 1–10 and matched all 95 reference rows across
source, observation time, FDV, optional ATH date, holders and liquidity, with
zero missing or different rows. Restored counts include 28,495,679 launches,
198,207 migrations, 1,366,918 ATH-table rows and 941 cursors. The quota row,
daemon state and 30 delivery records were present; their inventory fingerprints
were not compared against a separate source-side table export.

The container ran as UID/GID 1000:1000, with 1 CPU, 1 GiB memory, 512 MiB shared
memory, no network or ports, all capabilities dropped and no-new-privileges.
The existing Docker daemon was rootful; the CLI and container process ran as
the current user without sudo. Only a new private test directory was bound.
The test container, data directory and extracted dump were removed afterwards.
Production kept running with no restart or incident; its cursor age was 25
seconds at the post-restore check. Complete archives 80/95 remain retained.

The first invocation used an unsupported dash filename and exited 1 before
restoring any user tables; the successful stdin retry and both exit codes are
recorded separately. The archive/progress/reconciliation hashes checked before
the restore still matched afterwards. The frozen bundle provenance retains its
historical “not restored yet” statement; this later sidecar records the new
result without changing the archive. Proof:
`.scout-data/deployment/2026-10-14-b84-95-closures-restore-audit-v1/restore-verification-v1.json`,
SHA-256 `c0d91c6c81897748b15892feca80e41a557e4d1970762a5f0ef69f33195954d7`.
This validates the local restore of this snapshot. B8.4 annual ATH gaps,
B9.1 actual GitHub release execution and the final stopped-source VPS cutover
remain pending.
