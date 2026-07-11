from __future__ import annotations

import json
import time
from typing import Any, Iterator
from uuid import uuid4

import httpx

from . import __version__
from .config import (
    APP_CLIENT_ID,
    ConfigStore,
    DEFAULT_API_KEY,
    discover_local_api_key,
    expires_in_to_iso,
    is_local_api_url,
    is_expiring,
    resolve_service_urls,
    utcnow,
)
from . import diagnostics


class HelloFoodError(RuntimeError):
    pass


class LoginRequired(HelloFoodError):
    pass


class ChannelToolUnavailable(HelloFoodError):
    pass


OPTIONAL_CHANNEL_TOOLS = {
    "get_menu_status": "menu_acquisition_polling",
    "list_saved_recipes": "recipe_cookbook",
    "save_recipe": "recipe_cookbook",
    "search_recipes": "recipe_search",
}


class HelloFoodClient:
    def __init__(self, store: ConfigStore | None = None, *, create_device: bool = True):
        self.store = store or ConfigStore()
        self.config = self.store.load()
        self.api_url, self.auth_url, self.context_name = resolve_service_urls(self.config)
        existing_device = self.config.get("device_id")
        self.device_id = (
            self.store.get_device_id()
            if create_device
            else (str(existing_device) if isinstance(existing_device, str) else "")
        )
        if create_device:
            self.config = self.store.load()
        self._ensure_local_api_key(persist=create_device)

    def _save(self) -> None:
        self.store.save(self.config)

    def _ensure_local_api_key(self, *, persist: bool = True) -> None:
        if self.config.get("api_key"):
            return
        api_key = DEFAULT_API_KEY
        if not api_key and is_local_api_url(self.api_url):
            api_key = discover_local_api_key() or ""
        if api_key:
            self.config["api_key"] = api_key
            if persist:
                self._save()

    def _headers(
        self,
        token: str | None = None,
        *,
        request_id: str | None = None,
    ) -> dict[str, str]:
        headers = {
            "Accept": "application/json",
            "Content-Type": "application/json",
            "User-Agent": f"heyfood-cli/{__version__}",
            "X-App-Client-ID": APP_CLIENT_ID,
            "X-Device-ID": self.device_id,
        }
        api_key = self.config.get("api_key")
        if isinstance(api_key, str) and api_key:
            headers["X-API-Key"] = api_key
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if request_id:
            headers["X-Request-ID"] = request_id
        return headers

    def _request(
        self,
        method: str,
        path: str,
        *,
        auth: str | None = None,
        json_body: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
        timeout: float | None = 30.0,
    ) -> dict[str, Any]:
        request_id = str(uuid4())
        started_at = time.monotonic()
        token = None
        if auth == "session":
            token = self.session_access_token()
        elif auth == "channel":
            token = self.channel_access_token()

        diagnostics.reporter.emit(
            "http.start",
            request_id=request_id,
            context=self.context_name,
            method=method.upper(),
            endpoint=path,
        )

        try:
            with httpx.Client(timeout=timeout) as client:
                response = client.request(
                    method,
                    f"{self.api_url}{path}",
                    headers=self._headers(token, request_id=request_id),
                    json=json_body,
                    params=params,
                )
        except httpx.HTTPError as exc:
            diagnostics.reporter.emit(
                "http.error",
                request_id=request_id,
                context=self.context_name,
                method=method.upper(),
                endpoint=path,
                duration_ms=round((time.monotonic() - started_at) * 1000, 1),
                error=type(exc).__name__,
            )
            raise HelloFoodError(f"Could not reach hello.food API: {exc}") from exc
        diagnostics.reporter.emit(
            "http.complete",
            request_id=request_id,
            server_request_id=response.headers.get("X-Request-ID"),
            context=self.context_name,
            method=method.upper(),
            endpoint=path,
            status=response.status_code,
            duration_ms=round((time.monotonic() - started_at) * 1000, 1),
        )
        if response.status_code >= 400:
            raise HelloFoodError(_error_message(response))
        if not response.content:
            return {}
        data = response.json()
        if not isinstance(data, dict):
            return {"data": data}
        return data

    def session_access_token(self) -> str:
        session = self.config.get("session")
        if not isinstance(session, dict) or not session.get("access_token"):
            raise LoginRequired("Run `heyfood login` first.")
        if is_expiring(session.get("access_expires_at")):
            self.refresh_session()
            session = self.config.get("session") or {}
        token = session.get("access_token")
        if not isinstance(token, str) or not token:
            raise LoginRequired("Run `heyfood login` first.")
        return token

    def channel_access_token(self) -> str:
        oauth = self.config.get("oauth")
        if not isinstance(oauth, dict) or not oauth.get("access_token"):
            raise LoginRequired("Run `heyfood login` first.")
        if is_expiring(oauth.get("access_expires_at")):
            self.refresh_channel()
            oauth = self.config.get("oauth") or {}
        token = oauth.get("access_token")
        if not isinstance(token, str) or not token:
            raise LoginRequired("Run `heyfood login` first.")
        return token

    def refresh_session(self) -> None:
        diagnostics.reporter.emit("auth.refresh_session", context=self.context_name)
        session = self.config.get("session")
        refresh_token = session.get("refresh_token") if isinstance(session, dict) else None
        if not refresh_token:
            self._reexchange_session()
            return
        try:
            data = self._request(
                "POST",
                "/v1/auth/session/refresh",
                json_body={"refresh_token": refresh_token},
                auth=None,
            )
        except LoginRequired:
            raise
        except HelloFoodError:
            # /v1/auth/session/refresh sits behind the X-API-Key gate, which the
            # CLI deliberately does not hold outside local dev. The channel OAuth
            # surface is the CLI's keyless re-auth path.
            diagnostics.reporter.emit(
                "auth.session_reexchange_fallback",
                context=self.context_name,
            )
            self._reexchange_session()
            return
        self.config["session"] = data
        self._save()

    def _reexchange_session(self) -> None:
        """Mint a fresh app session from the channel OAuth credentials."""
        diagnostics.reporter.emit("auth.session_reexchange", context=self.context_name)
        try:
            data = self._request(
                "POST",
                "/v1/channel/oauth/cli/session",
                json_body={"device_id": self.device_id},
                auth="channel",
            )
        except LoginRequired:
            raise
        except HelloFoodError as exc:
            raise LoginRequired(
                "Session expired and could not be renewed. Run `heyfood login` again."
            ) from exc
        if not data.get("access_token"):
            raise LoginRequired(
                "Session expired and could not be renewed. Run `heyfood login` again."
            )
        self.config["session"] = data
        self._save()

    def refresh_channel(self) -> None:
        diagnostics.reporter.emit("auth.refresh_channel", context=self.context_name)
        oauth = self.config.get("oauth")
        if not isinstance(oauth, dict):
            raise LoginRequired("Run `heyfood login` first.")
        refresh_token = oauth.get("refresh_token")
        client_id = oauth.get("client_id")
        if not refresh_token or not client_id:
            raise LoginRequired("Run `heyfood login` first.")
        try:
            data = self._request(
                "POST",
                "/v1/channel/oauth/token",
                json_body={
                    "grant_type": "refresh_token",
                    "client_id": client_id,
                    "refresh_token": refresh_token,
                },
                auth=None,
            )
        except HelloFoodError as exc:
            raise LoginRequired("Run `heyfood login` again to refresh this CLI session.") from exc
        oauth.update(
            {
                "access_token": data["access_token"],
                "refresh_token": data["refresh_token"],
                "access_expires_at": expires_in_to_iso(int(data.get("expires_in", 3600))),
                "scope": data.get("scope", oauth.get("scope", "")),
                "link_id": data.get("link_id", oauth.get("link_id")),
            }
        )
        self.config["oauth"] = oauth
        self._save()

    def me(self) -> dict[str, Any]:
        return self._request("GET", "/v1/auth/me", auth="session")

    def channel_whoami(self) -> dict[str, Any]:
        try:
            return self._request("GET", "/v1/channel/oauth/whoami", auth="channel")
        except HelloFoodError as exc:
            if not _is_invalid_channel_token_error(str(exc)):
                raise
            try:
                self.refresh_channel()
                return self._request("GET", "/v1/channel/oauth/whoami", auth="channel")
            except HelloFoodError as retry_exc:
                raise LoginRequired(
                    "Run `heyfood login` again to refresh this CLI session."
                ) from retry_exc

    def channel_tool(self, name: str, payload: dict[str, Any]) -> dict[str, Any]:
        for attempt in range(2):
            try:
                return self._request(
                    "POST",
                    f"/v1/channel/tools/{name}",
                    auth="channel",
                    json_body=payload,
                    timeout=60.0,
                )
            except HelloFoodError as exc:
                message = str(exc).strip()
                if message == "404: Not Found":
                    raise ChannelToolUnavailable(
                        f"The connected API does not expose the `{name}` channel tool yet."
                    ) from exc
                if _is_invalid_channel_token_error(message):
                    if attempt == 0:
                        diagnostics.reporter.emit(
                            "http.retry_after_channel_refresh",
                            context=self.context_name,
                            endpoint=f"/v1/channel/tools/{name}",
                            attempt=2,
                        )
                        self.refresh_channel()
                        continue
                    raise LoginRequired(
                        "Run `heyfood login` again to refresh this CLI session."
                    ) from exc
                raise
        raise LoginRequired("Run `heyfood login` again to refresh this CLI session.")

    def get_menu_status(self, *, restaurant_id: str, job_id: str) -> dict[str, Any]:
        return self.channel_tool(
            "get_menu_status",
            {"restaurant_id": restaurant_id, "job_id": job_id},
        )

    def list_profile_members(self) -> dict[str, Any]:
        return self._request("GET", "/v1/profile/sync/members", auth="session")

    def last_conversation(self) -> dict[str, Any]:
        value = self.config.get("last_conversation")
        return value if isinstance(value, dict) else {}

    def last_conversation_id(self) -> str | None:
        conversation = self.last_conversation()
        value = conversation.get("conversation_id")
        return value if isinstance(value, str) and value else None

    def pending_confirmation(self) -> dict[str, str] | None:
        conversation = self.last_conversation()
        pending = conversation.get("pending_confirmation")
        if not isinstance(pending, dict):
            return None
        confirmation_id = pending.get("confirmation_id")
        idempotency_key = pending.get("idempotency_key")
        if isinstance(confirmation_id, str) and isinstance(idempotency_key, str):
            return {
                "confirmation_id": confirmation_id,
                "idempotency_key": idempotency_key,
            }
        return None

    def remember_conversation(self, result: dict[str, Any]) -> None:
        conversation_id = result.get("conversation_id")
        if not isinstance(conversation_id, str) or not conversation_id:
            return

        pending_confirmation = None
        structured = result.get("structured")
        if isinstance(structured, dict) and structured.get("type") == "action_confirmation":
            confirmation_id = structured.get("confirmation_id")
            idempotency_key = structured.get("idempotency_key")
            if isinstance(confirmation_id, str) and isinstance(idempotency_key, str):
                pending_confirmation = {
                    "confirmation_id": confirmation_id,
                    "idempotency_key": idempotency_key,
                }

        self.config["last_conversation"] = {
            "conversation_id": conversation_id,
            "pending_confirmation": pending_confirmation,
            "updated_at": utcnow().isoformat(),
        }
        if isinstance(structured, dict) and structured.get("type") == "recipe_search":
            self.remember_recipe_search(structured, save=False)
        self._save()

    def clear_last_conversation(self) -> bool:
        if "last_conversation" not in self.config:
            return False
        self.config.pop("last_conversation", None)
        self._save()
        return True

    def remember_recipe_search(self, result: dict[str, Any], *, save: bool = True) -> None:
        recipes = result.get("recipes")
        if not isinstance(recipes, list):
            return
        self.config["last_recipe_search"] = {
            "query_used": result.get("query_used"),
            "original_query": result.get("original_query"),
            "recipes": recipes[:20],
            "updated_at": utcnow().isoformat(),
        }
        if save:
            self._save()

    def last_recipe_search(self) -> dict[str, Any]:
        value = self.config.get("last_recipe_search")
        return value if isinstance(value, dict) else {}

    def remember_restaurant_search(self, result: dict[str, Any]) -> None:
        restaurants = result.get("restaurants")
        if not isinstance(restaurants, list):
            return
        self.config["last_restaurant_search"] = {
            "restaurants": restaurants[:20],
            "updated_at": utcnow().isoformat(),
        }
        self._save()

    def last_restaurant_search(self) -> dict[str, Any]:
        value = self.config.get("last_restaurant_search")
        return value if isinstance(value, dict) else {}

    def geocode_location(self, query: str) -> dict[str, Any]:
        """Resolve a place name to coordinates via the backend geocode channel tool."""
        return self.channel_tool("geocode_location", {"query": query})

    def save_location(
        self,
        *,
        label: str,
        latitude: float,
        longitude: float,
        radius_miles: float = 5.0,
    ) -> None:
        self.config["location"] = {
            "label": label,
            "latitude": float(latitude),
            "longitude": float(longitude),
            "radius_miles": float(radius_miles),
            "updated_at": utcnow().isoformat(),
        }
        self._save()

    def saved_location(self) -> dict[str, Any] | None:
        """Return the persisted default location, or None if unset or malformed.

        Defensively validates coordinates are numeric (config may be hand-edited
        or half-written) so a bad value can never reach a search payload.
        """
        value = self.config.get("location")
        if not isinstance(value, dict):
            return None
        latitude = value.get("latitude")
        longitude = value.get("longitude")
        if not isinstance(latitude, (int, float)) or isinstance(latitude, bool):
            return None
        if not isinstance(longitude, (int, float)) or isinstance(longitude, bool):
            return None
        return value

    def clear_location(self) -> bool:
        """Forget the saved location. Returns True if one was present."""
        existed = self.config.pop("location", None) is not None
        if existed:
            self._save()
        return existed

    def restaurant_from_selector(self, selector: str) -> dict[str, Any] | None:
        normalized = selector.strip()
        if not normalized.isdigit():
            return None
        restaurants = self.last_restaurant_search().get("restaurants")
        if not isinstance(restaurants, list):
            raise HelloFoodError("No previous restaurant search found. Run `heyfood search ...` first.")
        index = int(normalized)
        if not 1 <= index <= len(restaurants):
            raise HelloFoodError(
                f"Restaurant selection {index} is out of range for the last search."
            )
        restaurant = restaurants[index - 1]
        return restaurant if isinstance(restaurant, dict) else None

    def restaurant_id_from_selector(self, selector: str) -> str:
        restaurant = self.restaurant_from_selector(selector)
        if restaurant is None:
            return selector
        restaurant_id = restaurant.get("id")
        if not isinstance(restaurant_id, str) or not restaurant_id:
            raise HelloFoodError("That restaurant result does not have a HelloFood id.")
        return restaurant_id

    def recipe_save_payload(self, selector: str) -> dict[str, Any]:
        normalized = selector.strip()
        if not normalized:
            raise HelloFoodError("Provide a recipe ref like spoonacular:645753 or an index from the last search.")

        if normalized.isdigit():
            search = self.last_recipe_search()
            recipes = search.get("recipes")
            if isinstance(recipes, list):
                index = int(normalized)
                if 1 <= index <= len(recipes):
                    recipe = recipes[index - 1]
                    if isinstance(recipe, dict):
                        return _recipe_payload_from_card(recipe)
            return {"spoonacular_id": int(normalized)}

        if ":" in normalized:
            provider, external_id = normalized.split(":", 1)
            provider = provider.strip()
            external_id = external_id.strip()
            if provider and external_id:
                return {
                    "recipe_ref": {
                        "provider": provider,
                        "external_id": external_id,
                    }
                }

        raise HelloFoodError(
            "Use a recipe ref like spoonacular:645753, a Spoonacular id, or a number from the last recipe search."
        )

    def daily_summary(self, date_value: str, member_id: str | None = None) -> dict[str, Any]:
        params: dict[str, Any] = {"date": date_value}
        if member_id:
            params["member_id"] = member_id
        return self._request("GET", "/v1/meals/daily-summary", auth="session", params=params)

    def profile_consent_status(self) -> dict[str, Any]:
        return self._request("GET", "/v1/profile/consent", auth="session")

    def grant_profile_consent(self, consent_version: int = 1) -> dict[str, Any]:
        return self._request(
            "POST",
            "/v1/profile/consent",
            auth="session",
            json_body={"consent_version": consent_version},
        )

    def download_profile(self, member_id: str = "_self") -> dict[str, Any]:
        return self._request(
            "GET",
            "/v1/profile/sync",
            auth="session",
            params={"member_id": member_id},
        )

    def upload_profile(
        self,
        profile_data: dict[str, Any],
        *,
        member_id: str = "_self",
        expected_version: int | None = None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "member_id": member_id,
            "profile_data": profile_data,
        }
        if expected_version is not None:
            body["expected_version"] = expected_version
        return self._request(
            "PUT",
            "/v1/profile/sync",
            auth="session",
            json_body=body,
        )

    def revoke_local_session(self) -> dict[str, Any]:
        # Server-side teardown is best-effort, but ordering matters: the
        # session token authenticates the link and device calls, so the
        # session itself must be revoked LAST.
        session = self.config.get("session")
        token = session.get("access_token") if isinstance(session, dict) else None
        teardown: dict[str, dict[str, Any]] = {}

        link_id = (self.config.get("oauth") or {}).get("link_id")
        if link_id:
            try:
                self._request("DELETE", f"/v1/channel/links/{link_id}", auth="session")
            except HelloFoodError:
                teardown["link"] = {"attempted": True, "ok": False, "error": "request_failed"}
            else:
                teardown["link"] = {"attempted": True, "ok": True}
        else:
            teardown["link"] = {"attempted": False, "ok": True}

        device_id = self.config.get("device_id")
        if device_id:
            try:
                self._request(
                    "POST",
                    "/v1/auth/device/revoke",
                    auth="session",
                    json_body={"device_id": device_id, "reason": "cli_logout"},
                )
            except HelloFoodError:
                teardown["device"] = {
                    "attempted": True,
                    "ok": False,
                    "error": "request_failed",
                }
            else:
                teardown["device"] = {"attempted": True, "ok": True}
        else:
            teardown["device"] = {"attempted": False, "ok": True}

        if token:
            try:
                self._request(
                    "POST",
                    "/v1/auth/session/revoke",
                    auth="session",
                    json_body={"reason": "cli_logout"},
                )
            except HelloFoodError:
                teardown["session"] = {
                    "attempted": True,
                    "ok": False,
                    "error": "request_failed",
                }
            else:
                teardown["session"] = {"attempted": True, "ok": True}
        else:
            teardown["session"] = {"attempted": False, "ok": True}
        self.store.delete()
        remote_complete = all(step["ok"] for step in teardown.values())
        return {
            "ok": True,
            "remote_complete": remote_complete,
            "teardown": teardown,
            "local_credentials_cleared": True,
        }

    def stream_agent(
        self,
        payload: dict[str, Any],
    ) -> Iterator[tuple[str, dict[str, Any]]]:
        token = self.session_access_token()
        request_id = str(uuid4())
        started_at = time.monotonic()
        status_code: int | None = None
        server_request_id: str | None = None
        diagnostics.reporter.emit(
            "http.stream_start",
            request_id=request_id,
            context=self.context_name,
            method="POST",
            endpoint="/v1/agent/converse",
        )
        try:
            with httpx.Client(timeout=None) as client:
                with client.stream(
                    "POST",
                    f"{self.api_url}/v1/agent/converse",
                    headers=self._headers(token, request_id=request_id),
                    json=payload,
                ) as response:
                    status_code = response.status_code
                    server_request_id = response.headers.get("X-Request-ID")
                    if response.status_code >= 400:
                        body = response.read()
                        raise HelloFoodError(_error_message(response, body))
                    yield from _iter_sse(response)
        except httpx.HTTPError as exc:
            diagnostics.reporter.emit(
                "http.stream_error",
                request_id=request_id,
                context=self.context_name,
                method="POST",
                endpoint="/v1/agent/converse",
                duration_ms=round((time.monotonic() - started_at) * 1000, 1),
                error=type(exc).__name__,
            )
            raise HelloFoodError(f"Could not reach hello.food API: {exc}") from exc
        finally:
            diagnostics.reporter.emit(
                "http.stream_complete",
                request_id=request_id,
                server_request_id=server_request_id,
                context=self.context_name,
                method="POST",
                endpoint="/v1/agent/converse",
                status=status_code,
                duration_ms=round((time.monotonic() - started_at) * 1000, 1),
            )


def _iter_sse(response: httpx.Response) -> Iterator[tuple[str, dict[str, Any]]]:
    event = "message"
    data_lines: list[str] = []
    for line in response.iter_lines():
        if line == "":
            if data_lines:
                raw = "\n".join(data_lines)
                try:
                    payload = json.loads(raw)
                except json.JSONDecodeError:
                    payload = {"text": raw}
                yield event, payload
            event = "message"
            data_lines = []
            continue
        if line.startswith("event:"):
            event = line.split(":", 1)[1].strip()
        elif line.startswith("data:"):
            data_lines.append(line.split(":", 1)[1].lstrip())


def _error_message(response: httpx.Response, body: bytes | None = None) -> str:
    try:
        data = response.json() if body is None else json.loads(body.decode("utf-8"))
    except Exception:
        text = response.text if body is None else body.decode("utf-8", errors="replace")
        return f"{response.status_code}: {text}"
    if isinstance(data, dict):
        detail = data.get("detail") or data.get("message") or data.get("error")
        if detail:
            return f"{response.status_code}: {detail}"
    return f"{response.status_code}: {data}"


def _is_invalid_channel_token_error(message: str) -> bool:
    normalized = message.lower()
    return normalized.startswith("401:") and "channel token" in normalized


def _recipe_payload_from_card(recipe: dict[str, Any]) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    for key in (
        "spoonacular_id",
        "recipe_ref",
        "title",
        "image_url",
        "source_name",
        "source_url",
        "provider_source_url",
        "ready_in_minutes",
        "servings",
        "calories_per_serving",
        "dietary_tags",
        "can_save",
    ):
        value = recipe.get(key)
        if value is not None:
            payload[key] = value
    payload["recipe_data"] = {
        key: value
        for key, value in recipe.items()
        if key
        in {
            "spoonacular_id",
            "recipe_ref",
            "provider",
            "external_recipe_id",
            "title",
            "image_url",
            "source_name",
            "source_url",
            "provider_source_url",
            "ready_in_minutes",
            "servings",
            "calories_per_serving",
            "dietary_tags",
            "can_open_detail",
            "can_save",
        }
    }
    return payload
