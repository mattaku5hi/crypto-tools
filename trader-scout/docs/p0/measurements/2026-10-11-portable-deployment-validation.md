# Portable deployment validation, 2026-10-11

B9.2 is implemented and locally exercised. No VPS deployment or production
restart/restore occurred. GitHub release workflow execution remains pending.
The current transferable package includes four guarded Solana FDV records
and the later Base 67d5 record. Earlier round-trips below are separately dated
proofs; the latest package is described in the final section.

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

## Current package after four Solana records

The updated package is
`.scout-data/deployment/2026-10-11-schema10-four-solana/trader-scout-state-schema10-20261011T175108Z-four-solana.tar`.
Its 39 allowlisted files occupy 2,842,306,560 bytes; SHA-256 is
`defd186671ad8c9fc5a0fe23271d2fe6ab1b1a957ac2a1937796a33492dfe344`.
The database snapshot is dated 17:51:08.201718 UTC; the 36 exports were
generated separately at 18:05 UTC with no provider calls or Telegram send.
Source commit for all four release binaries is `7bd0635`.

The dump SHA-256 is
`716f3b67270e2375b6590e31a0085ded55f867c6a336c5c6d2bc8d0e230697e6`.
A full PostgreSQL 16 restore into a disposable database completed in
490.13 seconds with a 1 GiB memory ceiling, one CPU and no network/host ports.
It verified migrations 1–10; 28,300,208 launches; 194,654 migrations;
1,357,128 ATH rows; 941 cursors; all 100 exact source-price observations;
tracked application quota 7+97; the frozen cohort 429 numeric/250 unknown;
and the exact FDV, source, null timestamp, holders and liquidity of all four
guarded Solana writes. The test container and volume were removed.

Package verification, extraction and independent checksums/permissions
passed in 23.66 seconds. Config uses the relative `exports` path; credentials
and logs are excluded. The extracted dump matches the fully restored dump
byte for byte; bundle verification did not repeat the restore.

The current four binaries also passed `--help` inside Bookworm. A second
isolated runtime smoke from an arbitrary `/tmp` directory passed schema 10,
nine synthetic empty exports and clean shutdown, with external requests and
Telegram disabled. This remains a local build/runtime proof, not evidence of
a GitHub workflow run, VPS deployment or live rootless-Docker startup.
Private manifests are in
`.scout-data/checks/2026-10-11/b9-2-portable-current/`; snapshot/restore manifests
are `.scout-data/diagnostics/schema10-four-solana-{snapshot,restore}-manifest.json`.

## Current package after Base 67d5

The latest package is
`.scout-data/deployment/2026-10-11-schema10-five-scoped/trader-scout-state-schema10-20261011T183937Z-five-scoped.tar`.
It has 39 payload files plus the retained `manifest.json` (40 tar entries),
2,842,726,400 bytes, SHA-256
`33919254447f4edceea78fb70cf40f0684aa31b87b1084775b343f020e2e7d87`.
Database as-of is 18:39:37.402419 UTC; the 36 exports are separately dated
18:42:32.692970 UTC. Source commit is `de3fd42`. Verification, extraction,
independent file hashes and 0600/0700 permissions passed in 19.82 seconds.
Configuration uses the relative `exports` path; credentials remain separate.

The included dump is 2,842,272,900 bytes, SHA-256
`998cf85e1910a0863f5f86631fa274cd6c9749e958245239a48334f4606a033a`.
Full isolated PostgreSQL 16 restore passed in 459.63 seconds under the same
1 GiB/one-CPU/no-network limits. It verified 28,303,621 launches,
194,715 migrations, 1,357,128 ATH rows, 941 cursors, 229 price windows,
schema 1–10, all 100 exact source observations and application quota 7+97.
The frozen cohort is 430 numeric/249 unknown, including three absent ATH
rows. All four Solana values and the exact new Base row/window match their
guarded-write proofs. Container and volume cleanup passed.

All four current CLI binaries passed Bookworm `--help`; the newly rebuilt
daemon also passed isolated startup/export/clean-stop runtime smoke from
an arbitrary `/tmp` directory. No provider requests, Telegram sends, production
restart/restore, VPS deployment or GitHub workflow execution occurred in these
checks. Package verification checked dump byte identity against the full
restore proof, without repeating that restore. Manifests are under
`.scout-data/checks/2026-10-11/b9-2-portable-five-scoped/` and
`.scout-data/diagnostics/schema10-five-scoped-{snapshot,restore}-manifest.json`.
