#!/usr/bin/env bash
# Restore a snapshot made by db-dump.sh into an EMPTY (or disposable) database:
# existing dev tracker tables are dropped and replaced.
#   SCOUT_DEVTRACKER_DATABASE_URL=postgres://… deploy/db-restore.sh devtracker-2026-10-07.dump
# Stop the daemon first; start it again afterwards (it applies any newer schema
# migrations itself). The URL is never printed.
set -euo pipefail
in=${1:?usage: db-restore.sh <file.dump>}
: "${SCOUT_DEVTRACKER_DATABASE_URL:?set SCOUT_DEVTRACKER_DATABASE_URL}"
docker run --rm -i --network host -e PGURL="$SCOUT_DEVTRACKER_DATABASE_URL" postgres:16 \
  sh -c 'pg_restore --clean --if-exists --no-owner --no-privileges --single-transaction --dbname="$PGURL"' < "$in"
echo "db-restore: restored $in"
