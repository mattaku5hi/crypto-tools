#!/usr/bin/env python3
"""Create and verify a private, allowlisted trader-scout state bundle."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import io
import json
import os
import pathlib
import re
import shutil
import stat
import sys
import tarfile
from contextlib import contextmanager
from typing import BinaryIO, Iterator

MAX_FILES = 4096
MAX_FILE_BYTES = 35 * 1024**3
MAX_TOTAL_BYTES = 40 * 1024**3
MAX_MANIFEST_BYTES = 4 * 1024**2
CHUNK_BYTES = 1024 * 1024
MAX_TAR_READ_BYTES = 4 * 1024 * 1024
COMMIT_RE = re.compile(r"[0-9a-f]{40}")


class BundleError(Exception):
    pass


def _regular_file(path_text: str) -> tuple[pathlib.Path, int, str]:
    path = pathlib.Path(path_text).absolute()
    current = pathlib.Path(path.anchor)
    for component in path.parts[1:]:
        current /= component
        if current.is_symlink():
            raise BundleError(f"input path contains a symlink: {path_text}")
    try:
        metadata = path.stat(follow_symlinks=False)
    except OSError as error:
        raise BundleError(f"cannot read input file: {path_text}") from error
    if not stat.S_ISREG(metadata.st_mode):
        raise BundleError(f"input is not a regular file: {path_text}")
    if metadata.st_size > MAX_FILE_BYTES:
        raise BundleError(f"input exceeds the per-file limit: {path_text}")
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_BYTES):
            digest.update(chunk)
    return path, metadata.st_size, digest.hexdigest()


def _directory_without_symlinks(path: pathlib.Path) -> bool:
    current = pathlib.Path(path.anchor)
    for component in path.parts[1:]:
        current /= component
        if current.is_symlink():
            return False
    return path.is_dir()


def _safe_member(path: str) -> bool:
    if not path or "\\" in path or path.startswith("/"):
        return False
    try:
        encoded = path.encode("utf-8")
    except UnicodeEncodeError:
        return False
    if len(encoded) > 4096 or any(ord(character) < 32 or ord(character) == 127 for character in path):
        return False
    parts = path.split("/")
    return all(
        part not in ("", ".", "..") and len(part.encode("utf-8")) <= 255
        for part in parts
    )


def _allowed_export_path(path: str) -> bool:
    if not path.startswith("exports/") or not _safe_member(path):
        return False
    name = pathlib.PurePosixPath(path).name.lower()
    if name.startswith(".env") or name.endswith(("~", ".log", ".swp", ".swo")):
        return False
    return pathlib.PurePosixPath(name).suffix in {".json", ".csv", ".txt"}


def _allowed_bundle_path(path: str) -> bool:
    if path in ("config/dev-tracker.toml", "backups/database.dump", "provenance/provenance.json"):
        return True
    return _allowed_export_path(path) and len(path.split("/")) >= 2


def _parse_as_of(value: str) -> str:
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise BundleError("--as-of must be an ISO-8601 timestamp with timezone") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise BundleError("--as-of must include a timezone")
    return parsed.astimezone(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def _export_mapping(value: str) -> tuple[str, str]:
    if "=" not in value:
        raise BundleError("--export requires RELATIVE_PATH=SOURCE_FILE")
    relative, source = value.split("=", 1)
    member = f"exports/{relative}"
    if not relative or not source or not _allowed_export_path(member):
        raise BundleError("export destination must be a safe JSON, CSV, or TXT relative path")
    return member, source


def _hash_stream(stream: BinaryIO, output: BinaryIO | None = None) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    while chunk := stream.read(CHUNK_BYTES):
        size += len(chunk)
        if size > MAX_FILE_BYTES:
            raise BundleError("bundle file exceeds the per-file limit")
        digest.update(chunk)
        if output is not None:
            output.write(chunk)
    return size, digest.hexdigest()


def _read_json(stream: BinaryIO, size: int) -> dict[str, object]:
    if size > MAX_MANIFEST_BYTES:
        raise BundleError("bundle manifest exceeds the size limit")
    try:
        value = json.loads(stream.read(size))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise BundleError("bundle manifest is not valid JSON") from error
    if not isinstance(value, dict):
        raise BundleError("bundle manifest must be an object")
    return value


class _BoundedTarReader:
    """Prevent tarfile from requesting an unbounded PAX/GNU metadata read."""

    def __init__(self, source: BinaryIO):
        self._source = source

    def read(self, size: int = -1) -> bytes:
        if size < 0 or size > MAX_TAR_READ_BYTES:
            raise BundleError("tar metadata read exceeds the bounded read limit")
        return self._source.read(size)

    def seek(self, offset: int, whence: int = os.SEEK_SET) -> int:
        return self._source.seek(offset, whence)

    def tell(self) -> int:
        return self._source.tell()


@contextmanager
def _open_archive(bundle: pathlib.Path) -> Iterator[tarfile.TarFile]:
    source = bundle.open("rb")
    try:
        with tarfile.open(fileobj=_BoundedTarReader(source), mode="r:") as archive:
            yield archive
    finally:
        source.close()


def _bundle_members(bundle: pathlib.Path) -> tuple[dict[str, object], dict[str, tarfile.TarInfo]]:
    if bundle.stat().st_size > MAX_TOTAL_BYTES:
        raise BundleError("bundle archive exceeds the size limit")
    try:
        archive_context = _open_archive(bundle)
    except (OSError, tarfile.TarError) as error:
        raise BundleError("bundle is not a readable tar archive") from error
    with archive_context as archive:
        members: dict[str, tarfile.TarInfo] = {}
        total = 0
        manifest: dict[str, object] | None = None
        for member in archive:
            if len(members) >= MAX_FILES + 1:
                raise BundleError("bundle has too many entries")
            name = member.name
            if not _safe_member(name) or name in members:
                raise BundleError("bundle contains an unsafe or duplicate path")
            if not member.isfile() or member.issym() or member.islnk():
                raise BundleError("bundle contains a non-regular file")
            if member.size < 0 or member.size > MAX_FILE_BYTES:
                raise BundleError("bundle file size is outside limits")
            total += member.size
            if total > MAX_TOTAL_BYTES:
                raise BundleError("bundle contents exceed the total size limit")
            members[name] = member
            if name == "manifest.json":
                data = archive.extractfile(member)
                if data is None:
                    raise BundleError("bundle manifest cannot be read")
                manifest = _read_json(data, member.size)
        if manifest is None:
            raise BundleError("bundle manifest is missing")
        return manifest, members


def verify_bundle(bundle: pathlib.Path) -> dict[str, object]:
    manifest, members = _bundle_members(bundle)
    required = {
        "bundle_version",
        "software_version",
        "source_commit",
        "database_schema_version",
        "as_of_utc",
        "scope",
        "files",
    }
    if (
        set(manifest) != required
        or type(manifest["bundle_version"]) is not int
        or manifest["bundle_version"] != 1
    ):
        raise BundleError("bundle manifest fields or version are unsupported")
    if not isinstance(manifest["files"], list) or not manifest["files"]:
        raise BundleError("bundle file inventory is invalid")
    if not isinstance(manifest["source_commit"], str) or not COMMIT_RE.fullmatch(manifest["source_commit"]):
        raise BundleError("bundle source commit is invalid")
    if (
        not isinstance(manifest["software_version"], str)
        or not manifest["software_version"].strip()
        or type(manifest["database_schema_version"]) is not int
        or manifest["database_schema_version"] < 1
        or not isinstance(manifest["scope"], str)
        or not manifest["scope"].strip()
        or len(manifest["scope"]) > 2048
        or len(manifest["software_version"]) > 128
    ):
        raise BundleError("bundle provenance fields are invalid")
    if not isinstance(manifest["as_of_utc"], str):
        raise BundleError("bundle as-of timestamp is invalid")
    _parse_as_of(str(manifest["as_of_utc"]))
    if len(manifest["files"]) > MAX_FILES:
        raise BundleError("bundle has too many file records")
    paths: set[str] = set()
    total_expected_size = 0
    for entry in manifest["files"]:
        if not isinstance(entry, dict) or set(entry) != {"path", "size_bytes", "sha256"}:
            raise BundleError("bundle file inventory entry is invalid")
        path, expected_size, expected_hash = entry["path"], entry["size_bytes"], entry["sha256"]
        if not isinstance(path, str) or not _allowed_bundle_path(path) or path in paths:
            raise BundleError("bundle manifest contains an unsafe or duplicate path")
        if type(expected_size) is not int or expected_size < 0 or expected_size > MAX_FILE_BYTES:
            raise BundleError("bundle manifest file size is invalid")
        total_expected_size += expected_size
        if total_expected_size > MAX_TOTAL_BYTES:
            raise BundleError("bundle manifest exceeds the total size limit")
        if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
            raise BundleError("bundle manifest SHA-256 is invalid")
        paths.add(path)
        member = members.get(path)
        if member is None or member.size != expected_size:
            raise BundleError("bundle file is absent or has a different size")
    if not {"manifest.json", *paths} == set(members):
        raise BundleError("bundle contains files outside its manifest")
    if "config/dev-tracker.toml" not in paths or "backups/database.dump" not in paths:
        raise BundleError("bundle is missing its config or PostgreSQL dump")
    if not any(path.startswith("exports/") for path in paths) or "provenance/provenance.json" not in paths:
        raise BundleError("bundle is missing exports or provenance")

    with _open_archive(bundle) as archive:
        by_name = {member.name: member for member in archive}
        for entry in manifest["files"]:
            member = by_name[entry["path"]]
            stream = archive.extractfile(member)
            if stream is None:
                raise BundleError("bundle file cannot be read")
            size, digest = _hash_stream(stream)
            if size != entry["size_bytes"] or digest != entry["sha256"]:
                raise BundleError(f"bundle checksum mismatch: {entry['path']}")
    return manifest


def pack(args: argparse.Namespace) -> None:
    if not COMMIT_RE.fullmatch(args.source_commit):
        raise BundleError("--source-commit must be a 40-character lowercase git SHA")
    if args.database_schema_version < 1:
        raise BundleError("--database-schema-version must be positive")
    if (
        not args.software_version.strip()
        or len(args.software_version) > 128
        or not args.scope.strip()
        or len(args.scope) > 2048
    ):
        raise BundleError("--software-version and --scope must be nonempty")
    as_of = _parse_as_of(args.as_of)
    output = pathlib.Path(args.output).absolute()
    if output.exists() or output.is_symlink() or not _directory_without_symlinks(output.parent):
        raise BundleError("output must be a new file in an existing directory")
    inputs: dict[str, str] = {
        "config/dev-tracker.toml": args.config,
        "backups/database.dump": args.db_dump,
        "provenance/provenance.json": args.provenance,
    }
    for export in args.export:
        destination, source = _export_mapping(export)
        if destination in inputs or destination in ("manifest.json",):
            raise BundleError("duplicate bundle path")
        inputs[destination] = source
    if len(inputs) > MAX_FILES:
        raise BundleError("bundle has too many input files")
    if not any(path.startswith("exports/") for path in inputs):
        raise BundleError("at least one --export is required")

    inventory: list[dict[str, object]] = []
    sources: dict[str, pathlib.Path] = {}
    total_size = 0
    for destination, source in sorted(inputs.items()):
        path, size, digest = _regular_file(source)
        if destination == "backups/database.dump":
            with path.open("rb") as dump:
                if dump.read(5) != b"PGDMP":
                    raise BundleError("database dump is not a PostgreSQL custom-format archive")
        if destination == "provenance/provenance.json":
            if size > MAX_MANIFEST_BYTES:
                raise BundleError("provenance input exceeds the size limit")
            try:
                provenance = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
                raise BundleError("provenance input must be valid UTF-8 JSON") from error
            if not isinstance(provenance, dict):
                raise BundleError("provenance input must contain a JSON object")
        total_size += size
        if total_size > MAX_TOTAL_BYTES:
            raise BundleError("bundle contents exceed the total size limit")
        sources[destination] = path
        inventory.append({"path": destination, "size_bytes": size, "sha256": digest})
    manifest: dict[str, object] = {
        "bundle_version": 1,
        "software_version": args.software_version.strip(),
        "source_commit": args.source_commit,
        "database_schema_version": args.database_schema_version,
        "as_of_utc": as_of,
        "scope": args.scope.strip(),
        "files": inventory,
    }
    manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    if len(manifest_bytes) > MAX_MANIFEST_BYTES:
        raise BundleError("bundle manifest exceeds the size limit")

    descriptor = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as destination_file:
            with tarfile.open(fileobj=destination_file, mode="w", format=tarfile.PAX_FORMAT) as archive:
                manifest_info = tarfile.TarInfo("manifest.json")
                manifest_info.size = len(manifest_bytes)
                manifest_info.mode = 0o600
                archive.addfile(manifest_info, fileobj=io.BytesIO(manifest_bytes))
                for destination, source in sources.items():
                    metadata = source.stat()
                    info = tarfile.TarInfo(destination)
                    info.size = metadata.st_size
                    info.mode = 0o600
                    with source.open("rb") as file_stream:
                        archive.addfile(info, fileobj=file_stream)
            destination_file.flush()
            os.fsync(destination_file.fileno())
        verify_bundle(output)
    except BaseException:
        try:
            output.unlink()
        except OSError:
            pass
        raise


def _extract_verified(bundle: pathlib.Path, output: pathlib.Path, manifest: dict[str, object]) -> None:
    if output.exists() or output.is_symlink() or not _directory_without_symlinks(output.parent):
        raise BundleError("extraction target must be a new directory in an existing directory")
    output.mkdir(mode=0o700)
    try:
        with _open_archive(bundle) as archive:
            by_name = {member.name: member for member in archive}
            for entry in manifest["files"]:
                relative = pathlib.PurePosixPath(entry["path"])
                destination = output.joinpath(*relative.parts)
                destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                descriptor = os.open(destination, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                with os.fdopen(descriptor, "wb") as sink:
                    source = archive.extractfile(by_name[entry["path"]])
                    if source is None:
                        raise BundleError("verified bundle file cannot be reopened")
                    size, digest = _hash_stream(source, sink)
                    if size != entry["size_bytes"] or digest != entry["sha256"]:
                        raise BundleError(f"bundle changed during extraction: {entry['path']}")
            manifest_member = by_name["manifest.json"]
            if manifest_member.size > MAX_MANIFEST_BYTES:
                raise BundleError("bundle manifest exceeds the size limit")
            source = archive.extractfile(manifest_member)
            if source is None:
                raise BundleError("verified bundle manifest cannot be reopened")
            manifest_bytes = source.read(manifest_member.size)
            try:
                extracted_manifest = json.loads(manifest_bytes)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise BundleError("verified bundle manifest is not valid JSON") from error
            if extracted_manifest != manifest:
                raise BundleError("bundle manifest changed during extraction")
            manifest_path = output / "manifest.json"
            descriptor = os.open(manifest_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(descriptor, "wb") as sink:
                sink.write(manifest_bytes)
        if stat.S_IMODE(output.stat().st_mode) != 0o700:
            output.chmod(0o700)
    except BaseException:
        shutil.rmtree(output, ignore_errors=True)
        raise


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("pack", help="pack explicit state files into a private tar bundle")
    create.add_argument("--config", required=True)
    create.add_argument("--db-dump", required=True)
    create.add_argument("--provenance", required=True)
    create.add_argument("--export", action="append", default=[], metavar="RELATIVE_PATH=SOURCE_FILE")
    create.add_argument("--output", required=True)
    create.add_argument("--software-version", required=True)
    create.add_argument("--source-commit", required=True)
    create.add_argument("--database-schema-version", type=int, required=True)
    create.add_argument("--as-of", required=True)
    create.add_argument("--scope", required=True)
    create.set_defaults(run=pack)
    for command in ("verify", "extract"):
        verify = commands.add_parser(command)
        verify.add_argument("--bundle", required=True)
        if command == "extract":
            verify.add_argument("--output", required=True)
        verify.set_defaults(run=None)
    return parser


def main() -> int:
    args = _parser().parse_args()
    try:
        if args.command == "pack":
            args.run(args)
            bundle = pathlib.Path(args.output)
            manifest = verify_bundle(bundle)
        else:
            bundle = pathlib.Path(args.bundle)
            manifest = verify_bundle(bundle)
            if args.command == "extract":
                _extract_verified(bundle, pathlib.Path(args.output).absolute(), manifest)
        print(
            json.dumps(
                {
                    "result": "verified",
                    "software_version": manifest["software_version"],
                    "source_commit": manifest["source_commit"],
                    "database_schema_version": manifest["database_schema_version"],
                    "file_count": len(manifest["files"]),
                },
                sort_keys=True,
            )
        )
        return 0
    except (BundleError, OSError, tarfile.TarError) as error:
        print(f"state bundle: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
