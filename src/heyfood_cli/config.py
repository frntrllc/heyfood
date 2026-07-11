from __future__ import annotations

import json
import os
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4

from .credentials import CredentialStore, preferred_credential_store

try:
    import fcntl
except ImportError:  # pragma: no cover - Windows is not a release target yet.
    fcntl = None  # type: ignore[assignment]


# Wire-protocol identifier recorded on user_devices rows by the CLI session
# exchange. The backend stamps this value itself; keep the two in sync
# (channel_oauth.py exchange_cli_session).
APP_CLIENT_ID = "heyfood-cli"
DEFAULT_API_URL = os.environ.get("HEYFOOD_API_URL", "https://api.hello.food")
DEFAULT_AUTH_URL = os.environ.get(
    "HEYFOOD_AUTH_URL",
    "https://auth.hello.food/authorize",
)
DEFAULT_API_KEY = os.environ.get("HEYFOOD_API_KEY", "")
DEFAULT_LOCAL_API_URL = "http://localhost:8000"
DEFAULT_LOCAL_AUTH_URL = "http://localhost:3002/authorize"
TOKEN_EXPIRY_SKEW = timedelta(seconds=45)
BUILTIN_CONTEXTS: dict[str, dict[str, str]] = {
    "production": {
        "api_url": "https://api.hello.food",
        "auth_url": "https://auth.hello.food/authorize",
    },
    "local": {
        "api_url": DEFAULT_LOCAL_API_URL,
        "auth_url": DEFAULT_LOCAL_AUTH_URL,
    },
}


class ConfigError(RuntimeError):
    pass


def configured_config_path() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME")
    root = Path(base) if base else Path.home() / ".config"
    return root / "heyfood" / "config.json"


def default_config_path() -> Path:
    path = configured_config_path()
    root = path.parent.parent
    legacy = root / "hellofood" / "config.json"
    if not path.exists() and legacy.exists():
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        legacy.replace(path)
    return path


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


def parse_datetime(value: str | None) -> datetime | None:
    if not value:
        return None
    normalized = value.replace("Z", "+00:00")
    parsed = datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def is_expiring(value: str | None) -> bool:
    expires_at = parse_datetime(value)
    if expires_at is None:
        return True
    return expires_at <= utcnow() + TOKEN_EXPIRY_SKEW


def expires_in_to_iso(seconds: int) -> str:
    return (utcnow() + timedelta(seconds=max(1, seconds))).isoformat()


def discover_local_api_key(start: Path | None = None) -> str | None:
    """Find API_KEY in a nearby backend/.env for local developer workflows."""
    current = (start or Path.cwd()).resolve()
    candidates = [current, *current.parents]
    for directory in candidates:
        env_path = directory / "backend" / ".env"
        if not env_path.exists():
            continue
        try:
            for line in env_path.read_text(encoding="utf-8").splitlines():
                stripped = line.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                if stripped.startswith("API_KEY="):
                    value = stripped.split("=", 1)[1].strip().strip('"').strip("'")
                    return value or None
        except OSError:
            return None
    return None


def is_local_api_url(api_url: str) -> bool:
    return api_url.startswith("http://localhost") or api_url.startswith("http://127.0.0.1")


def configured_contexts(config: dict[str, Any]) -> dict[str, dict[str, str]]:
    contexts = {name: dict(value) for name, value in BUILTIN_CONTEXTS.items()}
    custom = config.get("contexts")
    if isinstance(custom, dict):
        for name, value in custom.items():
            if not isinstance(name, str) or not isinstance(value, dict):
                continue
            api_url = value.get("api_url")
            auth_url = value.get("auth_url")
            if isinstance(api_url, str) and isinstance(auth_url, str):
                contexts[name] = {"api_url": api_url, "auth_url": auth_url}
    return contexts


def resolve_service_urls(config: dict[str, Any]) -> tuple[str, str, str]:
    contexts = configured_contexts(config)
    active = config.get("active_context")
    context_name = active if isinstance(active, str) and active in contexts else "production"
    context = contexts[context_name]
    stored_api = config.get("api_url")
    stored_auth = config.get("auth_url")
    api_url = os.environ.get("HEYFOOD_API_URL") or (
        stored_api if isinstance(stored_api, str) and stored_api else context["api_url"]
    )
    auth_url = os.environ.get("HEYFOOD_AUTH_URL") or (
        stored_auth if isinstance(stored_auth, str) and stored_auth else context["auth_url"]
    )
    return api_url.rstrip("/"), auth_url.rstrip("/"), context_name


def redacted_config(config: dict[str, Any]) -> dict[str, Any]:
    document = json.loads(json.dumps(config))
    if document.get("api_key"):
        document["api_key"] = "<redacted>"
    for bundle_name in ("oauth", "session"):
        bundle = document.get(bundle_name)
        if not isinstance(bundle, dict):
            continue
        for token_name in ("access_token", "refresh_token"):
            if bundle.get(token_name):
                bundle[token_name] = "<redacted>"
    return document


class ConfigStore:
    def __init__(
        self,
        path: Path | None = None,
        credential_store: CredentialStore | None | object = ...,
    ):
        self.path = path or default_config_path()
        self.credential_store = (
            preferred_credential_store(self.path)
            if credential_store is ...
            else credential_store
        )
        self._snapshot: dict[str, Any] | None = None

    def load(self) -> dict[str, Any]:
        if not self.path.exists():
            self._snapshot = {}
            return {}
        with self._file_lock(exclusive=False):
            data = self._load_unlocked()
        self._snapshot = json.loads(json.dumps(data))
        return data

    def _load_unlocked(self) -> dict[str, Any]:
        if not self.path.exists():
            return {}
        try:
            with self.path.open("r", encoding="utf-8") as handle:
                data = json.load(handle)
        except json.JSONDecodeError as exc:
            raise ConfigError(
                f"Invalid JSON in {self.path} at line {exc.lineno}, column {exc.colno}. "
                "Run `heyfood config validate` for repair guidance."
            ) from exc
        except OSError as exc:
            raise ConfigError(f"Could not read {self.path}: {exc}") from exc
        if not isinstance(data, dict):
            raise ConfigError(
                f"Invalid configuration in {self.path}: the top-level JSON value must be an object."
            )
        if data.get("credential_store") == "keyring" and self.credential_store is not None:
            try:
                self._merge_secrets(data, self.credential_store.load())
            except Exception:
                # Keep non-secret configuration usable. Authenticated commands
                # will provide the normal login-required guidance.
                pass
        return data

    def save(self, data: dict[str, Any]) -> None:
        with self._file_lock(exclusive=True):
            current = self._load_unlocked()
            effective = self._merge_changed_top_level(current, data)
            self._save_unlocked(effective)
        self._snapshot = json.loads(json.dumps(effective))

    def _save_unlocked(self, data: dict[str, Any]) -> None:
        document = json.loads(json.dumps(data))
        secrets = (
            self._extract_secrets(document)
            if self.credential_store is not None
            else {}
        )
        if self.credential_store is not None and secrets:
            try:
                self.credential_store.save(secrets)
                document["credential_store"] = self.credential_store.name
            except Exception:
                # Headless/locked keyrings degrade to the existing 0600 file.
                document = json.loads(json.dumps(data))
                document["credential_store"] = "file"
        elif self.credential_store is None and self._contains_secrets(document):
            document["credential_store"] = "file"

        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".tmp")
        with tmp.open("w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2, sort_keys=True)
            handle.write("\n")
        os.chmod(tmp, 0o600)
        tmp.replace(self.path)
        os.chmod(self.path, 0o600)

    def delete(self) -> None:
        if not self.path.exists():
            if self.credential_store is not None:
                self.credential_store.delete()
            self._snapshot = None
            return
        with self._file_lock(exclusive=True):
            if self.credential_store is not None:
                self.credential_store.delete()
            if self.path.exists():
                self.path.unlink()
        self._snapshot = None

    def get_device_id(self) -> str:
        data = self.load()
        device_id = data.get("device_id")
        if isinstance(device_id, str) and device_id:
            return device_id
        device_id = f"heyfood-cli-{uuid4()}"
        data["device_id"] = device_id
        self.save(data)
        return device_id

    def _merge_changed_top_level(
        self,
        current: dict[str, Any],
        incoming: dict[str, Any],
    ) -> dict[str, Any]:
        if self._snapshot is None or not current:
            return json.loads(json.dumps(incoming))
        merged = json.loads(json.dumps(current))
        for key in set(self._snapshot) | set(incoming):
            before_present = key in self._snapshot
            after_present = key in incoming
            before = self._snapshot.get(key)
            after = incoming.get(key)
            if before_present == after_present and before == after:
                continue
            if after_present:
                merged[key] = json.loads(json.dumps(after))
            else:
                merged.pop(key, None)
        return merged

    @contextmanager
    def _file_lock(self, *, exclusive: bool):
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        lock_path = self.path.with_suffix(".lock")
        with lock_path.open("a+", encoding="utf-8") as handle:
            os.chmod(lock_path, 0o600)
            if fcntl is not None:
                operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
                fcntl.flock(handle.fileno(), operation)
            try:
                yield
            finally:
                if fcntl is not None:
                    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)

    @staticmethod
    def _extract_secrets(document: dict[str, Any]) -> dict[str, str]:
        secrets: dict[str, str] = {}
        api_key = document.pop("api_key", None)
        if isinstance(api_key, str) and api_key:
            secrets["api_key"] = api_key
        for bundle_name in ("oauth", "session"):
            bundle = document.get(bundle_name)
            if not isinstance(bundle, dict):
                continue
            for token_name in ("access_token", "refresh_token"):
                value = bundle.pop(token_name, None)
                if isinstance(value, str) and value:
                    secrets[f"{bundle_name}.{token_name}"] = value
        return secrets

    @staticmethod
    def _contains_secrets(document: dict[str, Any]) -> bool:
        if isinstance(document.get("api_key"), str) and document["api_key"]:
            return True
        for bundle_name in ("oauth", "session"):
            bundle = document.get(bundle_name)
            if not isinstance(bundle, dict):
                continue
            if any(bundle.get(name) for name in ("access_token", "refresh_token")):
                return True
        return False

    @staticmethod
    def _merge_secrets(document: dict[str, Any], secrets: dict[str, str]) -> None:
        api_key = secrets.get("api_key")
        if api_key:
            document["api_key"] = api_key
        for bundle_name in ("oauth", "session"):
            for token_name in ("access_token", "refresh_token"):
                value = secrets.get(f"{bundle_name}.{token_name}")
                if not value:
                    continue
                bundle = document.setdefault(bundle_name, {})
                if isinstance(bundle, dict):
                    bundle[token_name] = value
