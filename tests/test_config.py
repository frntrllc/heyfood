import json
import stat
import threading

import pytest

from heyfood_cli.config import (
    ConfigError,
    ConfigStore,
    discover_local_api_key,
    redacted_config,
    resolve_service_urls,
)
from heyfood_cli.client import HelloFoodClient


def test_config_store_persists_device_id(tmp_path):
    store = ConfigStore(tmp_path / "config.json")
    first = store.get_device_id()
    second = store.get_device_id()
    assert first == second
    assert first.startswith("heyfood-cli-")


def test_client_remembers_last_conversation(tmp_path):
    store = ConfigStore(tmp_path / "config.json")
    client = HelloFoodClient(store)
    client.remember_conversation(
        {
            "conversation_id": "123",
            "structured": {"type": "general_response"},
        }
    )
    assert client.last_conversation_id() == "123"
    assert client.pending_confirmation() is None


def test_client_remembers_pending_confirmation(tmp_path):
    store = ConfigStore(tmp_path / "config.json")
    client = HelloFoodClient(store)
    client.remember_conversation(
        {
            "conversation_id": "123",
            "structured": {
                "type": "action_confirmation",
                "confirmation_id": "confirm-1",
                "idempotency_key": "idem-1",
            },
        }
    )
    assert client.pending_confirmation() == {
        "confirmation_id": "confirm-1",
        "idempotency_key": "idem-1",
    }


def test_discovers_local_api_key(tmp_path, monkeypatch):
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / ".env").write_text("DEBUG=false\nAPI_KEY=local-test-key\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)

    assert discover_local_api_key() == "local-test-key"


def test_client_backfills_local_api_key(tmp_path, monkeypatch):
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / ".env").write_text("API_KEY=local-test-key\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)

    store = ConfigStore(tmp_path / "config.json")
    store.save({"api_url": "http://localhost:8000"})
    client = HelloFoodClient(store)

    assert client.config["api_key"] == "local-test-key"
    assert client._headers()["X-API-Key"] == "local-test-key"


class FakeCredentialStore:
    name = "keyring"

    def __init__(self, *, fail_save: bool = False):
        self.secrets = {}
        self.fail_save = fail_save
        self.deleted = False

    def load(self):
        return dict(self.secrets)

    def save(self, secrets):
        if self.fail_save:
            raise RuntimeError("keyring locked")
        self.secrets = dict(secrets)

    def delete(self):
        self.deleted = True
        self.secrets = {}


def _authenticated_config():
    return {
        "api_url": "https://api.hello.food",
        "api_key": "local-api-key",
        "device_id": "device-1",
        "oauth": {
            "client_id": "client-1",
            "access_token": "hf_ct_secret",
            "refresh_token": "hf_cr_secret",
        },
        "session": {
            "access_token": "hf_at_secret",
            "refresh_token": "hf_rt_secret",
        },
    }


def test_keyring_store_keeps_tokens_out_of_config_file(tmp_path):
    credentials = FakeCredentialStore()
    path = tmp_path / "config.json"
    store = ConfigStore(path, credential_store=credentials)
    store.save(_authenticated_config())

    raw = path.read_text(encoding="utf-8")
    assert "hf_ct_secret" not in raw
    assert "hf_rt_secret" not in raw
    assert "local-api-key" not in raw
    assert json.loads(raw)["credential_store"] == "keyring"
    assert store.load()["session"]["access_token"] == "hf_at_secret"
    assert credentials.secrets["oauth.refresh_token"] == "hf_cr_secret"


def test_unavailable_keyring_falls_back_to_mode_0600_file(tmp_path):
    path = tmp_path / "config.json"
    store = ConfigStore(path, credential_store=FakeCredentialStore(fail_save=True))
    store.save(_authenticated_config())

    raw = path.read_text(encoding="utf-8")
    assert "hf_rt_secret" in raw
    assert json.loads(raw)["credential_store"] == "file"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600


def test_new_login_while_keyring_unavailable_replaces_stale_marker(tmp_path):
    path = tmp_path / "config.json"
    path.write_text(
        json.dumps({"device_id": "device-1", "credential_store": "keyring"}),
        encoding="utf-8",
    )
    store = ConfigStore(path, credential_store=None)
    data = store.load()
    data["session"] = {"access_token": "hf_at_new", "refresh_token": "hf_rt_new"}
    store.save(data)

    written = json.loads(path.read_text(encoding="utf-8"))
    assert written["credential_store"] == "file"
    assert written["session"]["refresh_token"] == "hf_rt_new"


def test_logout_deletes_keyring_credentials_and_file(tmp_path):
    credentials = FakeCredentialStore()
    path = tmp_path / "config.json"
    store = ConfigStore(path, credential_store=credentials)
    store.save(_authenticated_config())
    store.delete()

    assert credentials.deleted is True
    assert not path.exists()


def test_stale_writer_cannot_erase_refreshed_tokens_or_saved_state(tmp_path):
    path = tmp_path / "config.json"
    initial = ConfigStore(path, credential_store=None)
    initial.save(_authenticated_config())

    refresh_writer = ConfigStore(path, credential_store=None)
    state_writer = ConfigStore(path, credential_store=None)
    refreshed = refresh_writer.load()
    state = state_writer.load()
    refreshed["session"] = {
        "access_token": "hf_at_refreshed",
        "refresh_token": "hf_rt_refreshed",
    }
    refresh_writer.save(refreshed)
    state["location"] = {"label": "Fresno", "latitude": 36.7, "longitude": -119.8}
    state_writer.save(state)

    final = ConfigStore(path, credential_store=None).load()
    assert final["session"]["refresh_token"] == "hf_rt_refreshed"
    assert final["location"]["label"] == "Fresno"


def test_parallel_writers_merge_disjoint_top_level_changes(tmp_path):
    path = tmp_path / "config.json"
    ConfigStore(path, credential_store=None).save({"device_id": "device-1"})
    barrier = threading.Barrier(2)

    def write(key, value):
        store = ConfigStore(path, credential_store=None)
        data = store.load()
        barrier.wait()
        data[key] = value
        store.save(data)

    first = threading.Thread(target=write, args=("location", {"label": "Fresno"}))
    second = threading.Thread(target=write, args=("last_conversation", {"id": "conv-1"}))
    first.start()
    second.start()
    first.join()
    second.join()

    final = ConfigStore(path, credential_store=None).load()
    assert final["location"] == {"label": "Fresno"}
    assert final["last_conversation"] == {"id": "conv-1"}


def test_explicit_top_level_clear_survives_merge_strategy(tmp_path):
    path = tmp_path / "config.json"
    store = ConfigStore(path, credential_store=None)
    store.save({"device_id": "device-1", "location": {"label": "Fresno"}})
    data = store.load()
    data.pop("location")
    store.save(data)
    assert "location" not in store.load()


def test_named_context_and_environment_precedence(monkeypatch):
    config = {
        "active_context": "staging",
        "contexts": {
            "staging": {
                "api_url": "https://api.staging.example",
                "auth_url": "https://auth.staging.example/authorize",
            }
        },
    }
    assert resolve_service_urls(config) == (
        "https://api.staging.example",
        "https://auth.staging.example/authorize",
        "staging",
    )
    monkeypatch.setenv("HEYFOOD_API_URL", "https://api.env.example/")
    assert resolve_service_urls(config)[0] == "https://api.env.example"


def test_redacted_config_never_returns_tokens_or_api_key():
    redacted = redacted_config(_authenticated_config())
    assert redacted["api_key"] == "<redacted>"
    assert redacted["oauth"]["access_token"] == "<redacted>"
    assert redacted["session"]["refresh_token"] == "<redacted>"
    assert "secret" not in json.dumps(redacted)


def test_malformed_config_has_repair_guidance(tmp_path):
    path = tmp_path / "config.json"
    path.write_text('{"broken":', encoding="utf-8")
    with pytest.raises(ConfigError, match="config validate"):
        ConfigStore(path, credential_store=None).load()
