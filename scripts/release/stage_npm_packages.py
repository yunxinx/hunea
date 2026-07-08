#!/usr/bin/env python3
"""Stage and pack hunea npm packages from release binary archives."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
NPM_ROOT = REPO_ROOT / "npm" / "hunea"
ROOT_PACKAGE_NAME = "hunea"


@dataclass(frozen=True)
class PlatformPackage:
    package_name: str
    npm_tag: str
    target: str
    os_name: str
    cpu: str

    @property
    def executable_name(self) -> str:
        return "hunea.exe" if self.os_name == "win32" else "hunea"


PLATFORM_PACKAGES: tuple[PlatformPackage, ...] = (
    PlatformPackage(
        "hunea-linux-x64",
        "linux-x64",
        "x86_64-unknown-linux-musl",
        "linux",
        "x64",
    ),
    PlatformPackage(
        "hunea-linux-arm64",
        "linux-arm64",
        "aarch64-unknown-linux-musl",
        "linux",
        "arm64",
    ),
    PlatformPackage(
        "hunea-darwin-x64",
        "darwin-x64",
        "x86_64-apple-darwin",
        "darwin",
        "x64",
    ),
    PlatformPackage(
        "hunea-darwin-arm64",
        "darwin-arm64",
        "aarch64-apple-darwin",
        "darwin",
        "arm64",
    ),
    PlatformPackage(
        "hunea-win32-x64",
        "win32-x64",
        "x86_64-pc-windows-msvc",
        "win32",
        "x64",
    ),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-version",
        required=True,
        help="SemVer version to write into package.json.",
    )
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        required=True,
        help="Directory containing hunea release archives.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="Directory where npm tarballs should be written.",
    )
    parser.add_argument(
        "--staging-root",
        type=Path,
        help="Optional directory for staged package contents. Must not already exist.",
    )
    parser.add_argument(
        "--keep-staging",
        action="store_true",
        help="Keep staged package directories for inspection.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    artifacts_dir = args.artifacts_dir.resolve()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    if args.staging_root:
        staging_root = args.staging_root.resolve()
        if staging_root.exists():
            raise FileExistsError(f"staging root already exists: {staging_root}")
        staging_root.mkdir(parents=True)
        remove_staging = False
    else:
        staging_root = Path(tempfile.mkdtemp(prefix="hunea-npm-stage-"))
        remove_staging = not args.keep_staging

    try:
        platform_tarballs = []
        for platform_package in PLATFORM_PACKAGES:
            package_dir = staging_root / platform_package.package_name
            stage_platform_package(
                package_dir,
                platform_package,
                args.release_version,
                artifacts_dir,
            )
            platform_tarballs.append(
                run_npm_pack(
                    package_dir,
                    output_dir,
                    f"{platform_package.package_name}-{args.release_version}.tgz",
                )
            )

        root_dir = staging_root / ROOT_PACKAGE_NAME
        stage_root_package(root_dir, args.release_version, artifacts_dir)
        root_tarball = run_npm_pack(
            root_dir,
            output_dir,
            f"{ROOT_PACKAGE_NAME}-{args.release_version}.tgz",
        )

        print("npm tarballs:")
        for tarball in [*platform_tarballs, root_tarball]:
            print(f"  {tarball}")
        if not remove_staging:
            print(f"staged packages: {staging_root}")
    finally:
        if remove_staging:
            shutil.rmtree(staging_root, ignore_errors=True)

    return 0


def stage_root_package(package_dir: Path, version: str, artifacts_dir: Path) -> None:
    package_dir.mkdir(parents=True)
    copy_common_files(package_dir)
    bin_dir = package_dir / "bin"
    bin_dir.mkdir()
    shutil.copy2(NPM_ROOT / "bin" / "hunea.js", bin_dir / "hunea.js")
    (bin_dir / "hunea.js").chmod(0o755)

    # 仅用于 staged package smoke test；`files` 不包含 vendor，因此不会进入 root npm 包。
    for platform_package in PLATFORM_PACKAGES:
        vendor_bin = package_dir / "vendor" / platform_package.target / "bin"
        vendor_bin.mkdir(parents=True)
        binary_path = vendor_bin / platform_package.executable_name
        extract_binary_archive(artifacts_dir, platform_package, binary_path)
        if platform_package.os_name != "win32":
            executable_mode = stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            binary_path.chmod(binary_path.stat().st_mode | executable_mode)

    package_json = {
        "name": ROOT_PACKAGE_NAME,
        "version": version,
        "description": "A terminal-based AI assistant client built with Rust and Ratatui.",
        "license": "Apache-2.0",
        "type": "module",
        "bin": {"hunea": "bin/hunea.js"},
        "engines": {"node": ">=18"},
        "files": ["bin/hunea.js"],
        "repository": {
            "type": "git",
            "url": "git+https://github.com/yunxinx/hunea.git",
        },
        "optionalDependencies": {
            platform_package.package_name: version for platform_package in PLATFORM_PACKAGES
        },
    }
    write_package_json(package_dir, package_json)


def stage_platform_package(
    package_dir: Path,
    platform_package: PlatformPackage,
    version: str,
    artifacts_dir: Path,
) -> None:
    package_dir.mkdir(parents=True)
    copy_common_files(package_dir)

    vendor_bin = package_dir / "vendor" / platform_package.target / "bin"
    vendor_bin.mkdir(parents=True)
    binary_path = vendor_bin / platform_package.executable_name
    extract_binary_archive(artifacts_dir, platform_package, binary_path)
    if platform_package.os_name != "win32":
        executable_mode = stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
        binary_path.chmod(binary_path.stat().st_mode | executable_mode)

    package_json = {
        "name": platform_package.package_name,
        "version": version,
        "description": f"Native hunea binary for {platform_package.os_name}/{platform_package.cpu}.",
        "license": "Apache-2.0",
        "os": [platform_package.os_name],
        "cpu": [platform_package.cpu],
        "files": ["vendor"],
        "repository": {
            "type": "git",
            "url": "git+https://github.com/yunxinx/hunea.git",
        },
    }
    write_package_json(package_dir, package_json)


def copy_common_files(package_dir: Path) -> None:
    for source_name in ["README.md", "LICENSE"]:
        source_path = REPO_ROOT / source_name
        if source_path.exists():
            shutil.copy2(source_path, package_dir / source_name)


def write_package_json(package_dir: Path, package_json: dict) -> None:
    with open(package_dir / "package.json", "w", encoding="utf-8") as package_file:
        json.dump(package_json, package_file, indent=2)
        package_file.write("\n")


def extract_binary_archive(
    artifacts_dir: Path,
    platform_package: PlatformPackage,
    destination: Path,
) -> None:
    archive_path = find_archive(artifacts_dir, platform_package.target)
    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path) as archive:
            archive.extract(platform_package.executable_name, destination.parent)
    else:
        with tarfile.open(archive_path, "r:gz") as archive:
            member = archive.getmember(platform_package.executable_name)
            if not member.isfile():
                raise RuntimeError(f"archive member is not a file: {member.name}")
            source = archive.extractfile(member)
            if source is None:
                raise RuntimeError(f"unable to read archive member: {member.name}")
            with source, open(destination, "wb") as output:
                shutil.copyfileobj(source, output)

    extracted_path = destination.parent / platform_package.executable_name
    if extracted_path != destination:
        shutil.move(extracted_path, destination)
    if not destination.is_file():
        raise FileNotFoundError(f"archive did not produce expected binary: {destination}")


def find_archive(artifacts_dir: Path, target: str) -> Path:
    names = [f"hunea-{target}.tar.gz", f"hunea-{target}.zip"]
    for name in names:
        matches = sorted(artifacts_dir.rglob(name))
        if matches:
            return matches[0]
    raise FileNotFoundError(f"no release archive found for {target} under {artifacts_dir}")


def run_npm_pack(package_dir: Path, output_dir: Path, output_name: str) -> Path:
    if shutil.which("npm") is None:
        raise RuntimeError("npm is required to pack release packages")

    with tempfile.TemporaryDirectory(prefix="hunea-npm-pack-") as pack_dir_string:
        pack_dir = Path(pack_dir_string)
        env = os.environ.copy()
        env["NPM_CONFIG_CACHE"] = str(pack_dir / "cache")
        env["NPM_CONFIG_LOGS_DIR"] = str(pack_dir / "logs")
        stdout = subprocess.check_output(
            ["npm", "pack", "--json", "--pack-destination", str(pack_dir)],
            cwd=package_dir,
            env=env,
            text=True,
        )
        pack_output = json.loads(stdout)
        if not pack_output:
            raise RuntimeError(f"npm pack produced no tarball for {package_dir}")

        filename = pack_output[0].get("filename")
        if not filename:
            raise RuntimeError(f"npm pack output missing filename for {package_dir}")

        source = pack_dir / filename
        destination = output_dir / output_name
        shutil.move(source, destination)
        return destination


if __name__ == "__main__":
    raise SystemExit(main())
