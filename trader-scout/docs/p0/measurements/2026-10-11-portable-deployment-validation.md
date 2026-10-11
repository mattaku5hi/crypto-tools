# Portable deployment validation, 2026-10-11

B9.2 is implemented and locally exercised. No VPS deployment or production
restart/restore occurred. GitHub release workflow execution remains pending.

The public package carries four binaries, runtime Compose files, the Bookworm
runtime Dockerfile, example config/env and the stdlib Python state verifier.
Private config/database/exports and real credentials are excluded. Docker
build context permits only `bin/dev-tracker`; credentials are supplied
separately as `deploy/.env`. The PostgreSQL volume name is explicit. Runtime
setup uses host UID:GID for rootful Docker and container 0:0 mapped to the
operator for rootless Docker; rootful setup as host root is refused.

## Actual state round-trip

The protected local package is
`.scout-data/deployment/2026-10-11-schema10-source/trader-scout-state-schema10-20261011T152507Z.tar`.
It has 39 allowlisted files: config, PostgreSQL custom dump, 36 selected
developer exports and JSON provenance. Its size is 2,841,313,280 bytes and
SHA-256 is `8741a9c3527cbe8a8b4b2e571d3d628b54c6db2e20b330d57005c8533331e08c`.
Archive/files are mode 0600; the new extraction root is mode 0700, and the
validated manifest is retained. Verification, extraction and independent
checks of every extracted file passed in 33.14 seconds on this workstation.

Database as-of is 2026-10-11 15:25:07 UTC; exports are separately dated
12:03:27 UTC. The only configuration adjustment is the relative export path
`exports`. Source commit is `b21db97`; the manifest preserves differing source
windows and the incomplete ATH cohort (425 numeric, 254 unknown). Subsequent
database changes require a newer snapshot; this archive is not a live mirror.

The extracted dump SHA-256 is
`4d822a82df72c917c70c89922077cb7ae7f2831c3ca8c1a342a23be0531536a5`, matching
the prior full isolated PostgreSQL 16 restore proof. That restore verified
schema versions 1–10, all 100 exact Solana Tracker source observations, tracked
application usage 104, 941 cursors and the frozen 679-token cohort. This
round-trip checked byte identity; it did not repeat that full restore.

## Actual runtime smoke

At 16:45:20–16:45:46 UTC, the public package was extracted into a new `/tmp`
directory outside the source checkout. The runtime image built in 8.75 seconds
and started with a fresh disposable PostgreSQL database on an internal Docker
network without host ports. A synthetic config disabled chain ingestion, ATH
requests, alerts and Telegram; no external provider request was made.

The daemon ran as UID:GID 1000:1000, with read-only root, all capabilities
dropped and a 512 MiB memory/swap ceiling. It applied schema 10, wrote all nine
empty Solana category/format exports through the writable bind mount and
recorded `clean_stop=true` on shutdown. The test container, database volume,
network and temporary runtime directory were removed. Existing production
daemon and database were untouched.

This validates paths, libraries, permissions, migrations, export I/O and
shutdown with locally built release binaries. It does not attest a GitHub
release build, a Bookworm compiler build of the current commit, live provider
credentials, rootless-Docker startup on this rootful host, or concurrent
resource use by other services/ML models.

Seven focused Python tests cover checksums/permissions, existing-target and
symlink refusal, traversal/links/tampering, oversized tar metadata, path/control
limits, export allowlist and Docker identity selection. Shellcheck, `bash -n`,
Compose configuration and scoped `git diff --check` also passed. Packaging was
previously checked for deterministic archives and per-template hashes.

Private proof manifests/logs are under
`.scout-data/checks/2026-10-11/b9-2-portable-live/`.
