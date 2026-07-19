"""Validate profile names before they become filesystem path components."""

from __future__ import annotations

import re

from qdu.errors import UsageError

_PROFILE_NAME_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")


def validate_profile_name(name: str) -> str:
    """Return a profile name that is safe to use as one path component.

    Args:
        name: User-provided profile name.

    Returns:
        The unchanged validated name.

    Raises:
        UsageError: If the name is empty, too long, or contains path syntax.
    """
    if _PROFILE_NAME_PATTERN.fullmatch(name) is None:
        raise UsageError(
            "profile names must start with an alphanumeric character and "
            "contain only letters, numbers, '.', '_', or '-'"
        )
    return name
