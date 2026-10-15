import io
import json
import pathlib
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest

from state_bundle import BundleError, _export_mapping, _safe_member, verify_bundle

HELPER = pathlib.Path(__file__).with_name("state_bundle.py")
COMMIT = "0123456789abcdef0123456789abcdef01234567"


class StateBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="scout-state-bundle-test-")
        self.root = pathlib.Path(self.temp.name)
        self.sources = self.root / "sources"
        self.sources.mkdir()
        (self.sources / "config.toml").write_text("pending_window_days = 3\n")
        (self.sources / "database.dump").write_bytes(b"PGDMPsynthetic-custom-dump")
        (self.sources / "provenance.json").write_text('{"snapshot": "fixture"}\n')
        (self.sources / "export.json").write_text('{"result": "fixture"}\n')
        self.bundle = self.root / "private-state.tar"
        self.extracted = self.root / "restored"

    def tearDown(self):
        self.temp.cleanup()

    def pack(self, *, config=None, output=None):
        return subprocess.run(
            [
                sys.executable,
                str(HELPER),
                "pack",
                "--config",
                str(config or self.sources / "config.toml"),
                "--db-dump",
                str(self.sources / "database.dump"),
                "--provenance",
                str(self.sources / "provenance.json"),
                "--export",
                f"categories/base/runners.json={self.sources / 'export.json'}",
                "--output",
                str(output or self.bundle),
                "--software-version",
                "v1.2.3",
                "--source-commit",
                COMMIT,
                "--database-schema-version",
                "10",
                "--as-of",
                "2026-10-11T12:00:00Z",
                "--scope",
                "synthetic fixture scope",
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_round_trip_manifest_hashes_and_private_permissions(self):
        packed = self.pack()
        self.assertEqual(packed.returncode, 0, packed.stderr)
        self.assertEqual(stat.S_IMODE(self.bundle.stat().st_mode), 0o600)
        manifest = verify_bundle(self.bundle)
        self.assertEqual(manifest["database_schema_version"], 10)
        self.assertEqual(manifest["scope"], "synthetic fixture scope")
        self.assertNotIn(str(self.sources), json.dumps(manifest))
        paths = {entry["path"] for entry in manifest["files"]}
        self.assertEqual(
            paths,
            {
                "config/dev-tracker.toml",
                "backups/database.dump",
                "provenance/provenance.json",
                "exports/categories/base/runners.json",
            },
        )
        self.assertNotIn("deploy/.env", paths)
        output = subprocess.run(
            [sys.executable, str(HELPER), "extract", "--bundle", str(self.bundle), "--output", str(self.extracted)],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertEqual(
            (self.extracted / "config/dev-tracker.toml").read_bytes(),
            (self.sources / "config.toml").read_bytes(),
        )
        self.assertEqual(
            (self.extracted / "backups/database.dump").read_bytes(),
            (self.sources / "database.dump").read_bytes(),
        )
        export = self.extracted / "exports/categories/base/runners.json"
        self.assertEqual(export.read_bytes(), (self.sources / "export.json").read_bytes())
        self.assertEqual(stat.S_IMODE(self.extracted.stat().st_mode), 0o700)
        extracted_manifest = self.extracted / "manifest.json"
        self.assertEqual(json.loads(extracted_manifest.read_text()), manifest)
        self.assertEqual(stat.S_IMODE(extracted_manifest.stat().st_mode), 0o600)
        for path in self.extracted.rglob("*"):
            if path.is_file():
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_refuses_existing_targets_and_symlink_inputs(self):
        self.assertEqual(self.pack().returncode, 0)
        overwrite = self.pack()
        self.assertNotEqual(overwrite.returncode, 0)
        self.assertIn("new file", overwrite.stderr)
        existing_dir = self.root / "exists"
        existing_dir.mkdir()
        extract = subprocess.run(
            [sys.executable, str(HELPER), "extract", "--bundle", str(self.bundle), "--output", str(existing_dir)],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(extract.returncode, 0)
        link = self.sources / "config-link.toml"
        link.symlink_to(self.sources / "config.toml")
        symlink_pack = self.pack(config=link, output=self.root / "symlink.tar")
        self.assertNotEqual(symlink_pack.returncode, 0)
        self.assertIn("symlink", symlink_pack.stderr)

    def test_verifier_rejects_traversal_links_and_content_tampering(self):
        with tarfile.open(self.root / "traversal.tar", "w") as archive:
            info = tarfile.TarInfo("../escaped")
            info.size = 1
            archive.addfile(info, fileobj=io.BytesIO(b"x"))
        with self.assertRaises(BundleError):
            verify_bundle(self.root / "traversal.tar")

        with tarfile.open(self.root / "symlink.tar", "w") as archive:
            link = tarfile.TarInfo("config/dev-tracker.toml")
            link.type = tarfile.SYMTYPE
            link.linkname = "/etc/passwd"
            archive.addfile(link)
        with self.assertRaises(BundleError):
            verify_bundle(self.root / "symlink.tar")

        huge_pax = self.root / "huge-pax.tar"
        pax_header = tarfile.TarInfo("PaxHeaders/oversized")
        pax_header.type = tarfile.XHDTYPE
        pax_header.size = 8 * 1024 * 1024
        huge_pax.write_bytes(pax_header.tobuf(format=tarfile.USTAR_FORMAT))
        with self.assertRaisesRegex(BundleError, "bounded read limit"):
            verify_bundle(huge_pax)

        self.assertEqual(self.pack().returncode, 0)
        altered = self.root / "altered.tar"
        data = self.bundle.read_bytes().replace(b'{"result": "fixture"}', b'{"result": "changed"}')
        self.assertNotEqual(data, self.bundle.read_bytes())
        altered.write_bytes(data)
        with self.assertRaises(BundleError):
            verify_bundle(altered)

    def test_export_allowlist_rejects_secret_and_log_filenames(self):
        for destination in (
            ".env",
            ".env.json",
            "nested/.env.production.txt",
            "trace.log",
            "state.swp",
            "backup.json~",
            "payload.yaml",
        ):
            with self.subTest(destination=destination), self.assertRaises(BundleError):
                _export_mapping(f"{destination}={self.sources / 'export.json'}")
        self.assertEqual(
            _export_mapping(f"nested/result.csv={self.sources / 'export.json'}")[0],
            "exports/nested/result.csv",
        )

    def test_member_paths_reject_controls_and_excessive_utf8_lengths(self):
        for path in (
            "exports/name\nreport.json",
            "exports/name\x7freport.json",
            "exports/" + "é" * 128 + ".json",
            "exports/" + "/".join(["x" * 200] * 21) + "/report.json",
        ):
            with self.subTest(path=path[:80]), self.assertRaises(BundleError):
                _export_mapping(f"{path.removeprefix('exports/')}={self.sources / 'export.json'}")
        self.assertFalse(_safe_member("exports/" + "é" * 128 + ".json"))


if __name__ == "__main__":
    unittest.main()
