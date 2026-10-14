# Portable Linux deployment

The versioned x86_64 release archive contains the daemon binary, this Compose
template, the Bookworm runtime Dockerfile, an example config, and the
stdlib-only state-bundle verifier. Extract it into a new runtime directory;
after that, Docker builds from this directory and no source checkout is needed.

Create the mutable paths, example config and private environment file with the
setup helper:

```sh
bash deploy/portable/prepare-runtime.sh
```

The helper checks Docker's rootless mode without printing its security options.
For rootless Docker it selects container UID 0, which maps to the invoking host
user. For rootful Docker it selects the setup operator's numeric UID:GID and
refuses to run as host root. Edit `deploy/.env`: use a random hexadecimal
database password and choose a new unique `SCOUT_POSTGRES_VOLUME`. Add only the
provider and alert credentials this deployment needs. The env file is not in
the public archive or private state bundle and remains mode 0600. The setup
helper refuses to overwrite an existing env file.
The config and export bind paths default to `./config/dev-tracker.toml` and
`./exports`; set `SCOUT_RUNTIME_CONFIG_PATH` or
`SCOUT_RUNTIME_EXPORTS_PATH` in the env file to relocate either path.

Private state is an explicit allowlist, not a copy of `.scout-data` or a home
directory. From the source workstation, pack a PostgreSQL custom dump, config,
selected exports and a JSON provenance record:

```sh
python3 deploy/state_bundle.py pack \
  --config /private/input/dev-tracker.toml \
  --db-dump /private/input/database.dump \
  --provenance /private/input/provenance.json \
  --export categories/base/top-runners.json=/private/input/top-runners.json \
  --output /private/output/trader-scout-state.tar \
  --software-version v1.2.3 \
  --source-commit 0123456789abcdef0123456789abcdef01234567 \
  --database-schema-version 10 \
  --as-of 2026-10-11T12:00:00Z \
  --scope "document the snapshot and export windows"
```

Use the actual commit, schema version, UTC as-of time and scope. Repeat
`--export RELATIVE_PATH=SOURCE_FILE` for each required export. The bundle
contains only `config/dev-tracker.toml`, `backups/database.dump`, explicitly
named `exports/` files, and `provenance/provenance.json`. It rejects source
symlinks and non-PostgreSQL custom dumps. The resulting archive is mode 0600.
Codex request budgets and the Solana Tracker allocation ledger are part of the
PostgreSQL dump; no separate SQLite/raw-segment state is invented.

On the server, verify before extracting to a new empty path:

```sh
python3 deploy/state_bundle.py verify --bundle /private/input/trader-scout-state.tar
python3 deploy/state_bundle.py extract \
  --bundle /private/input/trader-scout-state.tar \
  --output /srv/trader-scout-restore
```

The verifier checks the manifest and every SHA-256 before extraction. Extraction
retains the verified `manifest.json` (mode 0600) beside the state files for
review. It
rejects path traversal, links, duplicates, unlisted files, oversized archives,
and an existing extraction target. Extracted files are mode 0600 under a mode
0700 directory. Review the manifest, then copy the selected config, exports
and database dump into the matching runtime paths. Never extract a state
bundle over the running directory.

Before the first start, validate the Compose model and inspect the custom dump
using the official PostgreSQL client image:

```sh
docker compose --env-file deploy/.env -f compose.yml -f compose.compact.yml config --quiet
docker run --rm -i postgres:16 pg_restore --list - < backups/database.dump
```

The compact overlay applies the measured PostgreSQL 1 GiB / daemon 512 MiB
container ceilings and database settings. These limits do not reserve host
RAM and shared XGBoost concurrency has not been benchmarked. Omit
`compose.compact.yml` if the host has a separately reviewed resource profile.

For a snapshot restore, select a fresh, uniquely named PostgreSQL volume and
confirm it contains no prior database. Start only PostgreSQL, then restore
without `--clean`; a nonempty target should fail rather than drop existing
objects. The database archive contains facts and cursors. Start exactly one
daemon for that database after checking the restored cursors and exports:

```sh
docker compose --env-file deploy/.env -f compose.yml -f compose.compact.yml up -d db
docker compose --env-file deploy/.env -f compose.yml -f compose.compact.yml exec -T db \
  sh -ec 'PGPASSWORD="$POSTGRES_PASSWORD" pg_restore --no-owner --no-privileges --single-transaction --dbname="postgresql://scout@127.0.0.1:5432/scout" -' \
  < backups/database.dump
docker compose --env-file deploy/.env -f compose.yml -f compose.compact.yml up -d
```

The restore command streams the protected dump over stdin, so the PostgreSQL
container does not need access to a world-readable host backup. Do not run it
against a volume whose contents have not been confirmed disposable. The daemon
container drops Linux capabilities, uses a read-only root filesystem and a
bounded `/tmp`; keep `exports/` writable by its mapped host identity and config
readable. The database volume name is explicit so changing
the runtime directory does not silently create a different volume.

For the actual cutover, stop the workstation daemon before taking the final
dump and packing the private state bundle. That freezes ingestion cursors,
provider usage counters and Telegram delivery fingerprints at the transfer
point. Keep the workstation daemon stopped while the server uses that state.
Older prepared bundles are checkpoints rather than a mirror of later writes.
Before enabling the server daemon, verify restored schema, cursors, provider
usage and delivery state, then check its first progress/heartbeat and export
pass. Reuse the verified release manifest and state manifest to record which
binary and database snapshot were deployed.
