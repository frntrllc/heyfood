"""Deterministic public checks for the Phase 0 contract/evidence freeze."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def _tool():
    path = ROOT / "scripts/phase0/export_contract_freeze.py"
    spec = importlib.util.spec_from_file_location("phase0_contract_freeze", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_frozen_evidence_and_assets_verify_without_private_repository() -> None:
    _tool().verify()


def test_legacy_endpoint_fixture_is_preserved_while_stable_contract_closes_gap() -> None:
    compatibility = json.loads((ROOT / "tests/fixtures/called_endpoints.json").read_text())
    stable = json.loads((ROOT / "fixtures/contracts/called-endpoints.json").read_text())
    assert len(compatibility["endpoints"]) == 25
    assert stable["endpoints"][:-1] == compatibility["endpoints"]
    assert stable["endpoints"][-1]["endpoint"] == "/.well-known/oauth-authorization-server"
    assert len(stable["endpoints"]) == 26


def test_initial_ledger_is_complete_but_truthfully_unmapped() -> None:
    ledger = json.loads((ROOT / "tests/migration/python-test-ledger.json").read_text())
    assert ledger["baseline"]["pytest_node_count"] == 601
    assert ledger["baseline"]["non_pytest_invariant_count"] == 32
    assert ledger["summary"] == {
        "entry_count": 633,
        "mapped_count": 0,
        "unmapped_count": 633,
    }
    assert all(entry["migration_status"] == "unmapped" for entry in ledger["entries"])
    assert all(entry["disposition"] is None for entry in ledger["entries"])
    assert all(entry["replacements"] == [] for entry in ledger["entries"])


def test_frame_manifest_is_a_single_exact_python_baseline_frame() -> None:
    frames = json.loads((ROOT / "assets/brand/banner.frames.json").read_text())
    assert frames["geometry"] == {"width": 44, "height": 5, "encoding": "utf-8"}
    assert frames["frames"] == [
        {
            "index": 0,
            "duration_ms": 0,
            "visible_lines": [0, 1, 2, 3, 4],
            "accent_spans": [{"line": 3, "start": 18, "length": 2}],
            "plain_text_sha256": "8f97c59f5eba7075891cb1aa31c300ea776a0ac57117ef290a7f7c6a07e4c50e",
        }
    ]
