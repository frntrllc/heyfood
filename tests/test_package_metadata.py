from __future__ import annotations

import re
import tomllib
from pathlib import Path

from heyfood_cli import __version__
from heyfood_cli.client import HelloFoodClient
from heyfood_cli.config import ConfigStore


ROOT = Path(__file__).resolve().parents[1]


def _metadata() -> dict:
    with (ROOT / "pyproject.toml").open("rb") as handle:
        return tomllib.load(handle)


def test_public_package_metadata_uses_pep_639_and_frntr_ownership() -> None:
    metadata = _metadata()
    project = metadata["project"]

    assert metadata["build-system"]["requires"] == ["hatchling>=1.27"]
    assert "version" not in project
    assert project["dynamic"] == ["version"]
    assert metadata["tool"]["hatch"]["version"] == {
        "path": "src/heyfood_cli/__init__.py"
    }
    assert project["license"] == "Apache-2.0"
    assert project["license-files"] == ["LICENSE", "COPYRIGHT"]
    assert project["authors"] == [{"name": "FRNTR, LLC"}]
    assert "click>=8.1,<8.2" in project["dependencies"]
    assert "typer>=0.20,<0.24" in project["dependencies"]
    assert not any(value.startswith("License ::") for value in project["classifiers"])
    assert project["urls"] == {
        "Homepage": "https://hello.food/heyfood",
        "Documentation": "https://github.com/frntrllc/heyfood#readme",
        "Repository": "https://github.com/frntrllc/heyfood",
        "Issues": "https://github.com/frntrllc/heyfood/issues",
        "Changelog": "https://github.com/frntrllc/heyfood/releases",
    }


def test_public_legal_and_contributor_documents_are_present() -> None:
    required = {
        "LICENSE",
        "COPYRIGHT",
        "README.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        "CODE_OF_CONDUCT.md",
        "SUPPORT.md",
        "DEVELOPMENT.md",
        "CHANGELOG.md",
        "RELEASING.md",
        "install.sh",
        "install.sh.sha256",
        "docs/CLI_CONTRACT.md",
        "docs/COMMAND_GRAMMAR.md",
        "docs/JSON_SCHEMAS.md",
        "schemas/v1/heyfood-output.schema.json",
    }

    assert {name for name in required if (ROOT / name).is_file()} == required

    readme = (ROOT / "README.md").read_text()
    assert "Copyright 2026 FRNTR, LLC" in readme
    assert "does not license the proprietary hello.food" in readme

    contributing = (ROOT / "CONTRIBUTING.md").read_text()
    assert "Apache License 2.0" in contributing
    assert "separate contributor license agreement" in contributing

    grammar = (ROOT / "docs" / "COMMAND_GRAMMAR.md").read_text()
    assert "members list" in grammar
    assert "conversation resume" in grammar


def test_cli_version_and_user_agent_share_the_runtime_version(tmp_path: Path) -> None:
    client = HelloFoodClient(store=ConfigStore(tmp_path / "config.json"))

    assert re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[abrc][0-9]+)?", __version__)
    assert client._headers()["User-Agent"] == f"heyfood-cli/{__version__}"


def test_dietary_catalog_is_packaged_as_runtime_data() -> None:
    catalog = ROOT / "src" / "heyfood_cli" / "data" / "dietary_options.json"

    assert catalog.is_file()
    assert '"version": 2' in catalog.read_text()
