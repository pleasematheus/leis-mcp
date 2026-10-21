import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.manifest = self.root / "Cargo.toml"
        self.lock = self.root / "Cargo.lock"
        self.set_version("0.1.1")
        (self.root / "README.md").write_text("Instrucoes de uso", encoding="utf-8")
        (self.root / "CHANGELOG.md").write_text("Alteracoes", encoding="utf-8")
        (self.root / "LICENSE").write_text("MIT License", encoding="utf-8")

    def set_version(self, value):
        self.manifest.write_text(f'[package]\nname = "leis-mcp"\nversion = "{value}"\n', encoding="utf-8")
        self.lock.write_text(f'version = 4\n[[package]]\nname = "leis-mcp"\nversion = "{value}"\n', encoding="utf-8")

    def binary(self, target):
        path = self.root / "target" / target / "release" / release.TARGETS[target][1]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"binary-fixture")
        return path

    def test_manifest_lock_and_tag_must_match(self):
        self.assertEqual(release.version(self.root, "v0.1.1"), "0.1.1")
        with self.assertRaises(ValueError):
            release.version(self.root, "v0.1.0")
        self.lock.write_text('[[package]]\nname = "leis-mcp"\nversion = "0.1.0"\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            release.version(self.root)

    def test_semver_accepts_prerelease_and_rejects_invalid_numbers(self):
        for value in ["0.1.1", "1.0.0-rc.1", "1.2.3+build.42", "1.2.3-alpha.0"]:
            with self.subTest(value=value):
                self.set_version(value)
                self.assertEqual(release.version(self.root), value)
        for value in ["01.1.1", "1.01.0", "1.0", "v1.0.0", "1.0.0-01", "1.0.0-", "1.0.0\n"]:
            with self.subTest(value=value):
                # Um manifesto com uma quebra de linha literal tambem e invalido.
                self.set_version(value)
                with self.assertRaises(ValueError):
                    release.version(self.root)

    def test_package_requires_compiled_target(self):
        with self.assertRaisesRegex(ValueError, "Compile primeiro"):
            release.package("x86_64-pc-windows-msvc", self.root)

    def test_each_package_has_only_binary_instructions_and_configuration(self):
        private = self.root / "data-teste"
        private.mkdir()
        (private / "acervo.db").write_bytes(b"do-not-distribute")
        for target, (_, binary_name, suffix) in release.TARGETS.items():
            with self.subTest(target=target):
                self.binary(target)
                archive = release.package(target, self.root)
                expected = {binary_name, "README.md", "CHANGELOG.md", "LICENSE", "mcp-config.example.json", "LEIA-ME.txt"}
                if suffix == ".zip":
                    with zipfile.ZipFile(archive) as handle:
                        names = handle.namelist()
                        config_bytes = handle.read(next(n for n in names if n.endswith("mcp-config.example.json")))
                else:
                    with tarfile.open(archive) as handle:
                        members = [item for item in handle.getmembers() if item.isfile()]
                        names = [item.name for item in members]
                        config_bytes = handle.extractfile(next(m for m in members if m.name.endswith("mcp-config.example.json"))).read()
                        binary = next(m for m in members if Path(m.name).name == binary_name)
                        self.assertEqual(binary.mode & 0o777, 0o755)
                self.assertEqual({Path(n).name for n in names}, expected)
                self.assertTrue(all(len(Path(n).parts) == 2 for n in names))
                config = json.loads(config_bytes)["mcpServers"]["leis"]
                self.assertTrue(config["command"].endswith(binary_name))
                self.assertEqual(config["args"][0], "--data-dir")
                self.assertEqual(config["args"][-1], "stdio")
                self.assertEqual(archive.with_name(archive.name + ".sha256").read_text(encoding="ascii"),
                                 f"{release.digest(archive)}  {archive.name}\n")

    def test_checksums_require_all_platforms_and_detect_tampering(self):
        with self.assertRaisesRegex(ValueError, "Pacote ausente"):
            release.checksums(self.root)
        for target in release.TARGETS:
            self.binary(target)
            release.package(target, self.root)
        checksums = release.checksums(self.root)
        self.assertEqual(len(checksums.read_text(encoding="ascii").splitlines()), 4)
        archive = self.root / "dist" / release.package_name("0.1.1", "x86_64-pc-windows-msvc")
        archive.write_bytes(archive.read_bytes() + b"tampered")
        with self.assertRaisesRegex(ValueError, "Checksum invalido"):
            release.checksums(self.root)

    def test_smoke_checks_extracted_executable_and_fresh_database(self):
        target = "x86_64-pc-windows-msvc"
        source = self.binary(target)
        release.package(target, self.root)
        responses = [mock.Mock(stdout="leis-mcp 0.1.1\n"), mock.Mock(stdout='{"normas":0,"commit":null}')]
        with mock.patch("release.subprocess.run", side_effect=responses) as run:
            release.smoke(target, self.root)
            first, second = [call.args[0] for call in run.call_args_list]
            self.assertNotEqual(Path(first[0]), source)
            self.assertEqual(first[1:], ["--version"])
            self.assertEqual(second[1], "--data-dir")
            self.assertEqual(second[-1], "status")
            self.assertFalse(Path(second[2]).exists(), "Os dados do smoke devem ser temporarios")

    def test_smoke_rejects_wrong_executable_version(self):
        target = "x86_64-pc-windows-msvc"
        self.binary(target)
        release.package(target, self.root)
        with mock.patch("release.subprocess.run", return_value=mock.Mock(stdout="leis-mcp 0.1.0\n")):
            with self.assertRaisesRegex(ValueError, "Versao do executavel"):
                release.smoke(target, self.root)


if __name__ == "__main__":
    unittest.main()
