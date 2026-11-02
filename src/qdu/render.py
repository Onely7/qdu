"""Render qdu results as safe tables, TSV, JSON, and diagnostics."""

from __future__ import annotations

import json
import shutil
import sys
import unicodedata
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from typing import TextIO


@dataclass(frozen=True, slots=True)
class Palette:
    """ANSI sequences used by styled terminal output."""

    red: str = ""
    green: str = ""
    yellow: str = ""
    blue: str = ""
    orange: str = ""
    cyan: str = ""
    bold: str = ""
    dim: str = ""
    reset: str = ""


@dataclass(frozen=True, slots=True)
class TableCell:
    """Display value with an optional palette style name."""

    value: object
    style: str | None = None


@dataclass(frozen=True, slots=True)
class RenderOptions:
    """Output format, terminal style, color policy, and destination stream."""

    output_format: str = "table"
    style: str = "auto"
    color: str = "auto"
    stream: TextIO = sys.stdout

    @property
    def rich(self) -> bool:
        """Whether tables should use Unicode borders and rich headings."""
        if self.output_format != "table":
            return False
        if self.style == "rich":
            return True
        if self.style == "plain":
            return False
        encoding = (getattr(self.stream, "encoding", None) or "").lower()
        return self.stream.isatty() and "utf" in encoding

    @property
    def palette(self) -> Palette:
        """Effective ANSI palette after applying the color policy."""
        enabled = self.color == "always" or (
            self.color == "auto" and self.stream.isatty()
        )
        if not enabled:
            return Palette()
        return Palette(
            red="\033[31m",
            green="\033[32m",
            yellow="\033[33m",
            blue="\033[34m",
            orange="\033[38;5;208m",
            cyan="\033[36m",
            bold="\033[1m",
            dim="\033[2m",
            reset="\033[0m",
        )


class Renderer:
    """Write deterministic machine output or terminal-friendly reports."""

    def __init__(self, options: RenderOptions) -> None:
        self.options = options
        self.stream = options.stream
        self.palette = options.palette

    def json(self, value: object) -> None:
        """Write one pretty-printed JSON value followed by a newline."""
        json.dump(value, self.stream, ensure_ascii=False, indent=2, sort_keys=False)
        self.stream.write("\n")

    def tsv(self, headers: Sequence[str], rows: Iterable[Sequence[object]]) -> None:
        """Write escaped tab-separated headers and rows."""
        self.stream.write("\t".join(headers) + "\n")
        for row in rows:
            self.stream.write("\t".join(_tsv_value(value) for value in row) + "\n")

    def message(self, text: str) -> None:
        """Write a plain line to the configured output stream."""
        self.stream.write(text + "\n")

    def warning(self, text: str) -> None:
        """Write a prefixed warning to standard error."""
        sys.stderr.write(
            f"{self.palette.yellow}qdu: warning: {text}{self.palette.reset}\n"
        )

    def error(self, text: str) -> None:
        """Write a prefixed error to standard error."""
        sys.stderr.write(f"{self.palette.red}qdu: {text}{self.palette.reset}\n")

    def heading(self, text: str) -> None:
        """Write a heading, using bold style only when enabled."""
        if self.options.rich:
            self.stream.write(f"{self.palette.bold}{text}{self.palette.reset}\n")
        else:
            self.stream.write(text + "\n")

    def key_values(self, values: Sequence[tuple[str, object]]) -> None:
        """Write aligned label-value lines; a list or tuple takes a line per item."""
        if not values:
            return
        width = max(display_width(label) for label, _ in values)
        for label, value in values:
            items = value if isinstance(value, (list, tuple)) else (value,)
            for index, item in enumerate(items or ("",)):
                head = pad(label, width) if index == 0 else " " * width
                text = display_safe(str(item))
                self.stream.write(f"{head}  {text}\n" if text else f"{head.rstrip()}\n")

    def table(
        self,
        headers: Sequence[str],
        rows: Sequence[Sequence[object]],
        *,
        alignments: Sequence[str] | None = None,
        max_width: int | None = None,
        empty_message: str = "No matching entries.",
    ) -> None:
        """Write a width-aware table, truncating its final column as needed."""
        if not rows:
            self.message(empty_message)
            return
        text_rows = [[_normalize_cell(value) for value in row] for row in rows]
        aligns = list(alignments or ["left"] * len(headers))
        widths = [display_width(str(header)) for header in headers]
        for row in text_rows:
            for index, cell in enumerate(row):
                widths[index] = max(widths[index], display_width(cell.value))

        terminal_width = max_width or shutil.get_terminal_size((120, 24)).columns
        border_overhead = (
            (3 * len(headers)) + 1 if self.options.rich else (2 * (len(headers) - 1))
        )
        total = sum(widths) + border_overhead
        if total > terminal_width and widths:
            path_index = len(widths) - 1
            widths[path_index] = max(12, widths[path_index] - (total - terminal_width))

        if self.options.rich:
            self._rich_table(headers, text_rows, widths, aligns)
        else:
            self._plain_table(headers, text_rows, widths, aligns)

    def _rich_table(
        self,
        headers: Sequence[str],
        rows: Sequence[Sequence[TableCell]],
        widths: Sequence[int],
        alignments: Sequence[str],
    ) -> None:
        top = "┌" + "┬".join("─" * (width + 2) for width in widths) + "┐"
        middle = "├" + "┼".join("─" * (width + 2) for width in widths) + "┤"
        bottom = "└" + "┴".join("─" * (width + 2) for width in widths) + "┘"
        self.stream.write(top + "\n")
        self.stream.write(
            "│"
            + "│".join(
                f" {self.palette.bold}{fit(str(value), width, alignment='center')}{self.palette.reset} "
                for value, width in zip(headers, widths, strict=False)
            )
            + "│\n"
        )
        self.stream.write(middle + "\n")
        for row in rows:
            self.stream.write(
                "│"
                + "│".join(
                    f" {_paint(fit(cell.value, width, alignment=alignment), cell.style, self.palette)} "
                    for cell, width, alignment in zip(
                        row, widths, alignments, strict=False
                    )
                )
                + "│\n"
            )
        self.stream.write(bottom + "\n")

    def _plain_table(
        self,
        headers: Sequence[str],
        rows: Sequence[Sequence[TableCell]],
        widths: Sequence[int],
        alignments: Sequence[str],
    ) -> None:
        # Headers line up with their column, as in df or ps.
        self._plain_line([TableCell(header) for header in headers], widths, alignments)
        self.stream.write("  ".join("-" * width for width in widths) + "\n")
        for row in rows:
            self._plain_line(row, widths, alignments)

    def _plain_line(
        self,
        cells: Sequence[TableCell],
        widths: Sequence[int],
        alignments: Sequence[str],
    ) -> None:
        last = len(widths) - 1
        self.stream.write(
            "  ".join(
                _paint(
                    # An unpadded last column leaves no trailing blanks.
                    truncate(str(cell.value), width)
                    if index == last and alignment == "left"
                    else fit(str(cell.value), width, alignment=alignment),
                    cell.style,
                    self.palette,
                )
                for index, (cell, width, alignment) in enumerate(
                    zip(cells, widths, alignments, strict=False)
                )
            )
            + "\n"
        )


class ProgressLine:
    """Redraw one status line in place on an interactive terminal stream."""

    def __init__(self, stream: TextIO) -> None:
        self.stream = stream
        self._visible = False

    def update(self, text: str, detail: str = "") -> None:
        """Show ``text``, then as much of the end of ``detail`` as fits."""
        width = max(20, shutil.get_terminal_size((80, 24)).columns - 1)
        line = display_safe(text)
        if detail:
            room = width - display_width(line) - 2
            tail = display_safe(detail)
            if room >= 8:
                line = f"{line}: {_truncate_left(tail, room)}"
        self.stream.write(f"\r{truncate(line, width)}\033[K")
        self.stream.flush()
        self._visible = True

    def clear(self) -> None:
        """Erase the status line if one is shown."""
        if self._visible:
            self.stream.write("\r\033[K")
            self.stream.flush()
            self._visible = False


def _truncate_left(value: str, width: int) -> str:
    if display_width(value) <= width:
        return value
    return truncate(value[::-1], width)[::-1]


def _normalize_cell(value: object) -> TableCell:
    if isinstance(value, TableCell):
        return TableCell(display_safe(str(value.value)), value.style)
    return TableCell(display_safe(str(value)))


def _paint(value: str, style: str | None, palette: Palette) -> str:
    if style is None:
        return value
    color = getattr(palette, style, "")
    if not color:
        return value
    return f"{color}{value}{palette.reset}"


def display_safe(value: str) -> str:
    """Escape control characters that would corrupt terminal or table layout."""
    result: list[str] = []
    for character in value:
        codepoint = ord(character)
        if character == "\n":
            result.append("\\n")
        elif character == "\r":
            result.append("\\r")
        elif character == "\t":
            result.append("\\t")
        elif codepoint < 32 or codepoint == 127:
            result.append(f"\\u{codepoint:04x}")
        else:
            result.append(character)
    return "".join(result)


def _tsv_value(value: object) -> str:
    return (
        str(value)
        .replace("\\", "\\\\")
        .replace("\t", "\\t")
        .replace("\r", "\\r")
        .replace("\n", "\\n")
    )


def display_width(value: str) -> int:
    """Measure terminal columns while ignoring ANSI and combining characters."""
    width = 0
    index = 0
    while index < len(value):
        character = value[index]
        if character == "\033" and index + 1 < len(value) and value[index + 1] == "[":
            index += 2
            while index < len(value) and not value[index].isalpha():
                index += 1
            index += 1
            continue
        if unicodedata.combining(character):
            index += 1
            continue
        category = unicodedata.category(character)
        if category.startswith("C"):
            index += 1
            continue
        width += 2 if unicodedata.east_asian_width(character) in {"W", "F"} else 1
        index += 1
    return width


def truncate(value: str, width: int) -> str:
    """Fit text within terminal columns and append an ellipsis when shortened."""
    if display_width(value) <= width:
        return value
    if width <= 1:
        return "…"[:width]
    result: list[str] = []
    used = 0
    target = width - 1
    for character in value:
        character_width = (
            0
            if unicodedata.combining(character)
            else (2 if unicodedata.east_asian_width(character) in {"W", "F"} else 1)
        )
        if used + character_width > target:
            break
        result.append(character)
        used += character_width
    return "".join(result) + "…"


def fit(value: str, width: int, *, alignment: str = "left") -> str:
    """Truncate and pad text to an exact display width."""
    shortened = truncate(value, width)
    missing = width - display_width(shortened)
    if alignment == "right":
        return " " * missing + shortened
    if alignment == "center":
        left = missing // 2
        return " " * left + shortened + " " * (missing - left)
    return shortened + " " * missing


def pad(value: str, width: int) -> str:
    """Pad text on the right to an exact display width."""
    return fit(value, width)


def bar(numerator: int, denominator: int, *, width: int = 12) -> str:
    """Render a clamped proportional block bar."""
    if denominator <= 0:
        return "░" * width
    ratio = min(1.0, max(0.0, numerator / denominator))
    filled = round(ratio * width)
    return "█" * filled + "░" * (width - filled)


def percent(numerator: int, denominator: int) -> str:
    """Render a percentage, treating a non-positive denominator as zero."""
    if denominator <= 0:
        return "0.0%"
    return f"{numerator * 100 / denominator:.1f}%"
