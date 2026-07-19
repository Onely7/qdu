"""Normalize and apply root-relative filesystem exclusion patterns."""

from __future__ import annotations

import fnmatch
import hashlib
import json
from pathlib import PurePosixPath


class PathPatternMatcher:
    """Apply one documented exclusion contract across all scanning features.

    Patterns without a slash match any single path component. Patterns with a
    slash match the full root-relative POSIX path. `*`, `?`, and character
    classes follow fnmatch rules; `**` works naturally across slash boundaries
    because full-path matching is performed on a normalized POSIX string.
    """

    def __init__(self, patterns: tuple[str, ...]) -> None:
        self._patterns = tuple(
            self._normalize(pattern) for pattern in patterns if pattern.strip()
        )

    @staticmethod
    def _normalize(pattern: str) -> str:
        normalized = pattern.strip().replace("\\", "/")
        while normalized.startswith("./"):
            normalized = normalized[2:]
        return normalized.strip("/")

    @property
    def patterns(self) -> tuple[str, ...]:
        """Normalized immutable patterns used for matching."""
        return self._patterns

    @property
    def digest(self) -> str:
        """Stable SHA-256 digest used to compare snapshot scan policies."""
        payload = json.dumps(self._patterns, ensure_ascii=False, separators=(",", ":"))
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()

    def matches(self, relative_path: str) -> bool:
        """Return whether a normalized root-relative path is excluded."""
        normalized = relative_path.replace("\\", "/").strip("/")
        if not normalized or normalized == ".":
            return False
        parts = PurePosixPath(normalized).parts
        for pattern in self._patterns:
            if "/" in pattern:
                if PurePosixPath(normalized).match(pattern):
                    return True
            elif any(fnmatch.fnmatchcase(part, pattern) for part in parts):
                return True
        return False
