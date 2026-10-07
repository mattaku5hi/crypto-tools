#!/usr/bin/env bash
# Snapshot the dev tracker database to one compressed file (pg_dump custom
# format). Facts AND cursors are inside, so a service restored from it resumes
# ingestion where the snapshot ended (the gap is read on its first passes).
#   SCOUT_DEVTRACKER_DATABASE_URL=postgres://… deploy/db-dump.sh devtracker-2026-10-07.dump
# Runs pg_dump from the postgres:16 image (no local client needed); the URL is
# never printed.
set -euo pipefail
out=${1:?usage: db-dump.sh <file.dump>}
: "${SCOUT_DEVTRACKER_DATABASE_URL:?set SCOUT_DEVTRACKER_DATABASE_URL}"
docker run --rm --network host -e PGURL="$SCOUT_DEVTRACKER_DATABASE_URL" postgres:16 \
  sh -c 'pg_dump --format=custom --compress=6 --no-owner --no-privileges "$PGURL"' > "$out.tmp"
mv "$out.tmp" "$out"
echo "db-dump: wrote $out ($(du -h "$out" | cut -f1))"
