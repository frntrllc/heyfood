from __future__ import annotations

import json
import time
from copy import deepcopy
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
from . import household


class HelloFoodError(RuntimeError):
    pass


class LoginRequired(HelloFoodError):
    pass


class ChannelToolUnavailable(HelloFoodError):
    pass


class TranscriptionUnavailable(HelloFoodError):
    """The transcription endpoint is dark, pre-deploy, or unreachable.

    Callers treat this as "degrade to browser capture", never a hard failure —
    CLI releases are decoupled from backend deploy timing.
    """


class TranscriptionScopeRequired(HelloFoodError):
    """The channel token predates the ``audio:transcribe`` scope; re-login."""


class TranscriptionRejected(HelloFoodError):
    """The audio was rejected for size, duration, or format."""


class TranscriptionRateLimited(HelloFoodError):
    """Per-device transcription rate limit hit; carries the retry hint."""

    def __init__(self, message: str, *, retry_after: int | None = None):
        super().__init__(message)
        self.retry_after = retry_after


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
        self._profile_consent_cache: bool | None = None

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

    def channel_scopes(self) -> set[str]:
        """The scopes granted to the stored channel token, as persisted at login."""
        oauth = self.config.get("oauth")
        if not isinstance(oauth, dict):
            return set()
        scope = oauth.get("scope")
        if not isinstance(scope, str):
            return set()
        return {part for part in scope.split() if part}

    def has_transcribe_scope(self) -> bool:
        """True when the stored channel token was granted ``audio:transcribe``.

        Checked before opening the microphone so an older session minted before
        the scope existed is asked to re-login *before* any audio is recorded or
        uploaded, rather than recording first and hitting a 403 afterward.
        """
        return "audio:transcribe" in self.channel_scopes()

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

    def profile_readiness(self) -> dict[str, Any]:
        """Return the strictly validated least-privilege first-run state."""
        from .auth_application import AuthContractError, validate_profile_readiness

        for attempt in range(2):
            try:
                data = self._request(
                    "GET",
                    "/v1/channel/tools/profile/readiness",
                    auth="channel",
                )
                return validate_profile_readiness(data).document()
            except AuthContractError as exc:
                raise HelloFoodError(str(exc)) from exc
            except HelloFoodError as exc:
                if attempt == 0 and _is_invalid_channel_token_error(str(exc)):
                    self.refresh_channel()
                    continue
                raise
        raise LoginRequired("Run `heyfood login` again to refresh this CLI session.")

    def begin_account_deletion(self, request_nonce: str) -> dict[str, Any]:
        return self._request(
            "POST",
            "/v1/auth/account-deletion/begin",
            auth="session",
            json_body={"schema_version": 1, "request_nonce": request_nonce},
        )

    def account_deletion_status(self, status_token: str) -> dict[str, Any]:
        return self._request(
            "POST",
            "/v1/auth/account-deletion/status",
            auth=None,
            json_body={"schema_version": 1, "status_token": status_token},
        )

    def cancel_account_deletion(self, status_token: str) -> dict[str, Any]:
        return self._request(
            "POST",
            "/v1/auth/account-deletion/cancel",
            auth=None,
            json_body={"schema_version": 1, "status_token": status_token},
        )

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

    def transcribe_audio(
        self,
        wav_bytes: bytes,
        *,
        purpose: str,
        language: str | None = None,
        timeout: float | None = 60.0,
    ) -> dict[str, Any]:
        """Upload a WAV clip and return the server-side transcript.

        This is the CLI's only multipart request, so it does not reuse
        ``_request``: that path hardcodes ``Content-Type: application/json`` and
        would clobber the multipart boundary httpx sets for us. Auth is the
        channel access token (the endpoint requires channel scopes; the session
        JWT is the wrong credential).
        """
        # Lazily imported to keep the client free of a top-level dependency on
        # the contract module (which imports HelloFoodError from here).
        from . import transcription_contract as contract

        # The WAV file itself must stay under the audio-file ceiling. The
        # recorder already caps capture, but a malformed or oversized buffer is
        # rejected locally rather than sent to be 413'd.
        if len(wav_bytes) > contract.MAX_AUDIO_BYTES:
            raise TranscriptionRejected(
                "The recording is larger than the transcription size limit."
            )

        token = self.channel_access_token()
        request_id = str(uuid4())
        started_at = time.monotonic()
        headers = self._headers(token, request_id=request_id)
        # Drop the JSON content type so httpx negotiates the multipart boundary.
        headers.pop("Content-Type", None)
        files = {"file": ("audio.wav", wav_bytes, "audio/wav")}
        data: dict[str, str] = {"purpose": purpose}
        if language:
            data["language"] = language

        diagnostics.reporter.emit(
            "http.start",
            request_id=request_id,
            context=self.context_name,
            method="POST",
            endpoint="/v1/audio/transcriptions",
            byte_size=len(wav_bytes),
        )
        try:
            with httpx.Client(timeout=timeout) as client:
                # Build the request first so the whole multipart envelope (WAV +
                # boundary framing + form fields) can be measured against the
                # request ceiling, which is separate from the audio-file ceiling
                # so framing overhead can never reject a valid maximum-size WAV.
                request = client.build_request(
                    "POST",
                    f"{self.api_url}/v1/audio/transcriptions",
                    headers=headers,
                    files=files,
                    data=data,
                )
                envelope = request.read()
                if len(envelope) > contract.MAX_REQUEST_BYTES:
                    raise TranscriptionRejected(
                        "The transcription upload is larger than the request "
                        "size limit."
                    )
                response = client.send(request)
        except TranscriptionRejected:
            raise
        except httpx.HTTPError as exc:
            diagnostics.reporter.emit(
                "http.error",
                request_id=request_id,
                context=self.context_name,
                method="POST",
                endpoint="/v1/audio/transcriptions",
                duration_ms=round((time.monotonic() - started_at) * 1000, 1),
                error=type(exc).__name__,
            )
            # A network-level failure is indistinguishable, from the user's seat,
            # from a dark endpoint — degrade to browser capture rather than error.
            raise TranscriptionUnavailable(
                f"Could not reach the transcription service: {exc}"
            ) from exc
        diagnostics.reporter.emit(
            "http.complete",
            request_id=request_id,
            server_request_id=response.headers.get("X-Request-ID"),
            context=self.context_name,
            method="POST",
            endpoint="/v1/audio/transcriptions",
            status=response.status_code,
            duration_ms=round((time.monotonic() - started_at) * 1000, 1),
        )
        if response.status_code >= 400:
            _raise_transcription_error(response)
        # A malformed or empty *success* body is a contract violation, not a
        # reason to degrade to a different capture processor. Return the parsed
        # value (or an empty dict) and let the caller's contract validation turn
        # it into a typed service error rather than a silent browser fallback.
        if not response.content:
            return {}
        try:
            parsed = response.json()
        except ValueError:
            return {}
        if not isinstance(parsed, dict):
            return {}
        return parsed

    def voice_settings(self) -> dict[str, Any]:
        """Local, device-scoped voice preferences (capture mode + input device).

        These describe this machine's microphone, not the account, so they are
        intentionally kept out of the account-scoped state that logout clears.
        """
        value = self.config.get("voice")
        if not isinstance(value, dict):
            return {}
        settings: dict[str, Any] = {}
        mode = value.get("capture_mode")
        if isinstance(mode, str) and mode:
            settings["capture_mode"] = mode
        device = value.get("device")
        if isinstance(device, (str, int)) and not isinstance(device, bool):
            settings["device"] = device
        return settings

    def remember_voice_settings(
        self,
        *,
        capture_mode: str | None = None,
        device: str | int | None = None,
        clear: bool = False,
    ) -> dict[str, Any]:
        """Persist a voice preference. Only provided fields change.

        ``clear=True`` wipes all persisted voice preferences (used by
        ``voice reset``). An explicit ``capture_mode`` (including ``"auto"``) is
        recorded verbatim, so an omitted preference and an explicit ``auto`` stay
        distinguishable.
        """
        if clear:
            self.config.pop("voice", None)
            self._save()
            return {}
        current = self.voice_settings()
        if capture_mode is not None:
            current["capture_mode"] = capture_mode
        if device is not None:
            current["device"] = device
        if current:
            self.config["voice"] = current
        else:
            self.config.pop("voice", None)
        self._save()
        return current

    def list_profile_members(self) -> dict[str, Any]:
        return self._request("GET", "/v1/profile/sync/members", auth="session")

    def household_state(self) -> dict[str, Any]:
        return household.normalize_state(
            self.config.get("household"),
            owner_name=self.config.get("first_name"),
        )

    def remember_household_state(self, state: dict[str, Any]) -> None:
        self.config["household"] = household.normalize_state(
            state,
            owner_name=self.config.get("first_name"),
        )
        self._save()

    def refresh_household_state(self) -> dict[str, Any]:
        profiles = self.list_profile_members()
        profile_rows = profiles.get("profiles")
        member_ids = [
            str(item.get("member_id"))
            for item in profile_rows or []
            if isinstance(item, dict) and item.get("member_id")
        ]
        state = household.reconcile_profile_members(
            self.household_state(),
            member_ids,
            owner_name=self.config.get("first_name"),
        )
        pending_outbox = self.household_profile_outbox()
        for member in state["members"]:
            if (
                member["id"] in pending_outbox
                and member.get("relationship") != "child"
            ):
                member["profile_synced"] = False
        self.remember_household_state(state)
        return state

    def set_household_scope(self, selector: str) -> dict[str, Any]:
        state = household.set_active_scope(self.household_state(), selector)
        self.remember_household_state(state)
        return state

    def label_household_member(
        self,
        selector: str,
        *,
        name: str,
        relationship: str | None = None,
    ) -> dict[str, Any]:
        state = household.label_member(
            self.household_state(),
            selector,
            name=name,
            relationship=relationship,
        )
        self.remember_household_state(state)
        return state

    def local_household_profiles(self) -> dict[str, dict[str, Any]]:
        value = self.config.get("household_local_profiles")
        if not isinstance(value, dict):
            return {}
        return {
            str(member_id): deepcopy(profile)
            for member_id, profile in value.items()
            if isinstance(member_id, str) and isinstance(profile, dict)
        }

    def _remember_local_household_profile(
        self,
        member_id: str,
        profile_data: dict[str, Any] | None,
    ) -> None:
        profiles = self.local_household_profiles()
        if profile_data is None:
            profiles.pop(member_id, None)
        else:
            profiles[member_id] = deepcopy(profile_data)
        if profiles:
            self.config["household_local_profiles"] = profiles
        else:
            self.config.pop("household_local_profiles", None)
        self._save()

    def save_local_child_profile(
        self,
        member_id: str,
        profile_data: dict[str, Any],
    ) -> dict[str, Any]:
        state = self.household_state()
        member = household.find_member(state, member_id)
        if member is None:
            raise household.HouseholdError(f"Unknown household member '{member_id}'.")
        if member.get("relationship") != "child":
            raise household.HouseholdError(
                "Only child profiles use local-only dietary storage."
            )
        self._remember_local_household_profile(member_id, profile_data)
        member["profile_synced"] = False
        self.remember_household_state(state)
        self._remember_household_profile_outbox(member_id, None)
        return {
            "member_id": member_id,
            "profile_data": deepcopy(profile_data),
            "storage": "local_only",
            "synced": False,
            "updated_at": utcnow().isoformat(),
        }

    def mark_household_profile_synced(self, member_id: str) -> None:
        state = self.household_state()
        member = household.find_member(state, member_id)
        if member is None or member.get("relationship") == "child":
            return
        member["profile_synced"] = True
        self.remember_household_state(state)
        self._remember_household_profile_outbox(member_id, None)

    def household_profile_outbox(self) -> dict[str, dict[str, Any]]:
        value = self.config.get("household_profile_outbox")
        if not isinstance(value, dict):
            return {}
        entries: dict[str, dict[str, Any]] = {}
        for member_id, entry in value.items():
            if not isinstance(member_id, str) or not isinstance(entry, dict):
                continue
            fields = entry.get("fields")
            local_context = entry.get("local_context")
            if not isinstance(fields, dict) or not isinstance(local_context, dict):
                continue
            entries[member_id] = {
                "version": 1,
                "fields": deepcopy(fields),
                "local_context": deepcopy(local_context),
                "updated_at": str(entry.get("updated_at") or utcnow().isoformat()),
            }
        return entries

    def _remember_household_profile_outbox(
        self,
        member_id: str,
        entry: dict[str, Any] | None,
    ) -> None:
        outbox = self.household_profile_outbox()
        if entry is None:
            outbox.pop(member_id, None)
        else:
            outbox[member_id] = {
                "version": 1,
                "fields": deepcopy(entry.get("fields") or {}),
                "local_context": deepcopy(entry.get("local_context") or {}),
                "updated_at": utcnow().isoformat(),
            }
        if outbox:
            self.config["household_profile_outbox"] = outbox
        else:
            self.config.pop("household_profile_outbox", None)
        self._save()

    def _retry_household_profile_outbox(
        self,
        member: dict[str, Any],
        entry: dict[str, Any],
    ) -> dict[str, Any]:
        member_id = str(member["id"])
        fields = entry.get("fields") if isinstance(entry.get("fields"), dict) else {}
        fallback = (
            deepcopy(entry.get("local_context"))
            if isinstance(entry.get("local_context"), dict)
            else {}
        )
        try:
            downloaded = self.download_profile(member_id=member_id)
        except HelloFoodError as exc:
            if not str(exc).startswith("404:"):
                return fallback
            base: dict[str, Any] = {}
            expected_version = None
        else:
            downloaded_profile = downloaded.get("profile_data")
            base = downloaded_profile if isinstance(downloaded_profile, dict) else {}
            version = downloaded.get("version")
            expected_version = int(version) if isinstance(version, int) else None
        desired = household.apply_profile_fields(base, fields)
        try:
            self.upload_profile(
                desired,
                member_id=member_id,
                expected_version=expected_version,
            )
        except (HelloFoodError, LoginRequired):
            return fallback or desired
        self.mark_household_profile_synced(member_id)
        return desired

    def _profile_for_agent(
        self,
        member: dict[str, Any],
        *,
        has_consent: bool,
    ) -> dict[str, Any]:
        member_id = str(member["id"])
        local_profile = self.local_household_profiles().get(member_id, {})
        if member.get("relationship") == "child":
            return local_profile
        outbox_entry = self.household_profile_outbox().get(member_id)
        if outbox_entry is not None:
            if has_consent:
                return self._retry_household_profile_outbox(member, outbox_entry)
            local_context = outbox_entry.get("local_context")
            return deepcopy(local_context) if isinstance(local_context, dict) else {}
        if not has_consent:
            return {}
        try:
            downloaded = self.download_profile(member_id=member_id)
        except HelloFoodError as exc:
            return {}
        profile_data = downloaded.get("profile_data")
        return profile_data if isinstance(profile_data, dict) else {}

    def agent_household_context(
        self,
        selector: str | None = None,
    ) -> dict[str, Any]:
        """Build the iOS-compatible household context for one agent turn.

        Roster metadata stays local. Adult dietary graphs are read from the
        server only for the selected scope. Child graphs use protected local
        storage and never enter profile sync; a failed adult sync write uses
        the same store as an outbox until repaired.
        """
        state = self.household_state()
        scope_id = household.resolve_scope(state, selector)
        consent = self.profile_consent_status()
        has_consent = bool(consent.get("has_consent"))
        self._profile_consent_cache = has_consent
        context: dict[str, Any] = {}
        if has_consent:
            context["device_context"] = {
                "household": household.roster_wire_context(state),
            }
        active = household.active_members(state)
        owner = household.find_member(state, household.OWNER_ID) or active[0]
        if scope_id == household.EVERYONE_ID:
            profiles = {
                str(member["id"]): self._profile_for_agent(
                    member,
                    has_consent=has_consent,
                )
                for member in active
            }
            context["dietary_context"] = household.household_dietary_context(
                state,
                profiles,
            )
            context["meal_context"] = {
                "is_cook_mode": True,
            }
        else:
            member = household.find_member(state, scope_id)
            if member is None:
                raise household.HouseholdError(f"Unknown household member '{scope_id}'.")
            profile_data = self._profile_for_agent(
                member,
                has_consent=has_consent,
            )
            context["dietary_context"] = household.member_dietary_context(
                member,
                profile_data,
                owner_name=str(owner["name"]),
            )
            context["meal_context"] = {
                "active_member_id": scope_id,
                "active_member_name": member["name"],
                "is_cook_mode": False,
            }
        context["scope"] = {
            "id": scope_id,
            "label": household.scope_label(state, scope_id),
            "mode": "household" if scope_id == household.EVERYONE_ID else "member",
        }
        return context

    def last_conversation(self) -> dict[str, Any]:
        value = self.config.get("last_conversation")
        return value if isinstance(value, dict) else {}

    def last_conversation_id(self) -> str | None:
        conversation = self.last_conversation()
        value = conversation.get("conversation_id")
        return value if isinstance(value, str) and value else None

    def last_conversation_household_scope(self) -> str | None:
        value = self.last_conversation().get("household_scope_id")
        return value if isinstance(value, str) and value else None

    def remember_conversation_household_scope(self, scope_id: str | None) -> None:
        if not scope_id:
            return
        conversation = self.last_conversation()
        if not conversation.get("conversation_id"):
            return
        conversation["household_scope_id"] = scope_id
        self.config["last_conversation"] = conversation
        self._save()

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

    def pending_confirmation_details(self) -> dict[str, Any] | None:
        conversation = self.last_conversation()
        pending = conversation.get("pending_confirmation")
        return deepcopy(pending) if isinstance(pending, dict) else None

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
                    "action": structured.get("action"),
                    "preview": structured.get("preview"),
                    "structured_preview": deepcopy(structured.get("structured_preview")),
                }

        self.config["last_conversation"] = {
            "conversation_id": conversation_id,
            "pending_confirmation": pending_confirmation,
            "updated_at": utcnow().isoformat(),
        }
        if isinstance(structured, dict) and structured.get("type") == "recipe_search":
            self.remember_recipe_search(structured, save=False)
        self._save()

    def apply_pending_household_confirmation(self) -> dict[str, Any] | None:
        pending = self.pending_confirmation_details()
        if not pending or pending.get("action") not in {
            "add_household_member",
            "update_household_member",
            "remove_household_member",
        }:
            return None
        mutation = pending.get("structured_preview")
        if not isinstance(mutation, dict):
            return {"applied": False, "reason": "missing_structured_preview"}
        return self._apply_and_sync_household_mutation(mutation)

    def apply_household_result(self, result: dict[str, Any]) -> dict[str, Any] | None:
        structured = result.get("structured")
        if not isinstance(structured, dict):
            return None
        mutation = structured.get("household_mutation")
        if not isinstance(mutation, dict):
            return None
        pending = self.pending_confirmation_details()
        pending_preview = (
            pending.get("structured_preview")
            if isinstance(pending, dict)
            else None
        )
        local_first_matches = (
            isinstance(pending_preview, dict)
            and pending_preview.get("operation") == mutation.get("operation")
        )
        return self._apply_and_sync_household_mutation(
            mutation,
            sync_profile=not local_first_matches,
        )

    def _apply_and_sync_household_mutation(
        self,
        mutation: dict[str, Any],
        *,
        sync_profile: bool = True,
    ) -> dict[str, Any]:
        previous_state = self.household_state()
        previous_member_id = str(mutation.get("member_id") or "")
        previous_member = household.find_member(previous_state, previous_member_id)
        mutation_fields_container = mutation.get("fields")
        requested_relationship = (
            mutation_fields_container.get("relationship")
            if isinstance(mutation_fields_container, dict)
            and "relationship" in mutation_fields_container
            else mutation.get("relationship")
        )
        if (
            previous_member is not None
            and previous_member.get("relationship") != "child"
            and requested_relationship == "child"
            and previous_member.get("profile_synced")
        ):
            return {
                "applied": False,
                "operation": mutation.get("operation"),
                "reason": "synced_profile_cannot_become_child",
                "member_id": previous_member_id,
                "name": previous_member.get("name"),
            }
        state, effect = household.apply_mutation(previous_state, mutation)
        self.remember_household_state(state)
        if effect.get("reason") == "already_applied":
            return effect
        matching_existing = effect.get("reason") == "matching_member_exists"
        if not effect.get("applied") and not matching_existing:
            return effect

        operation = mutation.get("operation")
        member_id = effect.get("member_id")
        if operation == "remove_member" and member_id:
            self._remember_local_household_profile(str(member_id), None)
            self._remember_household_profile_outbox(str(member_id), None)
            return effect
        if operation not in {"add_member", "update_member"} or not member_id:
            return effect
        updated_member = household.find_member(state, str(member_id))
        if updated_member is None:
            return effect
        if matching_existing:
            if updated_member.get("relationship") == "child":
                if str(member_id) in self.local_household_profiles():
                    return effect
            elif (
                updated_member.get("profile_synced")
                and str(member_id) not in self.household_profile_outbox()
            ):
                return effect

        if updated_member.get("relationship") == "child":
            local_profiles = self.local_household_profiles()
            base = local_profiles.get(str(member_id), {})
            if (
                not base
                and operation == "update_member"
                and previous_member is not None
                and previous_member.get("relationship") != "child"
            ):
                try:
                    downloaded = self.download_profile(member_id=str(member_id))
                except (HelloFoodError, LoginRequired):
                    pass
                else:
                    downloaded_profile = downloaded.get("profile_data")
                    if isinstance(downloaded_profile, dict):
                        base = downloaded_profile
            profile_data = household.profile_patch_from_mutation(base, mutation)
            self._remember_local_household_profile(str(member_id), profile_data)
            self._remember_household_profile_outbox(str(member_id), None)
            updated = self.household_state()
            stored_member = household.find_member(updated, str(member_id))
            if stored_member is not None:
                stored_member["profile_synced"] = False
                self.remember_household_state(updated)
            effect["profile_sync"] = {
                "ok": True,
                "source": "local_only",
                "server": False,
            }
            return effect

        outbox_entry = self.household_profile_outbox().get(str(member_id), {})
        pending_fields = (
            deepcopy(outbox_entry.get("fields"))
            if isinstance(outbox_entry.get("fields"), dict)
            else {}
        )
        mutation_profile_fields = household.profile_fields_from_mutation(mutation)
        if previous_member is not None and previous_member.get("relationship") == "child":
            child_profile = self.local_household_profiles().get(str(member_id), {})
            pending_fields.update(household.profile_fields_from_profile(child_profile))
        pending_fields.update(mutation_profile_fields)

        if operation == "update_member" and not pending_fields:
            effect["profile_sync"] = {"ok": True, "source": "not_required"}
            return effect
        if (
            not sync_profile
            and updated_member.get("profile_synced")
            and not outbox_entry
        ):
            effect["profile_sync"] = {"ok": True, "source": "local_first"}
            return effect

        profile_data: dict[str, Any] | None = None
        sync_error: HelloFoodError | LoginRequired | None = None
        if self._profile_consent_cache is False:
            sync_error = HelloFoodError("Profile sync consent is required.")
        try:
            if sync_error is not None:
                raise sync_error
            if previous_member is not None and previous_member.get("relationship") == "child":
                base = {}
                expected_version = None
            elif operation == "update_member" or matching_existing or outbox_entry:
                try:
                    downloaded = self.download_profile(member_id=str(member_id))
                except HelloFoodError as exc:
                    if not str(exc).startswith("404:"):
                        raise
                    base = {}
                    expected_version = None
                else:
                    downloaded_profile = downloaded.get("profile_data")
                    base = downloaded_profile if isinstance(downloaded_profile, dict) else {}
                    version = downloaded.get("version")
                    expected_version = int(version) if isinstance(version, int) else None
            else:
                base = {}
                expected_version = None
            profile_data = household.apply_profile_fields(base, pending_fields)
            self.upload_profile(
                profile_data,
                member_id=str(member_id),
                expected_version=(
                    int(expected_version)
                    if isinstance(expected_version, int)
                    else None
                ),
            )
        except (HelloFoodError, LoginRequired) as exc:
            if profile_data is None:
                local_context = outbox_entry.get("local_context")
                base = local_context if isinstance(local_context, dict) else {}
                profile_data = household.apply_profile_fields(base, pending_fields)
            self._remember_household_profile_outbox(
                str(member_id),
                {
                    "fields": pending_fields,
                    "local_context": profile_data,
                },
            )
            updated = self.household_state()
            member = household.find_member(updated, str(member_id))
            if member is not None:
                member["profile_synced"] = False
                self.remember_household_state(updated)
            effect["profile_sync"] = {
                "ok": False,
                "error": str(exc),
                "repair": (
                    "A future scoped agent turn will retry automatically. "
                    f"For an adult profile, `heyfood onboard --member-id {member_id}` "
                    "can also repair sync."
                ),
            }
        else:
            self.mark_household_profile_synced(str(member_id))
            self._remember_local_household_profile(str(member_id), None)
            effect["profile_sync"] = {"ok": True}
        return effect

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


def _raise_transcription_error(response: httpx.Response) -> None:
    """Map a transcription error response onto a typed exception.

    Keyed on the contract body ``{"error": <code>, "message": <human text>}`` —
    the channel-tools convention, NOT ``detail``.
    """
    status = response.status_code
    body: dict[str, Any] = {}
    try:
        parsed = response.json()
        if isinstance(parsed, dict):
            body = parsed
    except Exception:
        body = {}
    error = str(body.get("error") or "")[:200]
    # Bound the server-controlled human string so an oversized error document
    # can never flood the terminal; callers render it literally, never as markup.
    message = str(body.get("message") or body.get("error") or f"HTTP {status}")[:500]

    if status == 429:
        raise TranscriptionRateLimited(
            message,
            retry_after=_parse_retry_after(response.headers.get("Retry-After")),
        )
    if status in (400, 413):
        raise TranscriptionRejected(message)
    if status == 403 and error == "insufficient_scope":
        raise TranscriptionScopeRequired(message)
    if status == 401:
        raise LoginRequired("Run `heyfood login` first.")
    if status in (404, 503):
        raise TranscriptionUnavailable(message)
    raise HelloFoodError(f"{status}: {message}")


def _parse_retry_after(value: str | None) -> int | None:
    """Parse a ``Retry-After`` header as a bounded, non-negative integer.

    Only the delta-seconds form is honored; an HTTP-date, a malformed value, or
    an absurdly large number yields ``None`` (the caller falls back to its own
    fixed guidance) or a value clamped to a sane ceiling. Never trusts the raw
    header as a display string.
    """
    if value is None:
        return None
    text = value.strip()
    if not text.isdigit():
        return None
    try:
        seconds = int(text)
    except ValueError:
        return None
    if seconds < 0:
        return None
    return min(seconds, 3600)


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
