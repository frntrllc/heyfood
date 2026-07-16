"""Application-level authentication and first-run contract validation.

Commands provide presentation callbacks; this module owns intent, capability
preflight, and the shared loopback/device authentication decision. It never
prints, prompts, or returns credential material.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Literal
from uuid import uuid4

import httpx

from . import __version__, diagnostics
from .auth import AuthIntent, perform_device_login, perform_login
from .config import APP_CLIENT_ID, ConfigStore


IdentityMethod = Literal["sms", "email", "apple", "google"]
RegistrationStatus = Literal["available", "disabled", "unavailable"]
ProfileStatus = Literal["ready", "missing", "unknown"]

_IDENTITY_METHODS = frozenset({"sms", "email", "apple", "google"})
_CAPABILITY_KEYS = frozenset(
    {"schema_version", "self_registration", "authorization", "profile_readiness"}
)
_REGISTRATION_KEYS = frozenset({"status", "regions", "identity_methods"})
_AUTHORIZATION_KEYS = frozenset(
    {"loopback_pkce", "device_code", "identity_methods"}
)
_READINESS_KEYS = frozenset(
    {
        "schema_version",
        "status",
        "has_profile_sync_consent",
        "member_id",
        "profile_version",
    }
)


class AuthApplicationError(RuntimeError):
    kind = "authentication_failed"
    hint: str | None = None


class AuthContractError(AuthApplicationError):
    kind = "auth_contract_error"
    hint = (
        "Update heyfood and try again. If the problem continues, "
        "check https://status.hello.food."
    )


class RegistrationUnavailable(AuthApplicationError):
    kind = "registration_unavailable"
    hint = (
        "Retry `heyfood register` when registration is available. "
        "Existing users can still run `heyfood login`."
    )

    def __init__(self, status: str) -> None:
        self.status = status
        if status == "disabled":
            message = "New hello.food account registration is not enabled right now."
        else:
            message = "New hello.food account registration is temporarily unavailable."
        super().__init__(message)


@dataclass(frozen=True)
class AuthCapabilities:
    registration_status: RegistrationStatus
    registration_regions: tuple[str, ...]
    registration_identity_methods: tuple[IdentityMethod, ...]
    loopback_pkce: bool
    device_code: bool
    authorization_identity_methods: tuple[IdentityMethod, ...]
    profile_readiness: bool


@dataclass(frozen=True)
class ProfileReadiness:
    status: ProfileStatus
    has_profile_sync_consent: bool | None
    profile_version: int | None

    def document(self) -> dict[str, Any]:
        return {
            "profile_status": self.status,
            "has_profile_sync_consent": self.has_profile_sync_consent,
            "profile_version": self.profile_version,
        }


@dataclass(frozen=True)
class AuthApplicationResult:
    intent: AuthIntent
    capabilities: AuthCapabilities | None


def authenticate(
    *,
    intent: AuthIntent,
    store: ConfigStore,
    api_url: str,
    auth_url: str,
    api_key: str | None,
    device: bool,
    open_browser: bool,
    timeout_seconds: int,
    authorize_url_callback: Callable[[str], None],
    device_authorization_callback: Callable[[str, str], None],
    capabilities_callback: Callable[[AuthCapabilities], None] | None = None,
    capability_loader: Callable[[str], AuthCapabilities] | None = None,
    login_runner: Callable[..., dict[str, Any]] = perform_login,
    device_login_runner: Callable[..., dict[str, Any]] = perform_device_login,
) -> AuthApplicationResult:
    """Authenticate once through the shared login implementation.

    Registration always preflights the exact API origin before opening a
    browser or creating an OAuth client. Login deliberately retains legacy
    compatibility and therefore does not require the additive capabilities
    endpoint.
    """
    loader = capability_loader or fetch_auth_capabilities
    capabilities = loader(api_url) if intent == "register" else None
    if capabilities is not None:
        ensure_registration_available(capabilities, device=device)
        if capabilities_callback is not None:
            capabilities_callback(capabilities)

    if device:
        device_login_runner(
            store=store,
            api_url=api_url,
            auth_url=auth_url,
            api_key=api_key,
            open_browser=open_browser,
            timeout_seconds=timeout_seconds,
            authorization_callback=device_authorization_callback,
            intent=intent,
        )
    else:
        login_runner(
            store=store,
            api_url=api_url,
            auth_url=auth_url,
            api_key=api_key,
            open_browser=open_browser,
            timeout_seconds=timeout_seconds,
            authorize_url_callback=authorize_url_callback,
            intent=intent,
        )
    return AuthApplicationResult(intent=intent, capabilities=capabilities)


def fetch_auth_capabilities(api_url: str) -> AuthCapabilities:
    request_id = str(uuid4())
    path = "/v1/auth/capabilities"
    diagnostics.reporter.emit(
        "http.start",
        request_id=request_id,
        context="authentication",
        method="GET",
        endpoint=path,
    )
    try:
        with httpx.Client(timeout=20.0) as client:
            response = client.get(
                f"{api_url.rstrip('/')}/v1/auth/capabilities",
                headers={
                    "Accept": "application/json",
                    "User-Agent": f"heyfood-cli/{__version__}",
                    "X-App-Client-ID": APP_CLIENT_ID,
                    "X-Request-ID": request_id,
                },
            )
    except httpx.HTTPError as exc:
        diagnostics.reporter.emit(
            "http.error",
            request_id=request_id,
            context="authentication",
            method="GET",
            endpoint=path,
            error=type(exc).__name__,
        )
        raise AuthApplicationError(
            "Could not reach hello.food to check registration availability."
        ) from exc
    diagnostics.reporter.emit(
        "http.complete",
        request_id=request_id,
        server_request_id=response.headers.get("X-Request-ID"),
        context="authentication",
        method="GET",
        endpoint=path,
        status=response.status_code,
    )
    if response.status_code in {404, 405}:
        raise RegistrationUnavailable("unavailable")
    if response.status_code >= 400:
        raise AuthApplicationError(
            f"hello.food registration preflight failed with HTTP {response.status_code}."
        )
    try:
        payload = response.json()
    except ValueError as exc:
        raise AuthContractError(
            "hello.food returned an unreadable registration capability response."
        ) from exc
    return validate_auth_capabilities(payload)


def validate_auth_capabilities(payload: Any) -> AuthCapabilities:
    if not isinstance(payload, dict) or frozenset(payload) != _CAPABILITY_KEYS:
        raise AuthContractError(
            "hello.food returned an unsupported registration capability response."
        )
    if payload.get("schema_version") != 1 or not isinstance(
        payload.get("profile_readiness"), bool
    ):
        raise AuthContractError(
            "hello.food returned an unsupported registration capability response."
        )
    registration = payload.get("self_registration")
    authorization = payload.get("authorization")
    if (
        not isinstance(registration, dict)
        or frozenset(registration) != _REGISTRATION_KEYS
        or not isinstance(authorization, dict)
        or frozenset(authorization) != _AUTHORIZATION_KEYS
    ):
        raise AuthContractError(
            "hello.food returned an unsupported registration capability response."
        )

    status = registration.get("status")
    if status not in {"available", "disabled", "unavailable"}:
        raise AuthContractError(
            "hello.food returned an unsupported registration status."
        )
    regions = _string_list(registration.get("regions"), field="registration regions")
    registration_methods = _identity_methods(registration.get("identity_methods"))
    authorization_methods = _identity_methods(authorization.get("identity_methods"))
    loopback = authorization.get("loopback_pkce")
    device_code = authorization.get("device_code")
    if not isinstance(loopback, bool) or not isinstance(device_code, bool):
        raise AuthContractError(
            "hello.food returned an unsupported authorization capability response."
        )

    # An available launch is US-only and must advertise at least one account
    # creation method. Disabled/unavailable surfaces must not advertise either.
    if status == "available":
        if regions != ("US",) or not registration_methods:
            raise AuthContractError(
                "hello.food returned inconsistent registration capabilities."
            )
    elif regions or registration_methods:
        raise AuthContractError(
            "hello.food returned inconsistent registration capabilities."
        )

    return AuthCapabilities(
        registration_status=status,
        registration_regions=regions,
        registration_identity_methods=registration_methods,
        loopback_pkce=loopback,
        device_code=device_code,
        authorization_identity_methods=authorization_methods,
        profile_readiness=payload["profile_readiness"],
    )


def ensure_registration_available(
    capabilities: AuthCapabilities,
    *,
    device: bool,
) -> None:
    if capabilities.registration_status != "available":
        raise RegistrationUnavailable(capabilities.registration_status)
    if not capabilities.profile_readiness:
        raise AuthContractError(
            "hello.food registration is missing the required profile-readiness capability."
        )
    if device and not capabilities.device_code:
        raise AuthApplicationError("Device-code authorization is unavailable right now.")
    if not device and not capabilities.loopback_pkce:
        raise AuthApplicationError(
            "Browser callback authorization is unavailable. Try `heyfood register --device`."
        )
    if not capabilities.authorization_identity_methods:
        raise AuthContractError(
            "hello.food did not advertise an available identity method."
        )
    if not set(capabilities.registration_identity_methods).issubset(
        capabilities.authorization_identity_methods
    ):
        raise AuthContractError("hello.food returned inconsistent identity methods.")


def validate_profile_readiness(payload: Any) -> ProfileReadiness:
    if not isinstance(payload, dict) or frozenset(payload) != _READINESS_KEYS:
        raise AuthContractError(
            "hello.food returned an unsupported profile-readiness response."
        )
    if payload.get("schema_version") != 1 or payload.get("member_id") != "_self":
        raise AuthContractError(
            "hello.food returned an unsupported profile-readiness response."
        )
    status = payload.get("status")
    consent = payload.get("has_profile_sync_consent")
    version = payload.get("profile_version")
    if status not in {"ready", "missing", "unknown"}:
        raise AuthContractError(
            "hello.food returned an unsupported profile-readiness status."
        )
    if consent is not None and not isinstance(consent, bool):
        raise AuthContractError("hello.food returned an invalid profile consent state.")
    if version is not None and (
        not isinstance(version, int) or isinstance(version, bool) or version < 1
    ):
        raise AuthContractError("hello.food returned an invalid profile version.")
    if status == "ready" and (consent is not True or version is None):
        raise AuthContractError(
            "hello.food returned an inconsistent ready profile state."
        )
    if status == "missing" and (consent not in {True, False} or version is not None):
        raise AuthContractError(
            "hello.food returned an inconsistent missing profile state."
        )
    if status == "unknown" and (consent not in {None, True} or version is not None):
        raise AuthContractError(
            "hello.food returned an inconsistent unknown profile state."
        )
    return ProfileReadiness(
        status=status,
        has_profile_sync_consent=consent,
        profile_version=version,
    )


def _identity_methods(value: Any) -> tuple[IdentityMethod, ...]:
    methods = _string_list(value, field="identity methods")
    if any(method not in _IDENTITY_METHODS for method in methods):
        raise AuthContractError("hello.food returned an unknown identity method.")
    return methods  # type: ignore[return-value]


def _string_list(value: Any, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(
        not isinstance(item, str) or not item or len(item) > 32 for item in value
    ):
        raise AuthContractError(f"hello.food returned invalid {field}.")
    if len(value) != len(set(value)):
        raise AuthContractError(f"hello.food returned duplicate {field}.")
    return tuple(value)
