#!/usr/bin/env bash
# Safely reclaim old Cargo debug artifacts. Release outputs are always retained.
set -euo pipefail

days=7
max=10GiB
dry_run=0
recursive=0
roots=()
here=$(cd "$(dirname "$0")/.." && pwd -P)

usage() {
  echo "usage: $0 [--dry-run] [--recursive PATH] [PATH ...]" >&2
}

while (($#)); do
  case "$1" in
    --dry-run) dry_run=1; shift ;;
    --recursive)
      (($# >= 2)) || { usage; exit 2; }
      recursive=1
      roots+=("$2")
      shift 2
      ;;
    --help|-h) usage; exit 0 ;;
    --*) usage; exit 2 ;;
    *) roots+=("$1"); shift ;;
  esac
done
((${#roots[@]})) || roots=("$here")

if ((dry_run == 0)); then
  command -v flock >/dev/null || { echo "target-gc: flock is required" >&2; exit 1; }
  lock=${XDG_RUNTIME_DIR:-/tmp}/target-gc-${UID:-$(id -u)}.lock
  exec 9>"$lock"
  flock -n 9 || { echo "target-gc: another cleanup is running; skipped"; exit 0; }
fi

proc_root=${GC_PROC_ROOT:-/proc}
# Return success if any Rust build process is active.
has_active_build() {
  local proc comm
  [[ -d "$proc_root" ]] || return 0
  for proc in "$proc_root"/[0-9]*; do
    [[ -d "$proc" ]] || continue
    comm=$(cat "$proc/comm" 2>/dev/null || true)
    if [[ "$comm" == cargo* || "$comm" == rustc* || "$comm" == rustdoc* ||
          "$comm" == clippy-driver* || "$comm" == nextest* ]]; then
      return 0
    fi
  done
  return 1
}

has_debug_executable() {
  local target="$1" proc exe
  for proc in "$proc_root"/[0-9]*; do
    [[ -d "$proc" ]] || continue
    exe=$(readlink -f "$proc/exe" 2>/dev/null || true)
    [[ "$exe" == "$target/debug/"* ]] && return 0
  done
  return 1
}

human_size() { du -sh -- "$1" 2>/dev/null | cut -f1; }
size_kb() { du -sk -- "$1" 2>/dev/null | awk '{print $1}'; }

discover_targets() {
  local root="$1" manifest project target
  root=$(realpath -e -- "$root") || { echo "target-gc: path missing: $1" >&2; return; }
  if ((recursive)); then
    while IFS= read -r -d '' manifest; do
      project=${manifest%/Cargo.toml}
      target="$project/target"
      [[ -d "$target" && ! -L "$target" ]] && printf '%s\0' "$target"
    done < <(find -P "$root" \
      \( -type d \( -name target -o -name .git -o -name node_modules -o -name .venv -o -name venv -o -name .cache -o -name .scout-data \) -prune \) -o \
      \( -type f -name Cargo.toml -print0 \))
  else
    target="$root/target"
    [[ -d "$target" && ! -L "$target" ]] && printf '%s\0' "$target"
  fi
}

declare -A seen=()
if has_active_build; then
  echo "target-gc: active Rust build process found; all cleanup skipped"
  exit 0
fi

for root in "${roots[@]}"; do
  while IFS= read -r -d '' target; do
    target=$(realpath -e -- "$target")
    [[ ${seen[$target]+yes} ]] && continue
    seen[$target]=1
    project=${target%/target}
    debug="$target/debug"
    if [[ -L "$debug" ]]; then
      echo "target-gc: $debug is a symlink; skipped"
      continue
    fi
    [[ -d "$debug" ]] || { echo "target-gc: $target has no debug/, skipped"; continue; }
    if ((dry_run == 0)); then
      lockfile="$debug/.cargo-lock"
      exec {target_lock_fd}>>"$lockfile"
      if ! flock -n "$target_lock_fd"; then
        echo "target-gc: Cargo target lock is held in $debug; skipped"
        exec {target_lock_fd}>&-
        continue
      fi
    fi
    if has_debug_executable "$target"; then
      echo "target-gc: debug executable is running in $project; skipped"
      if ((dry_run == 0)); then
        flock -u "$target_lock_fd"
        exec {target_lock_fd}>&-
      fi
      continue
    fi

    before=$(human_size "$target")
    if ((dry_run)); then
      echo "target-gc: would run cargo sweep --time $days and --maxsize $max in $debug (release and other target data preserved)"
    else
      (cd "$project" && CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$debug" cargo sweep --time "$days") >/dev/null
      (cd "$project" && CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$debug" cargo sweep --maxsize "$max") >/dev/null
    fi

    incremental="$debug/incremental"
    if [[ -d "$incremental" && ! -L "$incremental" ]]; then
      # Old compilation sessions are safe to remove. If the cache still exceeds
      # the cap, evict oldest sessions first; the active-work check protects builds.
      cutoff=$(date -d "$days days ago" '+%Y-%m-%d %H:%M:%S')
      while IFS= read -r -d '' entry; do
        if find -P "$entry" -newermt "$cutoff" -print -quit | grep -q .; then
          continue
        fi
        if ((dry_run)); then
          echo "target-gc: would remove stale incremental cache $entry"
        else
          rm -rf -- "$entry"
        fi
      done < <(find -P "$incremental" -mindepth 1 -maxdepth 1 -type d -print0)

      total=$(size_kb "$incremental")
      debug_kb=$(size_kb "$debug")
      cap_kb=$((10 * 1024 * 1024 - debug_kb + total))
      ((cap_kb < 0)) && cap_kb=0
      if ((total > cap_kb)); then
        while IFS= read -r -d '' record; do
          entry=${record#*$'\t'}
          [[ -d "$entry" ]] || continue
          [[ -L "$entry" ]] && continue
          entry_kb=$(size_kb "$entry")
          if ((dry_run)); then
            echo "target-gc: would remove incremental cache to keep debug target within 10GiB: $entry"
          else
            rm -rf -- "$entry"
          fi
          total=$((total - entry_kb))
          ((total <= cap_kb)) && break
        done < <(find -P "$incremental" -mindepth 1 -maxdepth 1 -type d -printf '%T@\t%p\0' | sort -z -n)
      fi
    fi
    after=$(human_size "$target")
    echo "$(date -Is) target-gc: $target $before -> $after"
    if ((dry_run == 0)); then
      flock -u "$target_lock_fd"
      exec {target_lock_fd}>&-
    fi
  done < <(discover_targets "$root")
done
