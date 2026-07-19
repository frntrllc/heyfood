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
OFFICIAL_API_URL = "https://api.hello.food"
# Stable public OAuth identifier for the official native CLI. This is not a
# secret: PKCE and the server-owned loopback redirect policy protect the flow.
OFFICIAL_CLI_OAUTH_CLIENT_ID = "hf_cid_heyfood_cli"
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
ACCOUNT_SCOPED_CONFIG_KEYS = {
    "first_name",
    "first_name_updated_at",
    "welcomed_at",
    "household",
    "household_local_profiles",
    "household_profile_outbox",
    "last_conversation",
    "last_recipe_search",
    "last_restaurant_search",
    "location",
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


EXACT_LOOPBACK_HOSTS = frozenset({"localhost", "127.0.0.1", "::1"})


def is_exact_loopback_host(host: str | None) -> bool:
    """True only for an exact loopback host.

    Deliberately exact: ``127.0.0.1`` and ``localhost`` qualify, but
    ``localhost.evil.example`` or ``127.0.0.1.evil.example`` do not, so a
    look-alike host can never be treated as a development loopback and waved
    through the HTTPS requirement.
    """
    value = (host or "").strip().lower()
    if value.startswith("[") and value.endswith("]"):
        value = value[1:-1]
    return value in EXACT_LOOPBACK_HOSTS


def validate_service_url(url: str, *, field: str = "Service URL") -> str:
    """Enforce the transport contract for any API/auth URL at every ingress.

    Remote (non-loopback) hosts must use verified HTTPS; only an exact loopback
    development host may use plain HTTP. URL userinfo, fragments, and unexpected
    base-URL query strings are rejected. Returns the trimmed URL, or raises
    :class:`ConfigError`.
    """
    from urllib.parse import urlsplit

    if not isinstance(url, str) or not url.strip():
        raise ConfigError(f"{field} must be a non-empty URL.")
    candidate = url.strip()
    parts = urlsplit(candidate)
    if parts.scheme not in {"http", "https"}:
        raise ConfigError(
            f"{field} must use http or https, not "
            f"'{parts.scheme or 'an empty scheme'}'."
        )
    if not parts.hostname:
        raise ConfigError(f"{field} must include a host.")
    if parts.username or parts.password:
        raise ConfigError(f"{field} must not embed credentials (userinfo).")
    if parts.fragment:
        raise ConfigError(f"{field} must not contain a URL fragment.")
    if parts.query:
        raise ConfigError(f"{field} must not contain a query string.")
    if parts.scheme == "http" and not is_exact_loopback_host(parts.hostname):
        raise ConfigError(
            f"{field} must use https for the remote host '{parts.hostname}'; "
            "plain http is allowed only for an exact loopback development host."
        )
    return candidate


def is_local_api_url(api_url: str) -> bool:
    from urllib.parse import urlsplit

    parts = urlsplit(api_url)
    return parts.scheme == "http" and is_exact_loopback_host(parts.hostname)


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
    # Enforce the transport contract at resolution time, so a bad URL from any
    # ingress (env, stored config, or a named context) fails closed here rather
    # than reaching an httpx client that would happily talk plain HTTP to a
    # remote host.
    api_url = validate_service_url(api_url, field="API URL")
    auth_url = validate_service_url(auth_url, field="Auth URL")
    return api_url.rstrip("/"), auth_url.rstrip("/"), context_name


def bind_config_to_account(config: dict[str, Any], user_id: str) -> bool:
    """Bind account-scoped local state to one authenticated principal.

    Returns True when state from another or unbound account was removed. The
    fail-closed unbound case protects upgrades from older CLI builds that did
    not persist an account identity beside household data.
    """
    normalized_user_id = str(user_id or "").strip()
    if not normalized_user_id:
        raise ConfigError("Authenticated session did not identify an account.")
    session = config.get("session")
    prior = config.get("account_user_id")
    if not prior and isinstance(session, dict):
        prior = session.get("user_id")
    prior_id = str(prior or "").strip()
    has_account_state = any(key in config for key in ACCOUNT_SCOPED_CONFIG_KEYS)
    should_clear = has_account_state and prior_id != normalized_user_id
    if should_clear:
        for key in ACCOUNT_SCOPED_CONFIG_KEYS:
            config.pop(key, None)
    config["account_user_id"] = normalized_user_id
    return should_clear


def redacted_config(config: dict[str, Any]) -> dict[str, Any]:
    document = json.loads(json.dumps(config))
    if document.get("account_user_id"):
        document["account_user_id"] = "<redacted>"
    if document.get("api_key"):
        document["api_key"] = "<redacted>"
    for bundle_name in ("oauth", "session"):
        bundle = document.get(bundle_name)
        if not isinstance(bundle, dict):
            continue
        for token_name in ("access_token", "refresh_token"):
            if bundle.get(token_name):
                bundle[token_name] = "<redacted>"
    household_state = document.get("household")
    if isinstance(household_state, dict):
        members = household_state.get("members")
        document["household"] = {
            "version": household_state.get("version"),
            "active_scope": "<redacted>",
            "member_count": len(members) if isinstance(members, list) else 0,
            "local_roster": "<redacted>",
        }
    local_profiles = document.get("household_local_profiles")
    if isinstance(local_profiles, dict):
        document["household_local_profiles"] = {
            "profile_count": len(local_profiles),
            "dietary_data": "<redacted>",
        }
    profile_outbox = document.get("household_profile_outbox")
    if isinstance(profile_outbox, dict):
        document["household_profile_outbox"] = {
            "entry_count": len(profile_outbox),
            "dietary_data": "<redacted>",
        }
    last_conversation = document.get("last_conversation")
    if isinstance(last_conversation, dict):
        pending = last_conversation.get("pending_confirmation")
        document["last_conversation"] = {
            "conversation_id": "<redacted>",
            "household_scope_id": (
                "<redacted>" if last_conversation.get("household_scope_id") else None
            ),
            "updated_at": last_conversation.get("updated_at"),
            "pending_confirmation": (
                {
                    "present": True,
                    "action": pending.get("action"),
                    "details": "<redacted>",
                }
                if isinstance(pending, dict)
                else None
            ),
        }
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
        protected_secrets = self._extract_protected_secrets(document)
        secrets = (
            self._extract_secrets(document)
            if self.credential_store is not None
            else {}
        )
        secrets.update(protected_secrets)
        if self.credential_store is not None:
            try:
                self.credential_store.save(secrets)
                if secrets:
                    document["credential_store"] = self.credential_store.name
                else:
                    document.pop("credential_store", None)
            except Exception:
                # Ordinary credentials and household state may use the
                # documented 0600 fallback. Confirmation previews are more
                # sensitive: drop them rather than writing dietary PII to a
                # plaintext config file when the vault is unavailable.
                document = json.loads(json.dumps(data))
                self._strip_protected_secrets(document)
                if self._contains_secrets(document):
                    document["credential_store"] = "file"
                else:
                    document.pop("credential_store", None)
        else:
            self._strip_protected_secrets(document)
            if self._contains_secrets(document):
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
        # Household identity can include names and dates of birth. Child
        # profiles are deliberately local-only, and failed adult sync writes
        # remain local as a repair outbox. Keep all of that in the OS vault
        # whenever available; the documented 0600 fallback remains for
        # headless environments.
        for field_name, secret_name in (
            ("household", "household.state"),
            ("household_local_profiles", "household.local_profiles"),
            ("household_profile_outbox", "household.profile_outbox"),
        ):
            value = document.pop(field_name, None)
            if isinstance(value, dict) and value:
                secrets[secret_name] = json.dumps(value, sort_keys=True)
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
        if isinstance(document.get("household"), dict) and document["household"]:
            return True
        if (
            isinstance(document.get("household_local_profiles"), dict)
            and document["household_local_profiles"]
        ):
            return True
        if (
            isinstance(document.get("household_profile_outbox"), dict)
            and document["household_profile_outbox"]
        ):
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
        for field_name, secret_name in (
            ("household", "household.state"),
            ("household_local_profiles", "household.local_profiles"),
            ("household_profile_outbox", "household.profile_outbox"),
        ):
            raw = secrets.get(secret_name)
            if not raw:
                continue
            try:
                value = json.loads(raw)
            except (TypeError, json.JSONDecodeError):
                continue
            if isinstance(value, dict):
                document[field_name] = value
        raw_preview = secrets.get("conversation.pending_preview")
        if raw_preview:
            try:
                preview = json.loads(raw_preview)
            except (TypeError, json.JSONDecodeError):
                preview = None
            conversation = document.get("last_conversation")
            pending = (
                conversation.get("pending_confirmation")
                if isinstance(conversation, dict)
                else None
            )
            if (
                isinstance(preview, dict)
                and isinstance(pending, dict)
                and preview.get("conversation_id") == conversation.get("conversation_id")
                and preview.get("confirmation_id") == pending.get("confirmation_id")
            ):
                for key in ("preview", "structured_preview"):
                    if key in preview:
                        pending[key] = preview[key]

    @staticmethod
    def _extract_protected_secrets(document: dict[str, Any]) -> dict[str, str]:
        conversation = document.get("last_conversation")
        pending = (
            conversation.get("pending_confirmation")
            if isinstance(conversation, dict)
            else None
        )
        if not isinstance(pending, dict):
            return {}
        payload = {
            "conversation_id": conversation.get("conversation_id"),
            "confirmation_id": pending.get("confirmation_id"),
        }
        found = False
        for key in ("preview", "structured_preview"):
            if key in pending:
                payload[key] = pending.pop(key)
                found = True
        if not found:
            return {}
        return {
            "conversation.pending_preview": json.dumps(payload, sort_keys=True),
        }

    @staticmethod
    def _strip_protected_secrets(document: dict[str, Any]) -> None:
        ConfigStore._extract_protected_secrets(document)
