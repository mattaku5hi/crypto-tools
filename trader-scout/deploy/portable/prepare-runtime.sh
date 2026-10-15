#!/usr/bin/env bash
set -euo pipefail
umask 077

runtime_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
env_file="$runtime_root/deploy/.env"
if [[ -e "$env_file" || -L "$env_file" ]]; then
  echo "deploy/.env already exists; refusing to overwrite it" >&2
  exit 1
fi

if ! security_options=$(docker info --format '{{json .SecurityOptions}}' 2>/dev/null); then
  echo "cannot inspect Docker security options" >&2
  exit 1
fi
if [[ "${security_options,,}" == *rootless* ]]; then
  container_user=0:0
  docker_mode=rootless
else
  host_uid=$(id -u)
  host_gid=$(id -g)
  if [[ "$host_uid" == 0 ]]; then
    echo "run setup as the non-root runtime owner on rootful Docker" >&2
    exit 1
  fi
  container_user="$host_uid:$host_gid"
  docker_mode=rootful
fi

mkdir -p "$runtime_root/exports" "$runtime_root/backups" "$runtime_root/deploy"
if [[ ! -e "$runtime_root/config/dev-tracker.toml" ]]; then
  install -m 0600 "$runtime_root/config/dev-tracker.example.toml" "$runtime_root/config/dev-tracker.toml"
fi
install -m 0600 "$runtime_root/deploy/.env.example" "$env_file"
printf '\nSCOUT_CONTAINER_USER=%s\n' "$container_user" >> "$env_file"
chmod 0600 "$env_file"
printf 'Prepared runtime directories and deploy/.env (Docker mode: %s; container user: %s).\n' "$docker_mode" "$container_user"
