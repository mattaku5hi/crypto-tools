#!/usr/bin/env bash
set -euo pipefail

script=$(cd "$(dirname "$0")/.." && pwd -P)/target-gc.sh
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
fixture="$tmp/projects"
fakebin="$tmp/bin"
proc="$tmp/proc"
empty_proc="$tmp/empty-proc"
real_cargo=$(command -v cargo)
system_path=$PATH
mkdir -p "$fixture/.hidden/worktree/src" "$fixture/.hidden/worktree/target/debug/incremental/stale" \
  "$fixture/.hidden/worktree/target/debug/incremental/recent" "$fixture/.hidden/worktree/target/release" \
  "$fixture/.hidden/worktree/target/debug/incremental/old-dir-recent-file" \
  "$fixture/.hidden/worktree/target/reports" "$fixture/target-link" "$fixture/debug-link/target" \
  "$fixture/.scout-data/project/target/debug" "$tmp/outside/target/debug" "$fakebin" "$proc" "$empty_proc"
touch "$fixture/.hidden/worktree/Cargo.toml"
printf 'release artifact\n' >"$fixture/.hidden/worktree/target/release/app"
printf 'report\n' >"$fixture/.hidden/worktree/target/reports/report.json"
printf 'stale cache\n' >"$fixture/.hidden/worktree/target/debug/incremental/stale/cache"
touch -d '8 days ago' "$fixture/.hidden/worktree/target/debug/incremental/stale/cache" \
  "$fixture/.hidden/worktree/target/debug/incremental/stale"
printf 'recent cache\n' >"$fixture/.hidden/worktree/target/debug/incremental/recent/cache"
printf 'recent file in an old directory\n' >"$fixture/.hidden/worktree/target/debug/incremental/old-dir-recent-file/cache"
touch -d '8 days ago' "$fixture/.hidden/worktree/target/debug/incremental/old-dir-recent-file"
printf 'outside sentinel\n' >"$tmp/outside/target/debug/sentinel"
touch "$tmp/outside/Cargo.toml"
touch "$fixture/target-link/Cargo.toml"
touch "$fixture/debug-link/Cargo.toml"
touch "$fixture/.scout-data/project/Cargo.toml"
ln -s "$tmp/outside" "$fixture/linked-outside"
ln -s "$tmp/outside/target" "$fixture/target-link/target"
ln -s "$tmp/outside/target/debug" "$fixture/debug-link/target/debug"

cat >"$fakebin/cargo" <<'SH'
#!/usr/bin/env bash
printf '%s\t%s\n' "$CARGO_TARGET_DIR" "$*" >>"$GC_CARGO_LOG"
SH
chmod +x "$fakebin/cargo"
export GC_PROC_ROOT="$proc" GC_CARGO_LOG="$tmp/cargo.log"
export PATH="$fakebin:$PATH"

# Dry run discovers hidden nested worktrees and makes no changes or cargo calls.
output=$("$script" --dry-run --recursive "$fixture")
[[ "$output" == *"would run cargo sweep --time 7 and --maxsize 10GiB"* ]]
[[ "$output" == *"would remove stale incremental cache"* ]]
[[ ! -e "$GC_CARGO_LOG" ]]
[[ ! -e "$fixture/.hidden/worktree/target/debug/.cargo-lock" ]]
[[ -f "$fixture/.hidden/worktree/target/debug/incremental/stale/cache" ]]

# Real fixture cleanup uses the debug target only and retains release/report data.
"$script" --recursive "$fixture" >/dev/null
[[ ! -e "$fixture/.hidden/worktree/target/debug/incremental/stale" ]]
[[ -f "$fixture/.hidden/worktree/target/debug/incremental/recent/cache" ]]
[[ -f "$fixture/.hidden/worktree/target/debug/incremental/old-dir-recent-file/cache" ]]
[[ -f "$fixture/.hidden/worktree/target/release/app" ]]
[[ -f "$fixture/.hidden/worktree/target/reports/report.json" ]]
[[ -f "$tmp/outside/target/debug/sentinel" ]]
[[ $(wc -l <"$GC_CARGO_LOG") -eq 2 ]]
[[ $(cut -f1 "$GC_CARGO_LOG" | sort -u) == "$fixture/.hidden/worktree/target/debug" ]]
grep -q $'\tsweep --time 7' "$GC_CARGO_LOG"
grep -q $'\tsweep --maxsize 10GiB' "$GC_CARGO_LOG"
[[ -f "$fixture/.hidden/worktree/target/debug/.cargo-lock" ]]

# A lock shared with Cargo prevents cleanup from racing a build.
lockfile="$fixture/.hidden/worktree/target/debug/.cargo-lock"
flock "$lockfile" -c 'sleep 2' &
locker_pid=$!
for _ in $(seq 1 40); do
  if ! flock -n "$lockfile" -c true; then break; fi
  sleep 0.05
done
before=$(wc -l <"$GC_CARGO_LOG")
output=$("$script" --recursive "$fixture")
[[ "$output" == *"Cargo target lock is held"* ]]
[[ $(wc -l <"$GC_CARGO_LOG") -eq "$before" ]]
wait "$locker_pid"

# Any build command rooted in the project blocks all cleanup for that target.
mkdir -p "$proc/99999999"
printf 'cargo\n' >"$proc/99999999/comm"
printf 'cargo test --manifest-path %s/.hidden/worktree/Cargo.toml\0' "$fixture" >"$proc/99999999/cmdline"
ln -s "$fixture/.hidden/worktree" "$proc/99999999/cwd"
before=$(wc -l <"$GC_CARGO_LOG")
output=$("$script" --recursive "$fixture")
[[ "$output" == *"active Rust build process found"* ]]
[[ $(wc -l <"$GC_CARGO_LOG") -eq "$before" ]]
[[ -f "$fixture/.hidden/worktree/target/debug/incremental/recent/cache" ]]

# A running executable blocks its own target even when no compiler is active.
printf '#!/bin/sh\n' >"$fixture/.hidden/worktree/target/debug/app"
chmod +x "$fixture/.hidden/worktree/target/debug/app"
printf 'app\n' >"$proc/99999999/comm"
printf '%s\0' "$fixture/.hidden/worktree/target/debug/app" >"$proc/99999999/cmdline"
ln -s "$fixture/.hidden/worktree/target/debug/app" "$proc/99999999/exe"
rm -rf -- "$proc/99999998"
output=$("$script" "$fixture/.hidden/worktree")
[[ "$output" == *"debug executable is running"* ]]
[[ $(wc -l <"$GC_CARGO_LOG") -eq "$before" ]]

# A minimal offline crate exercises the installed cargo-sweep with project cwd
# and a debug-only target directory; release output stays intact.
real_project="$tmp/real-crate"
mkdir -p "$real_project/src" "$real_project/target/release" "$empty_proc"
cat >"$real_project/Cargo.toml" <<'TOML'
[package]
name = "target-gc-smoke"
version = "0.1.0"
edition = "2021"
TOML
printf 'fn main() {}\n' >"$real_project/src/main.rs"
printf 'release sentinel\n' >"$real_project/target/release/sentinel"
"$real_cargo" build --offline --manifest-path "$real_project/Cargo.toml" >/dev/null
GC_PROC_ROOT="$empty_proc" PATH="$system_path" "$script" "$real_project" >/dev/null
[[ -f "$real_project/target/release/sentinel" ]]

echo 'target-gc fixture checks passed'
