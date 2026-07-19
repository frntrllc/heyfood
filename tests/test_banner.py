from __future__ import annotations

from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
import re

from rich.console import Console

from heyfood_cli import banner


PROJECT_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = (
    PROJECT_ROOT
    if (PROJECT_ROOT / "docs" / "references").is_dir()
    else PROJECT_ROOT.parent
)
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def _terminal_console(*, width: int = 80, color: bool = True, file=None) -> tuple[Console, StringIO]:
    stream = file or StringIO()
    return (
        Console(
            file=stream,
            force_terminal=True,
            color_system="truecolor" if color else None,
            width=width,
            highlight=False,
        ),
        stream,
    )


def _clear_terminal_env(monkeypatch):
    for name in ("CI", "TERM", "HEYFOOD_NO_BANNER", "NO_COLOR"):
        monkeypatch.delenv(name, raising=False)


def test_packaged_geometry_matches_canonical_reference_byte_for_byte():
    reference = (REPO_ROOT / "docs/references/banner.txt").read_text(encoding="utf-8")
    assert banner.plain_banner() + "\n" == reference
    assert max(map(len, banner.plain_banner().splitlines())) == 44


def test_packaged_palette_matches_language_neutral_reference():
    reference = json.loads(
        (REPO_ROOT / "docs/references/banner.palette.json").read_text(encoding="utf-8")
    )
    assert banner.palette() == reference
    assert reference["accent_spans"] == [{"line": 3, "start": 18, "length": 2}]


def test_full_tty_banner_has_exact_plain_geometry_and_palette(monkeypatch):
    _clear_terminal_env(monkeypatch)
    console, stream = _terminal_console(width=44)
    controller = banner.BannerController()
    assert controller.welcome(console) is True
    rendered = stream.getvalue()
    assert ANSI.sub("", rendered).rstrip("\n") == banner.plain_banner()
    assert "\x1b[38;2;155;197;61m" in rendered
    assert "\x1b[38;2;237;234;224m" in rendered


def test_no_color_keeps_geometry_without_ansi(monkeypatch):
    _clear_terminal_env(monkeypatch)
    monkeypatch.setenv("NO_COLOR", "1")
    console, stream = _terminal_console(width=80, color=False)
    assert banner.BannerController().welcome(console) is True
    assert stream.getvalue().rstrip("\n") == banner.plain_banner()
    assert "\x1b" not in stream.getvalue()


def test_startup_banner_is_suppressed_for_inaccessible_surfaces(monkeypatch):
    _clear_terminal_env(monkeypatch)
    for name, value in (("CI", "1"), ("TERM", "dumb"), ("HEYFOOD_NO_BANNER", "1")):
        monkeypatch.setenv(name, value)
        console, stream = _terminal_console()
        assert banner.BannerController().welcome(console) is False
        assert stream.getvalue() == ""
        monkeypatch.delenv(name)

    stream = StringIO()
    console = Console(file=stream, force_terminal=False, width=80)
    assert banner.BannerController().welcome(console) is False
    assert stream.getvalue() == ""


def test_narrow_or_non_unicode_welcome_uses_compact_fallback(monkeypatch):
    _clear_terminal_env(monkeypatch)
    console, stream = _terminal_console(width=43, color=False)
    assert banner.BannerController().welcome(console) is True
    assert stream.getvalue().strip() == "hey.food"

    class AsciiStream(StringIO):
        @property
        def encoding(self):
            return "ascii"

    ascii_stream = AsciiStream()
    console, _ = _terminal_console(width=80, color=False, file=ascii_stream)
    assert banner.BannerController().welcome(console) is True
    assert ascii_stream.getvalue().strip() == "hey.food"


def test_welcome_suppresses_branding_on_noninteractive_surfaces(monkeypatch):
    _clear_terminal_env(monkeypatch)
    stream = StringIO()
    console = Console(file=stream, force_terminal=False, width=80)
    assert banner.BannerController().welcome(console) is False
    assert stream.getvalue() == ""

    for name, value in (("CI", ""), ("TERM", "dumb")):
        monkeypatch.setenv(name, value)
        console, stream = _terminal_console()
        assert banner.BannerController().welcome(console) is False
        assert stream.getvalue() == ""
        monkeypatch.delenv(name)


def test_startup_banner_renders_once_and_uses_no_live_cursor_controls(monkeypatch):
    _clear_terminal_env(monkeypatch)
    console, stream = _terminal_console()
    controller = banner.BannerController()
    assert controller.welcome(console) is True
    assert controller.welcome(console) is False
    rendered = stream.getvalue()
    assert ANSI.sub("", rendered).count(banner.plain_banner()) == 1
    assert "\x1b[?25" not in rendered


def test_agent_never_renders_startup_banner(monkeypatch):
    from heyfood_cli import main

    _clear_terminal_env(monkeypatch)

    class FakeClient:
        def saved_location(self):
            return None

        def stream_agent(self, _payload):
            yield "result", {"message": "done"}

        def remember_conversation(self, _result):
            pass

    monkeypatch.setattr(main, "HelloFoodClient", lambda: FakeClient())
    stdout = StringIO()
    stderr_console, stderr = _terminal_console()
    monkeypatch.setattr(main, "console", Console(file=stdout, force_terminal=False))
    monkeypatch.setattr(main, "stderr_console", stderr_console)

    banner.controller = banner.BannerController()
    monkeypatch.setattr(main.banner, "controller", banner.controller)
    main._ask_agent("hello", show_continue_hint=False)
    assert banner.plain_banner() not in ANSI.sub("", stderr.getvalue())
    assert banner.plain_banner() not in stdout.getvalue()

    stdout = StringIO()
    stderr_console, stderr = _terminal_console()
    monkeypatch.setattr(main, "stderr_console", stderr_console)
    banner.controller = banner.BannerController()
    monkeypatch.setattr(main.banner, "controller", banner.controller)
    with redirect_stdout(stdout):
        main._ask_agent("hello", json_output=True, show_continue_hint=False)
    assert json.loads(stdout.getvalue())["message"] == "done"
    assert banner.plain_banner() not in ANSI.sub("", stderr.getvalue())


def test_agent_interrupt_restores_terminal_without_live_cursor_state(monkeypatch):
    from heyfood_cli import main

    _clear_terminal_env(monkeypatch)

    class InterruptedClient:
        def saved_location(self):
            return None

        def stream_agent(self, _payload):
            raise KeyboardInterrupt
            yield  # pragma: no cover - makes this a generator

    monkeypatch.setattr(main, "HelloFoodClient", lambda: InterruptedClient())
    stderr_console, stderr = _terminal_console()
    monkeypatch.setattr(main, "stderr_console", stderr_console)
    banner.controller = banner.BannerController()
    monkeypatch.setattr(main.banner, "controller", banner.controller)

    try:
        main._ask_agent("hello", show_continue_hint=False)
    except KeyboardInterrupt:
        pass
    else:  # pragma: no cover - defensive assertion
        raise AssertionError("expected KeyboardInterrupt")

    rendered = stderr.getvalue()
    assert banner.plain_banner() not in ANSI.sub("", rendered)
    assert rendered.count("\x1b[?25l") == rendered.count("\x1b[?25h") == 1
    assert rendered.rfind("\x1b[?25h") > rendered.rfind("\x1b[?25l")
    assert "\x1b[?1049" not in rendered
