"""Parse and render byte sizes and human-readable durations."""

from __future__ import annotations

import re

from qdu.errors import UsageError

_SIZE_RE = re.compile(r"^([0-9]+(?:\.[0-9]+)?)([kmgtpe]?i?b)?$", re.IGNORECASE)
_DURATION_RE = re.compile(r"^([0-9]+(?:\.[0-9]+)?)(s|m|h|d|w)$", re.IGNORECASE)

_SIZE_MULTIPLIERS = {
    "": 1,
    "b": 1,
    "kb": 1000,
    "mb": 1000**2,
    "gb": 1000**3,
    "tb": 1000**4,
    "pb": 1000**5,
    "eb": 1000**6,
    "kib": 1024,
    "mib": 1024**2,
    "gib": 1024**3,
    "tib": 1024**4,
    "pib": 1024**5,
    "eib": 1024**6,
}

_DURATION_MULTIPLIERS = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}


def parse_size(value: str) -> int:
    """Parse a non-negative decimal or binary size into bytes.

    Raises:
        UsageError: If the value has an unsupported number or unit syntax.
    """
    match = _SIZE_RE.fullmatch(value.strip())
    if match is None:
        raise UsageError(f"invalid size: {value}")
    number, unit = match.groups()
    return int(float(number) * _SIZE_MULTIPLIERS[(unit or "").lower()])


def parse_duration(value: str) -> int:
    """Parse a duration with an s, m, h, d, or w suffix into seconds.

    Raises:
        UsageError: If the duration syntax or suffix is unsupported.
    """
    match = _DURATION_RE.fullmatch(value.strip())
    if match is None:
        raise UsageError(f"invalid duration: {value}")
    number, unit = match.groups()
    return int(float(number) * _DURATION_MULTIPLIERS[unit.lower()])


def format_bytes(value: int, *, signed: bool = False) -> str:
    """Render bytes with an appropriate IEC unit and optional explicit sign."""
    sign = ""
    number = float(value)
    if signed:
        sign = "+" if value > 0 else "-" if value < 0 else ""
        number = abs(number)
    units = ("B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB")
    unit_index = 0
    while number >= 1024 and unit_index < len(units) - 1:
        number /= 1024
        unit_index += 1
    if unit_index == 0 or number >= 10:
        rendered = f"{number:.0f}{units[unit_index]}"
    else:
        rendered = f"{number:.1f}{units[unit_index]}"
    return sign + rendered


def format_age(seconds: int) -> str:
    """Render a duration as compact day, hour, minute, and second parts."""
    if seconds < 0:
        seconds = 0
    parts: list[str] = []
    for suffix, unit_seconds in (("d", 86400), ("h", 3600), ("m", 60)):
        amount, seconds = divmod(seconds, unit_seconds)
        if amount:
            parts.append(f"{amount}{suffix}")
    if seconds or not parts:
        parts.append(f"{seconds}s")
    return "".join(parts)
