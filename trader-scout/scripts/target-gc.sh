#!/usr/bin/env bash
# Keep Cargo target/ directories small. Cargo never deletes stale test/bin
# copies on its own (target/ reached 165 GB here, 155 GB in a sibling project).
#
# Default mode (cargo-sweep): remove artifacts unused for $GC_DAYS days, then
# the oldest ones until target/ is under $GC_MAX.
# GC_DEBUG_ONLY=1: never touch target/release or non-Cargo files in target/
# (a project that runs services from target/release and keeps reports there):
# `cargo clean --profile dev` once target/debug exceeds $GC_MAX.
#
#   scripts/target-gc.sh                         # this repository
#   GC_DEBUG_ONLY=1 scripts/target-gc.sh ~/Projects/other
# Needs cargo-sweep for the default mode: `cargo install cargo-sweep --locked`.
set -euo pipefail
days=${GC_DAYS:-7}
max=${GC_MAX:-20GB}
here=$(cd "$(dirname "$0")/.." && pwd)
[ $# -eq 0 ] && set -- "$here"
max_kb=$(numfmt --from=iec "${max%B}" | awk '{printf "%d", $1/1024}')
for dir in "$@"; do
  [ -d "$dir/target" ] || { echo "target-gc: $dir has no target/, skipped"; continue; }
  before=$(du -sh "$dir/target" | cut -f1)
  if [ "${GC_DEBUG_ONLY:-0}" = 1 ]; then
    debug_kb=$(du -sk "$dir/target/debug" 2>/dev/null | cut -f1 || echo 0)
    if [ "${debug_kb:-0}" -gt "$max_kb" ]; then
      cargo clean --profile dev --manifest-path "$dir/Cargo.toml" >/dev/null 2>&1
    fi
  else
    cargo sweep --time "$days" "$dir" >/dev/null
    cargo sweep --maxsize "$max" "$dir" >/dev/null
  fi
  echo "$(date -Is) target-gc: $dir/target $before -> $(du -sh "$dir/target" | cut -f1)"
done
