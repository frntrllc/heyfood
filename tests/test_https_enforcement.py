"""HTTPS-at-every-ingress enforcement (blocker 4 / invariants 7 and 11)."""
from __future__ import annotations

import pytest

from heyfood_cli.config import (
    ConfigError,
    is_exact_loopback_host,
    is_local_api_url,
    resolve_service_urls,
    validate_service_url,
)


@pytest.mark.parametrize(
    "url",
    [
        "https://api.hello.food",
        "https://auth.hello.food/authorize",
        "http://localhost:8000",
        "http://127.0.0.1:3002/authorize",
        "http://[::1]:8000",
    ],
)
def test_accepts_https_and_exact_loopback_http(url):
    assert validate_service_url(url) == url


@pytest.mark.parametrize(
    "url",
    [
        "http://api.hello.food",  # remote plain http
        "http://localhost.evil.example",  # loopback look-alike
        "http://127.0.0.1.evil.example/x",  # loopback look-alike
        "https://user:pass@api.hello.food",  # userinfo
        "https://api.hello.food/#frag",  # fragment
        "https://api.hello.food/?token=abc",  # base query
        "ftp://api.hello.food",  # non-http scheme
        "not-a-url",
        "",
    ],
)
def test_rejects_unsafe_urls(url):
    with pytest.raises(ConfigError):
        validate_service_url(url)


def test_is_exact_loopback_host():
    assert is_exact_loopback_host("localhost")
    assert is_exact_loopback_host("127.0.0.1")
    assert is_exact_loopback_host("::1")
    assert not is_exact_loopback_host("localhost.evil.example")
    assert not is_exact_loopback_host("10.0.0.5")
    assert not is_exact_loopback_host("")


def test_is_local_api_url_requires_http_loopback():
    assert is_local_api_url("http://localhost:8000")
    assert is_local_api_url("http://127.0.0.1:8000")
    assert not is_local_api_url("https://localhost:8000")
    assert not is_local_api_url("http://api.hello.food")


def test_resolve_service_urls_rejects_stored_plain_http_remote():
    config = {"api_url": "http://api.hello.food", "auth_url": "https://auth.hello.food/authorize"}
    with pytest.raises(ConfigError):
        resolve_service_urls(config)


def test_resolve_service_urls_rejects_env_plain_http_remote(monkeypatch):
    monkeypatch.setenv("HEYFOOD_API_URL", "http://api.hello.food")
    with pytest.raises(ConfigError):
        resolve_service_urls({})


def test_resolve_service_urls_allows_local_context():
    api, auth, name = resolve_service_urls({"active_context": "local"})
    assert api == "http://localhost:8000"
    assert name == "local"
