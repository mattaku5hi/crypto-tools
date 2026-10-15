#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "usage: package-release.sh VERSION COMMIT TAG BINARY_DIR OUTPUT_DIR" >&2
  exit 2
fi

version=$1
commit=$2
tag=$3
binary_dir=$4
output_dir=$5
target=x86_64-unknown-linux-gnu
package_name="trader-scout-${version}"
archive_name="${package_name}-${target}.tar.gz"

[[ "$version" =~ ^[A-Za-z0-9][A-Za-z0-9.+-]*$ ]] || {
  echo "invalid package version" >&2
  exit 2
}
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || {
  echo "commit must be a 40-character lowercase git SHA" >&2
  exit 2
}
if [[ -n "$tag" && "$tag" != "trader-scout/$version" ]]; then
  echo "tag and package version do not match" >&2
  exit 2
fi

for binary in dev-tracker buyer-intersect wallet-rank wallet-stats; do
  if [[ ! -f "$binary_dir/$binary" || ! -x "$binary_dir/$binary" || -L "$binary_dir/$binary" ]]; then
    echo "missing executable release binary: $binary" >&2
    exit 1
  fi
done

mkdir -p "$output_dir"
stage=$(mktemp -d "${output_dir%/}/.release-stage.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$package_name/bin"
for binary in dev-tracker buyer-intersect wallet-rank wallet-stats; do
  install -m 0755 "$binary_dir/$binary" "$stage/$package_name/bin/$binary"
done
mkdir -p "$stage/$package_name/deploy/portable" "$stage/$package_name/config"
install -m 0644 deploy/portable/compose.yml "$stage/$package_name/compose.yml"
install -m 0644 deploy/portable/compose.compact.yml "$stage/$package_name/compose.compact.yml"
install -m 0644 deploy/portable/runtime.Dockerfile "$stage/$package_name/deploy/portable/runtime.Dockerfile"
install -m 0755 deploy/portable/prepare-runtime.sh "$stage/$package_name/deploy/portable/prepare-runtime.sh"
install -m 0644 deploy/portable/.dockerignore "$stage/$package_name/.dockerignore"
install -m 0644 deploy/.env.example "$stage/$package_name/deploy/.env.example"
install -m 0644 deploy/portable/README.md "$stage/$package_name/deploy/portable/README.md"
install -m 0644 config/dev-tracker.example.toml "$stage/$package_name/config/dev-tracker.example.toml"
install -m 0644 deploy/state_bundle.py "$stage/$package_name/deploy/state_bundle.py"

python3 - "$stage/$package_name/manifest.json" "$stage/$package_name/dependency-licenses.json" "$version" "$commit" "$tag" "$target" "$stage/$package_name/bin" "$stage/$package_name" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

manifest_path, licenses_path, version, commit, tag, target, binary_dir, package_root = sys.argv[1:]
names = ("dev-tracker", "buyer-intersect", "wallet-rank", "wallet-stats")
binary_root = pathlib.Path(binary_dir)
package_root = pathlib.Path(package_root)
metadata = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
    )
)
packages = {package["id"]: package for package in metadata["packages"]}
external_ids = {
    package_id
    for node in metadata["resolve"]["nodes"]
    for package_id in [node["id"]]
    if packages[package_id]["source"] is not None
}
license_inventory = []
for package_id in sorted(external_ids, key=lambda item: (packages[item]["name"], packages[item]["version"])):
    package = packages[package_id]
    entry = {
        "name": package["name"],
        "version": package["version"],
        "source": package["source"],
    }
    license_expression = package["license"]
    license_file = package["license_file"]
    if license_expression:
        entry["license_expression"] = license_expression
    elif license_file:
        manifest_dir = pathlib.Path(package["manifest_path"]).parent
        license_path = (manifest_dir / license_file).resolve()
        if not license_path.is_file():
            raise SystemExit(f"dependency license file is missing: {package['name']} {package['version']}")
        entry["license_file"] = license_file
        entry["license_file_sha256"] = hashlib.sha256(license_path.read_bytes()).hexdigest()
    else:
        raise SystemExit(f"dependency has no SPDX license expression or verifiable license file: {package['name']} {package['version']}")
    license_inventory.append(entry)
license_bytes = (json.dumps({"schema_version": 1, "packages": license_inventory}, indent=2, sort_keys=True) + "\n").encode()
pathlib.Path(licenses_path).write_bytes(license_bytes)
runtime_templates = {}
template_paths = [package_root / ".dockerignore", package_root / "compose.yml", package_root / "compose.compact.yml"]
template_paths += sorted((package_root / "deploy").rglob("*"))
template_paths += sorted((package_root / "config").rglob("*"))
for path in template_paths:
    if path.is_file():
        relative = path.relative_to(package_root).as_posix()
        content = path.read_bytes()
        runtime_templates[relative] = {
            "sha256": hashlib.sha256(content).hexdigest(),
            "bytes": len(content),
        }
manifest = {
    "schema_version": 1,
    "project": "trader-scout",
    "version": version,
    "git_commit": commit,
    "git_tag": tag or None,
    "target": target,
    "rust_toolchain": "1.98.1",
    "build_image": "rust:1.98.1-bookworm",
    "cargo_lock_sha256": hashlib.sha256(pathlib.Path("Cargo.lock").read_bytes()).hexdigest(),
    "dependency_licenses_sha256": hashlib.sha256(license_bytes).hexdigest(),
    "binaries": {},
    "runtime_templates": runtime_templates,
}
for name in names:
    content = (binary_root / name).read_bytes()
    manifest["binaries"][name] = {
        "sha256": hashlib.sha256(content).hexdigest(),
        "bytes": len(content),
    }
pathlib.Path(manifest_path).write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY

cp "$stage/$package_name/manifest.json" "$output_dir/manifest.json"
tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
  -cf - -C "$stage" "$package_name" | gzip -n -9 > "$output_dir/$archive_name"
(
  cd "$output_dir"
  sha256sum "$archive_name" manifest.json > SHA256SUMS
)
echo "Packaged $output_dir/$archive_name"
