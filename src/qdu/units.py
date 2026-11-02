"""Parse and render byte sizes and human-readable durations."""

from __future__ import annotations

import re
from datetime import datetime
from itertools import pairwise

from qdu.errors import UsageError

# A prefix alone, as in du and sort -h, or with "iB" is binary; with "B" it is
# decimal.
_SIZE_RE = re.compile(r"([0-9]+(?:\.[0-9]+)?)(?:([kmgtpe])(i?b)?|b)?", re.IGNORECASE)
_SIZE_PREFIXES = "kmgtpe"
_DURATION_RE = re.compile(r"^([0-9]+(?:\.[0-9]+)?)(s|m|h|d|w)$", re.IGNORECASE)

_DURATION_MULTIPLIERS = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}
_AGE_UNITS = (("d", 86400), ("h", 3600), ("m", 60), ("s", 1))


def parse_size(value: str) -> int:
    """Parse a non-negative size such as ``500MB``, ``1.5GiB``, or ``2T`` into bytes.

    ``B`` after a prefix makes it decimal (``GB`` is 1000**3 bytes), while
    ``iB`` or the prefix alone makes it binary (``GiB`` and ``G`` are 1024**3).

    Raises:
        UsageError: If the value has an unsupported number or unit syntax.
    """
    match = _SIZE_RE.fullmatch(value.strip())
    if match is None:
        raise UsageError(f"invalid size: {value}")
    number, prefix, suffix = match.groups()
    multiplier = 1
    if prefix:
        base = 1000 if suffix and suffix.lower() == "b" else 1024
        multiplier = base ** (_SIZE_PREFIXES.index(prefix.lower()) + 1)
    whole, _, fraction = number.partition(".")
    return int(whole + fraction) * multiplier // 10 ** len(fraction)


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


def format_duration(seconds: int) -> str:
    """Render seconds with the largest exact unit among days, hours, and minutes."""
    for suffix, unit_seconds in (("d", 86400), ("h", 3600), ("m", 60)):
        if seconds and seconds % unit_seconds == 0:
            return f"{seconds // unit_seconds}{suffix}"
    return f"{seconds}s"


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


def format_exact_size(value: int) -> str:
    """Render bytes exactly, with the shortest unit that :func:`parse_size` reads."""
    candidates = [f"{value}B"]
    for power, prefix in enumerate(_SIZE_PREFIXES.upper(), 1):
        for base, suffix in ((1024, "iB"), (1000, "B")):
            unit = base**power
            for decimals in range(3):
                scaled = value * 10**decimals
                if value and scaled % unit == 0:
                    whole, fraction = divmod(scaled // unit, 10**decimals)
                    number = (
                        f"{whole}.{fraction:0{decimals}d}" if decimals else f"{whole}"
                    )
                    candidates.append(f"{number}{prefix}{suffix}")
                    break
    return min(candidates, key=len)


def format_local_time(epoch: float) -> str:
    """Render an epoch as local wall-clock time to the minute."""
    return datetime.fromtimestamp(epoch).astimezone().strftime("%Y-%m-%d %H:%M")


def format_age(seconds: int) -> str:
    """Render a duration with its largest unit and the next one, as in 3d4h or 5m12s."""
    seconds = max(0, seconds)
    for (suffix, size), (next_suffix, next_size) in pairwise(_AGE_UNITS):
        if seconds >= size:
            amount, rest = divmod(seconds, size)
            part = rest // next_size
            return (
                f"{amount}{suffix}{part}{next_suffix}" if part else f"{amount}{suffix}"
            )
    return f"{seconds}s"
