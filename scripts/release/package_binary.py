#!/usr/bin/env python3
"""Package a built hunea binary into the release archive for one target."""

from __future__ import annotations

import argparse
import shutil
import stat
import tarfile
import tempfile
import zipfile
from pathlib import Path


WINDOWS_TARGET_SUFFIX = "windows-msvc"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple.")
    parser.add_argument(
        "--release-dir",
        type=Path,
        required=True,
        help="Directory containing the built release binary.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        required=True,
        help="Directory where the release archive should be written.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    target = args.target
    executable_name = "hunea.exe" if is_windows_target(target) else "hunea"
    binary_path = args.release_dir / executable_name
    if not binary_path.is_file():
        raise FileNotFoundError(f"built binary not found: {binary_path}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    archive_path = release_archive_path(args.output_dir, target)

    with tempfile.TemporaryDirectory(prefix=f"hunea-{target}-") as temp_dir_string:
        temp_dir = Path(temp_dir_string)
        staged_binary = temp_dir / executable_name
        shutil.copy2(binary_path, staged_binary)
        if not is_windows_target(target):
            executable_mode = stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            staged_binary.chmod(staged_binary.stat().st_mode | executable_mode)

        if is_windows_target(target):
            with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                archive.write(staged_binary, executable_name)
        else:
            with tarfile.open(archive_path, "w:gz") as archive:
                archive.add(staged_binary, executable_name)

    print(archive_path)
    return 0


def is_windows_target(target: str) -> bool:
    return target.endswith(WINDOWS_TARGET_SUFFIX)


def release_archive_path(output_dir: Path, target: str) -> Path:
    suffix = "zip" if is_windows_target(target) else "tar.gz"
    return output_dir / f"hunea-{target}.{suffix}"


if __name__ == "__main__":
    raise SystemExit(main())
