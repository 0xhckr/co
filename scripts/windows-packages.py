#!/usr/bin/env python3
"""Build Windows archives and generate packages only from verified archive bytes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import zipfile

TARGET = "x86_64-pc-windows-msvc"
ASSET = f"co-{TARGET}.zip"
RELEASES = "https://github.com/codotcodes/co/releases/download"
ROOT = Path(__file__).resolve().parent.parent


def stable_version(value):
    if not re.fullmatch(r"v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value):
        raise ValueError("expected a stable vX.Y.Z version")
    return value.removeprefix("v")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def check_pe(binary):
    if len(binary) < 64 or binary[:2] != b"MZ":
        raise ValueError("co.exe is not a Windows PE executable")
    offset = struct.unpack_from("<I", binary, 60)[0]
    if offset > len(binary) - 6 or binary[offset:offset + 4] != b"PE\0\0":
        raise ValueError("invalid PE header")
    if struct.unpack_from("<H", binary, offset + 4)[0] != 0x8664:
        raise ValueError("co.exe must be a native x64 executable")


def checksum_file(path):
    path.with_name(path.name + ".sha256").write_text(
        f"{sha256(path.read_bytes())}  {path.name}\n", encoding="utf-8")


def build_archive(version, executable, source, output):
    version = stable_version(version)
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("source must be a full commit OID")
    binary = executable.read_bytes()
    check_pe(binary)
    actual = subprocess.check_output([str(executable.resolve()), "version"], text=True).strip()
    if actual != f"co {version}":
        raise ValueError(f"binary version does not match tag: {actual}")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / ASSET
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as package:
        package.writestr("co.exe", binary)
        for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"]:
            package.write(ROOT / name, name)
        package.write(ROOT / "docs/install.md", "install.md")
        package.writestr("co-release.json", json.dumps({
            "version": version, "target": TARGET, "sourceCommit": source,
            "binarySha256": sha256(binary)
        }, indent=2) + "\n")
    checksum_file(archive)


def verify_archive(version, directory):
    version = stable_version(version)
    archive = directory / ASSET
    digest = sha256(archive.read_bytes())
    if (directory / (ASSET + ".sha256")).read_text().split() != [digest, ASSET]:
        raise ValueError(f"checksum mismatch for {ASSET}")
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        if len(set(names)) != len(names) or set(names) != {
            "co.exe", "README.md", "LICENSE-MIT", "LICENSE-APACHE", "install.md", "co-release.json"
        }:
            raise ValueError("unexpected or duplicate files in Windows archive")
        binary = package.read("co.exe")
        check_pe(binary)
        metadata = json.loads(package.read("co-release.json"))
        if metadata.get("version") != version or metadata.get("target") != TARGET:
            raise ValueError("archive version or architecture does not match")
        if metadata.get("binarySha256") != sha256(binary):
            raise ValueError("binary checksum does not match archive metadata")
        if not re.fullmatch(r"[0-9a-f]{40}", metadata.get("sourceCommit", "")):
            raise ValueError("archive lacks its exact source commit")
    return version, digest, metadata


def generate(version, directory, output, fixture_origin=None, fixture_version=None):
    version, digest, metadata = verify_archive(version, directory)
    package_version = stable_version(fixture_version) if fixture_version else version
    if fixture_origin:
        if not re.fullmatch(r"http://127\.0\.0\.1:[1-9]\d{0,4}", fixture_origin):
            raise ValueError("fixtures require a loopback HTTP origin")
        url = f"{fixture_origin}/{ASSET}"
    else:
        if fixture_version:
            raise ValueError("fixture version requires an explicit fixture origin")
        url = f"{RELEASES}/v{version}/{ASSET}"
    # Validate everything before creating any package metadata.
    output.mkdir(parents=True, exist_ok=True)
    scoop = {
        "version": package_version, "description": "Human command-line client for co.codes",
        "homepage": "https://co.codes", "license": "MIT OR Apache-2.0",
        "architecture": {"64bit": {"url": url, "hash": digest}}, "bin": "co.exe"
    }
    if not fixture_origin:
        scoop["checkver"] = {"github": "https://github.com/codotcodes/co"}
        scoop["autoupdate"] = {"architecture": {"64bit": {
            "url": f"{RELEASES}/v$version/{ASSET}", "hash": {"url": "$url.sha256"}
        }}}
    (output / "co-codes-cli.json").write_text(json.dumps(scoop, indent=4) + "\n", encoding="utf-8")
    winget = output / "winget"
    winget.mkdir(exist_ok=True)
    common = f"PackageIdentifier: CoCodes.Co\nPackageVersion: {package_version}\n"
    schema = "# yaml-language-server: $schema=https://aka.ms/winget-manifest.{}.1.12.0.schema.json\n\n"
    (winget / "CoCodes.Co.yaml").write_text(schema.format("version") + common + "DefaultLocale: en-US\nManifestType: version\nManifestVersion: 1.12.0\n", encoding="utf-8")
    (winget / "CoCodes.Co.locale.en-US.yaml").write_text(schema.format("defaultLocale") + common + """PackageLocale: en-US
Publisher: co.codes
PublisherUrl: https://co.codes
PackageName: co
PackageUrl: https://github.com/codotcodes/co
License: MIT OR Apache-2.0
LicenseUrl: https://github.com/codotcodes/co/blob/main/LICENSE-MIT
ShortDescription: Human command-line client for co.codes
Moniker: co-codes-cli
Tags:
- git
- cli
ManifestType: defaultLocale
ManifestVersion: 1.12.0
""", encoding="utf-8")
    (winget / "CoCodes.Co.installer.yaml").write_text(schema.format("installer") + common + f"""InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: co.exe
  PortableCommandAlias: co
MinimumOSVersion: 10.0.17763.0
UpgradeBehavior: install
Commands:
- co
Installers:
- Architecture: x64
  InstallerUrl: {url}
  InstallerSha256: {digest.upper()}
ManifestType: installer
ManifestVersion: 1.12.0
""", encoding="utf-8")
    chocolatey = output / "chocolatey"
    tools = chocolatey / "tools"
    tools.mkdir(parents=True, exist_ok=True)
    (chocolatey / "co-codes-cli.nuspec").write_text(f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd">
  <metadata>
    <id>co-codes-cli</id>
    <version>{package_version}</version>
    <title>co CLI</title>
    <authors>co.codes</authors>
    <owners>co.codes</owners>
    <projectUrl>https://co.codes</projectUrl>
    <projectSourceUrl>https://github.com/codotcodes/co</projectSourceUrl>
    <packageSourceUrl>https://github.com/codotcodes/co/blob/main/scripts/windows-packages.py</packageSourceUrl>
    <licenseUrl>https://github.com/codotcodes/co/blob/main/LICENSE-MIT</licenseUrl>
    <requireLicenseAcceptance>false</requireLicenseAcceptance>
    <summary>Human command-line client for co.codes</summary>
    <description>Authorize this machine, inspect repositories, and authenticate Git with the co.codes command-line client.</description>
    <releaseNotes>https://github.com/codotcodes/co/releases/tag/v{version}</releaseNotes>
    <tags>co codes git cli</tags>
  </metadata>
  <files><file src="tools\\**" target="tools" /></files>
</package>
""", encoding="utf-8")
    (tools / "chocolateyinstall.ps1").write_text(f"""$ErrorActionPreference = 'Stop'
if ((Get-OSArchitectureWidth) -ne 64) {{ throw 'co requires 64-bit Windows' }}
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
Install-ChocolateyZipPackage -PackageName 'co-codes-cli' -Url64bit '{url}' -Checksum64 '{digest}' -ChecksumType64 'sha256' -UnzipLocation $toolsDir
""", encoding="utf-8")
    (tools / "chocolateyuninstall.ps1").write_text(f"""$ErrorActionPreference = 'Stop'
Uninstall-ChocolateyZipPackage -PackageName 'co-codes-cli' -ZipFileName '{ASSET}'
""", encoding="utf-8")
    with zipfile.ZipFile(output / "co-winget.zip", "w", zipfile.ZIP_DEFLATED) as package:
        for path in sorted(winget.iterdir()):
            package.write(path, path.name)
    checksum_file(output / "co-winget.zip")
    checksum_file(output / "co-codes-cli.json")
    (output / "windows-packages.json").write_text(json.dumps({
        **metadata, "archive": ASSET, "archiveSha256": digest, "installerUrl": url,
        "packageVersion": package_version, "fixture": bool(fixture_origin)
    }, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    archive = commands.add_parser("archive")
    archive.add_argument("version")
    archive.add_argument("executable", type=Path)
    archive.add_argument("source")
    archive.add_argument("output", type=Path)
    manifests = commands.add_parser("manifests")
    manifests.add_argument("version")
    manifests.add_argument("directory", type=Path)
    manifests.add_argument("output", type=Path)
    manifests.add_argument("--fixture-origin")
    manifests.add_argument("--fixture-version")
    args = parser.parse_args()
    try:
        if args.command == "archive":
            build_archive(args.version, args.executable, args.source, args.output)
        else:
            generate(args.version, args.directory, args.output, args.fixture_origin, args.fixture_version)
    except (ValueError, OSError, KeyError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        sys.exit(f"windows-packages: {error}")
