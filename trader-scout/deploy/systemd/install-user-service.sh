#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  echo "usage: $0 [runtime-directory]" >&2
  exit 2
fi

runtime_root=${1:-"${HOME:?HOME is not set}/.local/share/trader-scout/runtime"}
if [[ "$runtime_root" != /* || ! -d "$runtime_root" ]]; then
  echo "runtime directory must be an existing absolute path" >&2
  exit 2
fi
runtime_root=$(cd "$runtime_root" && pwd -P)
binary="$runtime_root/bin/dev-tracker"
config="$runtime_root/config/dev-tracker.toml"
env_file="$runtime_root/deploy/.env"

if [[ ! -x "$binary" || ! -r "$config" || ! -r "$env_file" ]]; then
  echo "runtime must contain executable bin/dev-tracker, config/dev-tracker.toml, and readable deploy/.env" >&2
  exit 1
fi
env_mode=$(stat -c '%a' "$env_file")
if (( (8#$env_mode & 077) != 0 )); then
  echo "deploy/.env must not be accessible by group or others (chmod 600)" >&2
  exit 1
fi
if ! grep -Eq '^[[:space:]]*SCOUT_DEVTRACKER_DATABASE_URL=.+$' "$env_file"; then
  echo "deploy/.env must set SCOUT_DEVTRACKER_DATABASE_URL for the existing database" >&2
  exit 1
fi

config_home=${XDG_CONFIG_HOME:-"${HOME:?HOME is not set}/.config"}
if [[ "$config_home" != /* ]]; then
  echo "XDG_CONFIG_HOME must be an absolute path" >&2
  exit 2
fi
unit_dir="$config_home/systemd/user"
unit_path="$unit_dir/trader-scout-dev-tracker.service"
if [[ -e "$unit_path" ]]; then
  echo "refusing to overwrite existing unit: $unit_path" >&2
  exit 1
fi
mkdir -p "$unit_dir"

template="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)/trader-scout-dev-tracker.service.in"
python3 - "$template" "$unit_path" "$runtime_root" <<'PY'
from pathlib import Path
import sys

template, destination, root = map(Path, sys.argv[1:])
root_text = str(root)
if any(ord(ch) < 32 for ch in root_text) or "$" in root_text:
    raise SystemExit("runtime path may not contain control characters or '$'")
def escaped_path(path: Path) -> str:
    result = []
    for byte in str(path).encode("utf-8"):
        char = chr(byte)
        if char.isascii() and (char.isalnum() or char in "/._-"):
            result.append(char)
        elif char == "%":
            result.append("%%")
        else:
            result.append(f"\\x{byte:02x}")
    return "".join(result)

def quoted_arg(path: Path) -> str:
    value = str(path).replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    return f'"{value}"'

replacements = {
    "@RUNTIME_ROOT_ESCAPED@": escaped_path(root),
    "@ENV_FILE_ESCAPED@": escaped_path(root / "deploy/.env"),
    "@BINARY_QUOTED@": quoted_arg(root / "bin/dev-tracker"),
    "@CONFIG_QUOTED@": quoted_arg(root / "config/dev-tracker.toml"),
}
text = template.read_text(encoding="utf-8")
for marker, value in replacements.items():
    text = text.replace(marker, value)
if "@" in text:
    raise SystemExit("service template contains an unresolved path marker")
destination.write_text(text, encoding="utf-8")
destination.chmod(0o644)
PY

systemctl --user daemon-reload
systemctl --user enable trader-scout-dev-tracker.service
printf 'Installed and enabled %s. Start it after confirming the old daemon is stopped.\n' "$unit_path"
