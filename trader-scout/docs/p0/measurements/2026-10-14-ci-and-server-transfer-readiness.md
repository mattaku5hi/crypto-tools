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


## Later owner-scoped checkpoint: 110 new references, 2026-10-14 20:13 UTC

After the owner accepted the remaining 15 Solana price disagreements, the
frozen cohort reached 544 numeric / 135 unknown values. The 100 Base gaps are
parked by explicit owner choice, and 35 Robinhood values remain unknown.
The new 110-reference private bundle passed packing, verification and extraction
for all 39 payloads (36 exports). The consistent compressed dump completed in
73.35 seconds; all 110 frozen reference rows matched source, observation time,
FDV, optional ATH date, holders and liquidity exactly. Runtime config and all
export contents were unchanged across the separately dated file capture.

Archive SHA-256:
`5c8ab1f028c239692a1e9c4e40104ebff828d4cf9a8002c92880d90eb0e9fd0e`.
Proof directory: `.scout-data/deployment/2026-10-14-b84-110-closures/`.
This 110-reference dump was not full-restored; the successful 95-reference
restore above remains the latest full-restore proof. Do not transfer that
older proof as validation of a full restore of the new dump. Final cutover
still needs a stopped-source snapshot and restoration/state verification.
Latest unchanged code validation passed 1248 tests plus fmt and Clippy.

The approved two-archive retention policy now retains complete bundles 95/110;
bundle 80 and temporary dump copies are pruned only after hash verification.
Small provenance and raw audits remain. No production restart, restore,
GitHub publication or VPS deployment was performed by this acceptance task.

## Published preview workflow: 2026-10-15

The release workflow is now published on `ci/trader-scout-release`. A push to
that exact branch produces a `preview-<commit12>-run-<number>` Actions artifact;
the tagged GitHub Release publishing step remains restricted to
`trader-scout/vMAJOR.MINOR.PATCH`. No release tag was created by this task.

The first real run on `8956d0a` failed its secret scan. The
[v2 action default](https://github.com/gitleaks/gitleaks-action/blob/v2/src/index.js#L138)
was Gitleaks 8.24.3, which did not apply the newly required allowlists. Replaying
the same public-data controls with checksum-verified official 8.24.3 and
8.30.1 binaries reproduced the difference. Both CI workflows now use
`gitleaks-action@v3` (Node 24) and explicitly pin Gitleaks 8.30.1. New exceptions
combine paths, affected rules, public field contexts and value shapes or exact
known public values. They match individual findings, not whole JSON lines.
Eight independent controls retain synthetic credentials beside public data
on the same line, including API keys and Coinbase tokens.

The fresh RustSec audit initially found four advisories against locked
`rustls 0.23.45` (RUSTSEC-2026-0346 through -0349). Updating only this package
to 0.23.46 cleared all vulnerabilities. Two unmaintained dependency warnings
remain: `derivative 2.2.0` and `paste 1.0.15`. The new Cargo.lock SHA-256 is
`3ba5cc8a1edf203ce8390a0f054699937af3430079455e5711671eddc75484e3`.
Local fmt, workspace/all-target/all-feature Clippy, 1,248 Rust tests and all
seven portable deployment tests passed after this update; database tests used
only isolated `scout_test`. The final committed-history scan also passed.

The next real CI run exposed a stale database-test assertion:
`null_extrema_keeps_provider_metadata_and_validates_batch_identity` expected
null after a `codex-missing` refresh, despite first inserting a known ATH of
900 cents. Fresh PostgreSQL 16 reproduced the failure. Retaining 900 is the
existing ADR-021 contract; production SQL needed no change. A read-only check
of the older local test DB found the fixed fixture token already stored as
`codex-missing` with `observed_at=1800000321`, which suppressed the test's
older numeric seed and masked the stale assertion.

The test now uses unique identities per run and checks retained ATH, original
source/ATH date, refreshed observation time and holder/liquidity metadata.
The test DB helper fails loudly on configured connection/migration errors.
The focused regression passed twice against the same new test DB, followed
by all 114 `scout-devtracker` tests. Full validation then passed fmt, Clippy,
1,248 workspace tests and seven deployment tests against that disposable
PostgreSQL 16 instance. Read-only before/after counts confirmed execution of
the ATH regression, rather than a skipped connection. Temporary test-failure
annotations used to diagnose the remote run were then removed from the
workflow. The disposable DB, its private connection file and the separate
verification build target were removed after capturing the proof.

Reusing the main build target for a temporary checkout had retained a test
binary with a removed `CARGO_MANIFEST_DIR`, causing unrelated mock-server
fixture reads to fail. Validation was rerun with its own target. Stale
workspace dev artifacts were removed while keeping dependency and release
artifacts; Cargo reported 4,085 files / 7.3 GiB removed. No production state
or runtime binary was changed.

[Run 38645526529](https://github.com/mattaku5hi/crypto-tools/actions/runs/38645526529)
passed all gates and compiled the four Bookworm release binaries, but failed
packaging. A bounded diagnostic rerun,
[38646988897](https://github.com/mattaku5hi/crypto-tools/actions/runs/38646988897),
identified the exact failure: host `cargo metadata --locked --format-version 1`
could not open the `anstyle-wincon-3.0.11.crate` cache path after the container
build (`Permission denied`, OS error 13). Earlier local packaging/cache probes
had passed and did not reproduce this directory-ownership condition. Neither
failed run uploaded a release artifact.

A minimized two-platform crate reproduced this exact write-permission seam:
the root container built Linux dependencies, then host metadata tried to add
an uncached Windows dependency beneath a root-owned 0755 cache directory.
Running the same container as the host UID/GID made metadata pass. The release
builder now uses that identity; temporary package-error annotations are
removed. Private red/green proof:
`.scout-data/checks/2026-10-15/b91/package-cache-permission-repro-v6/probe-result-v1.json`,
SHA-256 `1b782350f18107858ad0198f8c8685a542f9a487ff22da4a1a5658f7987ae4fe`.

The final run **succeeded** on exact source
`7a4d34e825740c93a9fa6078fb3a19914e350d35`:
[GitHub Actions run 38648161675](https://github.com/mattaku5hi/crypto-tools/actions/runs/38648161675).
Offline checks, PostgreSQL-backed workspace tests, secret scan, dependency
audit, Bookworm build, package creation, all four CLI `--help` smoke checks and
artifact upload passed. Tagged publishing was skipped for this preview push.

The generated Actions artifact is
[`trader-scout-preview-7a4d34e82574-run-7-x86_64-unknown-linux-gnu`](https://github.com/mattaku5hi/crypto-tools/actions/runs/38648161675/artifacts/11933184967),
16,240,734 bytes, retained until 2026-11-13 22:38:10 UTC. GitHub's artifact ZIP
digest is `sha256:3bbc6d78826b63e6b7487588300f398158dcca8e06e11b8b8293433ee6553c1c`;
this is distinct from the tarball checksum recorded in its `SHA256SUMS`.
After downloading and unpacking the Actions ZIP, run
`sha256sum --check SHA256SUMS` before extracting the versioned tarball.
The artifact was verified through its successful CI steps and public metadata;
it was not downloaded or independently rehashed on this workstation.

No release tag or GitHub Release was created. The workflow is published on the
preview/feature branches and has not been merged into default `main`; manual
dispatch requires registering the workflow there. The tagged publish path is
implemented but unexercised. Final server installation is B9 and still needs a
host, a fresh stopped-source state snapshot and restore/state checks. The
running workstation daemon and its executable were unchanged by this CI task.
At 22:27 UTC it was active (PID 8386, one restart), heartbeat age 31 seconds and
latest cursor age 246 seconds. The single existing `codex:solana` incident was
still open; it did not stop facts ingestion.

Immutable CI outcome proof:
`.scout-data/checks/2026-10-15/b91/actions-monitor-final/run-38648161675-final.json`,
SHA-256 `c4dd7d3217b6308591d0f6d2c2f3cd4338b069c592fbfb44948750bbeefca946`.
Workflow YAML/Bash/version/invalid-tag/failure-propagation checks:
`.scout-data/checks/2026-10-15/b91/workflow-uid-fix-validation-v1.json`,
SHA-256 `c6951ba60d27602f3982ff238c7d235ab2dffaa97eea247a64113d8ae985921b`.

Private proofs: `.scout-data/checks/2026-10-15/b91/`, especially
`reports/final-review-v1.json`, `reports/version-mismatch-v1.json` and
`reports/history-final-0647508.json.proof.json`; isolated Rust/Python validation:
`.scout-data/checks/2026-10-15/b9-1-cargo-validation/validation-summary-v2.json`.
The final fresh-DB full validation is
`.scout-data/checks/2026-10-15/b91/full-validation-v2/validation-summary-v2.json`,
SHA-256 `8d7f81ca0f73e14c7fbb4c3a64e2c2a08e36dd12cf33ed09190832963350dbfd`.

Changed files (relative to the `trader-scout` checkout):

- `../.github/workflows/trader-scout-release.yml`
- `../.github/workflows/ci.yml`
- `../.gitleaks.toml`
- `Cargo.lock`
- `crates/scout-devtracker/src/ath.rs`
- `AGENTS.md`
- `docs/ROADMAP.md`
- `docs/TICKETS.md`
- `docs/p0/measurements/2026-10-14-ci-and-server-transfer-readiness.md`
