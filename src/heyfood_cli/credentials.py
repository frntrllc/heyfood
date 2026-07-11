"""Pluggable secret storage for heyfood CLI credentials."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Protocol


class CredentialStore(Protocol):
    name: str

    def load(self) -> dict[str, str]: ...

    def save(self, secrets: dict[str, str]) -> None: ...

    def delete(self) -> None: ...


class KeyringCredentialStore:
    name = "keyring"
    service_name = "heyfood-cli"

    def __init__(self, config_path: Path):
        import keyring  # type: ignore[import-not-found]

        self._keyring = keyring
        path_hash = hashlib.sha256(str(config_path.resolve()).encode("utf-8")).hexdigest()
        self._username = f"config-{path_hash[:20]}"

    def load(self) -> dict[str, str]:
        raw = self._keyring.get_password(self.service_name, self._username)
        if not raw:
            return {}
        data = json.loads(raw)
        if not isinstance(data, dict):
            return {}
        return {str(key): str(value) for key, value in data.items() if value}

    def save(self, secrets: dict[str, str]) -> None:
        self._keyring.set_password(
            self.service_name,
            self._username,
            json.dumps(secrets, sort_keys=True),
        )

    def delete(self) -> None:
        try:
            self._keyring.delete_password(self.service_name, self._username)
        except Exception:
            # Missing entries and unavailable backends are already equivalent
            # to deleted credentials.
            pass


def preferred_credential_store(config_path: Path) -> CredentialStore | None:
    """Return the OS keyring adapter when installed and usable."""
    preference = os.environ.get("HEYFOOD_CREDENTIAL_STORE", "auto").strip().lower()
    if preference == "file":
        return None
    try:
        store = KeyringCredentialStore(config_path)
        # A read is a safe capability probe; some headless keyring backends
        # import successfully but fail on first operation.
        store.load()
        return store
    except Exception:
        if preference == "keyring":
            raise RuntimeError(
                "HEYFOOD_CREDENTIAL_STORE=keyring was requested, but no usable "
                "OS keyring is available."
            )
        return None
