from __future__ import annotations

import base64
import hashlib
import secrets
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, Callable
from urllib.parse import parse_qs, urlencode, urlparse
from uuid import uuid4

import httpx

from .config import (
    APP_CLIENT_ID,
    ConfigStore,
    DEFAULT_API_KEY,
    DEFAULT_AUTH_URL,
    DEFAULT_LOCAL_API_URL,
    DEFAULT_LOCAL_AUTH_URL,
    discover_local_api_key,
    expires_in_to_iso,
    bind_config_to_account,
    is_local_api_url,
)
from . import diagnostics


LOGIN_SCOPES = [
    "account:link",
    "knowledge:read",
    "menu:read",
    "recommend:read",
    "recipes:read",
    "recipes:write",
    "claims:read_derived",
    "profile:read",
    "profile:write",
    "meals:read",
    "meals:write",
]


class LoginFlowError(RuntimeError):
    pass


def _post_with_diagnostics(
    api_url: str,
    path: str,
    *,
    json_body: dict[str, Any],
    headers: dict[str, str] | None = None,
    client: httpx.Client | None = None,
) -> httpx.Response:
    request_id = str(uuid4())
    started_at = time.monotonic()
    request_headers = dict(headers or {})
    request_headers["X-Request-ID"] = request_id
    diagnostics.reporter.emit(
        "http.start",
        request_id=request_id,
        context="authentication",
        method="POST",
        endpoint=path,
    )
    try:
        if client is not None:
            response = client.post(
                f"{api_url}{path}",
                headers=request_headers,
                json=json_body,
            )
        else:
            with httpx.Client(timeout=20.0) as request_client:
                response = request_client.post(
                    f"{api_url}{path}",
                    headers=request_headers,
                    json=json_body,
                )
    except httpx.HTTPError as exc:
        diagnostics.reporter.emit(
            "http.error",
            request_id=request_id,
            context="authentication",
            method="POST",
            endpoint=path,
            duration_ms=round((time.monotonic() - started_at) * 1000, 1),
            error=type(exc).__name__,
        )
        raise
    diagnostics.reporter.emit(
        "http.complete",
        request_id=request_id,
        server_request_id=response.headers.get("X-Request-ID"),
        context="authentication",
        method="POST",
        endpoint=path,
        status=response.status_code,
        duration_ms=round((time.monotonic() - started_at) * 1000, 1),
    )
    return response


def perform_login(
    *,
    store: ConfigStore,
    api_url: str,
    auth_url: str,
    api_key: str | None,
    open_browser: bool,
    timeout_seconds: int,
    authorize_url_callback: Callable[[str], None] | None = None,
) -> dict[str, Any]:
    api_url = api_url.rstrip("/")
    auth_url = normalize_auth_url(auth_url)
    device_id = store.get_device_id()
    effective_api_key = api_key or DEFAULT_API_KEY
    if not effective_api_key and is_local_api_url(api_url):
        effective_api_key = discover_local_api_key() or ""

    try:
        callback_server = OAuthCallbackServer()
    except OSError as exc:
        raise LoginFlowError(
            "Could not start the local login callback server. "
            "Try `heyfood login --device` on restricted or remote systems."
        ) from exc

    with callback_server as callback:
        redirect_uri = f"http://127.0.0.1:{callback.port}/callback"
        verifier, challenge = pkce_pair()
        state = secrets.token_urlsafe(24)
        client_registration = register_client(api_url, redirect_uri)
        client_id = client_registration["client_id"]
        authorize_url = build_authorize_url(
            auth_url=auth_url,
            client_id=client_id,
            redirect_uri=redirect_uri,
            state=state,
            code_challenge=challenge,
        )

        if open_browser:
            try:
                opened = webbrowser.open(authorize_url)
            except webbrowser.Error:
                opened = False
            if not opened and authorize_url_callback:
                authorize_url_callback(authorize_url)
        elif authorize_url_callback:
            authorize_url_callback(authorize_url)

        callback_result = callback.wait(timeout_seconds)
        if callback_result.get("error"):
            error = str(callback_result["error"])
            if error == "access_denied":
                raise LoginFlowError("Login was denied in the browser.")
            raise LoginFlowError(f"Browser authorization failed: {error}")
        if callback_result.get("state") != state:
            raise LoginFlowError("OAuth state mismatch. Please try login again.")
        code = callback_result.get("code")
        if not code:
            raise LoginFlowError("Authorization code was not returned.")

    oauth_bundle = exchange_code(
        api_url=api_url,
        client_id=client_id,
        code=str(code),
        verifier=verifier,
        redirect_uri=redirect_uri,
    )
    session_bundle = exchange_cli_session(
        api_url=api_url,
        access_token=oauth_bundle["access_token"],
        device_id=device_id,
    )

    return _save_authenticated_config(
        store=store,
        api_url=api_url,
        auth_url=auth_url,
        api_key=effective_api_key,
        device_id=device_id,
        client_id=client_id,
        oauth_bundle=oauth_bundle,
        session_bundle=session_bundle,
    )


def perform_device_login(
    *,
    store: ConfigStore,
    api_url: str,
    auth_url: str,
    api_key: str | None,
    open_browser: bool,
    timeout_seconds: int,
    authorization_callback: Callable[[str, str], None],
) -> dict[str, Any]:
    """Authenticate without a loopback callback using a short user code."""
    api_url = api_url.rstrip("/")
    auth_url = normalize_auth_url(auth_url)
    device_id = store.get_device_id()
    effective_api_key = api_key or DEFAULT_API_KEY
    if not effective_api_key and is_local_api_url(api_url):
        effective_api_key = discover_local_api_key() or ""

    registration = register_client(api_url, "http://127.0.0.1:1/device-unused")
    client_id = str(registration["client_id"])
    authorization = start_device_authorization(api_url, client_id)
    verification_url = str(
        authorization.get("verification_uri_complete")
        or authorization.get("verification_uri")
        or auth_url
    )
    user_code = str(authorization["user_code"])
    authorization_callback(verification_url, user_code)
    if open_browser:
        try:
            webbrowser.open(verification_url)
        except webbrowser.Error:
            pass

    oauth_bundle = poll_device_authorization(
        api_url=api_url,
        client_id=client_id,
        device_code=str(authorization["device_code"]),
        interval_seconds=int(authorization.get("interval", 5)),
        timeout_seconds=min(timeout_seconds, int(authorization.get("expires_in", timeout_seconds))),
    )
    session_bundle = exchange_cli_session(
        api_url=api_url,
        access_token=oauth_bundle["access_token"],
        device_id=device_id,
    )
    return _save_authenticated_config(
        store=store,
        api_url=api_url,
        auth_url=auth_url,
        api_key=effective_api_key,
        device_id=device_id,
        client_id=client_id,
        oauth_bundle=oauth_bundle,
        session_bundle=session_bundle,
    )


def start_device_authorization(api_url: str, client_id: str) -> dict[str, Any]:
    try:
        response = _post_with_diagnostics(
            api_url,
            "/v1/channel/oauth/device/authorize",
            json_body={"client_id": client_id, "scope": " ".join(LOGIN_SCOPES)},
        )
    except httpx.HTTPError as exc:
        raise LoginFlowError(
            f"Could not reach the hello.food device authorization service: {exc}"
        ) from exc
    if response.status_code >= 400:
        raise LoginFlowError(_response_error(response))
    data = _response_json(response, "Device authorization")
    required = {"device_code", "user_code", "verification_uri", "expires_in", "interval"}
    if not isinstance(data, dict) or not required.issubset(data):
        raise LoginFlowError("Device authorization returned an unexpected response.")
    return data


def poll_device_authorization(
    *,
    api_url: str,
    client_id: str,
    device_code: str,
    interval_seconds: int,
    timeout_seconds: int,
) -> dict[str, Any]:
    deadline = time.monotonic() + max(1, timeout_seconds)
    interval = max(1, interval_seconds)
    with httpx.Client(timeout=20.0) as client:
        while time.monotonic() < deadline:
            try:
                response = _post_with_diagnostics(
                    api_url,
                    "/v1/channel/oauth/device/token",
                    json_body={"client_id": client_id, "device_code": device_code},
                    client=client,
                )
            except httpx.HTTPError as exc:
                raise LoginFlowError(
                    f"Lost connection to the device authorization service: {exc}"
                ) from exc
            data = _response_json(response, "Device token exchange") if response.content else {}
            if response.status_code < 400:
                if not isinstance(data, dict) or not data.get("access_token"):
                    raise LoginFlowError("Device token exchange returned an unexpected response.")
                return data
            error = data.get("error") if isinstance(data, dict) else None
            if error == "authorization_pending":
                diagnostics.reporter.emit(
                    "auth.device_pending",
                    context="authentication",
                    retry_in_seconds=interval,
                )
                time.sleep(interval)
                continue
            if error == "slow_down":
                interval += 5
                diagnostics.reporter.emit(
                    "auth.device_slow_down",
                    context="authentication",
                    retry_in_seconds=interval,
                )
                time.sleep(interval)
                continue
            if error == "access_denied":
                raise LoginFlowError("Login was denied in the browser.")
            if error == "expired_token":
                raise LoginFlowError(
                    "The device login code expired. Run `heyfood login --device` again."
                )
            raise LoginFlowError(_response_error(response))
    raise LoginFlowError("Timed out waiting for device authorization.")


def _save_authenticated_config(
    *,
    store: ConfigStore,
    api_url: str,
    auth_url: str,
    api_key: str,
    device_id: str,
    client_id: str,
    oauth_bundle: dict[str, Any],
    session_bundle: dict[str, Any],
) -> dict[str, Any]:
    config = store.load()
    user_id = str(session_bundle.get("user_id") or "").strip()
    if not user_id:
        raise LoginFlowError("CLI session exchange did not identify the authenticated account.")
    bind_config_to_account(config, user_id)
    config.update(
        {
            "api_url": api_url,
            "auth_url": auth_url,
            "api_key": api_key,
            "device_id": device_id,
            "oauth": {
                "client_id": client_id,
                "access_token": oauth_bundle["access_token"],
                "refresh_token": oauth_bundle["refresh_token"],
                "access_expires_at": expires_in_to_iso(int(oauth_bundle.get("expires_in", 3600))),
                "scope": oauth_bundle.get("scope", " ".join(LOGIN_SCOPES)),
                "link_id": oauth_bundle.get("link_id"),
            },
            "session": session_bundle,
        }
    )
    store.save(config)
    return config


def local_urls() -> tuple[str, str]:
    return DEFAULT_LOCAL_API_URL, DEFAULT_LOCAL_AUTH_URL


def normalize_auth_url(url: str) -> str:
    value = (url or DEFAULT_AUTH_URL).rstrip("/")
    if value.endswith("/authorize"):
        return value
    return f"{value}/authorize"


def register_client(api_url: str, redirect_uri: str) -> dict[str, Any]:
    payload = {
        # The backend maps both CLI brand names (hello.food / hey.food) to the
        # "hellofood_cli" channel; any other name falls back to "chatgpt" and
        # the CLI session exchange then fails with 403. Keep in sync with
        # channel_oauth_service._channel_for_oauth_client.
        "client_name": "hey.food CLI",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code"],
        "token_endpoint_auth_method": "none",
    }
    try:
        response = _post_with_diagnostics(
            api_url,
            "/v1/channel/oauth/register",
            json_body=payload,
        )
    except httpx.HTTPError as exc:
        raise LoginFlowError(
            "Could not reach the hello.food authorization service during client "
            f"registration: {exc}"
        ) from exc
    if response.status_code >= 400:
        raise LoginFlowError(_response_error(response))
    data = _response_json(response, "OAuth client registration")
    if not isinstance(data, dict) or not data.get("client_id"):
        raise LoginFlowError("OAuth client registration returned an unexpected response.")
    return data


def build_authorize_url(
    *,
    auth_url: str,
    client_id: str,
    redirect_uri: str,
    state: str,
    code_challenge: str,
) -> str:
    params = {
        "response_type": "code",
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "scope": " ".join(LOGIN_SCOPES),
        "state": state,
        "code_challenge": code_challenge,
        "code_challenge_method": "S256",
        "app_client_id": APP_CLIENT_ID,
    }
    return f"{auth_url}?{urlencode(params)}"


def exchange_code(
    *,
    api_url: str,
    client_id: str,
    code: str,
    verifier: str,
    redirect_uri: str,
) -> dict[str, Any]:
    payload = {
        "grant_type": "authorization_code",
        "client_id": client_id,
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": redirect_uri,
    }
    try:
        response = _post_with_diagnostics(
            api_url,
            "/v1/channel/oauth/token",
            json_body=payload,
        )
    except httpx.HTTPError as exc:
        raise LoginFlowError(f"Could not reach the OAuth token service: {exc}") from exc
    if response.status_code >= 400:
        raise LoginFlowError(_response_error(response))
    data = _response_json(response, "OAuth token exchange")
    if not isinstance(data, dict) or not data.get("access_token"):
        raise LoginFlowError("OAuth token exchange returned an unexpected response.")
    return data


def exchange_cli_session(api_url: str, access_token: str, device_id: str) -> dict[str, Any]:
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        "X-App-Client-ID": APP_CLIENT_ID,
        "X-Device-ID": device_id,
    }
    try:
        response = _post_with_diagnostics(
            api_url,
            "/v1/channel/oauth/cli/session",
            headers=headers,
            json_body={"device_id": device_id},
        )
    except httpx.HTTPError as exc:
        raise LoginFlowError(f"Could not reach the CLI session service: {exc}") from exc
    if response.status_code >= 400:
        raise LoginFlowError(_response_error(response))
    data = _response_json(response, "CLI session exchange")
    if not isinstance(data, dict) or not data.get("access_token"):
        raise LoginFlowError("CLI session exchange returned an unexpected response.")
    return data


def pkce_pair() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(64)[:96]
    digest = hashlib.sha256(verifier.encode("ascii")).digest()
    challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    return verifier, challenge


class OAuthCallbackServer:
    def __init__(self):
        self._event = threading.Event()
        self._result: dict[str, str] = {}
        try:
            self._server = HTTPServer(("127.0.0.1", 8765), self._handler())
        except OSError:
            self._server = HTTPServer(("127.0.0.1", 0), self._handler())
        self.port = int(self._server.server_address[1])
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def __enter__(self) -> "OAuthCallbackServer":
        self._thread.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=1)

    def wait(self, timeout_seconds: int) -> dict[str, str]:
        if not self._event.wait(timeout=max(1, timeout_seconds)):
            raise LoginFlowError("Timed out waiting for browser login.")
        return dict(self._result)

    def _handler(self):
        parent = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                parsed = urlparse(self.path)
                if parsed.path != "/callback":
                    self.send_response(404)
                    self.end_headers()
                    return
                query = parse_qs(parsed.query)
                parent._result = {
                    key: values[0]
                    for key, values in query.items()
                    if values
                }
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.end_headers()
                self.wfile.write(
                    b"<html><body><h1>hello.food CLI is connected.</h1>"
                    b"<p>You can close this tab and return to your terminal.</p>"
                    b"</body></html>"
                )
                parent._event.set()

            def log_message(self, format: str, *args: object) -> None:
                return

        return Handler


def _response_error(response: httpx.Response) -> str:
    try:
        data = response.json()
    except Exception:
        return f"{response.status_code}: {response.text}"
    if isinstance(data, dict):
        detail = (
            data.get("detail")
            or data.get("message")
            or data.get("error_description")
            or data.get("error")
        )
        if detail:
            return f"{response.status_code}: {detail}"
    return f"{response.status_code}: {data}"


def _response_json(response: httpx.Response, operation: str) -> dict[str, Any]:
    try:
        data = response.json()
    except Exception as exc:
        raise LoginFlowError(f"{operation} returned malformed JSON.") from exc
    if not isinstance(data, dict):
        raise LoginFlowError(f"{operation} returned an unexpected response.")
    return data
