from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal, TypeAlias

from rich.console import Console
from rich.table import Table
from rich.text import Text

from .theme import HEYFOOD_COLORS


Tone: TypeAlias = Literal[
    "default",
    "accent",
    "bright",
    "muted",
    "info",
    "warning",
    "danger",
]


@dataclass(frozen=True)
class Segment:
    text: str
    tone: Tone = "default"
    bold: bool = False


Cell: TypeAlias = tuple[Segment, ...]


@dataclass(frozen=True)
class Column:
    min_width: int = 0
    ratio: int = 1
    no_wrap: bool = False
    justify: Literal["left", "center", "right"] = "left"


@dataclass(frozen=True)
class Line:
    segments: tuple[Segment, ...] = ()
    kind: Literal["line"] = "line"


@dataclass(frozen=True)
class Rows:
    rows: tuple[tuple[Cell, ...], ...]
    columns: tuple[Column, ...]
    kind: Literal["rows"] = "rows"


Block: TypeAlias = Line | Rows


def segment(text: Any, tone: Tone = "default", *, bold: bool = False) -> Segment:
    return Segment(str(text), tone=tone, bold=bold)


def cell(text: Any, tone: Tone = "default", *, bold: bool = False) -> Cell:
    return (segment("" if text is None else text, tone, bold=bold),)


def line(*segments: Segment) -> Line:
    return Line(tuple(segments))


def text_line(text: Any, tone: Tone = "default", *, bold: bool = False) -> Line:
    return line(segment(text, tone, bold=bold))


def blank() -> Line:
    return Line()


def render(console: Console, blocks: list[Block] | tuple[Block, ...]) -> None:
    for block in blocks:
        if isinstance(block, Line):
            console.print(_rich_text(block.segments))
            continue

        if console.width < 64 and len(block.columns) > 1:
            for row in block.rows:
                lead = Text()
                for index, entry in enumerate(row[:2]):
                    if index:
                        lead.append("  ")
                    lead.append_text(_rich_text(entry))
                console.print(lead)
                for entry in row[2:]:
                    detail = Text("  ")
                    detail.append_text(_rich_text(entry))
                    console.print(detail)
            continue

        table = Table.grid(padding=(0, 2), expand=True)
        for column in block.columns:
            table.add_column(
                min_width=column.min_width,
                ratio=column.ratio,
                no_wrap=column.no_wrap,
                justify=column.justify,
                overflow="fold",
            )
        for row in block.rows:
            table.add_row(*(_rich_text(entry) for entry in row))
        console.print(table)


def to_data(blocks: list[Block] | tuple[Block, ...]) -> list[dict[str, Any]]:
    output: list[dict[str, Any]] = []
    for block in blocks:
        if isinstance(block, Line):
            output.append(
                {
                    "kind": "line",
                    "segments": [_segment_data(value) for value in block.segments],
                }
            )
            continue

        output.append(
            {
                "kind": "rows",
                "columns": [
                    {
                        "minWidth": column.min_width,
                        "ratio": column.ratio,
                        "noWrap": column.no_wrap,
                        "justify": column.justify,
                    }
                    for column in block.columns
                ],
                "rows": [
                    [
                        [_segment_data(value) for value in entry]
                        for entry in row
                    ]
                    for row in block.rows
                ],
            }
        )
    return output


def _segment_data(value: Segment) -> dict[str, Any]:
    data: dict[str, Any] = {"text": value.text}
    if value.tone != "default":
        data["tone"] = value.tone
    if value.bold:
        data["bold"] = True
    return data


def _rich_text(segments: tuple[Segment, ...]) -> Text:
    value = Text()
    for item in segments:
        style = HEYFOOD_COLORS.get(item.tone)
        if item.bold:
            style = f"bold {style}" if style else "bold"
        value.append(item.text, style=style)
    return value
