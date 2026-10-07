#!/usr/bin/env bash
# Keep Cargo target/ directories small: remove artifacts unused for
# $GC_DAYS days, then the oldest ones until each target/ is under $GC_MAX.
# Cargo never deletes stale test/bin copies on its own (target/ reached
# 165 GB here). Needs cargo-sweep: `cargo install cargo-sweep --locked`.
#   scripts/target-gc.sh                 # this repository
#   scripts/target-gc.sh ~/Projects/a ~/Projects/b
set -euo pipefail
days=${GC_DAYS:-7}
max=${GC_MAX:-20GB}
here=$(cd "$(dirname "$0")/.." && pwd)
[ $# -eq 0 ] && set -- "$here"
for dir in "$@"; do
  [ -d "$dir/target" ] || { echo "target-gc: $dir has no target/, skipped"; continue; }
  before=$(du -sh "$dir/target" | cut -f1)
  cargo sweep --time "$days" "$dir" >/dev/null
  cargo sweep --maxsize "$max" "$dir" >/dev/null
  echo "target-gc: $dir/target $before -> $(du -sh "$dir/target" | cut -f1)"
done
