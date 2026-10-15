# Cargo target cleanup

Run `scripts/target-gc.sh --recursive ~/Projects` to discover Cargo projects
under a directory, including hidden worktrees, and reclaim old debug artifacts.
Use `--dry-run` first to print planned work without invoking Cargo or deleting
files. Passing a project path directly keeps the original single-project mode.

The script skips all cleanup while any Rust build process is running, and skips
an individual target while one of its debug binaries is running. It serializes
mutating runs with `flock`, and holds Cargo's `target/debug/.cargo-lock` while
cleaning each target so a new build cannot race the cleanup. A locked target
is skipped. Cleanup is limited to a non-symlink `target/debug`:
release outputs and other target data are retained. Recursive discovery prunes
`.scout-data` and common generated/dependency directories. `cargo-sweep` is
required (`cargo install cargo-sweep --locked`) and runs offline from each
project directory. Old incremental sessions are removed after seven days only
when no file in the session was recently modified; oldest sessions are evicted
to keep the combined debug target within 10 GiB.

Fixture checks: `bash scripts/tests/target-gc.sh`.

The local user timer `trader-scout-target-gc.timer` runs daily at04:30 local
time, with up to10minutes delay and catch-up after downtime. Its service
scans `~/Projects`; release outputs, databases, snapshots and `.scout-data`
are outside cleanup scope. Templates are in `deploy/systemd/`; render
`@WORKSPACE_ROOT@` as the checkout's absolute path when installing the
service in `~/.config/systemd/user/`. The old weekly `target-gc.sh` cron
entry is replaced by this timer.

Check scheduling with `systemctl --user list-timers trader-scout-target-gc.timer`
and logs with `journalctl --user -u trader-scout-target-gc.service`.
