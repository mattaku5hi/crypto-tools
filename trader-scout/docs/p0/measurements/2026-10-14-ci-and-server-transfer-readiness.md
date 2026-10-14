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
