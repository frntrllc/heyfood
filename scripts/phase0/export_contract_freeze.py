#!/usr/bin/env python3
"""Generate and verify the immutable Phase 0 migration evidence.

The generator intentionally has no third-party dependencies.  It freezes the
reviewed Python baseline without importing product code and makes every output
stable across machines.  The checked-in outputs may only be regenerated from
``BASELINE_SHA`` and the pinned hellofood asset commit below.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
from pathlib import Path
from pathlib import PurePosixPath
import subprocess
import sys
import tarfile
import tempfile
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
BASELINE_SHA = "73494a57468dac83b4904ce6c390e36926f5c6fe"
BASELINE_TREE = "4c265cd9ae0623442dd8eba1f6f4388c4ebf5adf"
BASELINE_VERSION = "0.4.0"
EXPECTED_NODE_COUNT = 643
EXPECTED_NODE_SHA256 = "49e6fe5429174a9ad8f6cf47d209365ca852ebed5b6f6fa7f86b824dfa4b0cd3"
EXPECTED_ENDPOINT_COUNT = 26
HELLOFOOD_SHA = "27cab29dd3d17bb844462c8ec5340585b859b0ae"
HELLOFOOD_REPOSITORY = "https://github.com/frntrllc/hellofood.git"
DIETARY_SHA256 = "40a26e22d7e729289ef5bf4052af841adc76029711d89c89f046eba87d533556"
BANNER_SHA256 = "8f97c59f5eba7075891cb1aa31c300ea776a0ac57117ef290a7f7c6a07e4c50e"
PALETTE_SHA256 = "22978be9dd03ca5a194617940d0b78a495bdf7f3ecc40729af2f1322cca0d73e"
BANNER_TS_SHA256 = "49fe8509eeb0b3b4511b97cf81b383140ccae1d29b93bc7ca68ac07dd1d9da84"
TOOL_VERSION = 2

NODE_PATH = ROOT / "tests/migration/python-node-ids.txt"
NODE_METADATA_PATH = ROOT / "tests/migration/python-node-ids.metadata.json"
INVENTORY_PATH = ROOT / "tests/migration/non-pytest-invariants.json"
LEDGER_PATH = ROOT / "tests/migration/python-test-ledger.json"
STABLE_ENDPOINT_PATH = ROOT / "fixtures/contracts/called-endpoints.json"
COMPAT_ENDPOINT_PATH = ROOT / "tests/fixtures/called_endpoints.json"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha256(path: Path) -> str:
    return sha256(path.read_bytes())


def json_bytes(document: Any) -> bytes:
    return (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def write_json(path: Path, document: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes(document))


def git_blob(path: str, *, repository: Path = ROOT, commit: str = BASELINE_SHA) -> bytes:
    return subprocess.run(
        ["git", "show", f"{commit}:{path}"],
        cwd=repository,
        check=True,
        capture_output=True,
    ).stdout


def collect_node_ids(python: str) -> list[str]:
    archive = subprocess.run(
        ["git", "archive", "--format=tar", BASELINE_SHA],
        cwd=ROOT,
        check=True,
        capture_output=True,
    ).stdout
    with tempfile.TemporaryDirectory(prefix="heyfood-python-oracle-") as directory:
        checkout = Path(directory)
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as source:
            for member in source.getmembers():
                path = PurePosixPath(member.name)
                if path.is_absolute() or ".." in path.parts:
                    raise AssertionError(f"unsafe baseline archive path: {member.name}")
            source.extractall(checkout)
        result = subprocess.run(
            [python, "-m", "pytest", "--collect-only", "-q"],
            cwd=checkout,
            check=True,
            capture_output=True,
            text=True,
        )
    return [line for line in result.stdout.splitlines() if line.startswith("tests/")]


def normalized_node_bytes(node_ids: list[str]) -> bytes:
    return ("\n".join(node_ids) + "\n").encode("utf-8")


def assert_node_capture(node_ids: list[str]) -> bytes:
    data = normalized_node_bytes(node_ids)
    if len(node_ids) != EXPECTED_NODE_COUNT:
        raise AssertionError(f"expected {EXPECTED_NODE_COUNT} node IDs, found {len(node_ids)}")
    actual = sha256(data)
    if actual != EXPECTED_NODE_SHA256:
        raise AssertionError(f"node-ID digest mismatch: expected {EXPECTED_NODE_SHA256}, got {actual}")
    if len(set(node_ids)) != len(node_ids):
        raise AssertionError("the baseline collection contains duplicate node IDs")
    return data


def invariant(
    invariant_id: str,
    category: str,
    source_path: str,
    locator: str,
    contract: str,
) -> dict[str, Any]:
    source = git_blob(source_path)
    return {
        "invariant_id": invariant_id,
        "category": category,
        "source_path": source_path,
        "source_locator": locator,
        "source_sha256": sha256(source),
        "contract": contract,
    }


def non_pytest_invariants() -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    workflow_jobs = {
        ".github/workflows/ci.yml": {
            "installer-tests": "Hosted installer behavior passes on macOS and Linux.",
            "unit-tests": "The complete Python suite passes on Python 3.11, 3.12, and 3.13.",
            "build-distribution": "One wheel and one sdist build and pass metadata/content verification.",
            "install-smoke": "The built wheel installs through pipx and passes a clean-user smoke on the supported matrix.",
            "voice-wheel-smoke": "The voice extra imports and reports status on macOS and Linux without microphone access.",
        },
        ".github/workflows/release.yml": {
            "build": "An annotated main-line version tag is tested, built once, verified, and clean-installed.",
            "publish": "The exact verified artifacts publish through the protected PyPI environment.",
            "github-release": "The published artifacts attach to the matching verified GitHub tag.",
        },
        ".github/workflows/post-release-smoke.yml": {
            "public-pypi": "The exact public PyPI version installs, runs, emits valid JSON, and uninstalls on the supported matrix.",
        },
    }
    for path, jobs in workflow_jobs.items():
        for job, contract in jobs.items():
            entries.append(invariant(f"ci::{path}#{job}", "ci", path, f"jobs.{job}", contract))

    for function, contract in {
        "verify_wheel": "Wheel metadata, entry point, licenses, safe paths, and runtime data bytes match the source contract.",
        "verify_sdist": "The sdist has safe rooted paths and contains every required release file.",
    }.items():
        entries.append(
            invariant(
                f"release-script::scripts/verify_artifacts.py#{function}",
                "release_script",
                "scripts/verify_artifacts.py",
                function,
                contract,
            )
        )
    entries.append(
        invariant(
            "release-script::scripts/regenerate_compat_fixtures.py#help-export",
            "release_script",
            "scripts/regenerate_compat_fixtures.py",
            "main_cli",
            "Command help fixtures are normalized deterministically from the pinned command map.",
        )
    )

    for path, contract in {
        "schemas/v1/heyfood-output.schema.json": "Stable v1 machine output shapes and conservative safety vocabulary remain valid.",
        "schemas/v1/transcription.schema.json": "Stable v1 transcription request/result shapes remain valid.",
    }.items():
        entries.append(invariant(f"schema::{path}", "schema", path, "$", contract))

    for path, contract in {
        "CHANGELOG.md": "Published release history and compatibility notes remain available through the cutover.",
        "CODE_OF_CONDUCT.md": "The project code of conduct remains part of the public repository contract.",
        "CONTRIBUTING.md": "Public contribution, testing, and review guidance remains represented.",
        "DEVELOPMENT.md": "Supported local development, test, schema, and fixture workflows remain represented.",
        "README.md": "Public installation, command, configuration, privacy, and compatibility guidance remains represented.",
        "RELEASING.md": "The release checklist, exact-artifact discipline, and rollback guidance remain represented.",
        "SECURITY.md": "Security support, disclosure, credential, and transport guidance remains represented.",
        "SUPPORT.md": "Public support boundaries and escalation guidance remain represented.",
        "docs/CLI_CONTRACT.md": "Streams, JSON mode, exits, prompts, automation, and compatibility semantics remain represented.",
        "docs/COMMAND_GRAMMAR.md": "The public command grammar and compatibility aliases remain represented.",
        "docs/DIETARY_CATALOG.md": "Dietary catalog ownership, synchronization, versioning, and profile provenance remain represented.",
        "docs/JSON_SCHEMAS.md": "Published schema families, compatibility policy, and safety vocabulary remain represented.",
    }.items():
        entries.append(invariant(f"documentation::{path}", "documentation", path, "$", contract))

    for path, locator, contract in (
        (
            "pyproject.toml",
            "project-and-build-metadata",
            "Package identity, supported Python versions, dependencies, entry point, version source, and distribution inclusions remain accounted for.",
        ),
        (
            "LICENSE",
            "$",
            "The Apache-2.0 license remains present in source and release artifacts.",
        ),
        (
            "COPYRIGHT",
            "$",
            "The copyright notice remains present in source and release artifacts.",
        ),
        (
            "scripts/smoke_installed_cli.py",
            "main",
            "A clean pipx install exposes version/help/completions, emits valid unauthenticated diagnostics, and does not create config.",
        ),
        (
            "install.sh",
            "$",
            "The public installer validates platform/tool versions, isolates pipx, pins the public index, verifies the executable, and avoids root.",
        ),
        (
            "install.sh.sha256",
            "$",
            "The published installer checksum names the exact install.sh bytes.",
        ),
    ):
        entries.append(
            invariant(f"installed-artifact::{path}#{locator}", "installed_artifact", path, locator, contract)
        )
    return entries


def write_migration_evidence(node_ids: list[str]) -> None:
    node_data = assert_node_capture(node_ids)
    NODE_PATH.parent.mkdir(parents=True, exist_ok=True)
    NODE_PATH.write_bytes(node_data)
    metadata = {
        "schema_version": 1,
        "baseline_sha": BASELINE_SHA,
        "baseline_tree": BASELINE_TREE,
        "baseline_version": BASELINE_VERSION,
        "capture_command": "python -m pytest --collect-only -q",
        "normalization": "Keep stdout lines beginning with tests/, preserve collection order, UTF-8, LF, final LF.",
        "node_count": len(node_ids),
        "normalized_sha256": sha256(node_data),
        "reviewed_environment": {
            "python": "3.11.15",
            "pytest": "9.1.1",
            "click": "8.1.8",
            "httpx": "0.28.1",
            "rich": "14.3.4",
            "typer": "0.23.1",
        },
        "capture_tool": "scripts/phase0/export_contract_freeze.py",
        "capture_tool_version": TOOL_VERSION,
    }
    write_json(NODE_METADATA_PATH, metadata)

    invariants = non_pytest_invariants()
    inventory = {
        "$schema": "../../schemas/migration-ledger.v1.schema.json#/$defs/invariantInventoryDocument",
        "schema_version": 1,
        "baseline_sha": BASELINE_SHA,
        "inventory_count": len(invariants),
        "invariants": invariants,
    }
    write_json(INVENTORY_PATH, inventory)

    entries: list[dict[str, Any]] = []
    for node_id in node_ids:
        entries.append(
            {
                "baseline_sha": BASELINE_SHA,
                "invariant_id": f"pytest::{node_id}",
                "original": node_id,
                "category": "pytest",
                "migration_status": "unmapped",
                "disposition": None,
                "replacements": [],
                "rationale": None,
                "owner": None,
                "reviewer": None,
                "reviewed_commit_sha": None,
            }
        )
    for item in invariants:
        entries.append(
            {
                "baseline_sha": BASELINE_SHA,
                "invariant_id": item["invariant_id"],
                "original": f'{item["source_path"]}#{item["source_locator"]}',
                "category": item["category"],
                "migration_status": "unmapped",
                "disposition": None,
                "replacements": [],
                "rationale": None,
                "owner": None,
                "reviewer": None,
                "reviewed_commit_sha": None,
            }
        )
    ledger = {
        "$schema": "../../schemas/migration-ledger.v1.schema.json",
        "schema_version": 1,
        "baseline": {
            "commit_sha": BASELINE_SHA,
            "tree_sha": BASELINE_TREE,
            "python_version": BASELINE_VERSION,
            "pytest_node_ids_path": "tests/migration/python-node-ids.txt",
            "pytest_node_count": len(node_ids),
            "pytest_node_ids_sha256": sha256(node_data),
            "non_pytest_inventory_path": "tests/migration/non-pytest-invariants.json",
            "non_pytest_invariant_count": len(invariants),
        },
        "summary": {
            "entry_count": len(entries),
            "mapped_count": 0,
            "unmapped_count": len(entries),
        },
        "entries": entries,
    }
    write_json(LEDGER_PATH, ledger)


def write_endpoint_contract() -> None:
    compatibility_bytes = git_blob("tests/fixtures/called_endpoints.json")
    compatibility = json.loads(compatibility_bytes)
    endpoints = list(compatibility["endpoints"])
    endpoints.append(
        {
            "method": "GET",
            "endpoint": "/.well-known/oauth-authorization-server",
            "auth": "none",
            "note": "RFC 8414 authorization-server metadata discovery used before login; fail-soft and bounded by a five-second timeout.",
        }
    )
    contract = {
        "$comment": "Stable language-neutral inventory of outbound HTTP requests plus browser and loopback listener surfaces reachable from the final unpublished Python 0.4.0 candidate. The compatibility fixture is frozen from tests/fixtures/called_endpoints.json at provenance.baseline_sha; public Python releases ended at 0.3.2 and the live fixture may continue to evolve during consumer migration.",
        "schema_version": 1,
        "provenance": {
            "baseline_sha": BASELINE_SHA,
            "baseline_tree": BASELINE_TREE,
            "compatibility_fixture": "tests/fixtures/called_endpoints.json",
            "compatibility_fixture_sha256": sha256(compatibility_bytes),
            "compatibility_endpoint_count": len(compatibility["endpoints"]),
            "capture_tool": "scripts/phase0/export_contract_freeze.py",
            "capture_tool_version": TOOL_VERSION,
        },
        "endpoints": endpoints,
        "browser_navigations": [
            {
                "source": "oauth_authorization_url",
                "scheme_policy": "https_or_exact_loopback_http",
                "origin": "configured_auth_url",
                "note": "Loopback PKCE authorization page; URL is also printed when browser opening is disabled or fails.",
            },
            {
                "source": "device_verification_url",
                "scheme_policy": "server_provided_validated_service_url",
                "origin": "device_authorization_response",
                "note": "RFC 8628 user verification page.",
            },
            {
                "source": "account_deletion_browser_url",
                "scheme_policy": "https_or_exact_loopback_http",
                "origin": "account_deletion_begin_response",
                "note": "Browser identity-confirmation page for account deletion.",
            },
            {
                "source": "voice_capture_url",
                "scheme_policy": "exact_127_0_0_1_http",
                "origin": "local_voice_listener",
                "note": "Ephemeral one-shot browser speech capture page carrying an unguessable state query.",
            },
        ],
        "local_listeners": [
            {
                "name": "oauth_callback",
                "bind": "127.0.0.1",
                "port_policy": "8765_then_ephemeral",
                "routes": [{"method": "GET", "path": "/callback"}],
                "note": "One-shot PKCE callback validating OAuth state before code exchange.",
            },
            {
                "name": "voice_capture",
                "bind": "127.0.0.1",
                "port_policy": "ephemeral",
                "routes": [
                    {"method": "GET", "path": "/"},
                    {"method": "POST", "path": "/submit"},
                    {"method": "POST", "path": "/cancel"},
                ],
                "note": "Hardened one-shot listener requiring exact Host, state, and same-origin POST checks.",
            },
        ],
    }
    write_json(STABLE_ENDPOINT_PATH, contract)


def git_asset(repository: Path, path: str) -> bytes:
    return git_blob(path, repository=repository, commit=HELLOFOOD_SHA)


def write_assets(hellofood: Path) -> None:
    source_specs = {
        "shared/dietary_options.json": DIETARY_SHA256,
        "docs/references/banner.txt": BANNER_SHA256,
        "docs/references/banner.palette.json": PALETTE_SHA256,
        "docs/references/banner.ts": BANNER_TS_SHA256,
    }
    blobs: dict[str, bytes] = {}
    for path, expected_hash in source_specs.items():
        blob = git_asset(hellofood, path)
        actual = sha256(blob)
        if actual != expected_hash:
            raise AssertionError(f"canonical {path} hash mismatch: expected {expected_hash}, got {actual}")
        blobs[path] = blob

    dietary_target = ROOT / "assets/dietary/dietary_options.v2.json"
    dietary_target.parent.mkdir(parents=True, exist_ok=True)
    dietary_target.write_bytes(blobs["shared/dietary_options.json"])
    write_json(
        ROOT / "assets/dietary/provenance.json",
        {
            "schema_version": 1,
            "asset_contract_version": 2,
            "source_repository": HELLOFOOD_REPOSITORY,
            "source_commit": HELLOFOOD_SHA,
            "source_commit_clean": True,
            "source_path": "shared/dietary_options.json",
            "source_sha256": DIETARY_SHA256,
            "target_path": "assets/dietary/dietary_options.v2.json",
            "target_sha256": DIETARY_SHA256,
            "export_tool": "scripts/phase0/export_contract_freeze.py",
            "export_tool_version": TOOL_VERSION,
            "review": {"status": "pending", "reviewer": None, "reviewed_commit_sha": None},
        },
    )

    banner_target = ROOT / "assets/brand/banner.txt"
    palette_target = ROOT / "assets/brand/banner.palette.json"
    banner_target.parent.mkdir(parents=True, exist_ok=True)
    banner_target.write_bytes(blobs["docs/references/banner.txt"])
    palette_target.write_bytes(blobs["docs/references/banner.palette.json"])
    palette = json.loads(blobs["docs/references/banner.palette.json"])
    lines = blobs["docs/references/banner.txt"].decode("utf-8").splitlines()
    max_width = max(len(line) for line in lines)
    frame_text = "\n".join(lines) + "\n"
    frames = {
        "$schema": "../../schemas/banner-frames.v1.schema.json",
        "schema_version": 1,
        "source": {
            "banner_sha256": BANNER_SHA256,
            "palette_sha256": PALETTE_SHA256,
        },
        "geometry": {"width": max_width, "height": len(lines), "encoding": "utf-8"},
        "frames": [
            {
                "index": 0,
                "duration_ms": 0,
                "visible_lines": list(range(len(lines))),
                "accent_spans": palette["accent_spans"],
                "plain_text_sha256": sha256(frame_text.encode("utf-8")),
            }
        ],
    }
    write_json(ROOT / "assets/brand/banner.frames.json", frames)
    write_json(
        ROOT / "assets/brand/provenance.json",
        {
            "schema_version": 1,
            "source_repository": HELLOFOOD_REPOSITORY,
            "source_commit": HELLOFOOD_SHA,
            "source_commit_clean": True,
            "sources": [
                {"path": "docs/references/banner.txt", "sha256": BANNER_SHA256},
                {"path": "docs/references/banner.palette.json", "sha256": PALETTE_SHA256},
                {"path": "docs/references/banner.ts", "sha256": BANNER_TS_SHA256},
            ],
            "targets": [
                {"path": "assets/brand/banner.txt", "sha256": BANNER_SHA256},
                {"path": "assets/brand/banner.palette.json", "sha256": PALETTE_SHA256},
                {"path": "assets/brand/banner.frames.json", "sha256": file_sha256(ROOT / "assets/brand/banner.frames.json")},
            ],
            "export_tool": "scripts/phase0/export_contract_freeze.py",
            "export_tool_version": TOOL_VERSION,
            "review": {"status": "pending", "reviewer": None, "reviewed_commit_sha": None},
        },
    )


def verify() -> None:
    node_ids = NODE_PATH.read_text(encoding="utf-8").splitlines()
    node_data = assert_node_capture(node_ids)
    metadata = json.loads(NODE_METADATA_PATH.read_text(encoding="utf-8"))
    if metadata["normalized_sha256"] != sha256(node_data) or metadata["node_count"] != len(node_ids):
        raise AssertionError("node-ID sidecar does not describe the immutable capture")

    inventory = json.loads(INVENTORY_PATH.read_text(encoding="utf-8"))
    invariants = inventory["invariants"]
    if inventory["inventory_count"] != len(invariants):
        raise AssertionError("non-pytest invariant count is stale")
    for item in invariants:
        baseline_blob = git_blob(item["source_path"])
        if sha256(baseline_blob) != item["source_sha256"]:
            raise AssertionError(f'invariant source hash drift: {item["invariant_id"]}')

    ledger = json.loads(LEDGER_PATH.read_text(encoding="utf-8"))
    entries = ledger["entries"]
    ids = [entry["invariant_id"] for entry in entries]
    expected_ids = [f"pytest::{node}" for node in node_ids] + [item["invariant_id"] for item in invariants]
    if ids != expected_ids or len(ids) != len(set(ids)):
        raise AssertionError("ledger entries do not map the frozen inventories exactly once")
    mapped = sum(entry["migration_status"] == "mapped" for entry in entries)
    unmapped = sum(entry["migration_status"] == "unmapped" for entry in entries)
    if ledger["summary"] != {"entry_count": len(entries), "mapped_count": mapped, "unmapped_count": unmapped}:
        raise AssertionError("ledger summary is stale")
    for entry in entries:
        if entry["migration_status"] == "unmapped":
            if entry["disposition"] is not None or entry["replacements"]:
                raise AssertionError(f'unmapped entry claims replacement evidence: {entry["invariant_id"]}')

    baseline_compatibility_bytes = git_blob("tests/fixtures/called_endpoints.json")
    baseline_compatibility = json.loads(baseline_compatibility_bytes)
    compatibility = json.loads(COMPAT_ENDPOINT_PATH.read_text(encoding="utf-8"))
    stable = json.loads(STABLE_ENDPOINT_PATH.read_text(encoding="utf-8"))
    frozen_endpoints = baseline_compatibility["endpoints"]
    if len(frozen_endpoints) != EXPECTED_ENDPOINT_COUNT:
        raise AssertionError(
            f"baseline compatibility endpoint fixture must contain {EXPECTED_ENDPOINT_COUNT} rows"
        )
    if stable["endpoints"][:-1] != frozen_endpoints:
        raise AssertionError("stable endpoint contract no longer preserves the frozen compatibility rows")
    if not all(endpoint in compatibility["endpoints"] for endpoint in frozen_endpoints):
        raise AssertionError("live compatibility endpoint fixture no longer preserves the frozen rows")
    metadata_endpoint = stable["endpoints"][-1]
    if (metadata_endpoint["method"], metadata_endpoint["endpoint"]) != (
        "GET",
        "/.well-known/oauth-authorization-server",
    ):
        raise AssertionError("stable endpoint contract is missing RFC 8414 metadata discovery")
    provenance = stable["provenance"]
    if provenance["baseline_sha"] != BASELINE_SHA or provenance["baseline_tree"] != BASELINE_TREE:
        raise AssertionError("endpoint compatibility provenance does not name the frozen baseline")
    if provenance["compatibility_fixture_sha256"] != sha256(baseline_compatibility_bytes):
        raise AssertionError("endpoint compatibility provenance is stale for the frozen baseline")
    if provenance["compatibility_endpoint_count"] != len(frozen_endpoints):
        raise AssertionError("endpoint compatibility provenance has a stale baseline row count")

    installer_checksum = (ROOT / "install.sh.sha256").read_text(encoding="utf-8").split()[0]
    if installer_checksum != file_sha256(ROOT / "install.sh"):
        raise AssertionError("install.sh.sha256 does not name the exact installer bytes")

    if file_sha256(ROOT / "assets/dietary/dietary_options.v2.json") != DIETARY_SHA256:
        raise AssertionError("dietary asset digest mismatch")
    if file_sha256(ROOT / "assets/brand/banner.txt") != BANNER_SHA256:
        raise AssertionError("banner asset digest mismatch")
    if file_sha256(ROOT / "assets/brand/banner.palette.json") != PALETTE_SHA256:
        raise AssertionError("banner palette digest mismatch")

    dietary = json.loads((ROOT / "assets/dietary/dietary_options.v2.json").read_text(encoding="utf-8"))
    if dietary.get("version") != 2 or set(dietary.get("sections", {})) != {
        "health_conditions", "diet_style", "allergies", "ingredients_to_avoid", "activity_level", "cuisines"
    }:
        raise AssertionError("dietary asset does not satisfy its v2 top-level contract")
    identifiers: list[str] = []
    for section in dietary["sections"].values():
        for key in ("tier1", "tier2", "options"):
            for option in section.get(key, []):
                identifiers.append(option["id"])
    identifiers.extend(option["id"] for option in dietary["household_diet_extras"])
    if any(not identifier for identifier in identifiers):
        raise AssertionError("dietary option identifiers must be non-empty")

    palette = json.loads((ROOT / "assets/brand/banner.palette.json").read_text(encoding="utf-8"))
    banner_lines = (ROOT / "assets/brand/banner.txt").read_text(encoding="utf-8").splitlines()
    for span in palette["accent_spans"]:
        line = banner_lines[span["line"]]
        if span["start"] + span["length"] > len(line):
            raise AssertionError("palette accent span exceeds banner geometry")
    frames = json.loads((ROOT / "assets/brand/banner.frames.json").read_text(encoding="utf-8"))
    if frames["source"] != {"banner_sha256": BANNER_SHA256, "palette_sha256": PALETTE_SHA256}:
        raise AssertionError("banner frame source hashes are stale")
    if frames["geometry"] != {"width": 44, "height": 5, "encoding": "utf-8"}:
        raise AssertionError("banner frame geometry drifted")
    if frames["frames"][0]["accent_spans"] != palette["accent_spans"]:
        raise AssertionError("banner frame palette spans drifted")

    for schema in (
        "schemas/migration-ledger.v1.schema.json",
        "schemas/dietary-options.v2.schema.json",
        "schemas/banner-palette.v1.schema.json",
        "schemas/banner-frames.v1.schema.json",
    ):
        document = json.loads((ROOT / schema).read_text(encoding="utf-8"))
        if document.get("$schema") != "https://json-schema.org/draft/2020-12/schema":
            raise AssertionError(f"{schema} is not a draft 2020-12 schema")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    generate = subparsers.add_parser("generate")
    generate.add_argument("--python", default=sys.executable)
    generate.add_argument(
        "--node-ids-file",
        type=Path,
        help="Previously normalized baseline node-ID capture; otherwise collect from the checkout.",
    )
    generate.add_argument("--hellofood", type=Path, required=True)
    subparsers.add_parser("verify")
    args = parser.parse_args()
    if args.command == "generate":
        node_ids = (
            [
                line
                for line in args.node_ids_file.read_text(encoding="utf-8").splitlines()
                if line.startswith("tests/")
            ]
            if args.node_ids_file is not None
            else collect_node_ids(args.python)
        )
        write_migration_evidence(node_ids)
        write_endpoint_contract()
        write_assets(args.hellofood.resolve())
    verify()
    print("Phase 0 contract freeze verified")


if __name__ == "__main__":
    main()
