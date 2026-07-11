"""Terminal-safe hey.food banner rendering and once-per-process lifecycle."""
from __future__ import annotations

from dataclasses import dataclass
from importlib.resources import files
import json
import os
from typing import Any

from rich.console import Console
from rich.style import Style
from rich.text import Text


FULL_BANNER_WIDTH = 44
COMPACT_BANNER = "hey.food"


def plain_banner() -> str:
    return (
        files("heyfood_cli.data")
        .joinpath("banner.txt")
        .read_text(encoding="utf-8")
        .rstrip("\n")
    )


def palette() -> dict[str, Any]:
    return json.loads(
        files("heyfood_cli.data")
        .joinpath("banner.palette.json")
        .read_text(encoding="utf-8")
    )


def supports_banner(console: Console) -> bool:
    if not console.is_terminal or console.width < FULL_BANNER_WIDTH:
        return False
    if os.environ.get("TERM", "").lower() == "dumb" or "CI" in os.environ:
        return False
    encoding = getattr(console.file, "encoding", None) or "utf-8"
    try:
        plain_banner().encode(encoding)
    except (LookupError, UnicodeEncodeError):
        return False
    return True


def banner_disabled() -> bool:
    return os.environ.get("HEYFOOD_NO_BANNER", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


def banner_text(*, color: bool) -> Text:
    geometry = plain_banner()
    if not color:
        return Text(geometry)
    colors = palette()
    foreground = Style(color=colors["foreground"])
    accent = Style(color=colors["accent"])
    spans = {
        (int(span["line"]), int(span["start"]), int(span["length"]))
        for span in colors["accent_spans"]
    }
    result = Text()
    for line_number, line in enumerate(geometry.splitlines()):
        if line_number:
            result.append("\n")
        cursor = 0
        for span_line, start, length in sorted(spans):
            if span_line != line_number:
                continue
            result.append(line[cursor:start], foreground)
            result.append(line[start : start + length], accent)
            cursor = start + length
        result.append(line[cursor:], foreground)
    return result


@dataclass
class BannerController:
    disabled: bool = False
    rendered: bool = False

    def configure(self, *, disabled: bool) -> None:
        self.disabled = disabled

    def loading(
        self,
        console: Console,
        *,
        json_mode: bool = False,
        no_input: bool = False,
    ) -> bool:
        if (
            self.rendered
            or self.disabled
            or banner_disabled()
            or json_mode
            or no_input
            or not supports_banner(console)
        ):
            return False
        console.print(banner_text(color="NO_COLOR" not in os.environ))
        self.rendered = True
        return True

    def welcome(self, console: Console) -> bool:
        if (
            self.rendered
            or self.disabled
            or banner_disabled()
            or not console.is_terminal
            or os.environ.get("TERM", "").lower() == "dumb"
            or "CI" in os.environ
        ):
            return False
        if supports_banner(console):
            console.print(banner_text(color="NO_COLOR" not in os.environ))
        else:
            console.print(COMPACT_BANNER)
        self.rendered = True
        return True


controller = BannerController()
