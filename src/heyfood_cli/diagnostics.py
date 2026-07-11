"""Safe stderr-only diagnostics for developer troubleshooting."""

from __future__ import annotations

from dataclasses import dataclass
import re
from typing import Any

from rich.console import Console
from rich.text import Text


_SENSITIVE_FIELD_FRAGMENTS = {
    "authorization",
    "body",
    "cookie",
    "diet",
    "key",
    "payload",
    "phone",
    "profile",
    "query",
    "secret",
    "token",
}
_BEARER = re.compile(r"(?i)bearer\s+[a-z0-9._~-]+")
_TOKENISH = re.compile(r"(?i)\bhf_(?:at|ct|rt)_[a-z0-9._~-]+\b")
_PHONE = re.compile(r"(?<!\w)(?:\+?\d[\d(). -]{7,}\d)")


def _field_allowed(name: str) -> bool:
    normalized = name.lower()
    return not any(fragment in normalized for fragment in _SENSITIVE_FIELD_FRAGMENTS)


def _safe_value(value: Any, *, redact_phone: bool) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    text = str(value).replace("\n", " ").replace("\r", " ")[:200]
    text = _BEARER.sub("[redacted]", text)
    text = _TOKENISH.sub("[redacted]", text)
    return _PHONE.sub("[redacted-phone]", text) if redact_phone else text


@dataclass
class DiagnosticReporter:
    enabled: bool = False
    console: Console | None = None

    def configure(self, *, enabled: bool, console: Console) -> None:
        self.enabled = enabled
        self.console = console

    def emit(self, event: str, **fields: Any) -> None:
        if not self.enabled or self.console is None:
            return
        line = Text("verbose ", style="dim")
        line.append(event, style="bold dim")
        for name, value in fields.items():
            if value is None or not _field_allowed(name):
                continue
            line.append(f" {name}=", style="dim")
            line.append(
                _safe_value(value, redact_phone=not name.lower().endswith("_id")),
                style="dim",
            )
        self.console.print(line)


reporter = DiagnosticReporter()
