"""Issue #10 Part 1: the CLI's endpoint surface is an exhaustive, checked-in contract.

This test statically extracts every ``(method, endpoint)`` the CLI can actually
send from ``src/heyfood_cli/{client,auth}.py`` and asserts it matches
``tests/fixtures/called_endpoints.json`` exactly. Adding a new endpoint call
without updating the contract fails CI with an actionable message; the same file
is consumed by the backend authorization-contract suite (Part 2) to prove every
listed endpoint is reachable under exactly ``LOGIN_SCOPES``.

The extraction understands the four ways the CLI issues a request:

* ``self._request(METHOD, PATH, ...)`` — the JSON request path;
* ``client.build_request(METHOD, URL, ...)`` / ``client.stream(METHOD, URL, ...)``
  — the multipart and streaming paths;
* ``_post_with_diagnostics(api_url, PATH, ...)`` — the auth POST helper.

Only string/f-string literals containing ``/v1/`` are considered, so internal
plumbing that forwards a ``path`` variable is never double-counted.
"""
from __future__ import annotations

import ast
import json
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "src" / "heyfood_cli"
SCAN_FILES = ("client.py", "auth.py")
CONTRACT_PATH = ROOT / "tests" / "fixtures" / "called_endpoints.json"
API_MARKER = "/v1/"
HTTP_METHODS = {"GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"}
VERB_ATTRS = {"get", "post", "put", "delete", "patch", "head", "options"}


def _normalize_path(path: str) -> str:
    """Collapse ``{anything}`` path parameters to a single placeholder so the
    contract can use readable names while the comparison stays positional."""
    out: list[str] = []
    depth = 0
    for char in path:
        if char == "{":
            depth += 1
            if depth == 1:
                out.append("{}")
            continue
        if char == "}":
            if depth > 0:
                depth -= 1
            continue
        if depth == 0:
            out.append(char)
    return "".join(out)


def _literal_path(node: ast.AST) -> str | None:
    """Reconstruct a URL/path literal, keeping ``/v1/...`` and only if present."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        text = node.value
    elif isinstance(node, ast.JoinedStr):
        parts: list[str] = []
        for value in node.values:
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                parts.append(value.value)
            else:
                parts.append("{param}")
        text = "".join(parts)
    else:
        return None
    index = text.find(API_MARKER)
    if index == -1:
        return None
    return text[index:]


def _literal_method(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        method = node.value.upper()
        if method in HTTP_METHODS:
            return method
    return None


def _endpoints_in_source() -> set[tuple[str, str]]:
    found: set[tuple[str, str]] = set()
    for name in SCAN_FILES:
        tree = ast.parse((PACKAGE / name).read_text())
        for call in ast.walk(tree):
            if not isinstance(call, ast.Call):
                continue
            func = call.func
            attr = func.attr if isinstance(func, ast.Attribute) else None
            func_name = func.id if isinstance(func, ast.Name) else None
            args = call.args

            method: str | None = None
            path_node: ast.AST | None = None

            if attr == "_request" and len(args) >= 2:
                method = _literal_method(args[0])
                path_node = args[1]
            elif attr in {"build_request", "stream", "request"} and len(args) >= 2:
                method = _literal_method(args[0])
                path_node = args[1]
            elif attr in VERB_ATTRS and len(args) >= 1:
                method = attr.upper()
                path_node = args[0]
            elif func_name == "_post_with_diagnostics" and len(args) >= 2:
                method = "POST"
                path_node = args[1]

            if method is None or path_node is None:
                continue
            path = _literal_path(path_node)
            if path is None:
                continue
            found.add((method, _normalize_path(path)))
    return found


def _load_contract() -> list[dict]:
    document = json.loads(CONTRACT_PATH.read_text())
    return document["endpoints"]


def _contract_pairs() -> set[tuple[str, str]]:
    return {
        (str(item["method"]).upper(), _normalize_path(str(item["endpoint"])))
        for item in _load_contract()
    }


def test_contract_is_wellformed() -> None:
    contract = _load_contract()
    seen: set[tuple[str, str]] = set()
    for item in contract:
        assert item["method"].upper() in HTTP_METHODS, item
        assert item["endpoint"].startswith(API_MARKER), item
        assert item["auth"] in {"session", "channel", "none"}, item
        key = (item["method"].upper(), _normalize_path(item["endpoint"]))
        assert key not in seen, f"duplicate contract entry: {key}"
        seen.add(key)


def test_no_uncontracted_endpoint_call_slips_in() -> None:
    source = _endpoints_in_source()
    contract = _contract_pairs()
    missing = sorted(source - contract)
    assert not missing, (
        "These (method, endpoint) calls exist in the CLI source but are NOT in "
        "tests/fixtures/called_endpoints.json. Add each one (with its auth and a "
        "note) so the CLI's endpoint surface stays reviewable and the backend "
        f"reachability contract can cover it: {missing}"
    )


def test_contract_has_no_stale_endpoints() -> None:
    source = _endpoints_in_source()
    contract = _contract_pairs()
    stale = sorted(contract - source)
    assert not stale, (
        "These (method, endpoint) entries are in tests/fixtures/called_endpoints.json "
        "but no longer appear in the CLI source. Remove them so the contract stays "
        f"exhaustive and accurate: {stale}"
    )


def test_transcription_endpoint_is_covered() -> None:
    # The endpoint this release adds must be represented explicitly.
    assert ("POST", "/v1/audio/transcriptions") in _contract_pairs()
    assert ("POST", "/v1/audio/transcriptions") in _endpoints_in_source()


def test_v0_2_0_regression_endpoint_is_representable() -> None:
    # The v0.2.0 household-list 403 was GET /v1/profile/sync/members not being
    # admitted for least-privilege CLI sessions. It must be an enumerated case.
    assert ("GET", "/v1/profile/sync/members") in _contract_pairs()
    assert ("GET", "/v1/profile/sync/members") in _endpoints_in_source()
