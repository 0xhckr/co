#!/usr/bin/env python3
"""Exercise package boundary rejection; synthetic PE bytes are not native proof."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
import warnings
import zipfile

spec = importlib.util.spec_from_file_location("packages", Path(__file__).with_name("windows-packages.py"))
packages = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packages)


class Packages(unittest.TestCase):
    def fixture(self, root, **overrides):
        binary = bytearray(128)
        binary[:2] = b"MZ"
        struct.pack_into("<I", binary, 60, 64)
        binary[64:68] = b"PE\0\0"
        struct.pack_into("<H", binary, 68, 0x8664)
        metadata = {"version": "1.2.3", "target": packages.TARGET,
                    "sourceCommit": "a" * 40, "binarySha256": packages.sha256(binary)}
        metadata.update(overrides)
        archive = root / packages.ASSET
        with zipfile.ZipFile(archive, "w") as package:
            package.writestr("co.exe", binary)
            package.writestr("co-release.json", json.dumps(metadata))
            for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE", "install.md"]:
                package.writestr(name, "fixture only")
        packages.checksum_file(archive)
        return archive

    def test_manager_outputs_bind_the_same_archive_and_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            output = root / "output"
            packages.generate("v1.2.3", root, output)
            metadata = json.loads((output / "windows-packages.json").read_text())
            digest = metadata["archiveSha256"]
            url = metadata["installerUrl"]
            self.assertEqual(metadata["sourceCommit"], "a" * 40)
            self.assertFalse(metadata["fixture"])
            scoop = json.loads((output / "co-codes-cli.json").read_text())
            self.assertEqual(scoop["architecture"]["64bit"], {"hash": digest, "url": url})
            self.assertIn(digest.upper(), (output / "winget/CoCodes.Co.installer.yaml").read_text())
            self.assertIn(url, (output / "chocolatey/tools/chocolateyinstall.ps1").read_text())
            self.assertIn(digest, (output / "chocolatey/tools/chocolateyinstall.ps1").read_text())

    def test_missing_or_tampered_assets_never_emit_manifests(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "output"
            with self.assertRaises(FileNotFoundError):
                packages.generate("1.2.3", root, output)
            archive = self.fixture(root)
            archive.write_bytes(archive.read_bytes() + b"tampered")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                packages.generate("1.2.3", root, output)
            self.assertFalse(output.exists())

    def test_wrong_version_architecture_binary_and_source_fail_closed(self):
        for invalid in [{"version": "1.2.4"}, {"target": "aarch64-pc-windows-msvc"},
                        {"binarySha256": "0" * 64}, {"sourceCommit": "short"}]:
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.fixture(root, **invalid)
                with self.assertRaises(ValueError):
                    packages.generate("1.2.3", root, root / "output")
                self.assertFalse((root / "output").exists())

    def test_fixture_urls_and_versions_cannot_leak_into_release_mode(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            for origin in ["https://evil.example", "http://localhost:8000", "http://127.0.0.1:8000/a'bad"]:
                with self.assertRaises(ValueError):
                    packages.generate("1.2.3", root, root / "output", origin)
            with self.assertRaises(ValueError):
                packages.generate("1.2.3", root, root / "output", fixture_version="0.0.1")
            packages.generate("1.2.3", root, root / "output", "http://127.0.0.1:8000", "0.0.1")
            self.assertTrue(json.loads((root / "output/windows-packages.json").read_text())["fixture"])

    def test_archive_layout_and_actual_pe_machine_must_match(self):
        for member in ['../co.exe', 'co.exe']:
            with self.subTest(member=member), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive = self.fixture(root)
                with warnings.catch_warnings():
                    warnings.filterwarnings('ignore', message="Duplicate name: 'co.exe'", category=UserWarning)
                    with zipfile.ZipFile(archive, 'a') as package:
                        package.writestr(member, b'not executable')
                packages.checksum_file(archive)
                with self.assertRaisesRegex(ValueError, 'unexpected or duplicate'):
                    packages.generate('1.2.3', root, root / 'output')
        with self.assertRaisesRegex(ValueError, 'native x64'):
            binary = bytearray(128)
            binary[:2] = b'MZ'
            struct.pack_into('<I', binary, 60, 64)
            binary[64:68] = b'PE\0\0'
            struct.pack_into('<H', binary, 68, 0xAA64)
            packages.check_pe(binary)


if __name__ == "__main__":
    unittest.main()
