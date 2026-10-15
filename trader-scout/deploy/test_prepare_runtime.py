import pathlib
import stat
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("portable") / "prepare-runtime.sh"
class PrepareRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="scout-runtime-setup-test-")
        self.root = pathlib.Path(self.temp.name) / "runtime"
        (self.root / "deploy/portable").mkdir(parents=True)
        (self.root / "config").mkdir()
        (self.root / "config/dev-tracker.example.toml").write_text("fixture = true\n")
        (self.root / "deploy/.env.example").write_text("SCOUT_POSTGRES_VOLUME=fixture\n")
        self.script = self.root / "deploy/portable/prepare-runtime.sh"
        self.script.write_bytes(SCRIPT.read_bytes())
        self.script.chmod(0o755)
        self.bin = pathlib.Path(self.temp.name) / "bin"
        self.bin.mkdir()
        self._fake_command("docker", 'printf "%s\\n" "$SCOUT_DOCKER_OPTIONS"\n')
        self._fake_command("id", 'printf "%s\\n" "$SCOUT_TEST_UID"\n')

    def tearDown(self):
        self.temp.cleanup()

    def _fake_command(self, name, body):
        script = self.bin / name
        script.write_text(f"#!/bin/sh\n{body}")
        script.chmod(0o755)

    def _run(self, *, options, uid="1234", gid="5678"):
        env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "SCOUT_DOCKER_OPTIONS": options,
            "SCOUT_TEST_UID": uid,
            "SCOUT_TEST_GID": gid,
        }
        # The fake id returns the configured uid for -u and gid for -g.
        self._fake_command(
            "id",
            'if [ "$1" = "-u" ]; then printf "%s\\n" "$SCOUT_TEST_UID"; '
            'else printf "%s\\n" "$SCOUT_TEST_GID"; fi\n',
        )
        return subprocess.run(["bash", str(self.script)], env=env, capture_output=True, text=True)

    def test_selects_identity_for_rootless_and_rootful_docker(self):
        rootless = self._run(options='["name=rootless"]')
        self.assertEqual(rootless.returncode, 0, rootless.stderr)
        rootless_env = (self.root / "deploy/.env").read_text()
        self.assertTrue(rootless_env.endswith("SCOUT_CONTAINER_USER=0:0\n"))
        self.assertEqual(stat.S_IMODE((self.root / "deploy/.env").stat().st_mode), 0o600)
        self.assertIn("container user: 0:0", rootless.stdout)
        self.assertTrue((self.root / "exports").is_dir())
        self.assertTrue((self.root / "backups").is_dir())
        self.assertTrue((self.root / "config/dev-tracker.toml").is_file())

        (self.root / "deploy/.env").unlink()
        rootful = self._run(options='["name=seccomp,profile=default"]')
        self.assertEqual(rootful.returncode, 0, rootful.stderr)
        self.assertTrue((self.root / "deploy/.env").read_text().endswith("SCOUT_CONTAINER_USER=1234:5678\n"))

    def test_refuses_existing_env_and_rootful_host_root(self):
        existing = self.root / "deploy/.env"
        existing.write_text("private=unchanged\n")
        refused = self._run(options='["name=rootless"]')
        self.assertNotEqual(refused.returncode, 0)
        self.assertEqual(existing.read_text(), "private=unchanged\n")

        existing.unlink()
        rootful_root = self._run(options='["name=seccomp"]', uid="0", gid="0")
        self.assertNotEqual(rootful_root.returncode, 0)
        self.assertIn("non-root runtime owner", rootful_root.stderr)
        self.assertFalse(existing.exists())


if __name__ == "__main__":
    unittest.main()
