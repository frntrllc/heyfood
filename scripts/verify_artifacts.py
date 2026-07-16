#!/usr/bin/env python3
"""Verify the built heyfood wheel and sdist without installing either."""

from __future__ import annotations

import argparse
from email import policy
from email.parser import BytesParser
from pathlib import Path, PurePosixPath
import runpy
import tarfile
import zipfile


PROJECT_ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = PROJECT_ROOT / "src" / "heyfood_cli"
FORBIDDEN_PARTS = {".env", "__pycache__"}
REQUIRED_PACKAGE_DATA = {
    "heyfood_cli/data/banner.palette.json": PACKAGE_ROOT / "data" / "banner.palette.json",
    "heyfood_cli/data/banner.txt": PACKAGE_ROOT / "data" / "banner.txt",
    "heyfood_cli/data/dietary_options.json": PACKAGE_ROOT / "data" / "dietary_options.json",
}


def _version() -> str:
    namespace = runpy.run_path(str(PACKAGE_ROOT / "__init__.py"))
    value = namespace.get("__version__")
    if not isinstance(value, str) or not value:
        raise AssertionError("src/heyfood_cli/__init__.py must define __version__")
    return value


def _assert_safe_names(names: set[str]) -> None:
    for name in names:
        path = PurePosixPath(name)
        lowered = {part.lower() for part in path.parts}
        if path.is_absolute() or ".." in path.parts:
            raise AssertionError(f"unsafe artifact path: {name}")
        if (
            FORBIDDEN_PARTS & lowered
            or any(part.startswith(".env") for part in lowered)
            or name.endswith((".pyc", ".pem", ".key"))
        ):
            raise AssertionError(f"forbidden generated or credential-like artifact path: {name}")


def verify_wheel(wheel: Path, *, version: str) -> None:
    with zipfile.ZipFile(wheel) as archive:
        names = set(archive.namelist())
        _assert_safe_names(names)

        metadata_names = [name for name in names if name.endswith(".dist-info/METADATA")]
        if len(metadata_names) != 1:
            raise AssertionError("wheel must contain exactly one METADATA file")
        metadata = BytesParser(policy=policy.default).parsebytes(
            archive.read(metadata_names[0])
        )
        expected_metadata = {
            "Name": "heyfood-cli",
            "Version": version,
            "Author": "FRNTR, LLC",
            "License-Expression": "Apache-2.0",
            "Requires-Python": ">=3.11",
        }
        for field, expected in expected_metadata.items():
            actual = metadata.get(field)
            if actual != expected:
                raise AssertionError(f"{field}: expected {expected!r}, got {actual!r}")
        if set(metadata.get_all("License-File", [])) != {"LICENSE", "COPYRIGHT"}:
            raise AssertionError("wheel metadata must declare LICENSE and COPYRIGHT")

        entry_points = [name for name in names if name.endswith(".dist-info/entry_points.txt")]
        if len(entry_points) != 1:
            raise AssertionError("wheel must contain exactly one entry_points.txt")
        if "heyfood = heyfood_cli.main:app" not in archive.read(entry_points[0]).decode():
            raise AssertionError("wheel does not expose the heyfood console entry point")

        for member, source in REQUIRED_PACKAGE_DATA.items():
            if member not in names:
                raise AssertionError(f"wheel is missing package data: {member}")
            if archive.read(member) != source.read_bytes():
                raise AssertionError(f"wheel package data differs from source: {member}")

        license_members = {
            PurePosixPath(name).name
            for name in names
            if ".dist-info/licenses/" in name
        }
        if license_members != {"LICENSE", "COPYRIGHT"}:
            raise AssertionError("wheel must package LICENSE and COPYRIGHT")


def verify_sdist(sdist: Path, *, version: str) -> None:
    expected_prefix = f"heyfood_cli-{version}/"
    required = {
        "LICENSE",
        "COPYRIGHT",
        "README.md",
        "RELEASING.md",
        "install.sh",
        "install.sh.sha256",
        "docs/JSON_SCHEMAS.md",
        "pyproject.toml",
        "schemas/v1/heyfood-output.schema.json",
        "src/heyfood_cli/__init__.py",
        "src/heyfood_cli/data/banner.palette.json",
        "src/heyfood_cli/data/banner.txt",
        "src/heyfood_cli/data/dietary_options.json",
    }
    with tarfile.open(sdist, mode="r:gz") as archive:
        names = {member.name for member in archive.getmembers()}
        _assert_safe_names(names)
        if not all(name.startswith(expected_prefix) for name in names):
            raise AssertionError(f"sdist paths must live under {expected_prefix}")
        missing = {
            f"{expected_prefix}{relative}"
            for relative in required
            if f"{expected_prefix}{relative}" not in names
        }
        if missing:
            raise AssertionError(f"sdist is missing required files: {sorted(missing)}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("dist", type=Path, help="Directory containing one wheel and one sdist")
    args = parser.parse_args()
    wheels = sorted(args.dist.glob("*.whl"))
    sdists = sorted(args.dist.glob("*.tar.gz"))
    if len(wheels) != 1 or len(sdists) != 1:
        raise SystemExit("expected exactly one wheel and one .tar.gz sdist")

    version = _version()
    verify_wheel(wheels[0], version=version)
    verify_sdist(sdists[0], version=version)
    print(f"verified wheel and sdist for heyfood-cli {version}")


if __name__ == "__main__":
    main()
